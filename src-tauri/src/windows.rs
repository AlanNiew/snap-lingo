//! Window lifecycle: settings, result, and the per-monitor capture overlays.
//!
//! All three are created lazily and reused. Creating a webview is the single
//! most expensive thing this app does, so nothing is built at startup, and the
//! overlays are hidden rather than destroyed: rebuilding a webview on every
//! hotkey press is slow, and tearing one down while the next capture builds the
//! *same* label is a WebView2 race that can wedge the main thread (which looks
//! exactly like the whole app freezing).

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
        .inner_size(820.0, 480.0)
        .min_inner_size(420.0, 260.0)
        .decorations(false)
        // Opaque on purpose: a transparent window that is larger than its card
        // shows the desktop as a see-through band you can still click into.
        // Window-level shadow (drawn by Windows) replaces the old CSS shadow.
        .shadow(true)
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

    // Warm path: an overlay for this monitor already exists, so just move it
    // and keep it hidden until the page reports the new frame is painted. The
    // page reloads its image because `pipeline` emits `session:changed`.
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.hide();
        window.set_position(PhysicalPosition::new(frame.x, frame.y))?;
        window.set_size(PhysicalSize::new(frame.width, frame.height))?;
        window.set_always_on_top(true)?;
        log::debug!(
            "复用框选层（显示器 {}，{}×{} @{},{})",
            frame.id,
            frame.width,
            frame.height,
            frame.x,
            frame.y
        );
        return Ok(window);
    }

    build_overlay(app, frame.id, frame.x, frame.y, frame.width, frame.height)
}

/// Build the overlay webview for one monitor.
///
/// Split out of `create_overlay` so startup can pre-warm the window the capture
/// flow is going to need (see `prewarm`).
fn build_overlay(
    app: &AppHandle,
    id: u32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> tauri::Result<WebviewWindow> {
    // The monitor id travels in the window label, so the page never has to
    // parse a query string. See `src/overlay.tsx`.
    let window = WebviewWindowBuilder::new(
        app,
        overlay_label(id),
        WebviewUrl::App("overlay.html".into()),
    )
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
    window.set_position(PhysicalPosition::new(x, y))?;
    window.set_size(PhysicalSize::new(width, height))?;
    window.set_always_on_top(true)?;
    log::debug!("新建框选层（显示器 {id}，{width}×{height} @{x},{y})");
    Ok(window)
}

/// Build, up front, the window a capture always needs.
///
/// Creating a webview is the most expensive thing this app does, and doing it
/// while we are already inside another webview's IPC callback — which is
/// exactly where a selection lands — is what can wedge the main thread.
/// Building the result window here means the selection path only ever shows and
/// focuses a window that already exists.
pub fn prewarm(app: &AppHandle) {
    if let Err(err) = result_window(app) {
        log::warn!("预创建结果窗口失败: {err}");
    }
}

/// Warm the primary monitor's overlay a moment after startup: late enough that
/// it costs nothing at launch, early enough that the first hotkey press finds a
/// window instead of creating one.
pub fn prewarm_overlay(app: &AppHandle) {
    let Some(main) = crate::capture::monitors()
        .unwrap_or_default()
        .into_iter()
        .next()
    else {
        return;
    };

    if app.get_webview_window(&overlay_label(main.id)).is_some() {
        return;
    }

    if let Err(err) = build_overlay(app, main.id, main.x, main.y, main.width, main.height) {
        log::warn!("预创建框选层失败: {err}");
    }
}

/// Reveal one overlay once its frozen frame has been painted.
pub fn reveal_overlay(app: &AppHandle, monitor_id: u32, focus: bool) {
    let Some(window) = app.get_webview_window(&overlay_label(monitor_id)) else {
        log::warn!("框选层不存在（显示器 {monitor_id}）");
        return;
    };
    let _ = window.show();
    if focus {
        let _ = window.set_focus();
    }
}

/// Hide every overlay.
///
/// Deliberately *not* `destroy`: the webviews stay warm so the next capture is
/// instant, and nothing can collide with a teardown that is still unwinding.
pub fn hide_overlays(app: &AppHandle) {
    for label in overlay_labels(app) {
        if let Some(window) = app.get_webview_window(&label) {
            let _ = window.hide();
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
