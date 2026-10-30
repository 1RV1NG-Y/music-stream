//! Native playback (PLAN.md §6): a worker thread decodes (symphonia), applies ReplayGain,
//! maps channels and resamples (rubato) into a bounded lock-free ring; the output callback
//! (cpal) only drains the ring, applies volume and converts the sample format. It never
//! locks, allocates, does I/O or waits. Commands that change what plays set an interrupt flag
//! that releases a read stalled on a missing block.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use rubato::{
    Fft, FixedSync, Resampler,
    audioadapter_buffers::owned::InterleavedOwned,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions},
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
    units::Time,
};

/// About this much audio is decoded ahead of the output.
const RING_SECONDS: f64 = 1.0;
const RESAMPLE_CHUNK: usize = 1024;

pub use symphonia::core::io::MediaSource;

/// A track opened for decoding.
pub struct Opened {
    pub source: Box<dyn MediaSource>,
    /// File extension, a hint for the format probe.
    pub ext: String,
}

/// Where the player gets its tracks. `Item` is whatever the app queues.
pub trait Provider: Send + Sync + 'static {
    type Item: Clone + Send + Sync + 'static;
    /// Opens an item. Reads may wait for bytes; once `interrupt` is set they must fail.
    fn open(&self, item: &Self::Item, interrupt: Arc<AtomicBool>) -> Result<Opened>;
    /// Linear gain to apply (ReplayGain), 1.0 for none.
    fn gain(&self, _item: &Self::Item) -> f32 {
        1.0
    }
    /// The item will probably play next.
    fn prefetch(&self, _item: &Self::Item) {}
}

/// What happens at the end of a track (PLAN.md §7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub shuffle: bool,
    pub repeat: Repeat,
    pub index: Option<usize>,
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    /// Playing, but waiting for audio (the phone is slow or unreachable).
    pub buffering: bool,
    pub volume: f32,
    pub error: Option<String>,
    pub output: String,
}

enum Cmd<I> {
    SetQueue(Vec<I>, usize, bool),
    Insert(usize, Vec<I>),
    Remove(usize),
    Move(usize, usize),
    Jump(usize),
    Play,
    Pause,
    Next,
    Prev,
    Seek(u64),
    Stop,
    Shuffle(bool),
    MarkShuffled,
    Repeat(Repeat),
    Shutdown,
}

/// State shared with the output callback: atomics only.
struct Shared {
    volume: AtomicU32,
    paused: AtomicBool,
    flush_req: AtomicU64,
    flush_ack: AtomicU64,
    /// Frames of real audio the output has played since the last flush.
    played: AtomicU64,
    starved: AtomicBool,
    underruns: AtomicU64,
    failed: AtomicBool,
    /// The queue ran out; the output plays what's left.
    ended: AtomicBool,
}

impl Shared {
    fn new() -> Self {
        Shared {
            volume: AtomicU32::new(1f32.to_bits()),
            paused: AtomicBool::new(true),
            flush_req: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
            played: AtomicU64::new(0),
            starved: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        }
    }
}

/// How audio leaves the player.
pub enum OutputKind {
    /// The system's default output device.
    Device,
    /// Rendered for the caller to pull (tests): the format to produce.
    Manual { rate: u32, channels: u16 },
}

