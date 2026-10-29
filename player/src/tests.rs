//! Rendering to a buffer instead of a device (PLAN.md §8, Track C).

use std::{
    f32::consts::TAU,
    io::Cursor,
    time::{Duration, Instant},
};

use super::*;

/// 16-bit PCM WAV of `frames` frames, sample = `f(frame, channel)`.
fn wav(rate: u32, channels: u16, frames: usize, f: impl Fn(usize, usize) -> f32) -> Vec<u8> {
    let data_len = frames * channels as usize * 2;
    let mut w = Vec::with_capacity(44 + data_len);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    w.extend_from_slice(&(channels * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(data_len as u32).to_le_bytes());
    for i in 0..frames {
        for c in 0..channels as usize {
            w.extend_from_slice(&q(f(i, c)).to_le_bytes());
        }
    }
    w
}

fn q(x: f32) -> i16 {
    (x * 32767.0).round() as i16
}

fn sine(freq: f32, rate: u32) -> impl Fn(usize, usize) -> f32 {
    move |i, _| 0.5 * (TAU * freq * i as f32 / rate as f32).sin()
}

/// Blocks reads until the player interrupts them, like a phone that went away.
struct Stalled(Arc<AtomicBool>);

impl std::io::Read for Stalled {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        while !self.0.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(std::io::Error::other("interrupted"))
    }
}
impl std::io::Seek for Stalled {
    fn seek(&mut self, _: std::io::SeekFrom) -> std::io::Result<u64> {
        Ok(0)
    }
}
impl MediaSource for Stalled {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// Gives `bytes` up to `cut`, then stalls like a phone that went away mid-track.
struct StallsAfter {
    bytes: Vec<u8>,
    cut: usize,
    pos: usize,
    interrupt: Arc<AtomicBool>,
}

impl std::io::Read for StallsAfter {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.cut {
            return Stalled(self.interrupt.clone()).read(buf);
        }
        let n = buf.len().min(self.cut - self.pos);
        buf[..n].copy_from_slice(&self.bytes[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}
impl std::io::Seek for StallsAfter {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        if let std::io::SeekFrom::Start(p) = to {
            self.pos = p as usize;
        }
        Ok(self.pos as u64)
    }
}
impl MediaSource for StallsAfter {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.bytes.len() as u64)
    }
}

/// `None` entries stall; `cut` makes an entry stall after that many bytes; `delay` slows
/// every open after the first, like fetching the start of the next track from the phone.
struct Tracks(Vec<Option<Vec<u8>>>);

struct Slow {
    tracks: Vec<Vec<u8>>,
    delay: Duration,
    cut: Option<usize>,
    opened: std::sync::atomic::AtomicUsize,
}

impl Provider for Tracks {
    type Item = usize;
    fn open(&self, item: &usize, interrupt: Arc<AtomicBool>) -> Result<Opened> {
        let source: Box<dyn MediaSource> = match &self.0[*item] {
            Some(bytes) => Box::new(Cursor::new(bytes.clone())),
            None => Box::new(Stalled(interrupt)),
        };
        Ok(Opened { source, ext: "wav".into() })
    }
}

impl Provider for Slow {
    type Item = usize;
    fn open(&self, item: &usize, interrupt: Arc<AtomicBool>) -> Result<Opened> {
        if self.opened.fetch_add(1, Ordering::Relaxed) > 0 {
            std::thread::sleep(self.delay);
        }
        let bytes = self.tracks[*item].clone();
        let source: Box<dyn MediaSource> = match self.cut {
            Some(cut) => Box::new(StallsAfter { cut: cut.min(bytes.len()), bytes, pos: 0, interrupt }),
            None => Box::new(Cursor::new(bytes)),
        };
        Ok(Opened { source, ext: "wav".into() })
    }
}

fn manual(tracks: Vec<Option<Vec<u8>>>, rate: u32, channels: u16) -> (Player<Tracks>, ManualSink) {
    let (p, sink) = Player::new(Tracks(tracks), OutputKind::Manual { rate, channels }).unwrap();
    (p, sink.unwrap())
}

/// Pulls `frames` frames of real audio, waiting for the worker to produce them.
fn pull(sink: &mut ManualSink, frames: usize) -> Vec<f32> {
    let want = frames * sink.channels as usize;
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(20);
    while out.len() < want {
        assert!(Instant::now() < deadline, "only {} of {want} samples arrived", out.len());
        let got = sink.pull(1024.min((want - out.len()) / sink.channels as usize));
        if got.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
        out.extend(got);
    }
    out
}

