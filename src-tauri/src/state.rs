//! Process-wide application state.
//!
//! Everything mutable lives behind a `std::sync::Mutex` held only for the few
//! microseconds it takes to read or replace a value — never across an `await`,
//! so a stalled network call can't wedge the UI.

use crate::capture::MonitorFrame;
use crate::config::{AppConfig, ConfigStore};
use crate::model::{HistoryItem, ResultPayload};
use crate::providers::Providers;
use image::RgbaImage;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

/// Lock without the poisoning dance: a panic elsewhere must not take the app
/// down with it, and none of these values can be left in a torn state.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// One monitor's frozen pixels, plus the (possibly downscaled) preview the
/// overlay paints. The overlay sends its drag in **full-resolution frame**
/// pixels (it scales by `frame.width / innerWidth` on its side), so
/// `CaptureSession::frame_rect` only has to clamp, never rescale.
pub struct SessionEntry {
    pub frame: MonitorFrame,
    /// Pre-encoded backdrop handed to the overlay. The decoded `RgbaImage` is
    /// dropped right after encoding: the rectangle is cropped from `frame`, so
    /// keeping a second full copy only wasted memory.
    pub preview_data_url: String,
}

/// Everything frozen at hotkey time. Held until the user finishes or cancels.
pub struct CaptureSession {
    pub entries: Vec<SessionEntry>,
    pub started: Instant,
}

impl CaptureSession {
    pub fn entry(&self, monitor_id: u32) -> Option<&SessionEntry> {
        self.entries.iter().find(|e| e.frame.id == monitor_id)
    }

    /// Keep the selection inside the frozen frame.
    ///
    /// The rectangle arrives in **frame pixels** (physical): the overlay maps
    /// mouse positions onto the captured frame using `SessionImage::width` /
    /// `height`, which are the frame's dimensions — so how much the backdrop
    /// image was downscaled for display never enters the maths. (It used to:
    /// coordinates came in preview pixels and were scaled up here, which quietly
    /// broke as soon as the preview was smaller than the frame.)
    pub fn frame_rect(
        &self,
        entry: &SessionEntry,
        x: f64,
        y: f64,
        w: f64,
        h: f64,
    ) -> (i32, i32, i32, i32) {
        let (fw, fh) = (entry.frame.width as f64, entry.frame.height as f64);
        let x0 = x.max(0.0);
        let y0 = y.max(0.0);
        let x1 = (x + w).min(fw);
        let y1 = (y + h).min(fh);
        let w = (x1 - x0).max(0.0);
        let h = (y1 - y0).max(0.0);
        (
            x0.round() as i32,
            y0.round() as i32,
            w.round() as i32,
            h.round() as i32,
        )
    }
}

/// State of the one update download this process can run, shared with the UI so
/// it can be re-attached after the settings page is left and opened again.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateDownload {
    /// `idle` | `downloading` | `ready` | `error`
    pub status: String,
    pub received: u64,
    pub total: u64,
    /// Set once the installer is on disk.
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub error: String,
}

pub struct AppState {
    pub store: ConfigStore,
    pub providers: Providers,
    pub config: Mutex<AppConfig>,
    pub session: Mutex<Option<CaptureSession>>,
    pub last_result: Mutex<Option<ResultPayload>>,
    /// Kept so "重新翻译" can retry without asking the user to select again.
    pub last_source: Mutex<Option<String>>,
    /// Shared, not owned: the job, the retry path and the cached preview all
    /// need the same crop, and a 1440p frame is 14.7 MB per clone.
    pub last_crop: Mutex<Option<Arc<RgbaImage>>>,
    pub history: Mutex<Vec<HistoryItem>>,
    pub history_path: PathBuf,
    /// The notice currently on screen (`kind`, `message`), so the auto-hide
    /// watchdog can tell whether it is still the same one.
    pub last_toast: Mutex<Option<(String, String)>>,
    /// Physical size the user last dragged the result window to. The next popup
    /// reuses it instead of snapping back to half the screen.
    pub result_size: Mutex<Option<(u32, u32)>>,
    /// Download progress of an update installer. Lives here — not in the settings
    /// page — so switching tabs (or closing the window) cannot cancel it.
    pub update: Mutex<UpdateDownload>,
    /// True from the moment a capture starts until the overlays are gone.
    pub busy: AtomicBool,
    /// Set when the user picks 退出, so window-closing stops being vetoed.
    pub quitting: AtomicBool,
}