pub struct Player<P: Provider> {
    tx: mpsc::Sender<Cmd<P::Item>>,
    interrupt: Arc<AtomicBool>,
    shared: Arc<Shared>,
    status: Arc<Mutex<Status>>,
    queue: Arc<Mutex<Vec<P::Item>>>,
    timeline: Arc<Mutex<Timeline>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl<P: Provider> Player<P> {
    /// Starts the player. With `OutputKind::Manual`, also returns the sink to pull from.
    pub fn new(provider: P, output: OutputKind) -> Result<(Self, Option<ManualSink>)> {
        let shared = Arc::new(Shared::new());
        let interrupt = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status { volume: 1.0, ..Status::default() }));
        let queue = Arc::new(Mutex::new(Vec::new()));
        let timeline = Arc::new(Mutex::new(Timeline::default()));
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = {
            let (shared, interrupt, status, queue, timeline) = (shared.clone(), interrupt.clone(), status.clone(), queue.clone(), timeline.clone());
            std::thread::Builder::new()
                .name("player".into())
                .spawn(move || {
                    let mut w = Worker::new(provider, shared, interrupt, status, queue, timeline, rx);
                    let sink = match w.open_output(&output) {
                        Ok(sink) => sink,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            return;
                        }
                    };
                    let _ = ready_tx.send(Ok(sink));
                    w.run(output);
                })?
        };
        let sink = ready_rx.recv().context("the player didn't start")??;
        Ok((Player { tx, interrupt, shared, status, queue, timeline, worker: Some(worker) }, sink))
    }

    fn send(&self, cmd: Cmd<P::Item>, interrupt: bool) {
        let _ = self.tx.send(cmd);
        if interrupt {
            self.interrupt.store(true, Ordering::Release);
        }
    }

    /// The worker publishes the rest; playing, position and buffering come straight from the
    /// output, so they stay true while the worker waits on a read.
    pub fn status(&self) -> Status {
        let mut st = self.status.lock().unwrap().clone();
        let playing = !self.shared.paused.load(Ordering::Relaxed);
        let (index, position_ms, duration_ms) = self.timeline.lock().unwrap().at(self.shared.played.load(Ordering::Relaxed));
        st.index = index;
        st.position_ms = position_ms;
        st.duration_ms = duration_ms;
        st.playing = playing;
        st.buffering = playing && !self.shared.ended.load(Ordering::Relaxed) && self.shared.starved.load(Ordering::Relaxed);
        st
    }

    pub fn queue(&self) -> Vec<P::Item> {
        self.queue.lock().unwrap().clone()
    }

    /// Replaces the queue and starts at `start`.
    pub fn set_queue(&self, items: Vec<P::Item>, start: usize, play: bool) {
        self.send(Cmd::SetQueue(items, start, play), true);
    }
    /// Inserts items at `at` (after the current one for "play next").
    pub fn insert(&self, at: usize, items: Vec<P::Item>) {
        self.send(Cmd::Insert(at, items), false);
    }
    pub fn remove(&self, index: usize) {
        self.send(Cmd::Remove(index), true);
    }
    pub fn move_item(&self, from: usize, to: usize) {
        self.send(Cmd::Move(from, to), false);
    }
    pub fn jump(&self, index: usize) {
        self.send(Cmd::Jump(index), true);
    }
    /// Takes effect in the output at once, even while the worker waits on a read.
    pub fn play(&self) {
        if !self.queue.lock().unwrap().is_empty() && !self.shared.ended.load(Ordering::Relaxed) {
            self.shared.paused.store(false, Ordering::Relaxed);
        }
        self.send(Cmd::Play, false);
    }
    /// Takes effect in the output at once, even while the worker waits on a read.
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
        self.send(Cmd::Pause, false);
    }
    pub fn toggle(&self) {
        if self.shared.paused.load(Ordering::Relaxed) { self.play() } else { self.pause() }
    }
    pub fn next(&self) {
        self.send(Cmd::Next, true);
    }
    pub fn prev(&self) {
        self.send(Cmd::Prev, true);
    }
    pub fn seek(&self, ms: u64) {
        self.send(Cmd::Seek(ms), true);
    }
    pub fn stop(&self) {
        self.send(Cmd::Stop, true);
    }
    /// On: the current track moves to the top and everything else is shuffled after it.
    /// Off: the original order comes back, continuing after the current track.
    pub fn set_shuffle(&self, on: bool) {
        self.status.lock().unwrap().shuffle = on;
        self.send(Cmd::Shuffle(on), false);
    }
    /// Shuffle on without reshuffling: for a queue restored in its shuffled order.
    pub fn mark_shuffled(&self) {
        self.status.lock().unwrap().shuffle = true;
        self.send(Cmd::MarkShuffled, false);
    }
    pub fn set_repeat(&self, repeat: Repeat) {
        self.status.lock().unwrap().repeat = repeat;
        self.send(Cmd::Repeat(repeat), false);
    }
    /// Linear gain, applied in the output callback so it takes effect at once.
    pub fn set_volume(&self, volume: f32) {
        let v = volume.clamp(0.0, 1.0);
        self.shared.volume.store(v.to_bits(), Ordering::Relaxed);
        self.status.lock().unwrap().volume = v;
    }
    pub fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }
}