#[test]
fn gapless_is_sample_exact_at_the_native_rate() {
    let rate = 44100;
    let total = 3 * rate as usize;
    let cuts = [0, 50_000, 99_999, total];
    let f = sine(440.0, rate);
    let tracks = (0..3).map(|k| Some(wav(rate, 2, cuts[k + 1] - cuts[k], |i, c| f(i + cuts[k], c)))).collect();
    let (p, mut sink) = manual(tracks, rate, 2);
    p.set_queue(vec![0, 1, 2], 0, true);
    let out = pull(&mut sink, total);
    for (i, s) in out.iter().enumerate() {
        let expected = q(f(i / 2, 0)) as f32 / 32768.0;
        assert!((s - expected).abs() < 1e-4, "sample {i}: {s} vs {expected}");
    }
}

#[test]
fn gapless_stays_continuous_through_the_resampler() {
    let rate = 44100;
    let total = 3 * rate as usize;
    let cuts = [0, 50_000, 99_999, total];
    let f = sine(440.0, rate);
    let tracks = (0..3).map(|k| Some(wav(rate, 2, cuts[k + 1] - cuts[k], |i, c| f(i + cuts[k], c)))).collect();
    let (p, mut sink) = manual(tracks, 48000, 2);
    p.set_queue(vec![0, 1, 2], 0, true);
    let frames = 3 * 48000 - 2000;
    let out = pull(&mut sink, frames);
    // A 440 Hz sine at 0.5 moves at most 0.029 per 48 kHz sample; a gap or click jumps further.
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let worst = left.windows(2).skip(100).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(worst < 0.04, "discontinuity of {worst}");
    let peak = left.iter().skip(1000).fold(0.0f32, |a, s| a.max(s.abs()));
    assert!((peak - 0.5).abs() < 0.01, "level kept: {peak}");
}

#[test]
fn a_192k_mono_file_plays_at_the_right_pitch_on_a_48k_stereo_output() {
    let tracks = vec![Some(wav(192_000, 1, 192_000, sine(1000.0, 192_000)))];
    let (p, mut sink) = manual(tracks, 48000, 2);
    p.set_queue(vec![0], 0, true);
    let out = pull(&mut sink, 46_000);
    let (l, r): (Vec<f32>, Vec<f32>) = out.chunks(2).map(|f| (f[0], f[1])).unzip();
    assert_eq!(l, r, "mono goes to both sides");
    let crossings = l.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    // 46,000 frames at 48 kHz is 0.958 s: about 958 cycles of 1 kHz.
    assert!((950..=966).contains(&crossings), "{crossings} cycles");
    // The whole second arrives, and no more.
    std::thread::sleep(Duration::from_millis(300));
    let rest = sink.pull(10_000).len() / 2;
    assert!((1900..=2100).contains(&rest), "{rest} frames after the first 46,000");
}

