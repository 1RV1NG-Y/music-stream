//! Desktop: the phone's library as this PC sees it (`Remote`) and the native player. The UI
//! calls `music(method, params)`; player state goes out as `player` events, library changes
//! as `library-event`. Cover art is served on the `art:` URL scheme.

use std::{
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::AtomicBool,
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use musicstream_core::{
    Node,
    library::{Track, hex, parse_hex},
    remote::{Remote, TrackSource},
};
use player::{MediaSource, Opened, OutputKind, Player, Provider};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum GainMode {
    Off,
    Track,
    #[default]
    Album,
}

/// What survives a restart (PLAN.md §7: the queue comes back, paused).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    volume: f32,
    replay_gain: GainMode,
    queue: Vec<String>,
    index: Option<usize>,
    position_ms: u64,
}

impl Default for Saved {
    fn default() -> Self {
        Saved { volume: 0.8, replay_gain: GainMode::Album, queue: vec![], index: None, position_ms: 0 }
    }
}

struct Tracks {
    remote: Remote,
    gain: Arc<Mutex<GainMode>>,
}

/// A remote track as a symphonia source.
struct Source(TrackSource);

impl Read for Source {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}
impl Seek for Source {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.0.seek(pos)
    }
}
impl MediaSource for Source {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.0.len())
    }
}

impl Provider for Tracks {
    type Item = Track;

    fn open(&self, track: &Track, interrupt: Arc<AtomicBool>) -> Result<Opened> {
        let source = self.remote.open_track(track, interrupt)?;
        Ok(Opened { source: Box::new(Source(source)), ext: track.format.clone() })
    }

    fn gain(&self, t: &Track) -> f32 {
        let rg = &t.replay_gain;
        let (gain, peak) = match *self.gain.lock().unwrap() {
            GainMode::Off => return 1.0,
            GainMode::Track => (rg.track_gain.or(rg.album_gain), rg.track_peak.or(rg.album_peak)),
            GainMode::Album => (rg.album_gain.or(rg.track_gain), rg.album_peak.or(rg.track_peak)),
        };
        let Some(gain) = gain else { return 1.0 };
        let linear = 10f32.powf(gain / 20.0);
        // Never push a track's peak past full scale.
        match peak.filter(|p| *p > 0.0) {
            Some(p) => linear.min(1.0 / p),
            None => linear,
        }
    }

    fn prefetch(&self, track: &Track) {
        self.remote.prefetch(track);
    }
}

pub struct Music {
    remote: Remote,
    player: Player<Tracks>,
    gain: Arc<Mutex<GainMode>>,
    saved_path: PathBuf,
    saved: Mutex<Saved>,
}

/// Opens the library view and the player; must run inside the Tauri async runtime.
pub fn setup(app: &AppHandle, node: Node, dir: PathBuf) -> Result<()> {
    let remote = Remote::open(node, dir.join("remote"))?;
    let saved_path = dir.join("player.json");
    let saved: Saved = std::fs::read(&saved_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let gain = Arc::new(Mutex::new(saved.replay_gain));
    let (player, _) = Player::new(Tracks { remote: remote.clone(), gain: gain.clone() }, OutputKind::Device).context("Couldn't open the audio output")?;
    player.set_volume(curve(saved.volume));
    // Bring back the last queue, paused where it was.
    let index = remote.index();
    let queue: Vec<Track> = saved.queue.iter().filter_map(|id| parse_hex::<16>(id).ok()).filter_map(|id| index.resolve(&id).cloned()).collect();
    if let Some(i) = saved.index.filter(|&i| i < queue.len() && queue.len() == saved.queue.len()) {
        player.set_queue(queue, i, false);
        if saved.position_ms > 0 {
            player.seek(saved.position_ms);
        }
    }
    app.manage(Music { remote: remote.clone(), player, gain, saved_path, saved: Mutex::new(saved) });

    // Library changes → UI.
    let handle = app.clone();
    let mut events = remote.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(e) = events.recv().await {
            let _ = handle.emit("library-event", e);
        }
    });
    // Player state → UI (when it changes), and saved for the next start.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last = Value::Null;
        let mut ticks = 0u32;
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let music = handle.state::<Music>();
            let state = player_state(&music);
            if state != last {
                let _ = handle.emit("player", &state);
                last = state;
            }
            ticks += 1;
            if ticks % 25 == 0 {
                music.save_queue();
            }
        }
    });
    Ok(())
}

/// Slider position (0–1) → linear gain: a cubic curve, so the slider feels even.
fn curve(v: f32) -> f32 {
    v.clamp(0.0, 1.0).powi(3)
}

impl Music {
    fn save(&self) {
        let saved = self.saved.lock().unwrap().clone();
        if let Ok(bytes) = serde_json::to_vec_pretty(&saved) {
            let _ = std::fs::write(&self.saved_path, bytes);
        }
    }

    fn save_queue(&self) {
        let st = self.player.status();
        let queue: Vec<String> = self.player.queue().iter().map(|t| hex(&t.id)).collect();
        let mut saved = self.saved.lock().unwrap();
        if saved.queue == queue && saved.index == st.index && saved.position_ms / 1000 == st.position_ms / 1000 {
            return;
        }
        saved.queue = queue;
        saved.index = st.index;
        saved.position_ms = st.position_ms;
        drop(saved);
        self.save();
    }