impl<P: Provider> Drop for Player<P> {
    fn drop(&mut self) {
        self.send(Cmd::Shutdown, true);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

// ---- Output ------------------------------------------------------------------------------

struct Output {
    rate: u32,
    channels: usize,
    producer: rtrb::Producer<f32>,
    _stream: Option<cpal::Stream>,
    name: String,
}

/// Pulls rendered audio from a player made with `OutputKind::Manual`.
pub struct ManualSink {
    shared: Arc<Shared>,
    consumer: rtrb::Consumer<f32>,
    pub rate: u32,
    pub channels: u16,
}

impl ManualSink {
    /// Takes up to `max_frames` of the audio that is ready, as the output callback would.
    pub fn pull(&mut self, max_frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; max_frames * self.channels as usize];
        let n = fill(&self.shared, &mut self.consumer, &mut out, self.channels as usize);
        out.truncate(n);
        out
    }

    /// Renders exactly `frames` frames as the device would: silence where audio is missing.
    /// Also returns how many samples were real audio.
    pub fn render(&mut self, frames: usize) -> (Vec<f32>, usize) {
        let mut out = vec![0.0; frames * self.channels as usize];
        let n = fill(&self.shared, &mut self.consumer, &mut out, self.channels as usize);
        (out, n)
    }
}

/// The output callback's work. Returns how many samples were real audio (the rest is silence).
fn fill(shared: &Shared, consumer: &mut rtrb::Consumer<f32>, out: &mut [f32], channels: usize) -> usize {
    let req = shared.flush_req.load(Ordering::Acquire);
    if req != shared.flush_ack.load(Ordering::Relaxed) {
        let n = consumer.slots();
        if let Ok(chunk) = consumer.read_chunk(n) {
            chunk.commit_all();
        }
        shared.played.store(0, Ordering::Relaxed);
        shared.flush_ack.store(req, Ordering::Release);
    }
    if shared.paused.load(Ordering::Relaxed) {
        out.fill(0.0);
        return 0;
    }
    let want = out.len() - out.len() % channels;
    let n = consumer.slots().min(want) / channels * channels;
    let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
    if let Ok(chunk) = consumer.read_chunk(n) {
        let (a, b) = chunk.as_slices();
        for (o, s) in out.iter_mut().zip(a.iter().chain(b.iter())) {
            *o = s * volume;
        }
        chunk.commit_all();
    }
    out[n..].fill(0.0);
    shared.played.fetch_add((n / channels) as u64, Ordering::Relaxed);
    let starved = n < want;
    shared.starved.store(starved, Ordering::Relaxed);
    if starved {
        shared.underruns.fetch_add(1, Ordering::Relaxed);
    }
    n
}

fn build_stream(device: &cpal::Device, config: &cpal::SupportedStreamConfig, shared: Arc<Shared>, mut consumer: rtrb::Consumer<f32>) -> Result<cpal::Stream> {
    use cpal::traits::DeviceTrait;
    let channels = config.channels() as usize;
    let stream_config = config.config();
    let err_shared = shared.clone();
    let on_error = move |e: cpal::Error| {
        tracing::warn!("audio output error: {e}");
        err_shared.failed.store(true, Ordering::Release);
    };
    macro_rules! build {
        ($t:ty) => {{
            // Scratch space for the f32 frames; sized once, before the stream runs.
            let mut scratch = vec![0f32; 1 << 16];
            device.build_output_stream::<$t, _, _>(
                stream_config,
                move |data: &mut [$t], _| {
                    if scratch.len() < data.len() {
                        // Never expected (buffers are far smaller); silence rather than allocate.
                        data.fill(<$t as cpal::Sample>::EQUILIBRIUM);
                        return;
                    }
                    let buf = &mut scratch[..data.len()];
                    fill(&shared, &mut consumer, buf, channels);
                    for (d, s) in data.iter_mut().zip(buf.iter()) {
                        *d = <$t as cpal::FromSample<f32>>::from_sample_(*s);
                    }
                },
                on_error,
                None,
            )?
        }};
    }
    Ok(match config.sample_format() {
        cpal::SampleFormat::F32 => build!(f32),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::F64 => build!(f64),
        other => bail!("unsupported output sample format {other:?}"),
    })
}

// ---- Decoding ------------------------------------------------------------------------------

struct Decoding {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    rate: u32,
    channels: usize,
    duration_ms: u64,
    gain: f32,
    /// Discard decoded frames before this timestamp (after an accurate seek).
    skip_until: u64,
}

fn open_decoding(opened: Opened, gain: f32) -> Result<Decoding> {
    let mss = MediaSourceStream::new(opened.source, Default::default());
    let mut hint = Hint::new();
    if !opened.ext.is_empty() {
        hint.with_extension(&opened.ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions { enable_gapless: true, ..Default::default() }, &MetadataOptions::default())
        .context("unrecognized audio format")?;
    let format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).context("no audio track")?;
    let params = &track.codec_params;
    let rate = params.sample_rate.context("unknown sample rate")?;
    let channels = params.channels.map(|c| c.count()).unwrap_or(2);
    let duration_ms = match (params.n_frames, params.time_base) {
        (Some(n), Some(tb)) => {
            let t = tb.calc_time(n);
            t.seconds * 1000 + (t.frac * 1000.0) as u64
        }
        (Some(n), None) => n * 1000 / rate as u64,
        _ => 0,
    };
    let decoder = symphonia::default::get_codecs().make(params, &DecoderOptions::default()).context("unsupported codec")?;
    Ok(Decoding { track_id: track.id, format, decoder, rate, channels, duration_ms, gain, skip_until: 0 })
}

enum Decoded {
    Frames(Vec<f32>),
    End,
}

impl Decoding {
    fn seek(&mut self, ms: u64) -> Result<()> {
        let to = SeekTo::Time { time: Time::new(ms / 1000, (ms % 1000) as f64 / 1000.0), track_id: Some(self.track_id) };
        let seeked = self.format.seek(SeekMode::Accurate, to)?;
        self.decoder.reset();
        self.skip_until = seeked.required_ts;
        Ok(())
    }