impl AppState {
    pub fn new(dir: PathBuf) -> anyhow::Result<Self> {
        let store = ConfigStore::new(dir.clone());
        let mut config = store.load();
        config.sanitize();
        let history_path = dir.join("history.json");

        Ok(Self {
            store,
            providers: Providers::new()?,
            config: Mutex::new(config),
            session: Mutex::new(None),
            last_result: Mutex::new(None),
            last_source: Mutex::new(None),
            last_crop: Mutex::new(None),
            history: Mutex::new(load_history(&history_path)),
            history_path,
            last_toast: Mutex::new(None),
            result_size: Mutex::new(None),
            update: Mutex::new(UpdateDownload::default()),
            busy: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
        })
    }

    pub fn config(&self) -> AppConfig {
        lock(&self.config).clone()
    }

    pub fn replace_config(&self, cfg: AppConfig) -> anyhow::Result<()> {
        self.store.save(&cfg)?;
        *lock(&self.config) = cfg;
        Ok(())
    }

    pub fn push_history(&self, item: HistoryItem) {
        let mut items = lock(&self.history);
        items.retain(|existing| existing.id != item.id);
        items.insert(0, item);
        let limit = lock(&self.config).history.limit;
        if limit == 0 {
            items.clear();
        } else if items.len() > limit {
            items.truncate(limit);
        }
        let snapshot = items.clone();
        drop(items);
        save_history(&self.history_path, &snapshot);
    }

    pub fn history_snapshot(&self) -> Vec<HistoryItem> {
        lock(&self.history).clone()
    }

    pub fn clear_history(&self) {
        lock(&self.history).clear();
        save_history(&self.history_path, &[]);
    }

    pub fn remove_history(&self, id: &str) -> Vec<HistoryItem> {
        let mut items = lock(&self.history);
        items.retain(|item| item.id != id);
        let snapshot = items.clone();
        drop(items);
        save_history(&self.history_path, &snapshot);
        snapshot
    }

    pub fn mark_busy(&self, busy: bool) {
        self.busy.store(busy, Ordering::SeqCst);
    }

    /// Crop the selected region out of the frozen frame. `rect` is in frame
    /// pixels — the same space the overlay reports in — so the result is exactly
    /// what the user framed, whatever resolution the backdrop was encoded at.
    pub fn crop_selection(&self, rect: &crate::model::SelectionRect) -> Option<RgbaImage> {
        let session = lock(&self.session);
        let session = session.as_ref()?;
        let entry = session.entry(rect.monitor_id)?;
        let (x, y, w, h) = session.frame_rect(
            entry,
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        Some(crate::imageops::crop(&entry.frame.frame, x, y, w, h))
    }
}

/* --------------------------------------------------------------- history io */

fn load_history(path: &Path) -> Vec<HistoryItem> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    match serde_json::from_str::<Vec<HistoryItem>>(&raw) {
        Ok(items) => items,
        Err(err) => {
            log::warn!("history.json 解析失败（{err}），本次忽略历史记录");
            Vec::new()
        }
    }
}

fn save_history(path: &Path, items: &[HistoryItem]) {
    let write = || -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(items)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    };
    if let Err(err) = write() {
        log::error!("历史记录保存失败: {err:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn entry(frame: (u32, u32)) -> SessionEntry {
        SessionEntry {
            frame: MonitorFrame {
                id: 1,
                name: "t".into(),
                x: 0,
                y: 0,
                width: frame.0,
                height: frame.1,
                scale_factor: 2.0,
                frame: std::sync::Arc::new(RgbaImage::from_pixel(
                    frame.0,
                    frame.1,
                    Rgba([0, 0, 0, 255]),
                )),
            },
            preview_data_url: String::new(),
        }
    }

    #[test]
    fn rect_is_taken_as_frame_pixels() {
        let session = CaptureSession {
            entries: vec![entry((2560, 1440))],
            started: Instant::now(),
        };
        let e = session.entry(1).unwrap();
        // A 2×-downscaled backdrop must not change the rectangle: it is already
        // in frame pixels.
        assert_eq!(
            session.frame_rect(e, 10.0, 20.0, 100.0, 50.0),
            (10, 20, 100, 50)
        );
    }

    #[test]
    fn rect_is_clamped_to_the_frame() {
        let session = CaptureSession {
            entries: vec![entry((800, 600))],
            started: Instant::now(),
        };
        let e = session.entry(1).unwrap();
        assert_eq!(session.frame_rect(e, 5.0, 6.0, 7.0, 8.0), (5, 6, 7, 8));
        assert_eq!(
            session.frame_rect(e, -20.0, -5.0, 40.0, 40.0),
            (0, 0, 20, 35)
        );
        assert_eq!(
            session.frame_rect(e, 790.0, 590.0, 100.0, 100.0),
            (790, 590, 10, 10)
        );
    }
}