    fn tracks(&self, ids: &[String]) -> Result<Vec<Track>> {
        let index = self.remote.index();
        ids.iter()
            .map(|id| {
                let id = parse_hex::<16>(id)?;
                index.resolve(&id).cloned().context("That track is no longer in the library.")
            })
            .collect()
    }
}

fn track_view(t: &Track, cached: bool) -> Value {
    json!({
        "id": hex(&t.id),
        "title": t.title,
        "artist": t.artist,
        "album": t.album,
        "albumArtist": t.album_artist,
        "disc": t.disc,
        "track": t.track,
        "year": t.year,
        "genre": t.genre,
        "durationMs": t.duration_ms,
        "format": t.format,
        "sampleRate": t.sample_rate,
        "bitDepth": t.bit_depth,
        "channels": t.channels,
        "size": t.size(),
        "path": t.path,
        "art": t.art.map(|a| hex(&a)),
        "cached": cached,
    })
}

fn player_state(music: &Music) -> Value {
    let st = music.player.status();
    let queue: Vec<String> = music.player.queue().iter().map(|t| hex(&t.id)).collect();
    json!({
        "index": st.index,
        "playing": st.playing,
        "positionMs": st.position_ms,
        "durationMs": st.duration_ms,
        "buffering": st.buffering,
        "error": st.error,
        "output": st.output,
        "volume": music.saved.lock().unwrap().volume,
        "replayGain": *music.gain.lock().unwrap(),
        "queue": queue,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ids {
    ids: Vec<String>,
    #[serde(default)]
    start: usize,
    #[serde(default)]
    next: bool,
}

#[derive(Deserialize)]
struct Index {
    index: usize,
}

#[derive(Deserialize)]
struct Move {
    from: usize,
    to: usize,
}

#[derive(Deserialize)]
struct Ms {
    ms: u64,
}

#[derive(Deserialize)]
struct Level {
    volume: f32,
}

#[derive(Deserialize)]
struct Mode {
    mode: GainMode,
}

pub async fn dispatch(app: &AppHandle, method: &str, p: Value) -> Result<Value> {
    let music = app.state::<Music>();
    let player = &music.player;
    Ok(match method {
        "library" => {
            let index = music.remote.index();
            let cached = music.remote.cached();
            let tracks: Vec<Value> = index.tracks.iter().map(|t| track_view(t, cached.contains(&(t.id, t.rev)))).collect();
            json!({ "tracks": tracks, "generation": index.generation })
        }
        "remoteStatus" => serde_json::to_value(music.remote.status())?,
        "refresh" => {
            music.remote.refresh();
            Value::Null
        }
        "state" => player_state(&music),
        "play" => {
            let p: Ids = serde_json::from_value(p)?;
            let tracks = music.tracks(&p.ids)?;
            if tracks.is_empty() {
                bail!("Nothing to play.");
            }
            player.set_queue(tracks, p.start, true);
            Value::Null
        }
        "enqueue" => {
            let p: Ids = serde_json::from_value(p)?;
            let tracks = music.tracks(&p.ids)?;
            let st = player.status();
            let len = player.queue().len();
            let at = if p.next { st.index.map_or(len, |i| i + 1) } else { len };
            player.insert(at, tracks);
            Value::Null
        }
        "remove" => {
            let p: Index = serde_json::from_value(p)?;
            player.remove(p.index);
            Value::Null
        }
        "move" => {
            let p: Move = serde_json::from_value(p)?;
            player.move_item(p.from, p.to);
            Value::Null
        }
        "jump" => {
            let p: Index = serde_json::from_value(p)?;
            player.jump(p.index);
            Value::Null
        }
        "clear" => {
            player.set_queue(vec![], 0, false);
            Value::Null
        }
        "toggle" => {
            player.toggle();
            Value::Null
        }
        "next" => {
            player.next();
            Value::Null
        }
        "prev" => {
            player.prev();
            Value::Null
        }
        "seek" => {
            let p: Ms = serde_json::from_value(p)?;
            player.seek(p.ms);
            Value::Null
        }
        "setVolume" => {
            let p: Level = serde_json::from_value(p)?;
            let v = p.volume.clamp(0.0, 1.0);
            player.set_volume(curve(v));
            music.saved.lock().unwrap().volume = v;
            music.save();
            Value::Null
        }
        "setReplayGain" => {
            let p: Mode = serde_json::from_value(p)?;
            *music.gain.lock().unwrap() = p.mode;
            music.saved.lock().unwrap().replay_gain = p.mode;
            music.save();
            Value::Null
        }
        other => bail!("unknown method {other}"),
    })
}

/// `art://localhost/<hash>` (Linux) or `http://art.localhost/<hash>` (Windows).
pub fn art_protocol<R: tauri::Runtime>(ctx: tauri::UriSchemeContext<'_, R>, request: tauri::http::Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    let path = request.uri().path().trim_start_matches('/').to_owned();
    tauri::async_runtime::spawn(async move {
        let not_found = || tauri::http::Response::builder().status(404).body(Vec::new()).unwrap();
        let Some(music) = app.try_state::<Music>() else { return responder.respond(not_found()) };
        let Ok(hash) = parse_hex::<32>(&path) else { return responder.respond(not_found()) };
        match music.remote.art(hash).await {
            Ok(Some(bytes)) => responder.respond(
                tauri::http::Response::builder()
                    .header("Content-Type", "image/jpeg")
                    .header("Cache-Control", "max-age=31536000, immutable")
                    .body(bytes)
                    .unwrap(),
            ),
            _ => responder.respond(not_found()),
        }
    });
}
