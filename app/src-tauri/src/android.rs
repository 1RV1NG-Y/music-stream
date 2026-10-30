//! The Android service and Tauri activity share one node and one Tokio runtime. JNI works
//! without constructing an activity/WebView, including a START_STICKY process recreation.
use std::{path::PathBuf, sync::Mutex};
use jni::{JNIEnv, objects::{JObject, JString}, sys::jboolean};
use musicstream_core::{Node, NodeConfig};
use tauri::{Emitter, Manager, plugin::{Builder, PluginHandle, TauriPlugin}};

static NODE: super::node_slot::NodeSlot = super::node_slot::NodeSlot::new();
static UI: Mutex<Option<tauri::AppHandle>> = Mutex::new(None);
pub struct Controls(pub PluginHandle<tauri::Wry>);

pub struct PlayerHandle(pub PluginHandle<tauri::Wry>);

pub fn plugin() -> TauriPlugin<tauri::Wry> {
    Builder::new("lan-sync")
        .setup(|app, api| {
            app.manage(Controls(api.register_android_plugin("app.musicstream", "LanSyncPlugin")?));
            app.manage(PlayerHandle(api.register_android_plugin("app.musicstream", "PlayerPlugin")?));
            Ok(())
        }).build()
}

/// The phone's `music` commands: the shared library and user-data ones, the serving status,
/// and the player (PlayerPlugin.kt), which gets each queued track's file and cover.
pub async fn dispatch(app: &tauri::AppHandle, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    use anyhow::Context;
    use serde_json::{Value, json};
    let node = node().map_err(anyhow::Error::msg)?;
    match method {
        "phoneStatus" => return Ok(serde_json::to_value(node.library_status())?),
        "rescan" => return Ok(serde_json::to_value(node.rescan().await?)?),
        _ => {}
    }
    let library = node.served_library().context("The library isn't open yet.")?;
    let index = library.index();
    if let Some(result) = crate::userdata::dispatch(&node, &index, |_| true, method, params.clone()).await {
        return result;
    }
    let items = |ids: &Value| -> anyhow::Result<Vec<Value>> {
        let ids: Vec<String> = serde_json::from_value(ids.clone()).unwrap_or_default();
        Ok(crate::userdata::tracks(&index, &ids)?
            .iter()
            .filter_map(|t| {
                let path = library.file_path(t)?;
                Some(json!({
                    "id": musicstream_core::library::hex(&t.id),
                    "path": path,
                    "title": t.title,
                    "artist": t.artist,
                    "album": t.album,
                    "artwork": t.art.map(|a| library.art_path(&a)).filter(|p| p.exists()),
                }))
            })
            .collect())
    };
    let (command, args) = match method {
        "play" => {
            let list = items(&params["ids"])?;
            anyhow::ensure!(!list.is_empty(), "Nothing to play.");
            let shuffle = params["shuffle"].as_bool().unwrap_or(false);
            let start = match params["start"].as_u64() {
                Some(s) => s as usize,
                None if shuffle => (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos()) as usize) % list.len(),
                None => 0,
            };
            let mut args = json!({ "items": list, "start": start, "play": true });
            if shuffle {
                args["shuffle"] = json!(true);
            }
            ("setQueue", args)
        }
        "enqueue" => ("insert", json!({ "items": items(&params["ids"])?, "next": params["next"].as_bool().unwrap_or(false) })),
        "toggle" | "next" | "prev" | "seek" | "setShuffle" | "setRepeat" | "remove" | "move" | "jump" | "clear" | "state" | "pause" => (method, params),
        "play1" => ("play", Value::Null),
        other => anyhow::bail!("unknown method {other}"),
    };
    let handle = app.state::<PlayerHandle>();
    handle.0.run_mobile_plugin_async::<Value>(command, args).await.map_err(|e| anyhow::anyhow!("{e}"))
}

pub fn node() -> Result<Node, String> {
    NODE.node()
}

