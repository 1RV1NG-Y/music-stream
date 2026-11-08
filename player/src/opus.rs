//! Opus for symphonia 0.5, which demuxes Ogg Opus but has no decoder of its own. Decoding is
//! libopus, the reference decoder, built from source by `opusic-sys` (needs CMake). A
//! pure-Rust decoder was tried first and panicked on real music. Always decodes at 48 kHz,
//! Opus's own rate.

use std::ffi::c_int;

use opusic_sys as ffi;

use symphonia::core::{
    audio::{AsAudioBufferRef, AudioBuffer, AudioBufferRef, Signal, SignalSpec},
    codecs::{CODEC_TYPE_OPUS, CodecDescriptor, CodecParameters, Decoder, DecoderOptions, FinalizeResult},
    errors::{Result, decode_error, unsupported_error},
    formats::Packet,
    support_codec,
};

const RATE: u32 = 48_000;
/// The longest packet: 120 ms.
const MAX_FRAMES: usize = RATE as usize * 120 / 1000;

/// Where each output channel comes from: Opus (like Vorbis) orders surround channels
/// FL FC FR …, symphonia's layouts (and `map_channels`) expect FL FR FC LFE ….
const ORDER: [&[usize]; 8] = [
    &[0],
    &[0, 1],
    &[0, 2, 1],
    &[0, 1, 2, 3],
    &[0, 2, 1, 3, 4],
    &[0, 2, 1, 5, 3, 4],
    &[0, 2, 1, 6, 5, 3, 4],
    &[0, 2, 1, 7, 5, 6, 3, 4],
];

/// The parts of the `OpusHead` identification header (RFC 7845 §5.1) decoding needs.
#[derive(Debug, PartialEq)]
pub(crate) struct Head {
    pub channels: usize,
    /// Encoder warm-up samples (at 48 kHz) to drop from the start of the stream.
    pub pre_skip: u16,
    /// Gain to always apply, in dB.
    pub output_gain: f32,
    pub streams: usize,
    pub coupled: usize,
    pub mapping: Vec<u8>,
}

pub(crate) fn parse_head(b: &[u8]) -> Option<Head> {
    if b.len() < 19 || &b[..8] != b"OpusHead" || b[8] >> 4 != 0 {
        return None;
    }
    let channels = b[9] as usize;
    let pre_skip = u16::from_le_bytes([b[10], b[11]]);
    let output_gain = i16::from_le_bytes([b[16], b[17]]) as f32 / 256.0;
    let (streams, coupled, mapping) = match b[18] {
        0 if (1..=2).contains(&channels) => (1, channels - 1, (0..channels as u8).collect()),
        0 => return None,
        _ => {
            let table = b.get(19..21 + channels)?;
            (table[0] as usize, table[1] as usize, table[2..].to_vec())
        }
    };
    Some(Head { channels, pre_skip, output_gain, streams, coupled, mapping })
}

/// A libopus multistream decoder (plain mono/stereo is one stream).
struct Libopus(*mut ffi::OpusMSDecoder);

// SAFETY: the decoder state is only touched through `&mut self` and owns no thread-bound data.
unsafe impl Send for Libopus {}
unsafe impl Sync for Libopus {}

impl Libopus {
    fn new(head: &Head) -> Option<Self> {
        if head.mapping.len() != head.channels {
            return None;
        }
        let mut error = 0;
        // SAFETY: `mapping` holds `channels` entries, as libopus requires.
        let st = unsafe {
            ffi::opus_multistream_decoder_create(RATE as i32, head.channels as c_int, head.streams as c_int, head.coupled as c_int, head.mapping.as_ptr(), &mut error)
        };
        (error == ffi::OPUS_OK && !st.is_null()).then_some(Libopus(st))
    }

    /// Decodes one packet into `pcm` (interleaved); returns frames per channel.
    fn decode(&mut self, packet: &[u8], pcm: &mut [f32], channels: usize) -> Option<usize> {
        let frames = (pcm.len() / channels) as c_int;
        // SAFETY: `pcm` has room for `frames` frames of `channels` samples; `packet` is valid.
        let n = unsafe { ffi::opus_multistream_decode_float(self.0, packet.as_ptr(), packet.len() as i32, pcm.as_mut_ptr(), frames, 0) };
        (n >= 0).then_some(n as usize)
    }

    fn reset(&mut self) {
        // SAFETY: OPUS_RESET_STATE takes no arguments.
        unsafe { ffi::opus_multistream_decoder_ctl(self.0, ffi::OPUS_RESET_STATE) };
    }
}

impl Drop for Libopus {
    fn drop(&mut self) {
        // SAFETY: created by `opus_multistream_decoder_create`, destroyed once.
        unsafe { ffi::opus_multistream_decoder_destroy(self.0) }
    }
}

pub struct OpusDecoder {
    params: CodecParameters,
    inner: Libopus,
    channels: usize,
    order: &'static [usize],
    pre_skip: u64,
    gain: f32,
    pcm: Vec<f32>,
    buf: AudioBuffer<f32>,
}

impl Decoder for OpusDecoder {
    fn try_new(params: &CodecParameters, _: &DecoderOptions) -> Result<Self> {
        if params.codec != CODEC_TYPE_OPUS {
            return unsupported_error("opus: invalid codec type");
        }
        let Some(head) = params.extra_data.as_deref().and_then(parse_head) else {
            return unsupported_error("opus: missing or unsupported header");
        };
        let Some(layout) = params.channels.filter(|c| c.count() == head.channels && head.channels <= ORDER.len()) else {
            return unsupported_error("opus: unsupported channel layout");
        };
        let Some(inner) = Libopus::new(&head) else {
            return unsupported_error("opus: unsupported stream layout");
        };
        Ok(OpusDecoder {
            params: params.clone(),
            inner,
            channels: head.channels,
            order: ORDER[head.channels - 1],
            pre_skip: head.pre_skip as u64,
            gain: 10f32.powf(head.output_gain / 20.0),
            pcm: vec![0.0; MAX_FRAMES * head.channels],
            buf: AudioBuffer::new(MAX_FRAMES as u64, SignalSpec::new(RATE, layout)),
        })
    }

    fn supported_codecs() -> &'static [CodecDescriptor] {
        &[support_codec!(CODEC_TYPE_OPUS, "opus", "Opus")]
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn codec_params(&self) -> &CodecParameters {
        &self.params
    }

    fn decode(&mut self, packet: &Packet) -> Result<AudioBufferRef<'_>> {
        self.buf.clear();
        let Some(frames) = self.inner.decode(&packet.data, &mut self.pcm, self.channels) else {
            return decode_error("opus: invalid packet");
        };
        let frames = frames.min(MAX_FRAMES);
        self.buf.render_reserved(Some(frames));
        for (ch, &from) in self.order.iter().enumerate() {
            for (i, s) in self.buf.chan_mut(ch).iter_mut().enumerate() {
                *s = self.pcm[i * self.channels + from] * self.gain;
            }
        }
        // The Ogg demuxer counts timestamps from the encoder's first sample, warm-up included,
        // and trims only the end; the warm-up is dropped here.
        let warm_up = self.pre_skip.saturating_sub(packet.ts()).min(frames as u64) as usize;
        self.buf.trim(packet.trim_start() as usize + warm_up, packet.trim_end() as usize);
        Ok(self.buf.as_audio_buffer_ref())
    }

    fn finalize(&mut self) -> FinalizeResult {
        FinalizeResult::default()
    }

    fn last_decoded(&self) -> AudioBufferRef<'_> {
        self.buf.as_audio_buffer_ref()
    }
}
