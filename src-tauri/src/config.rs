//! Persistent application configuration.
//!
//! Stored as JSON next to the app's config dir. Every field carries a serde
//! default so that an older config file keeps loading after an upgrade, and a
//! hand-edited partial file is still accepted.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "SnapLingo";
pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+Z";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OcrConfig {
    /// `vision` = multimodal LLM, `winocr` = offline Windows OCR engine.
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Windows OCR language tag ("zh-Hans", "en-US", …); empty = user profile.
    pub language: String,
    /// Vision models: return text + translation from a single call.
    pub one_shot: bool,
    /// Custom OCR instruction; empty = built-in prompt.
    pub prompt: String,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            provider: "winocr".into(),
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            api_key: String::new(),
            model: String::new(),
            language: String::new(),
            one_shot: false,
            prompt: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TranslateConfig {
    /// `openai` | `deepl` | `none`
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub target_lang: String,
    pub deepl_plan: String,
    pub prompt: String,
}

impl Default for TranslateConfig {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            base_url: "https://integrate.api.nvidia.com/v1".into(),
            api_key: String::new(),
            model: "nvidia/nemotron-3-ultra-550b-a55b".into(),
            target_lang: "zh-CN".into(),
            deepl_plan: "free".into(),
            prompt: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CaptureConfig {
    pub all_monitors: bool,
    pub upscale_small: bool,
    pub max_image_side: u32,
    pub jpeg_quality: u8,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            all_monitors: true,
            upscale_small: true,
            max_image_side: 1568,
            jpeg_quality: 88,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiConfig {
    pub auto_copy: bool,
    pub show_original: bool,
    pub close_on_blur: bool,
    pub result_font_size: u32,
    pub theme: String,
    pub language: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            auto_copy: false,
            show_original: true,
            close_on_blur: false,
            result_font_size: 16,
            theme: "system".into(),
            language: "zh".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HistoryConfig {
    pub enabled: bool,
    pub limit: usize,
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self { enabled: true, limit: 100 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub shortcut: String,
    pub ocr: OcrConfig,
    pub translate: TranslateConfig,
    pub capture: CaptureConfig,
    pub ui: UiConfig,
    /// Launch on Windows login.
    pub startup: bool,
    pub history: HistoryConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            shortcut: DEFAULT_SHORTCUT.into(),
            ocr: OcrConfig::default(),
            translate: TranslateConfig::default(),
            capture: CaptureConfig::default(),
            ui: UiConfig::default(),
            startup: false,
            history: HistoryConfig::default(),
        }
    }
}

impl AppConfig {
    /// Clamp user input into ranges the rest of the app can rely on.
    pub fn sanitize(&mut self) {
        if self.shortcut.trim().is_empty() {
            self.shortcut = DEFAULT_SHORTCUT.into();
        }
        if self.ocr.provider != "vision" {
            self.ocr.provider = "winocr".into();
        }
        if !matches!(self.translate.provider.as_str(), "openai" | "deepl" | "none") {
            self.translate.provider = "openai".into();
        }
        if self.translate.deepl_plan != "pro" {
            self.translate.deepl_plan = "free".into();
        }
        self.capture.max_image_side = self.capture.max_image_side.clamp(320, 4096);
        self.capture.jpeg_quality = self.capture.jpeg_quality.clamp(40, 100);
        self.ui.result_font_size = self.ui.result_font_size.clamp(11, 32);
        self.history.limit = self.history.limit.clamp(0, 1000);
        if self.ocr.base_url.trim().is_empty() {
            self.ocr.base_url = "https://integrate.api.nvidia.com/v1".into();
        }
        if self.translate.base_url.trim().is_empty() {
            self.translate.base_url = "https://integrate.api.nvidia.com/v1".into();
        }
        self.ocr.base_url = self.ocr.base_url.trim_end_matches('/').to_string();
        self.translate.base_url = self.translate.base_url.trim_end_matches('/').to_string();
        if self.translate.target_lang.trim().is_empty() {
            self.translate.target_lang = "zh-CN".into();
        }
    }

    /// True when the configured pipeline can actually run without further setup.
    pub fn is_ready(&self) -> bool {
        let ocr_ok = self.ocr.provider == "winocr"
            || (!self.ocr.api_key.trim().is_empty() && !self.ocr.model.trim().is_empty());
        let tr_ok = match self.translate.provider.as_str() {
            "none" => true,
            "deepl" => !self.translate.api_key.trim().is_empty(),
            _ => {
                !self.translate.api_key.trim().is_empty()
                    && !self.translate.model.trim().is_empty()
            }
        };
        ocr_ok && tr_ok
    }
}

/* ------------------------------------------------------------------ storage */

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { path: dir.join("config.json") }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> AppConfig {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => match serde_json::from_str::<AppConfig>(&raw) {
                Ok(mut cfg) => {
                    cfg.sanitize();
                    cfg
                }
                Err(err) => {
                    log::warn!("config.json is not valid JSON ({err}); falling back to defaults");
                    // Keep the broken file around so the user can inspect it.
                    let _ = std::fs::rename(&self.path, self.path.with_extension("json.bak"));
                    AppConfig::default()
                }
            },
            Err(_) => {
                let cfg = AppConfig::default();
                let _ = self.save(&cfg);
                cfg
            }
        }
    }

    pub fn save(&self, cfg: &AppConfig) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(cfg)?;
        // Write-then-rename so a crash mid-write cannot truncate the config.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_uses_defaults() {
        let cfg: AppConfig = serde_json::from_str(r#"{"shortcut":"Ctrl+Alt+Q"}"#).unwrap();
        assert_eq!(cfg.shortcut, "Ctrl+Alt+Q");
        assert_eq!(cfg.ocr.provider, "winocr");
        assert_eq!(cfg.translate.target_lang, "zh-CN");
        assert!(cfg.capture.all_monitors);
    }

    #[test]
    fn sanitize_clamps_and_normalises() {
        let mut cfg = AppConfig::default();
        cfg.shortcut = "   ".into();
        cfg.ocr.base_url = "https://x.dev/v1/".into();
        cfg.capture.max_image_side = 99_999;
        cfg.ui.result_font_size = 2;
        cfg.history.limit = 999_999;
        cfg.translate.deepl_plan = "enterprise".into();
        cfg.sanitize();
        assert_eq!(cfg.shortcut, DEFAULT_SHORTCUT);
        assert_eq!(cfg.ocr.base_url, "https://x.dev/v1");
        assert_eq!(cfg.capture.max_image_side, 4096);
        assert_eq!(cfg.ui.result_font_size, 11);
        assert_eq!(cfg.history.limit, 1000);
        assert_eq!(cfg.translate.deepl_plan, "free");
    }

    #[test]
    fn readiness_requires_keys_only_for_api_paths() {
        let mut cfg = AppConfig::default();
        assert!(cfg.is_ready(), "offline winocr alone should be usable");

        cfg.ocr.provider = "vision".into();
        assert!(!cfg.is_ready(), "vision without key/model must not be ready");

        cfg.ocr.api_key = "k".into();
        cfg.ocr.model = "m".into();
        assert!(!cfg.is_ready(), "translation still needs a key");

        cfg.translate.provider = "none".into();
        assert!(cfg.is_ready());
    }
}