    /// Decodes the next packet into interleaved f32 at the track's own rate and channels.
    fn next(&mut self) -> Result<Decoded> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(symphonia::core::errors::Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(Decoded::End),
                Err(e) => return Err(e.into()),
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            let ts = packet.ts();
            let decoded = match self.decoder.decode(&packet) {
                Ok(d) => d,
                Err(symphonia::core::errors::Error::DecodeError(e)) => {
                    tracing::debug!("skipping a damaged packet: {e}");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let spec = *decoded.spec();
            let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            buf.copy_interleaved_ref(decoded);
            let mut samples = buf.samples().to_vec();
            let ch = spec.channels.count();
            if ch != self.channels {
                // A stream that changes layout mid-way: follow it.
                self.channels = ch;
            }
            if ts < self.skip_until {
                let skip = ((self.skip_until - ts) as usize * ch).min(samples.len());
                samples.drain(..skip);
            }
            if self.gain != 1.0 {
                samples.iter_mut().for_each(|s| *s *= self.gain);
            }
            if !samples.is_empty() {
                return Ok(Decoded::Frames(samples));
            }
        }
    }
}

/// Maps interleaved frames from `from` channels to `to` channels (PLAN.md §6).
pub fn map_channels(samples: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return samples.to_vec();
    }
    let frames = samples.len() / from;
    let mut out = vec![0.0; frames * to];
    const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for f in 0..frames {
        let i = &samples[f * from..(f + 1) * from];
        let o = &mut out[f * to..(f + 1) * to];
        match (from, to) {
            (1, _) => {
                o[0] = i[0];
                if to > 1 {
                    o[1] = i[0];
                }
            }
            (_, 1) => o[0] = i.iter().sum::<f32>() / from as f32,
            // 5.1 (FL FR FC LFE SL SR) → stereo, ITU-R BS.775, normalized to avoid clipping.
            (6, 2) => {
                let n = 1.0 / (1.0 + 2.0 * H);
                o[0] = (i[0] + H * i[2] + H * i[4]) * n;
                o[1] = (i[1] + H * i[2] + H * i[5]) * n;
            }
            _ => {
                let k = from.min(to);
                o[..k].copy_from_slice(&i[..k]);
            }
        }
    }
    out
}

/// Converts to the output rate, keeping its state across tracks of the same rate (gapless).
struct Rate {
    from: u32,
    channels: usize,
    resampler: Option<Fft<f32>>,
    input: Vec<f32>,
    /// Output frames still to drop: the resampler's startup delay.
    skip: usize,
    in_frames: u64,
    out_frames: u64,
}

impl Rate {
    fn new(from: u32, to: u32, channels: usize) -> Result<Self> {
        let resampler = if from == to {
            None
        } else {
            Some(Fft::<f32>::new(from as usize, to as usize, RESAMPLE_CHUNK, channels, FixedSync::Input).map_err(|e| anyhow!("resampler: {e}"))?)
        };
        let skip = resampler.as_ref().map_or(0, |r| r.output_delay());
        Ok(Rate { from, channels, resampler, input: Vec::new(), skip, in_frames: 0, out_frames: 0 })
    }

    fn push(&mut self, samples: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.in_frames += (samples.len() / self.channels) as u64;
        self.run(samples, out)
    }

    fn run(&mut self, samples: &[f32], out: &mut Vec<f32>) -> Result<()> {
        let Some(r) = &mut self.resampler else {
            out.extend_from_slice(samples);
            return Ok(());
        };
        self.input.extend_from_slice(samples);
        loop {
            let need = r.input_frames_next() * self.channels;
            if self.input.len() < need {
                return Ok(());
            }
            let chunk: Vec<f32> = self.input.drain(..need).collect();
            let input = InterleavedOwned::new_from(chunk, self.channels, need / self.channels).map_err(|e| anyhow!("{e:?}"))?;
            let produced = r.process(&input, None).map_err(|e| anyhow!("resampling: {e}"))?.take_data();
            let drop = (self.skip * self.channels).min(produced.len());
            self.skip -= drop / self.channels;
            self.out_frames += ((produced.len() - drop) / self.channels) as u64;
            out.extend_from_slice(&produced[drop..]);
        }
    }

