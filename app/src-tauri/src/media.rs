//! Media keys and the system's now-playing panel (PLAN.md §6–§7): MPRIS on Linux, SMTC on
//! Windows, through souvlaki. The controls live on a thread of their own, which also keeps
//! the panel's track, cover and position current.

use std::time::Duration;

use musicstream_core::library::hex;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
use tauri::{AppHandle, Manager};

use crate::desktop::{self, Music};

const SEEK_STEP: Duration = Duration::from_secs(10);

pub fn start(app: &AppHandle) {
    // Windows' controls belong to a window; a raw pointer can't cross threads, so pass its value.
    #[cfg(target_os = "windows")]
    let hwnd: Option<usize> = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map(|h| h.0 as usize);
    #[cfg(not(target_os = "windows"))]
    let hwnd: Option<usize> = None;
    let app = app.clone();
    let _ = std::thread::Builder::new().name("media-controls".into()).spawn(move || {
        let config = PlatformConfig { display_name: "music-stream", dbus_name: "music_stream", hwnd: hwnd.map(|h| h as *mut std::ffi::c_void) };
        let mut controls = match MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => return tracing::warn!("media keys unavailable: {e:?}"),
        };
        let handle = app.clone();
        if let Err(e) = controls.attach(move |event| handle_event(&handle, event)) {
            return tracing::warn!("media keys unavailable: {e:?}");
        }
        let mut shown: Option<(String, Option<String>)> = None;
        let mut state: Option<(bool, u64)> = None;
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let Some(music) = app.try_state::<Music>() else { continue };
            let st = music.player.status();
            let queue = music.player.queue();
            let track = st.index.and_then(|i| queue.get(i));
            let key = track.map(|t| (hex(&t.id), t.art.map(|a| hex(&a))));
            // The cover may only arrive in the cache a moment after the track starts.
            let cover = track.and_then(|t| t.art).and_then(|a| music.remote.art_file(&a)).map(|p| format!("file://{}", p.display()));
            let key = key.map(|(id, _)| (id, cover.clone()));
            if key != shown {
                let meta = match track {
                    Some(t) => MediaMetadata {
                        title: Some(&t.title),
                        artist: Some(&t.artist),
                        album: Some(&t.album),
                        cover_url: cover.as_deref(),
                        duration: (t.duration_ms > 0).then(|| Duration::from_millis(t.duration_ms)),
                    },
                    None => MediaMetadata::default(),
                };
                let _ = controls.set_metadata(meta);
                shown = key;
            }
            // Position updates every few seconds are enough; the panel extrapolates.
            let now = (st.playing, st.position_ms / 5000);
            if state != Some(now) {
                let progress = Some(MediaPosition(Duration::from_millis(st.position_ms)));
                let playback = match (track.is_some(), st.playing) {
                    (false, _) => MediaPlayback::Stopped,
                    (true, true) => MediaPlayback::Playing { progress },
                    (true, false) => MediaPlayback::Paused { progress },
                };
                let _ = controls.set_playback(playback);
                state = Some(now);
            }
        }
    });
}

fn handle_event(app: &AppHandle, event: MediaControlEvent) {
    let Some(music) = app.try_state::<Music>() else { return };
    let player = &music.player;
    let seek_by = |forward: bool, by: Duration| {
        let at = player.status().position_ms as i64 + if forward { by.as_millis() as i64 } else { -(by.as_millis() as i64) };
        player.seek(at.max(0) as u64);
    };
    match event {
        MediaControlEvent::Play => player.play(),
        MediaControlEvent::Pause | MediaControlEvent::Stop => player.pause(),
        MediaControlEvent::Toggle => player.toggle(),
        MediaControlEvent::Next => player.next(),
        MediaControlEvent::Previous => player.prev(),
        MediaControlEvent::Seek(dir) => seek_by(dir == SeekDirection::Forward, SEEK_STEP),
        MediaControlEvent::SeekBy(dir, by) => seek_by(dir == SeekDirection::Forward, by),
        MediaControlEvent::SetPosition(MediaPosition(at)) => player.seek(at.as_millis() as u64),
        MediaControlEvent::SetVolume(v) => desktop::set_volume(&music, (v as f32).cbrt()),
        MediaControlEvent::SetShuffle(on) => desktop::set_shuffle(&music, on),
        MediaControlEvent::Raise => crate::background::open_window(app),
        MediaControlEvent::Quit => crate::background::quit(app),
        _ => {}
    }
}
