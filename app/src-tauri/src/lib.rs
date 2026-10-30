//! music-stream app shell: links the core (`core/`) into the app process on every platform.
//! The UI reaches it through `rpc(method, params)` (devices and settings; see core/src/rpc.rs)
//! and `music(method, params)` (the library and player on a PC, serving status on the phone),
//! and hears about changes through `core-event`, `library-event` and `player` events.

#[cfg(target_os = "android")]
mod android;
#[cfg(desktop)]
mod background;
#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
mod media;
#[cfg(any(target_os = "android", test))]
mod node_slot;
mod userdata;

use musicstream_core::Node;
#[cfg(not(target_os = "android"))]
use musicstream_core::Event;
#[cfg(not(target_os = "android"))]
use musicstream_core::NodeConfig;
use serde_json::Value;
use tauri::{Manager, State};
#[cfg(not(target_os = "android"))]
use tauri::Emitter;

/// The node, or why it couldn't start (the UI shows it instead of the app crashing).
#[cfg(not(target_os = "android"))]
struct AppNode(Result<Node, String>);
// The Android runtime owns the node. A clone here would keep a shut-down node's file lock
// alive and prevent a later service/activity restart from reopening it.
#[cfg(target_os = "android")]
struct AppNode;

/// One node per process. Android can recreate the activity (and rerun setup) inside a live
/// process; a second node on the same files and port must never be opened.
#[cfg(not(target_os = "android"))]
static NODE: std::sync::OnceLock<Node> = std::sync::OnceLock::new();

fn node(state: &State<'_, AppNode>) -> Result<Node, String> {
    #[cfg(target_os = "android")]
    { let _ = state; android::node() }
    #[cfg(not(target_os = "android"))]
    { state.0.as_ref().cloned().map_err(Clone::clone) }
}

#[tauri::command]
async fn rpc(state: State<'_, AppNode>, method: String, params: Option<Value>) -> Result<Value, String> {
    musicstream_core::rpc::dispatch(&node(&state)?, &method, params.unwrap_or(Value::Null)).await.map_err(|e| format!("{e:#}"))
}

/// The library and player (PC) or the served library (phone).
#[tauri::command]
async fn music(app: tauri::AppHandle, state: State<'_, AppNode>, method: String, params: Option<Value>) -> Result<Value, String> {
    let params = params.unwrap_or(Value::Null);
    #[cfg(desktop)]
    {
        let _ = &state;
        if app.try_state::<desktop::Music>().is_none() {
            return Err("The player couldn't start.".into());
        }
        desktop::dispatch(&app, &method, params).await.map_err(|e| format!("{e:#}"))
    }
    #[cfg(target_os = "android")]
    {
        let _ = &state;
        android::dispatch(&app, &method, params).await.map_err(|e| format!("{e:#}"))
    }
    #[cfg(all(mobile, not(target_os = "android")))]
    {
        let _ = (app, state, params);
        Err(format!("unknown method {method}"))
    }
}