#[test]
fn channels_map_and_downmix() {
    assert_eq!(map_channels(&[0.5, -0.5], 1, 2), vec![0.5, 0.5, -0.5, -0.5]);
    assert_eq!(map_channels(&[0.2, 0.4], 2, 1), vec![0.3]);
    let surround = [1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let st = map_channels(&surround, 6, 2);
    assert!(st[0] > 0.0 && st[1] == 0.0 && st[0] <= 1.0);
    let all = map_channels(&[1.0; 6], 6, 2);
    assert!(all.iter().all(|s| *s <= 1.0 + 1e-6), "no clipping: {all:?}");
}

#[test]
fn seeking_lands_on_the_sample() {
    let rate = 44100;
    let f = sine(440.0, rate);
    let (p, mut sink) = manual(vec![Some(wav(rate, 2, 2 * rate as usize, &f))], rate, 2);
    p.set_queue(vec![0], 0, true);
    pull(&mut sink, 2000);
    p.seek(1000);
    // Keep the callback running so the flush is acknowledged, then read from the new place.
    let deadline = Instant::now() + Duration::from_secs(5);
    while p.status().position_ms < 1000 {
        assert!(Instant::now() < deadline, "seek not reflected: {:?}", p.status());
        sink.pull(64);
        std::thread::sleep(Duration::from_millis(1));
    }
    let st = p.status();
    assert!(st.position_ms >= 1000 && st.position_ms < 1100, "{st:?}");
}

#[test]
fn skipping_away_from_a_stalled_track_plays_the_next_at_once() {
    let rate = 44100;
    let (p, mut sink) = manual(vec![None, Some(wav(rate, 2, rate as usize, sine(440.0, rate)))], rate, 2);
    p.set_queue(vec![0, 1], 0, true);
    std::thread::sleep(Duration::from_millis(300));
    assert!(sink.pull(512).is_empty(), "the stalled track gives nothing");
    let asked = Instant::now();
    p.next();
    loop {
        if !sink.pull(512).is_empty() {
            break;
        }
        assert!(asked.elapsed() < Duration::from_millis(200), "next track not playing within 200 ms");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(p.status().index, Some(1));
    // Shutting down doesn't hang on a stalled read either.
    p.set_queue(vec![0], 0, true);
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    drop(p);
    assert!(t.elapsed() < Duration::from_millis(200));
}

/// Pulls 10 ms at a time on a real-time clock, like a sound card, silence included.
fn render_realtime(sink: &mut ManualSink, frames: usize) -> (Vec<f32>, usize) {
    let chunk = sink.rate as usize / 100;
    let mut out = Vec::with_capacity(frames * sink.channels as usize);
    let mut silent_chunks = 0;
    let start = Instant::now();
    let mut k = 0u32;
    while out.len() < frames * sink.channels as usize {
        let (samples, real) = sink.render(chunk);
        if real < samples.len() {
            silent_chunks += 1;
        }
        out.extend(samples);
        k += 1;
        let due = start + Duration::from_millis(10) * k;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
    }
    out.truncate(frames * sink.channels as usize);
    (out, silent_chunks)
}

#[test]
fn gapless_in_real_time_even_when_the_next_track_is_slow_to_open() {
    let rate = 44100;
    let total = 3 * rate as usize;
    let cuts = [0, 50_000, 99_999, total];
    let f = sine(440.0, rate);
    let tracks = (0..3).map(|k| wav(rate, 2, cuts[k + 1] - cuts[k], |i, c| f(i + cuts[k], c))).collect();
    // Each later track takes 300 ms to open, as fetching its start from the phone might.
    let provider = Slow { tracks, delay: Duration::from_millis(300), cut: None, opened: Default::default() };
    let (p, sink) = Player::new(provider, OutputKind::Manual { rate, channels: 2 }).unwrap();
    let mut sink = sink.unwrap();
    p.set_queue(vec![0, 1, 2], 0, true);
    // Startup may be silent; from the first audio on, the output must never miss a sample.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut first = loop {
        let (s, real) = sink.render(441);
        if real > 0 {
            assert_eq!(real, s.len(), "audio starts with a full buffer");
            break s;
        }
        assert!(Instant::now() < deadline, "no audio");
        std::thread::sleep(Duration::from_millis(2));
    };
    let (rest, silent) = render_realtime(&mut sink, total - 441 - 2000);
    first.extend(rest);
    assert_eq!(silent, 0, "{silent} chunks of 10 ms had gaps");
    for (i, s) in first.iter().enumerate() {
        let expected = q(f(i / 2, 0)) as f32 / 32768.0;
        assert!((s - expected).abs() < 1e-4, "sample {i}: {s} vs {expected}");
    }
}

#[test]
fn pause_and_status_respond_while_the_phone_is_unreachable() {
    let rate = 44100;
    // Half a second of audio arrives, then the "phone" stops answering.
    let track = wav(rate, 2, 4 * rate as usize, sine(440.0, rate));
    let provider = Slow { tracks: vec![track], delay: Duration::ZERO, cut: Some(44 + rate as usize * 2 * 2 / 2), opened: Default::default() };
    let (p, sink) = Player::new(provider, OutputKind::Manual { rate, channels: 2 }).unwrap();
    let mut sink = sink.unwrap();
    p.set_queue(vec![0], 0, true);
    let got = pull(&mut sink, 4410);
    assert!(!got.is_empty());
    std::thread::sleep(Duration::from_millis(200)); // the worker is now blocked on the read

    let t = Instant::now();
    p.pause();
    assert!(!p.status().playing, "status shows the pause at once");
    let (_, real) = sink.render(441);
    assert_eq!(real, 0, "the output goes quiet at once");
    assert!(t.elapsed() < Duration::from_millis(50));

    // Resuming plays what was buffered, then reports buffering instead of going quiet silently.
    p.play();
    assert!(p.status().playing);
    let before = p.status().position_ms;
    let (_, real) = sink.render(4410);
    assert!(real > 0, "buffered audio plays");
    assert!(p.status().position_ms > before, "position moves while the worker is blocked");
    while sink.render(4410).1 > 0 {}
    sink.render(441);
    let st = p.status();
    assert!(st.playing && st.buffering, "waiting for the phone shows as buffering: {st:?}");
}