    /// Pushes out what the resampler still holds (end of the queue or a rate change): exactly
    /// the frames the input so far is owed.
    fn flush(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let Some(r) = &self.resampler else { return Ok(()) };
        let owed = ((self.in_frames as f64 * r.resample_ratio()).round() as u64).saturating_sub(self.out_frames) as usize;
        let pad = (r.input_frames_next() * 2 + r.output_delay()) * self.channels;
        let mut tail = Vec::new();
        self.run(&vec![0.0; pad], &mut tail)?;
        out.extend_from_slice(&tail[..(owed * self.channels).min(tail.len())]);
        self.input.clear();
        Ok(())
    }
}

// ---- Worker ----------------------------------------------------------------------------------

/// Where in the written stream each track starts, for the position shown.
struct Mark {
    frame: u64,
    index: usize,
    start_ms: u64,
    duration_ms: u64,
}

/// Where each track starts in the output, shared so the status stays live while the worker
/// waits on a read.
#[derive(Default)]
struct Timeline {
    marks: VecDeque<Mark>,
    rate: u64,
    /// The queue position when nothing has been marked yet.
    index: Option<usize>,
}

impl Timeline {
    /// (queue index, position in ms, duration in ms) of what the output plays after `played` frames.
    fn at(&self, played: u64) -> (Option<usize>, u64, u64) {
        let rate = self.rate.max(1);
        match self.marks.iter().rev().find(|m| m.frame <= played) {
            Some(m) => (Some(m.index), (m.start_ms + (played - m.frame) * 1000 / rate).min(m.duration_ms.max(1)), m.duration_ms),
            None => match self.marks.front() {
                Some(m) => (Some(m.index), m.start_ms, m.duration_ms),
                None => (self.index, 0, 0),
            },
        }
    }
}

struct Worker<P: Provider> {
    provider: P,
    shared: Arc<Shared>,
    interrupt: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    queue: Arc<Mutex<Vec<P::Item>>>,
    rx: mpsc::Receiver<Cmd<P::Item>>,
    out: Option<Output>,
    index: Option<usize>,
    playing: bool,
    current: Option<Decoding>,
    /// Decoded frames of the current track so far, at its own rate (its position).
    decoded_frames: u64,
    rate: Option<Rate>,
    pending: Vec<f32>,
    written: u64,
    timeline: Arc<Mutex<Timeline>>,
    /// The queue ran out; the output plays what's left.
    ended: bool,
    /// The current item must be reopened at this position (its read was interrupted).
    reopen_at: Option<u64>,
    shuffle: bool,
    repeat: Repeat,
    /// Each queue entry's place in the unshuffled order, so shuffle can be turned off.
    keys: Vec<f64>,
    rng: u64,
    error: Option<String>,
}

impl<P: Provider> Worker<P> {
    fn new(provider: P, shared: Arc<Shared>, interrupt: Arc<AtomicBool>, status: Arc<Mutex<Status>>, queue: Arc<Mutex<Vec<P::Item>>>, timeline: Arc<Mutex<Timeline>>, rx: mpsc::Receiver<Cmd<P::Item>>) -> Self {
        Worker {
            provider,
            shared,
            interrupt,
            status,
            queue,
            rx,
            out: None,
            index: None,
            playing: false,
            current: None,
            decoded_frames: 0,
            rate: None,
            pending: Vec::new(),
            written: 0,
            timeline,
            ended: false,
            reopen_at: None,
            shuffle: false,
            repeat: Repeat::Off,
            keys: Vec::new(),
            rng: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x9e37_79b9, |d| d.as_nanos() as u64) | 1,
            error: None,
        }
    }

    fn open_output(&mut self, kind: &OutputKind) -> Result<Option<ManualSink>> {
        match kind {
            OutputKind::Manual { rate, channels } => {
                let (producer, consumer) = rtrb::RingBuffer::new((*rate as f64 * RING_SECONDS) as usize * *channels as usize);
                self.out = Some(Output { rate: *rate, channels: *channels as usize, producer, _stream: None, name: "buffer".into() });
                Ok(Some(ManualSink { shared: self.shared.clone(), consumer, rate: *rate, channels: *channels }))
            }
            OutputKind::Device => {
                use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
                let host = cpal::default_host();
                let device = host.default_output_device().context("no audio output device")?;
                let config = device.default_output_config().context("the audio output has no usable format")?;
                let (rate, channels) = (config.sample_rate(), config.channels() as usize);
                let (producer, consumer) = rtrb::RingBuffer::new((rate as f64 * RING_SECONDS) as usize * channels);
                self.shared.failed.store(false, Ordering::Release);
                let stream = build_stream(&device, &config, self.shared.clone(), consumer)?;
                stream.play()?;
                let name = device.description().map(|d| d.to_string()).unwrap_or_else(|_| "default output".into());
                tracing::info!(%name, rate, channels, format = ?config.sample_format(), "audio output open");
                self.out = Some(Output { rate, channels, producer, _stream: Some(stream), name });
                Ok(None)
            }
        }
    }

