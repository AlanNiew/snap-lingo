//! Screen capture. Blocking by nature, so callers run this on
//! `spawn_blocking` — never on the UI thread.
//!
//! Every monitor is frozen into its own RGBA frame *before* the overlay windows
//! appear. The overlay then renders that frozen frame, so what the user drags
//! over is exactly what gets translated: no flicker, no chance of capturing the
//! overlay itself, and no second capture that could race with a moving window.

use anyhow::{bail, Context, Result};
use image::RgbaImage;
use std::sync::Arc;
use xcap::Monitor;

#[derive(Clone)]
pub struct MonitorFrame {
    pub id: u32,
    pub name: String,
    /// Physical-pixel origin inside the virtual desktop.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub frame: Arc<RgbaImage>,
}

impl MonitorFrame {
    /// Monitor-local physical rectangle → pixels in this frame.
    pub fn crop(&self, x: i32, y: i32, w: i32, h: i32) -> RgbaImage {
        crate::imageops::crop(&self.frame, x, y, w, h)
    }
}

/// Grab a frame from every monitor, or just one when `all_monitors` is off —
/// and "the one" means the screen the pointer is on, because a capture is about
/// whatever the user is currently looking at. The primary monitor sorts first
/// so callers can treat the first entry as the main screen; in single-screen
/// mode the only entry is the screen under the cursor, primary or not.
pub fn capture(all_monitors: bool, cursor: Option<(i32, i32)>) -> Result<Vec<MonitorFrame>> {
    let mut monitors = Monitor::all().context("无法枚举显示器（xcap）")?;
    if monitors.is_empty() {
        bail!("没有检测到可用显示器。");
    }

    // Decide which monitors to freeze up front.
    let targets: Vec<Monitor> = if all_monitors {
        monitors
    } else {
        let under_cursor = cursor
            .and_then(|(x, y)| Monitor::from_point(x, y).ok())
            .filter(|m| m.width().unwrap_or(0) > 0 && m.height().unwrap_or(0) > 0);
        match under_cursor {
            Some(monitor) => vec![monitor],
            None => {
                // The OS could not resolve the point: fall back to the primary.
                let index = monitors
                    .iter()
                    .position(|m| m.is_primary().unwrap_or(false))
                    .unwrap_or(0);
                vec![monitors.swap_remove(index)]
            }
        }
    };

    let mut described: Vec<(bool, MonitorFrame)> = Vec::new();

    for monitor in targets {
        let is_primary = monitor.is_primary().unwrap_or(false);
        let id = monitor.id().unwrap_or(0);
        let width = monitor.width().unwrap_or(0);
        let height = monitor.height().unwrap_or(0);
        if width == 0 || height == 0 {
            log::warn!("skipping monitor {id}: reported size {width}x{height}");
            continue;
        }

        let started = std::time::Instant::now();
        let frame = monitor
            .capture_image()
            .with_context(|| format!("截取显示器 {id} 失败"))?;
        log::debug!(
            "captured monitor {id} {width}x{height} in {:?}",
            started.elapsed()
        );

        described.push((
            is_primary,
            MonitorFrame {
                id,
                name: monitor.name().unwrap_or_else(|_| format!("Display {id}")),
                x: monitor.x().unwrap_or(0),
                y: monitor.y().unwrap_or(0),
                width: frame.width(),
                height: frame.height(),
                scale_factor: monitor.scale_factor().unwrap_or(1.0) as f64,
                frame: Arc::new(frame),
            },
        ));
    }

    if described.is_empty() {
        bail!("截图失败：没有捕获到任何画面。");
    }

    // Primary first so the overlay we focus is the one the user is looking at.
    described.sort_by_key(|(primary, _)| !*primary);
    Ok(described.into_iter().map(|(_, frame)| frame).collect())
}

/// Monitor descriptor without the pixel payload — used to place windows.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
}

pub fn monitors() -> Result<Vec<MonitorInfo>> {
    let list = Monitor::all().context("无法枚举显示器")?;
    let mut described: Vec<(bool, MonitorInfo)> = list
        .iter()
        .map(|m| {
            (
                m.is_primary().unwrap_or(false),
                MonitorInfo {
                    id: m.id().unwrap_or(0),
                    name: m.name().unwrap_or_default(),
                    x: m.x().unwrap_or(0),
                    y: m.y().unwrap_or(0),
                    width: m.width().unwrap_or(0),
                    height: m.height().unwrap_or(0),
                    scale_factor: m.scale_factor().unwrap_or(1.0) as f64,
                },
            )
        })
        .collect();
    // Primary first, same as `capture`, so callers can treat the first entry as
    // the monitor the user is actually looking at.
    described.sort_by_key(|(primary, _)| !*primary);
    Ok(described.into_iter().map(|(_, info)| info).collect())
}