/// Opens Android's native background-serving controls; absent from the desktop UI.
#[tauri::command]
async fn android_sync_controls(_app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let handle = _app.state::<android::Controls>();
        handle.0.run_mobile_plugin_async::<Value>("showControls", Value::Null).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let builder = tauri::Builder::default();
    // First: a second launch (menu, login item) shows this app's window instead of starting
    // another node. `MUSIC_STREAM_MULTI_INSTANCE` turns that off for test instances.
    #[cfg(desktop)]
    let builder = if std::env::var_os("MUSIC_STREAM_MULTI_INSTANCE").is_some() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| background::open_window(app)))
    };
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec![background::BACKGROUND_ARG])))
        .register_asynchronous_uri_scheme_protocol("art", desktop::art_protocol);
    #[cfg(mobile)]
    let builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android::plugin()).register_asynchronous_uri_scheme_protocol("art", android::art_protocol);
    builder
        .setup(|app| {
            #[cfg(target_os = "android")]
            if let Err(error) = android::attach(app.handle().clone(), app.path().app_data_dir()?.join("library")) {
                tracing::error!("the node could not start: {error}");
            }
            #[cfg(not(target_os = "android"))]
            let node = match NODE.get() {
                Some(node) => Ok(node.clone()),
                None => {
                    let data_dir = app.path().app_data_dir()?.join("library");
                    tauri::async_runtime::block_on(Node::open(NodeConfig {
                        data_dir,
                        device_name: device_name(),
                        platform: std::env::consts::OS.into(),
                        port: musicstream_core::DEFAULT_PORT,
                        mdns: true,
                    }))
                    .map(|node| NODE.get_or_init(|| node).clone())
                    .map_err(|e| {
                        tracing::error!("the node could not start: {e:#}");
                        format!("{e:#}")
                    })
                }
            };
            #[cfg(not(target_os = "android"))]
            if let Ok(node) = &node {
                forward_events(app.handle().clone(), node);
                let dir = app.path().app_data_dir()?;
                let handle = app.handle().clone();
                let n = node.clone();
                if let Err(e) = tauri::async_runtime::block_on(async move { desktop::setup(&handle, n, dir) }) {
                    tracing::error!("the player could not start: {e:#}");
                }
                background::setup(app.handle(), node.clone())?;
                if app.try_state::<desktop::Music>().is_some() {
                    media::start(app.handle());
                }
            }
            // Without a node there is nothing to keep running: just show the error.
            #[cfg(not(target_os = "android"))]
            if node.is_err() && app.get_webview_window("main").is_none() {
                if let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main") {
                    tauri::WebviewWindowBuilder::from_config(app.handle(), config)?.build()?;
                }
            }
            #[cfg(not(target_os = "android"))]
            app.manage(AppNode(node));
            #[cfg(target_os = "android")]
            app.manage(AppNode);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            rpc,
            music,
            android_sync_controls,
            #[cfg(desktop)]
            background::keeps_running_in_background,
            #[cfg(desktop)]
            background::start_at_login,
            #[cfg(desktop)]
            background::set_start_at_login,
        ])
        .build(tauri::generate_context!())
        .expect("error while starting music-stream")
        .run(|_app, _event| {
            // Closing the last window doesn't end the app: it keeps playing in the tray, or
            // finishes sending before it quits (background.rs). `app.exit()` does end it.
            #[cfg(desktop)]
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = _event {
                api.prevent_exit();
                background::window_closed(_app);
            }
        });
}

#[cfg(not(target_os = "android"))]
fn forward_events(handle: tauri::AppHandle, node: &Node) {
    let mut events = node.subscribe();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match events.recv().await {
                Ok(event) => {
                    let _ = handle.emit("core-event", event);
                }
                // Missed events only mean the UI should refresh everything.
                Err(RecvError::Lagged(_)) => {
                    let _ = handle.emit("core-event", Event::LibraryChanged);
                    let _ = handle.emit("core-event", Event::StatusChanged);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}

fn init_logging() {
    use tracing_subscriber::{EnvFilter, prelude::*};
    let filter = EnvFilter::try_from_env("MUSIC_STREAM_LOG").unwrap_or_else(|_| EnvFilter::new("musicstream_core=info,player=info,music_stream_lib=info,iroh=warn"));
    #[cfg(target_os = "android")]
    let layer = tracing_android::layer("music-stream").ok();
    #[cfg(not(target_os = "android"))]
    let layer = Some(tracing_subscriber::fmt::layer());
    let _ = tracing_subscriber::registry().with(filter).with(layer).try_init();
}

#[cfg(target_os = "android")]
fn device_name() -> String {
    fn prop(name: &str) -> Option<String> {
        let name = std::ffi::CString::new(name).ok()?;
        let mut buf = [0 as libc::c_char; 92]; // PROP_VALUE_MAX
        // SAFETY: `buf` is PROP_VALUE_MAX bytes, the documented maximum for this call.
        let len = unsafe { libc::__system_property_get(name.as_ptr(), buf.as_mut_ptr()) };
        if len <= 0 {
            return None;
        }
        let bytes: Vec<u8> = buf[..len as usize].iter().map(|&c| c as u8).collect();
        String::from_utf8(bytes).ok().filter(|s| !s.trim().is_empty())
    }
    prop("ro.product.marketname").or_else(|| prop("ro.product.model")).unwrap_or_else(|| "Android phone".into())
}

/// "<computer name> (Linux)": a dual-boot PC is two devices with the same computer name, so
/// the OS is part of the name. `MUSIC_STREAM_DEVICE_NAME` overrides it (for test instances).
#[cfg(not(target_os = "android"))]
fn device_name() -> String {
    if let Ok(name) = std::env::var("MUSIC_STREAM_DEVICE_NAME") {
        return name;
    }
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty());
    let os = match std::env::consts::OS {
        "linux" => "Linux",
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    match host {
        Some(host) => format!("{host} ({os})"),
        None => os.to_owned(),
    }
}