    fn run(mut self, kind: OutputKind) {
        loop {
            self.interrupt.store(false, Ordering::Release);
            let mut reopen = false;
            while let Ok(cmd) = self.rx.try_recv() {
                match self.handle(cmd) {
                    Some(true) => reopen = true,
                    Some(false) => {}
                    None => return,
                }
            }
            if self.shared.failed.load(Ordering::Acquire) && matches!(kind, OutputKind::Device) {
                // The device went away or changed: reopen and restart the current track there.
                std::thread::sleep(Duration::from_millis(300));
                if let Err(e) = self.open_output(&kind) {
                    self.set_error(format!("{e:#}"));
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                reopen = true;
            }
            if reopen {
                let at = self.position().1;
                self.restart_at(at);
            } else if let Some(at) = self.reopen_at.take() {
                self.restart_at(at);
            }
            self.publish();
            if !self.step() {
                match self.rx.recv_timeout(Duration::from_millis(15)) {
                    Ok(cmd) => {
                        if self.handle(cmd).is_none() {
                            return;
                        }
                        // Re-run the loop: it drains the rest and reopens if needed.
                        self.interrupt.store(false, Ordering::Release);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }

    /// Does one unit of work; false when there was nothing to do.
    fn step(&mut self) -> bool {
        let Some(out) = &mut self.out else { return false };
        let (out_ch, out_rate) = (out.channels, out.rate);
        if !self.pending.is_empty() {
            let n = out.producer.slots().min(self.pending.len()) / out_ch * out_ch;
            if n == 0 {
                return false;
            }
            if let Ok(chunk) = out.producer.write_chunk_uninit(n) {
                chunk.fill_from_iter(self.pending.drain(..n));
            }
            self.written += (n / out_ch) as u64;
            return true;
        }
        if !self.playing || self.current.is_none() {
            if self.ended && self.playing && self.shared.played.load(Ordering::Relaxed) >= self.written {
                self.set_playing(false);
            }
            return false;
        }
        let dec = self.current.as_mut().unwrap();
        let (dec_rate, dec_ch) = (dec.rate, dec.channels);
        let result = dec.next();
        match result {
            Ok(Decoded::Frames(samples)) => {
                let ch = self.current.as_ref().map_or(dec_ch, |d| d.channels);
                self.decoded_frames += (samples.len() / ch) as u64;
                let mapped = map_channels(&samples, ch, out_ch);
                if self.rate.as_ref().is_none_or(|r| r.from != dec_rate || r.channels != out_ch) {
                    if let Some(r) = &mut self.rate {
                        let mut tail = Vec::new();
                        let _ = r.flush(&mut tail);
                        self.pending.extend(tail);
                    }
                    match Rate::new(dec_rate, out_rate, out_ch) {
                        Ok(r) => self.rate = Some(r),
                        Err(e) => {
                            self.fail_current(format!("{e:#}"));
                            return true;
                        }
                    }
                }
                let mut produced = Vec::new();
                let pushed = self.rate.as_mut().unwrap().push(&mapped, &mut produced);
                self.pending.extend(produced);
                if let Err(e) = pushed {
                    self.fail_current(format!("{e:#}"));
                }
            }
            Ok(Decoded::End) => self.advance(),
            Err(e) => {
                if self.interrupt.load(Ordering::Acquire) {
                    // A command is waiting. The decoder stopped mid-read: unless the command
                    // replaces the track, reopen it where it was.
                    self.current = None;
                    self.reopen_at = Some(self.decoded_ms());
                } else {
                    self.fail_current(format!("{e:#}"));
                }
            }
        }
        true
    }

    /// Position of the decoder in the current track.
    fn decoded_ms(&self) -> u64 {
        let start = self.timeline.lock().unwrap().marks.back().map_or(0, |m| m.start_ms);
        let rate = self.current.as_ref().map_or(0, |d| d.rate as u64);
        start + if rate > 0 { self.decoded_frames * 1000 / rate } else { 0 }
    }

    fn fail_current(&mut self, error: String) {
        tracing::warn!("can't play this track: {error}");
        self.set_error(error);
        self.advance();
    }

    fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    /// Moves on to the next track, gaplessly when the formats allow.
    fn advance(&mut self) {
        self.current = None;
        let len = self.queue.lock().unwrap().len();
        let next = match (self.repeat, self.index) {
            (Repeat::One, Some(i)) => i,
            (Repeat::All, Some(i)) if i + 1 >= len => 0,
            (_, i) => i.map_or(0, |i| i + 1),
        };
        if next >= len {
            self.ended = true;
            self.shared.ended.store(true, Ordering::Relaxed);
            if let Some(r) = &mut self.rate {
                let mut tail = Vec::new();
                let _ = r.flush(&mut tail);
                self.pending.extend(tail);
            }
            self.rate = None;
            return;
        }
        self.index = Some(next);
        self.open_current(0);
    }

    /// Opens the item at `self.index`, seeking to `at_ms`; marks where it starts in the output.
    fn open_current(&mut self, at_ms: u64) {
        let Some(i) = self.index else { return };
        let Some(item) = self.queue.lock().unwrap().get(i).cloned() else { return };
        self.ended = false;
        self.shared.ended.store(false, Ordering::Relaxed);
        self.decoded_frames = 0;
        let opened = self.provider.open(&item, self.interrupt.clone()).and_then(|o| open_decoding(o, self.provider.gain(&item)));
        match opened {
            Ok(mut dec) => {
                if at_ms > 0 {
                    if let Err(e) = dec.seek(at_ms) {
                        if self.interrupt.load(Ordering::Acquire) {
                            self.current = None;
                            self.reopen_at = Some(at_ms);
                            return;
                        }
                        tracing::debug!("seek failed: {e:#}");
                    }
                }
                let frame = self.written + (self.pending.len() / self.out_channels()) as u64;
                let mut tl = self.timeline.lock().unwrap();
                tl.marks.push_back(Mark { frame, index: i, start_ms: at_ms, duration_ms: dec.duration_ms });
                while tl.marks.len() > 8 {
                    tl.marks.pop_front();
                }
                drop(tl);
                self.error = None;
                self.current = Some(dec);
                let after = match self.repeat {
                    Repeat::One => Some(i),
                    Repeat::All => Some(i + 1).filter(|&n| n < self.queue.lock().unwrap().len()).or(Some(0)),
                    Repeat::Off => Some(i + 1),
                };
                if let Some(next) = after.and_then(|n| self.queue.lock().unwrap().get(n).cloned()) {
                    self.provider.prefetch(&next);
                }
            }
            Err(e) => {
                if self.interrupt.load(Ordering::Acquire) {
                    // Interrupted while opening: the loop reopens it unless a command moved on.
                    self.current = None;
                    self.reopen_at = Some(at_ms);
                    return;
                }
                self.fail_current(format!("{e:#}"));
            }
        }
    }

    fn out_channels(&self) -> usize {
        self.out.as_ref().map_or(2, |o| o.channels)
    }

    /// Drops everything buffered and starts the current item again at `at_ms`.
    fn restart_at(&mut self, at_ms: u64) {
        self.reopen_at = None;
        self.flush();
        self.rate = None;
        self.current = None;
        if self.index.is_some() {
            self.open_current(at_ms);
        }
    }

    /// Empties the ring: the callback drops what it holds and starts counting from zero.
    fn flush(&mut self) {
        self.pending.clear();
        let req = self.shared.flush_req.fetch_add(1, Ordering::AcqRel) + 1;
        // The callback acknowledges within one buffer period (even while paused).
        for _ in 0..100 {
            if self.shared.flush_ack.load(Ordering::Acquire) == req {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        self.written = 0;
        let mut tl = self.timeline.lock().unwrap();
        tl.marks.clear();
        tl.index = self.index;
        tl.rate = self.out.as_ref().map_or(48000, |o| o.rate) as u64;
        drop(tl);
        self.ended = false;
        self.shared.ended.store(false, Ordering::Relaxed);
    }

    /// (queue index, position in ms) of what the output is playing now.
    fn position(&self) -> (Option<usize>, u64) {
        let (index, ms, _) = self.timeline.lock().unwrap().at(self.shared.played.load(Ordering::Relaxed));
        (index, ms)
    }

    /// Publishes what only the worker knows; `Player::status` adds the live parts.
    fn publish(&self) {
        let mut st = self.status.lock().unwrap();
        st.shuffle = self.shuffle;
        st.repeat = self.repeat;
        st.error = self.error.clone();
        st.output = self.out.as_ref().map(|o| format!("{} · {} Hz", o.name, o.rate)).unwrap_or_default();
    }

    fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
        self.shared.paused.store(!playing, Ordering::Relaxed);
    }

    /// When the decoder has already moved on to the next track (gapless) while the output still
    /// plays the one before: (the heard track, its position).
    fn ahead(&self) -> Option<(usize, u64)> {
        let (heard, ms) = self.position();
        match (heard, self.index) {
            (Some(h), Some(i)) if h != i => Some((h, ms)),
            _ => None,
        }
    }

    /// After a queue edit while the decoder was ahead: what it decoded may no longer be what
    /// follows, so restart the heard track (now at `heard`) where it is.
    fn follow_heard(&mut self, heard: usize, at_ms: u64) {
        self.index = Some(heard);
        self.restart_at(at_ms);
    }

    fn random(&mut self) -> u64 {
        // xorshift64*: plenty for shuffling a queue.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// The current entry first, then every other entry in a random order.
    fn shuffle_around_current(&mut self) {
        let n = self.keys.len();
        let first = self.index.unwrap_or(0).min(n.saturating_sub(1));
        let mut rest: Vec<usize> = (0..n).filter(|&i| i != first).collect();
        for i in (1..rest.len()).rev() {
            let j = (self.random() % (i as u64 + 1)) as usize;
            rest.swap(i, j);
        }
        let order: Vec<usize> = std::iter::once(first).chain(rest).take(n).collect();
        self.reorder(&order);
    }

    /// Puts entry `order[k]` at position `k`, keeping what plays (index and marks) the same.
    fn reorder(&mut self, order: &[usize]) {
        let mut new_at = vec![0; order.len()];
        for (k, &old) in order.iter().enumerate() {
            new_at[old] = k;
        }
        let mut q = self.queue.lock().unwrap();
        let old_q = std::mem::take(&mut *q);
        let mut slots: Vec<Option<P::Item>> = old_q.into_iter().map(Some).collect();
        *q = order.iter().map(|&i| slots[i].take().expect("a permutation")).collect();
        drop(q);
        self.keys = order.iter().map(|&i| self.keys[i]).collect();
        self.index = self.index.map(|i| new_at[i]);
        let mut tl = self.timeline.lock().unwrap();
        tl.marks.iter_mut().for_each(|m| m.index = new_at[m.index]);
        tl.index = self.index;
    }

    /// Handles a command. `None`: shut down; `Some(true)`: the current item must be reopened.
    fn handle(&mut self, cmd: Cmd<P::Item>) -> Option<bool> {
        let len = self.queue.lock().unwrap().len();
        match cmd {
            Cmd::Shutdown => return None,
            Cmd::SetQueue(items, start, play) => {
                let n = items.len();
                *self.queue.lock().unwrap() = items;
                self.keys = (0..n).map(|i| i as f64).collect();
                self.index = (n > 0).then(|| start.min(n - 1));
                if self.shuffle && n > 0 {
                    self.shuffle_around_current();
                }
                self.set_playing(play && n > 0);
                self.restart_at(0);
            }
            Cmd::Insert(at, items) => {
                let mut q = self.queue.lock().unwrap();
                let at = at.min(q.len());
                let n = items.len();
                q.splice(at..at, items);
                drop(q);
                // Unshuffled, inserted tracks keep their place: right after the entry before
                // them (play next follows the current track), or at the end.
                let before = if at == 0 { -1.0 } else { self.keys[at - 1] };
                let after = self.keys.iter().copied().filter(|k| *k > before).fold(f64::INFINITY, f64::min);
                let step = if after.is_finite() { (after - before) / (n as f64 + 1.0) } else { 1.0 };
                self.keys.splice(at..at, (1..=n).map(|k| before + step * k as f64));
                if let Some(i) = self.index.filter(|&i| at <= i) {
                    self.index = Some(i + n);
                    self.timeline.lock().unwrap().marks.iter_mut().filter(|m| m.index >= at).for_each(|m| m.index += n);
                }
                if self.index.is_none() && n > 0 {
                    self.index = Some(at);
                    self.restart_at(0);
                } else if let Some((heard, ms)) = self.ahead() {
                    self.follow_heard(if at <= heard { heard + n } else { heard }, ms);
                }
            }
            Cmd::Remove(at) => {
                if at >= len {
                    return Some(false);
                }
                let ahead = self.ahead();
                self.queue.lock().unwrap().remove(at);
                self.keys.remove(at);
                if let Some((heard, ms)) = ahead {
                    if heard == at {
                        // The track being heard was removed: play what took its place.
                        self.index = Some(at);
                        if at >= len - 1 {
                            self.index = None;
                            self.set_playing(false);
                        }
                        self.restart_at(0);
                    } else {
                        self.follow_heard(if at < heard { heard - 1 } else { heard }, ms);
                    }
                    return Some(false);
                }
                match self.index {
                    Some(i) if i == at => {
                        // Removing what plays: play what took its place (or stop at the end).
                        if at >= len - 1 {
                            self.index = None;
                            self.set_playing(false);
                            self.restart_at(0);
                        } else {
                            self.restart_at(0);
                        }
                    }
                    Some(i) if i > at => {
                        self.index = Some(i - 1);
                        self.timeline.lock().unwrap().marks.iter_mut().filter(|m| m.index > at).for_each(|m| m.index -= 1);
                    }
                    _ => {}
                }
            }
            Cmd::Move(from, to) => {
                if from >= len || to >= len || from == to {
                    return Some(false);
                }
                let ahead = self.ahead();
                let mut q = self.queue.lock().unwrap();
                let item = q.remove(from);
                q.insert(to, item);
                drop(q);
                let key = self.keys.remove(from);
                self.keys.insert(to, key);
                let remap = |i: usize| {
                    if i == from {
                        to
                    } else if from < i && i <= to {
                        i - 1
                    } else if to <= i && i < from {
                        i + 1
                    } else {
                        i
                    }
                };
                self.index = self.index.map(remap);
                self.timeline.lock().unwrap().marks.iter_mut().for_each(|m| m.index = remap(m.index));
                if let Some((heard, ms)) = ahead {
                    self.follow_heard(remap(heard), ms);
                }
            }
            Cmd::Jump(i) => {
                if i < len {
                    self.index = Some(i);
                    self.set_playing(true);
                    self.restart_at(0);
                }
            }
            Cmd::Play => {
                if self.index.is_none() && len > 0 {
                    self.index = Some(0);
                    self.restart_at(0);
                } else if self.ended && self.current.is_none() && !self.playing {
                    self.index = Some(0);
                    self.restart_at(0);
                }
                self.set_playing(len > 0);
            }
            Cmd::Pause => self.set_playing(false),
            Cmd::Next => {
                let (shown, _) = self.position();
                let mut next = shown.map_or(0, |i| i + 1);
                if next >= len && self.repeat == Repeat::All {
                    next = 0;
                }
                if next < len {
                    self.index = Some(next);
                    self.set_playing(true);
                    self.restart_at(0);
                }
            }
            Cmd::Prev => {
                let (shown, pos) = self.position();
                let i = shown.unwrap_or(0);
                // Past the first seconds, "previous" restarts the track.
                let target = if pos > 3000 {
                    i
                } else if i == 0 {
                    if self.repeat == Repeat::All { len.saturating_sub(1) } else { 0 }
                } else {
                    i - 1
                };
                if target < len {
                    self.index = Some(target);
                    self.restart_at(0);
                }
            }
            Cmd::Seek(ms) => {
                let (shown, _) = self.position();
                if let Some(i) = shown {
                    self.index = Some(i);
                    self.restart_at(ms);
                }
            }
            Cmd::Shuffle(on) => {
                if on != self.shuffle {
                    self.shuffle = on;
                    // The decoder may already be into the next track (gapless); what matters is
                    // the track being heard. If it's ahead, what it decoded follows the old
                    // order, so restart the heard track where it is.
                    let (heard, at_ms) = self.position();
                    let ahead = heard.is_some() && heard != self.index;
                    if ahead {
                        self.index = heard;
                    }
                    if on {
                        self.shuffle_around_current();
                    } else {
                        let mut order: Vec<usize> = (0..self.keys.len()).collect();
                        order.sort_by(|&a, &b| self.keys[a].total_cmp(&self.keys[b]));
                        self.reorder(&order);
                    }
                    if ahead {
                        self.restart_at(at_ms);
                    }
                }
            }
            Cmd::MarkShuffled => self.shuffle = true,
            Cmd::Repeat(r) => self.repeat = r,
            Cmd::Stop => {
                self.set_playing(false);
                self.restart_at(0);
            }
        }
        Some(false)
    }
}

#[cfg(test)]
mod tests;
