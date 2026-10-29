//! The Android service and Tauri activity share one node and one Tokio runtime. JNI works
//! without constructing an activity/WebView, including a START_STICKY process recreation.
use std::{path::PathBuf, sync::Mutex};
use jni::{JNIEnv, objects::{JObject, JString}, sys::jboolean};
use musicstream_core::{Node, NodeConfig};
use tauri::{Emitter, Manager, plugin::{Builder, PluginHandle, TauriPlugin}};

static NODE: super::node_slot::NodeSlot = super::node_slot::NodeSlot::new();
static UI: Mutex<Option<tauri::AppHandle>> = Mutex::new(None);
pub struct Controls(pub PluginHandle<tauri::Wry>);

pub fn plugin() -> TauriPlugin<tauri::Wry> {
    Builder::new("lan-sync")
        .setup(|app, api| {
            app.manage(Controls(api.register_android_plugin("app.musicstream", "LanSyncPlugin")?));
            Ok(())
        }).build()
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
