//! Data exchanged with the frontend (event payloads + command arguments).

use serde::{Deserialize, Serialize};

/// Physical-pixel rectangle inside one monitor's coordinate space.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRect {
    pub monitor_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl SelectionRect {
    /// Normalised, sane rectangle (handles drag in any direction).
    pub fn normalized(self) -> Self {
        let x = if self.width < 0 { self.x + self.width } else { self.x };
        let y = if self.height < 0 { self.y + self.height } else { self.y };
        Self {
            monitor_id: self.monitor_id,
            x,
            y,
            width: self.width.abs(),
            height: self.height.abs(),
        }
    }

    pub fn is_usable(self) -> bool {
        let n = self.normalized();
        n.width >= 4 && n.height >= 4
    }
}

/// A frozen screenshot of one monitor, handed to the corresponding overlay window.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionImage {
    pub monitor_id: u32,
    pub data_url: String,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Idle,
    Capturing,
    Ocr,
    Translate,
    Done,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageEvent {
    pub stage: Stage,
    pub message: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultPayload {
    pub id: String,
    pub source_text: String,
    pub translated_text: Option<String>,
    pub target_lang: String,
    pub ocr_provider: String,
    pub translate_provider: String,
    pub model: String,
    pub elapsed_ms: u64,
    pub created_at: u64,
    pub preview: Option<String>,
    pub error: Option<String>,
}

impl ResultPayload {
    pub fn error(id: String, message: impl Into<String>, elapsed_ms: u64) -> Self {
        Self {
            id,
            source_text: String::new(),
            translated_text: None,
            target_lang: String::new(),
            ocr_provider: String::new(),
            translate_provider: String::new(),
            model: String::new(),
            elapsed_ms,
            created_at: now_ms(),
            preview: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub id: String,
    pub source_text: String,
    pub translated_text: Option<String>,
    pub target_lang: String,
    pub created_at: u64,
}

impl From<&ResultPayload> for HistoryItem {
    fn from(r: &ResultPayload) -> Self {
        Self {
            id: r.id.clone(),
            source_text: r.source_text.clone(),
            translated_text: r.translated_text.clone(),
            target_lang: r.target_lang.clone(),
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionCheck {
    pub ok: bool,
    pub message: String,
    pub elapsed_ms: u64,
    pub sample: Option<String>,
}

/// The text pair produced by a vision model in one-shot mode.
#[derive(Debug, Clone, Deserialize)]
pub struct OneShotReply {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub translation: String,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn new_id() -> String {
    // Time-ordered enough for a history list, collision-free in practice.
    format!("{:x}-{:04x}", now_ms(), fastrand_u16())
}

fn fastrand_u16() -> u16 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(now_ms());
    h.write_usize(&h as *const _ as usize);
    (h.finish() & 0xffff) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_normalises_negative_drag() {
        let r = SelectionRect { monitor_id: 1, x: 100, y: 100, width: -40, height: -30 }.normalized();
        assert_eq!((r.x, r.y, r.width, r.height), (60, 70, 40, 30));
    }

    #[test]
    fn tiny_selection_is_rejected() {
        assert!(!SelectionRect { monitor_id: 0, x: 0, y: 0, width: 3, height: 20 }.is_usable());
        assert!(SelectionRect { monitor_id: 0, x: 0, y: 0, width: 20, height: 20 }.is_usable());
    }

    #[test]
    fn ids_are_unique_enough() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
    }
}
