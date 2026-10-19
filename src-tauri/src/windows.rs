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
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};

pub const SETTINGS: &str = "settings";
pub const RESULT: &str = "result";
pub const TOAST: &str = "toast";
pub const OVERLAY_PREFIX: &str = "overlay-";

/// Size and screen margin of the in-app notice (logical pixels at 100% DPI;
/// the position is computed from the window's real physical size).
pub const TOAST_WIDTH: f64 = 400.0;
pub const TOAST_HEIGHT: f64 = 164.0;
const TOAST_MARGIN: i32 = 24;

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

/// Centre the result window on the monitor the cursor is on, at about half the
/// screen. Doing this on every show keeps the popup in a predictable place and
/// — together with the frontend no longer measuring its own content — means the
/// window never fights your own resizing.
fn place_result(app: &AppHandle, window: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|point| app.monitor_from_point(point.x, point.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());

    let Some(monitor) = monitor else {
        return;
    };

    // Physical pixels throughout: `work_area` is physical, so mixing in logical
    // sizes here would land the window off-screen on scaled displays.
    let area = monitor.work_area();
    let width = (area.size.width / 2).clamp(420, area.size.width.max(1));
    let height = (area.size.height / 2).clamp(260, area.size.height.max(1));
    let x = area.position.x + (area.size.width as i32 - width as i32) / 2;
    let y = area.position.y + (area.size.height as i32 - height as i32) / 2;
    let _ = window.set_size(PhysicalSize::new(width, height));
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// Bring the result window up (creating it on first use) and let the frontend
/// paint from `get_last_result`.
pub fn show_result(app: &AppHandle) {
    match result_window(app) {
        Ok(window) => {
            place_result(app, &window);
            let _ = window.show();
            let _ = window.set_focus();
        }
        Err(err) => log::error!("无法打开结果窗口: {err}"),
    }
}

/* ---------------------------------------------------------- notice ("toast") */

/// Our own, branded notice window.
///
/// Windows' native toasts only carry the app's identity when the app was
/// installed (and is not running from a `target/…` folder), so a dev build used
/// to pop up as “Windows PowerShell”. Drawing the notice ourselves keeps the
/// name, icon and theme right in every build, and it can point at the log page.
pub fn toast_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(TOAST) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(app, TOAST, WebviewUrl::App("toast.html".into()))
        .title("SnapLingo")
        .inner_size(TOAST_WIDTH, TOAST_HEIGHT)
        .decorations(false)
        .shadow(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .visible(false)
        // Never steal focus: the notice must not interrupt typing.
        .focused(false)
        .build()
}

/// Move the notice into the bottom-right corner of the monitor the cursor is on.
fn place_toast(app: &AppHandle, window: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|point| app.monitor_from_point(point.x, point.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());

    let Some(monitor) = monitor else {
        return;
    };

    let area = monitor.work_area();
    // `inner_size` above is logical, so ask the window for its *physical* size.
    // Using the logical numbers here pushed the card off the right edge and
    // under the taskbar on scaled displays (125% / 150%).
    let size = window
        .outer_size()
        .unwrap_or_else(|_| PhysicalSize::new(TOAST_WIDTH as u32, TOAST_HEIGHT as u32));
    let x = area.position.x + area.size.width as i32 - size.width as i32 - TOAST_MARGIN;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - TOAST_MARGIN;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// Show (or refresh) the branded notice. `payload` is what the page renders.
pub fn show_toast(app: &AppHandle, payload: serde_json::Value) {
    let window = match toast_window(app) {
        Ok(window) => window,
        Err(err) => {
            log::error!("无法创建通知窗口: {err}");
            return;
        }
    };
    place_toast(app, &window);
    let _ = window.emit(crate::pipeline::TOAST_EVENT, payload);
    let _ = window.show();
}

pub fn hide_toast(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(TOAST) {
        let _ = window.hide();
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
    if let Err(err) = toast_window(app) {
        log::warn!("预创建通知窗口失败: {err}");
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
    for label in [SETTINGS, RESULT, TOAST] {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.hide();
        }
    }
}
