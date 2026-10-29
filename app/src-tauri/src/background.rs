//! Desktop: the app keeps playing (and syncing) after its window closes. A tray icon shows
//! the connection and has Open and Quit. Quitting first finishes giving the
//! connected devices what they don't have yet (with "Quit now" to skip that). Where the desktop
//! has no tray (GNOME without the AppIndicator extension), closing the window quits the same way:
//! a hidden app nobody can see or stop would be worse.

use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use musicstream_core::{Node, Outgoing};
use tauri::{
    AppHandle, Manager, WebviewWindowBuilder, Wry,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use tauri_plugin_autostart::ManagerExt;

/// Passed by the login item: start in the tray without opening the window.
pub const BACKGROUND_ARG: &str = "--background";
/// Longest a quit waits for the other devices.
const QUIT_WAIT: Duration = Duration::from_secs(10 * 60);

pub struct Background {
    node: Node,
    tray: Option<Tray>,
    /// Set while finishing sync before quitting; opening the window again cancels it.
    quitting: Mutex<Option<Instant>>,
}

struct Tray {
    status: MenuItem<Wry>,
    quit: MenuItem<Wry>,
    shown: Mutex<String>,
}

/// Sets up the tray (when the desktop has one) and opens the window unless started at login.
pub fn setup(app: &AppHandle, node: Node) -> tauri::Result<()> {
    let tray = if tray_available() { Some(build_tray(app)?) } else { None };
    let in_tray = tray.is_some();
    app.manage(Background { node: node.clone(), tray, quitting: Mutex::new(None) });
    if !(in_tray && std::env::args().any(|a| a == BACKGROUND_ARG)) {
        open_window(app);
    }
    if in_tray {
        let app = app.clone();
        let mut events = node.subscribe();
        tauri::async_runtime::spawn(async move {
            loop {
                refresh(&app);
                // Acknowledgements and transfer progress don't all raise events; poll too.
                if let Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) =
                    tokio::time::timeout(Duration::from_secs(2), events.recv()).await
                {
                    break;
                }
            }
        });
    }
    Ok(())
}

/// Whether closing the window leaves the app running in the tray.
pub fn keeps_running(app: &AppHandle) -> bool {
    app.try_state::<Background>().is_some_and(|bg| bg.tray.is_some())
}

/// Shows the window, creating it again if it was closed. Cancels a pending quit.
pub fn open_window(app: &AppHandle) {
    if let Some(bg) = app.try_state::<Background>() {
        bg.quitting.lock().unwrap().take();
    }
    refresh(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main") else { return };
    if let Err(e) = WebviewWindowBuilder::from_config(app, config).and_then(|b| b.build()) {
        tracing::error!("could not open the window: {e}");
    }
}

/// The last window was closed.
pub fn window_closed(app: &AppHandle) {
    if app.try_state::<Background>().is_none() {
        app.exit(0); // The library never started; there is nothing to finish.
    } else if !keeps_running(app) {
        quit(app);
    }
}

/// Quits once the connected devices have everything from this device.
pub fn quit(app: &AppHandle) {
    let bg = app.state::<Background>();
    {
        let mut quitting = bg.quitting.lock().unwrap();
        if quitting.is_some() {
            return;
        }
        *quitting = Some(Instant::now());
    }
    refresh(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.close();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let bg = app.state::<Background>();
            if bg.quitting.lock().unwrap().is_none() {
                return; // The window was opened again.
            }
            if let Some(window) = app.get_webview_window("main") {
                // The page normally destroys the window right after saving; don't wait forever.
                if started.elapsed() > Duration::from_secs(10) {
                    let _ = window.destroy();
                }
                continue;
            }
            refresh(&app);
            if bg.node.outgoing().done() || started.elapsed() > QUIT_WAIT {
                break;
            }
        }
        exit(&app).await;
    });
}

async fn exit(app: &AppHandle) {
    let node = app.state::<Background>().node.clone();
    let _ = tokio::time::timeout(Duration::from_secs(3), node.shutdown()).await;
    app.exit(0);
}

fn build_tray(app: &AppHandle) -> tauri::Result<Tray> {
    let status = MenuItem::with_id(app, "status", "Starting…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open music-stream", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &PredefinedMenuItem::separator(app)?, &open, &quit_item])?;
    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("music-stream")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => open_window(app),
            "quit" => {
                let quitting = app.state::<Background>().quitting.lock().unwrap().is_some();
                if quitting {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move { exit(&app).await });
                } else {
                    quit(app);
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                open_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(Tray { status, quit: quit_item, shown: Mutex::new(String::new()) })
}

/// Updates the tray's status line and Quit item.
fn refresh(app: &AppHandle) {
    let Some(bg) = app.try_state::<Background>() else { return };
    let Some(tray) = &bg.tray else { return };
    let quitting = bg.quitting.lock().unwrap().is_some();
    let outgoing = bg.node.outgoing();
    let text = if quitting && !outgoing.done() { format!("Finishing before quitting — {}", describe(&outgoing)) } else { describe(&outgoing) };
    let mut shown = tray.shown.lock().unwrap();
    if *shown != text {
        let _ = tray.status.set_text(&text);
        let _ = tray.quit.set_text(if quitting { "Quit now" } else { "Quit" });
        if let Some(icon) = app.tray_by_id("main") {
            let _ = icon.set_tooltip(Some(format!("music-stream — {text}")));
        }
        *shown = text;
    }
}

fn describe(o: &Outgoing) -> String {
    let names = join_names(&o.connected);
    if o.connected.is_empty() {
        "Not connected to your phone".into()
    } else if o.changes {
        format!("Sending changes to {names}")
    } else {
        format!("Connected to {names}")
    }
}

fn join_names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Linux trays are StatusNotifierItem hosts on D-Bus; GNOME has none unless the AppIndicator
/// extension is on. Windows and macOS always have one.
fn tray_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        let has_host = || -> Result<bool, dbus::Error> {
            let conn = dbus::blocking::Connection::new_session()?;
            let proxy = conn.with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", Duration::from_millis(500));
            let (has,): (bool,) = proxy.method_call("org.freedesktop.DBus", "NameHasOwner", ("org.kde.StatusNotifierWatcher",))?;
            Ok(has)
        };
        has_host().unwrap_or_else(|e| {
            tracing::warn!("couldn't check for a tray: {e}");
            false
        })
    }
    #[cfg(not(target_os = "linux"))]
    true
}

#[tauri::command]
pub fn keeps_running_in_background(app: AppHandle) -> bool {
    keeps_running(&app)
}

#[tauri::command]
pub fn start_at_login(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_start_at_login(app: AppHandle, on: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if on { launcher.enable() } else { launcher.disable() }.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(connected: &[&str], changes: bool) -> Outgoing {
        Outgoing { connected: connected.iter().map(|s| s.to_string()).collect(), changes }
    }

    #[test]
    fn describes_the_connection() {
        assert_eq!(describe(&out(&[], true)), "Not connected to your phone");
        assert_eq!(describe(&out(&["Phone", "Laptop"], true)), "Sending changes to Phone and Laptop");
        assert_eq!(describe(&out(&["A", "B", "C"], false)), "Connected to A, B and C");
    }
}