fn start(path: PathBuf) -> Result<Node, String> {
    // Serialize open/shutdown across both JNI and Tauri setup, including activity recreation.
    super::init_logging();
    let (node, created) = NODE.start(NodeConfig {
        data_dir: path.clone(), device_name: super::device_name(), platform: "android".into(),
        port: musicstream_core::DEFAULT_PORT, mdns: true,
    })?;
    if created {
        // The phone serves its shared storage (PLAN.md §3; U1 walks the folders directly).
        let music_dir = path.parent().map(|p| p.join("music")).unwrap_or_else(|| path.join("music"));
        match musicstream_core::library::Library::open(music_dir, vec![musicstream_core::library::Root {
            volume: "external_primary".into(),
            path: PathBuf::from("/storage/emulated/0"),
        }]) {
            Ok(library) => {
                let n = node.clone();
                tauri::async_runtime::spawn(async move { n.serve_library(std::sync::Arc::new(library)) });
            }
            Err(e) => tracing::error!("the music library could not open: {e:#}"),
        }
        let mut events = node.subscribe();
        tauri::async_runtime::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => musicstream_core::Event::LibraryChanged,
                    Err(RecvError::Closed) => break,
                };
                if let Ok(ui) = UI.lock() {
                    if let Some(ui) = ui.as_ref() { let _ = ui.emit("core-event", event); }
                }
            }
        });
    }
    Ok(node)
}

pub fn attach(ui: tauri::AppHandle, path: PathBuf) -> Result<Node, String> {
    *UI.lock().map_err(|e| e.to_string())? = Some(ui);
    start(path)
}

// All JNI entry points run on LanSyncRuntime's serial executor, never Android's main thread.
// A Java exception reports failure instead of unwinding across JNI.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_musicstream_LanSyncRuntime_nativeSetRunning(
    mut env: JNIEnv, _this: JObject, path: JString, running: jboolean,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
        if running != 0 {
            let path: String = env.get_string(&path).map_err(|e| e.to_string())?.into();
            start(PathBuf::from(path))?;
        } else {
            NODE.stop()?;
        }
        Ok(())
    })).unwrap_or_else(|_| Err("The serving runtime panicked; reopen music-stream.".into()));
    if let Err(error) = result { let _ = env.throw_new("java/lang/IllegalStateException", error); }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_musicstream_LanSyncRuntime_nativeNetworkChanged(
    _env: JNIEnv, _this: JObject,
) {
    // Spawn rather than blocking the serial lifecycle executor on network probing.
    if let Ok(node) = node() {
        tauri::async_runtime::spawn(async move { node.network_changed().await; });
    }
}

/// The player service counts a play (PlayerService.kt) and records it here, through the node.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_musicstream_PlayerRuntime_nativeRecordPlay(mut env: JNIEnv, _this: JObject, id: JString) {
    let Ok(id) = env.get_string(&id).map(String::from) else { return };
    let _ = std::panic::catch_unwind(move || {
        let Ok(node) = node() else { return };
        let Some(library) = node.served_library() else { return };
        let Some(track) = musicstream_core::library::parse_hex::<16>(&id).ok().and_then(|id| library.track(&id)) else { return };
        let r = track.reference();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = node.record_play(r).await {
                tracing::warn!("couldn't count a play: {e:#}");
            }
        });
    });
}

/// `http://art.localhost/<hash>`: cover thumbnails from the phone's own library.
pub fn art_protocol<R: tauri::Runtime>(_ctx: tauri::UriSchemeContext<'_, R>, request: tauri::http::Request<Vec<u8>>, responder: tauri::UriSchemeResponder) {
    let path = request.uri().path().trim_start_matches('/').to_owned();
    std::thread::spawn(move || {
        let file = musicstream_core::library::parse_hex::<32>(&path)
            .ok()
            .and_then(|hash| node().ok()?.served_library().map(|l| l.art_path(&hash)))
            .and_then(|p| std::fs::read(p).ok());
        responder.respond(match file {
            Some(bytes) => tauri::http::Response::builder()
                .header("Content-Type", "image/jpeg")
                .header("Cache-Control", "max-age=31536000, immutable")
                .body(bytes)
                .unwrap(),
            None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap(),
        });
    });
}
