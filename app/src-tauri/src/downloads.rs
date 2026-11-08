//! Saving songs from the phone onto this PC: one at a time, in the background, into
//! `<Music folder>/music-stream/<artist>/<album>/<file name>`. The list goes out to the UI as
//! `downloads` events.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use musicstream_core::{
    library::{Track, hex, parse_hex},
    remote::Remote,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum JobState {
    Queued,
    Saving,
    Saved,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: String,
    title: String,
    artist: String,
    state: JobState,
    done: u64,
    total: u64,
    path: Option<PathBuf>,
    error: Option<String>,
    #[serde(skip)]
    track: Track,
}

pub struct Downloads {
    remote: Remote,
    folder: PathBuf,
    jobs: Mutex<Vec<Job>>,
    /// Cancels the song being saved.
    cancel: Arc<AtomicBool>,
    wake: Notify,
}

pub fn setup(app: &AppHandle, remote: Remote) {
    let music = app.path().audio_dir().or_else(|_| app.path().home_dir().map(|h| h.join("Music"))).unwrap_or_else(|_| PathBuf::from("Music"));
    app.manage(Downloads { remote, folder: music.join("music-stream"), jobs: Mutex::default(), cancel: Arc::default(), wake: Notify::new() });
    let handle = app.clone();
    tauri::async_runtime::spawn(async move { worker(handle).await });
}

fn state(d: &Downloads) -> Value {
    json!({ "folder": d.folder, "jobs": *d.jobs.lock().unwrap() })
}

fn emit(app: &AppHandle) {
    let _ = app.emit("downloads", state(&app.state::<Downloads>()));
}

#[derive(Deserialize)]
struct Ids {
    ids: Vec<String>,
}

#[derive(Deserialize)]
struct PathParam {
    path: Option<PathBuf>,
}

/// The `music` commands for downloads, or `None` when `method` isn't one.
pub async fn dispatch(app: &AppHandle, method: &str, p: &Value) -> Option<Result<Value>> {
    let d = app.state::<Downloads>();
    let result = match method {
        "downloads" => Ok(state(&d)),
        "download" => (|| {
            let Ids { ids } = serde_json::from_value(p.clone())?;
            let index = d.remote.index();
            let mut jobs = d.jobs.lock().unwrap();
            for id in ids {
                let track = index.resolve(&parse_hex::<16>(&id)?).cloned().context("That song is no longer in the library.")?;
                let id = hex(&track.id);
                // Asking again retries a failed one; one already waiting or saved is left alone.
                jobs.retain(|j| !(j.id == id && j.state == JobState::Failed));
                if jobs.iter().any(|j| j.id == id && j.state != JobState::Saved) {
                    continue;
                }
                jobs.retain(|j| j.id != id);
                jobs.push(Job { id, title: track.title.clone(), artist: track.artist.clone(), state: JobState::Queued, done: 0, total: track.size(), path: None, error: None, track });
            }
            drop(jobs);
            d.wake.notify_one();
            Ok(Value::Null)
        })(),
        "cancelDownloads" => {
            d.jobs.lock().unwrap().retain(|j| j.state != JobState::Queued);
            d.cancel.store(true, Ordering::Release);
            Ok(Value::Null)
        }
        "clearDownloads" => {
            d.jobs.lock().unwrap().retain(|j| matches!(j.state, JobState::Queued | JobState::Saving));
            Ok(Value::Null)
        }
        "showDownload" => (|| {
            let PathParam { path } = serde_json::from_value(p.clone())?;
            reveal(path.as_deref().unwrap_or(&d.folder))
        })()
        .map(|()| Value::Null),
        _ => return None,
    };
    if method != "downloads" && method != "showDownload" {
        emit(app);
    }
    Some(result)
}

async fn worker(app: AppHandle) {
    loop {
        let d = app.state::<Downloads>();
        let next = {
            let mut jobs = d.jobs.lock().unwrap();
            jobs.iter_mut().find(|j| j.state == JobState::Queued).map(|j| {
                j.state = JobState::Saving;
                j.clone()
            })
        };
        let Some(job) = next else {
            d.wake.notified().await;
            continue;
        };
        emit(&app);
        d.cancel.store(false, Ordering::Release);
        let (remote, folder, cancel, handle) = (d.remote.clone(), d.folder.clone(), d.cancel.clone(), app.clone());
        let id = job.id.clone();
        let result = tauri::async_runtime::spawn_blocking(move || save(&remote, &folder, &job.track, &cancel, &handle, &job.id)).await;
        let result = result.map_err(anyhow::Error::from).and_then(|r| r);
        {
            let mut jobs = d.jobs.lock().unwrap();
            if let Some(j) = jobs.iter_mut().find(|j| j.id == id) {
                match result {
                    Ok(path) => {
                        j.state = JobState::Saved;
                        j.done = j.total;
                        j.path = Some(path);
                    }
                    Err(e) => {
                        tracing::warn!("couldn't save {}: {e:#}", j.title);
                        j.state = JobState::Failed;
                        j.error = Some(format!("{e:#}"));
                    }
                }
            }
        }
        emit(&app);
    }
}

/// Saves one track; returns where. A file already there with the same size counts as saved.
fn save(remote: &Remote, folder: &Path, track: &Track, cancel: &AtomicBool, app: &AppHandle, id: &str) -> Result<PathBuf> {
    let file_name = track.path.rsplit('/').next().unwrap_or(&track.path);
    let artist = if track.album_artist.is_empty() { &track.artist } else { &track.album_artist };
    let dir = folder.join(safe_name(artist, "Unknown artist")).join(safe_name(&track.album, "Unknown album"));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut dest = dir.join(safe_name(file_name, "song"));
    let mut n = 2;
    while let Ok(meta) = std::fs::metadata(&dest) {
        if meta.len() == track.size() {
            return Ok(dest);
        }
        let stem = Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or("song");
        dest = dir.join(safe_name(&format!("{stem} ({n}).{}", track.format), "song"));
        n += 1;
    }
    let last = Mutex::new(Instant::now());
    remote.save_track(track, &dest, cancel, |done| {
        let d = app.state::<Downloads>();
        if let Some(j) = d.jobs.lock().unwrap().iter_mut().find(|j| j.id == id) {
            j.done = done;
        }
        let mut last = last.lock().unwrap();
        if last.elapsed() > Duration::from_millis(250) {
            *last = Instant::now();
            emit(app);
        }
    })?;
    Ok(dest)
}

/// A file or folder name that's valid on Windows and Linux.
fn safe_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() { fallback.to_owned() } else { cleaned }
}

/// Opens the file manager at a saved song (or the folder).
fn reveal(path: &Path) -> Result<()> {
    #[cfg(target_os = "windows")]
    let status = if path.is_file() {
        std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
    } else {
        std::process::Command::new("explorer").arg(path).spawn()
    };
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let status = {
        let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
        std::fs::create_dir_all(dir).ok();
        std::process::Command::new("xdg-open").arg(dir).spawn()
    };
    status.map(|_| ()).context("Couldn't open the file manager.")
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_are_safe_on_windows() {
        assert_eq!(super::safe_name("AC/DC: Live?", "x"), "AC_DC_ Live_");
        assert_eq!(super::safe_name("  Album...  ", "x"), "Album");
        assert_eq!(super::safe_name("", "Unknown album"), "Unknown album");
    }
}
