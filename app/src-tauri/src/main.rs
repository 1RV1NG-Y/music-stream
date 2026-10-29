// Desktop entry point (Linux, Windows). Android starts from `run()` in lib.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    linux_webview_workarounds();
    music_stream_lib::run()
}

/// WebKitGTK on Wayland with the NVIDIA driver aborts with "Error 71 (Protocol error)
/// dispatching to Wayland display" as soon as the window draws (seen with WebKitGTK 2.54 +
/// RTX 4060 Ti). Turning off the driver's explicit sync avoids it; the variable is ignored by
/// every other driver. A value the user already set wins.
#[cfg(target_os = "linux")]
fn linux_webview_workarounds() {
    if std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none() {
        // SAFETY: called at the very start of main, before any other thread exists.
        unsafe { std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1") };
    }
}
