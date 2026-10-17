//! Window lifecycle: settings, result, and the per-monitor capture overlays.
//!
//! All three are created lazily and reused. Creating a webview is the single
//! most expensive thing this app does, so nothing is built at startup and
//! nothing is destroyed unless its size has to change (the overlays).

use crate::capture::MonitorFrame;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const SETTINGS: &str = "settings";
pub const RESULT: &str = "result";
pub const OVERLAY_PREFIX: &str = "overlay-";

/// The overlay preview is capped so a 4K frame does not become a ~20 MB IPC
/// payload. Selection accuracy is unaffected: the user's rectangle is mapped
/// back onto the full-resolution frame before it is cropped.
pub const PREVIEW_LONG_EDGE: u32 = 2560;

pub fn overlay_label(monitor_id: u32) -> String {
    format!("{OVERLAY_PREFIX}{monitor_id}")
}

pub fn overlay_labels(app: &AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app
        .webview_windows()
        .into_keys()
        .filter(|label| label.starts_with(OVERLAY_PREFIX))
        .collect();
    labels.sort();
    labels
}

/* ----------------------------------------------------------- settings window */

pub fn settings_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(SETTINGS) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("index.html".into()))
        .title("SnapLingo 设置")
        .inner_size(940.0, 680.0)
        .min_inner_size(780.0, 560.0)
        .center()
        .build()
}

pub fn show_settings(app: &AppHandle) {
    match settings_window(app) {
        Ok(window) => {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        Err(err) => log::error!("无法打开设置窗口: {err}"),
    }
}

/* ------------------------------------------------------------- result window */

pub fn result_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(RESULT) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(app, RESULT, WebviewUrl::App("result.html".into()))
        .title("SnapLingo 翻译结果")
        .inner_size(560.0, 400.0)
        .min_inner_size(360.0, 220.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(false)
        .resizable(true)
        .visible(false)
        .build()
}

/// Bring the result window up (creating it on first use) and let the frontend
/// paint from `get_last_result`.
pub fn show_result(app: &AppHandle) {
    match result_window(app) {
        Ok(window) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        Err(err) => log::error!("无法打开结果窗口: {err}"),
    }
}

/* ------------------------------------------------------------ overlays (n) */

pub fn create_overlay(app: &AppHandle, frame: &MonitorFrame) -> tauri::Result<WebviewWindow> {
    let label = overlay_label(frame.id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.destroy();
    }

    // The monitor id travels in the window label, so the page never has to
    // parse a query string. See `src/overlay.tsx`.
    let window = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("overlay.html".into()))
        .title("SnapLingo 截图")
        .decorations(false)
        .transparent(false)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build()?;

    // Physical placement: the overlay must cover exactly one monitor even when
    // the monitors disagree about DPI.
    window.set_position(PhysicalPosition::new(frame.x, frame.y))?;
    window.set_size(PhysicalSize::new(frame.width, frame.height))?;
    window.set_always_on_top(true)?;
    Ok(window)
}

/// Reveal one overlay once its frozen frame has been painted.
pub fn reveal_overlay(app: &AppHandle, monitor_id: u32, focus: bool) {
    let Some(window) = app.get_webview_window(&overlay_label(monitor_id)) else {
        return;
    };
    let _ = window.show();
    if focus {
        let _ = window.set_focus();
    }
}

pub fn close_overlays(app: &AppHandle) {
    for label in overlay_labels(app) {
        if let Some(window) = app.get_webview_window(&label) {
            // `destroy` rather than `close`: overlays must never survive a
            // selection, and nothing gets to veto that.
            let _ = window.destroy();
        }
    }
}

/// Hide our own windows so they cannot end up inside the screenshot.
pub fn hide_transient(app: &AppHandle) {
    for label in [SETTINGS, RESULT] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
}
