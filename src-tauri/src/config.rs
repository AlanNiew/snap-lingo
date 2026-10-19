//! Persistent application configuration.
//!
//! Stored as JSON next to the app's config dir. Every field carries a serde
//! default so that an older config file keeps loading after an upgrade, and a
//! hand-edited partial file is still accepted.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const APP_DIR: &str = "SnapLingo";
pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+Z";
/// Default OpenAI-compatible endpoint, used by both the vision OCR engine and
/// the `openai` translation engine when the user has not typed one.
pub const DEFAULT_OPENAI_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
pub const DEFAULT_TEXT_MODEL: &str = "nvidia/nemotron-3-ultra-550b-a55b";
/// Local HTTP OCR service (Umi-OCR ships this endpoint by default).
pub const DEFAULT_LOCAL_OCR_URL: &str = "http://127.0.0.1:1224";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OcrConfig {
    /// `winocr` = offline Windows OCR engine, `vision` = multimodal LLM,
    /// `picture` = the translation engine reads the picture itself (one step),
    /// `local` = a local HTTP OCR service (Umi-OCR and compatible servers).
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
            base_url: DEFAULT_OPENAI_BASE_URL.into(),
            api_key: String::new(),
            model: String::new(),
            language: String::new(),
            one_shot: false,
            prompt: String::new(),
        }
    }
}

/// Credentials and endpoint for one translation engine. Every engine keeps its
/// own copy, which is what stops one service's API key from turning up in
/// another service's form after switching engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TranslateProfile {
    pub base_url: String,
    pub api_key: String,
    /// Secondary credential: Youdao app secret, Baidu app key, Tencent secretKey.
    pub api_secret: String,
    /// Azure Translator region, Tencent region.
    pub region: String,
    pub model: String,
    /// Service tier of the engine: `standard` / `llm` (Baidu, Youdao) or
    /// `llm` / `lite` (Tencent). Empty means the engine's own default.
    pub model_tier: String,
    /// How the request is authenticated: empty / `sign` (app id + secret) or
    /// `key` (Bearer token). Only Baidu's LLM endpoint offers both.
    pub auth: String,
    /// The Bearer credential: Baidu's `API Key` created in the console.
    pub bearer: String,
}

impl Default for TranslateProfile {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            region: String::new(),
            model: String::new(),
            model_tier: String::new(),
            auth: String::new(),
            bearer: String::new(),
        }
    }
}

impl TranslateProfile {
    /// True when the engine has nothing configured at all.
    pub fn is_blank(&self) -> bool {
        self.base_url.trim().is_empty()
            && self.api_key.trim().is_empty()
            && self.api_secret.trim().is_empty()
            && self.region.trim().is_empty()
            && self.model.trim().is_empty()
            && self.bearer.trim().is_empty()
    }

    /// The tier this engine will actually use, with blanks resolved.
    pub fn tier(&self, provider: &str) -> String {
        let tier = self.model_tier.trim();
        if !tier.is_empty() {
            return tier.to_string();
        }
        match provider {
            // Baidu's standard text translation works for everyone; the LLM
            // service has to be activated separately.
            "baidu" => "standard",
            // Youdao: 通用文本翻译 by default (the only one that works without
            // extra activation); `llm` picks 子曰 pro, `lite` picks 子曰 lite.
            "youdao" => "standard",
            // Tencent's picture translation is best on the LLM path, which is
            // also its default product.
            "tencent" => "llm",
            _ => "",
        }
        .to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TranslateConfig {
    /// `openai` | `deepl` | `microsoft` | `google` | `youdao` | `youdao-llm`
    /// | `baidu` | `tencent` | `none`
    pub provider: String,
    /// One entry per engine, keyed by provider id.
    pub profiles: BTreeMap<String, TranslateProfile>,
    pub target_lang: String,
    pub deepl_plan: String,
    pub prompt: String,
    /// Credentials from configs written before profiles existed. Read once to
    /// migrate them into `profiles`, never written back out.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub api_secret: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub region: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    /// Runtime carrier only: the tier of the selected engine, resolved from its
    /// profile just before a request is made. Never written to disk.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model_tier: String,
    /// Runtime carrier only: the authentication mode of the selected engine.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub auth: String,
    /// Runtime carrier only: the Bearer credential of the selected engine.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bearer: String,
}

impl TranslateConfig {
    /// Settings of one engine; a missing entry yields blanks.
    pub fn profile(&self, id: &str) -> TranslateProfile {
        self.profiles.get(id).cloned().unwrap_or_default()
    }

    /// Mutable settings of one engine, created on first use.
    pub fn profile_mut(&mut self, id: &str) -> &mut TranslateProfile {
        self.profiles.entry(id.to_string()).or_default()
    }

    /// Settings of the engine that is currently selected.
    pub fn active(&self) -> TranslateProfile {
        self.profile(&self.provider)
    }
}

impl Default for TranslateConfig {
    fn default() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "openai".to_string(),
            TranslateProfile {
                base_url: DEFAULT_OPENAI_BASE_URL.into(),
                model: DEFAULT_TEXT_MODEL.into(),
                ..Default::default()
            },
        );
        Self {
            provider: "openai".into(),
            profiles,
            target_lang: "zh-CN".into(),
            deepl_plan: "free".into(),
            prompt: String::new(),
            base_url: String::new(),
            api_key: String::new(),
            api_secret: String::new(),
            region: String::new(),
            model: String::new(),
            model_tier: String::new(),
            auth: String::new(),
            bearer: String::new(),
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
    /// `both` | `source` | `translation` — what the result window shows.
    pub result_view: String,
    /// Show the captured picture in the result window.
    pub result_show_image: bool,
    /// Let the result window grow/shrink to fit its content (off = fixed size).
    pub auto_fit: bool,
    pub close_on_blur: bool,
    pub result_font_size: u32,
    pub theme: String,
    pub language: String,
    /// Show the corner notice card when something fails.
    pub notify: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            auto_copy: false,
            result_view: "both".into(),
            result_show_image: true,
            auto_fit: false,
            close_on_blur: false,
            result_font_size: 16,
            theme: "system".into(),
            language: "zh".into(),
            notify: true,
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
        if !matches!(
            self.ocr.provider.as_str(),
            "winocr" | "vision" | "picture" | "local"
        ) {
            self.ocr.provider = "winocr".into();
        }
        if !matches!(
            self.translate.provider.as_str(),
            "openai"
                | "deepl"
                | "microsoft"
                | "google"
                | "youdao"
                | "baidu"
                | "tencent"
                | "none"
        ) {
            // Youdao's LLM translation used to be a channel of its own; it is now
            // the `llm` tier of the Youdao channel. Same for the stored selection.
            self.translate.provider = if self.translate.provider == "youdao-llm" {
                "youdao".into()
            } else {
                "openai".into()
            };
        }
        // Fold a left-over `youdao-llm` profile back into `youdao`, keeping the
        // credentials the user already entered.
        if let Some(old) = self.translate.profiles.remove("youdao-llm") {
            let target = self.translate.profiles.entry("youdao".into()).or_default();
            if target.api_key.trim().is_empty() {
                target.api_key = old.api_key;
            }
            if target.api_secret.trim().is_empty() {
                target.api_secret = old.api_secret;
            }
            if target.base_url.trim().is_empty() {
                target.base_url = old.base_url;
            }
            if target.model_tier.trim().is_empty() {
                target.model_tier = if old.model_tier.trim().is_empty() {
                    "llm".into()
                } else {
                    old.model_tier
                };
            }
        }
        // `pro` was the old name of the Youdao LLM tier; the merged channel and
        // the provider list both call it `llm` now.
        if let Some(p) = self.translate.profiles.get_mut("youdao") {
            if p.model_tier.trim() == "pro" {
                p.model_tier = "llm".into();
            }
        }
        if self.translate.deepl_plan != "pro" {
            self.translate.deepl_plan = "free".into();
        }
        self.capture.max_image_side = self.capture.max_image_side.clamp(320, 4096);
        self.capture.jpeg_quality = self.capture.jpeg_quality.clamp(40, 100);
        self.ui.result_font_size = self.ui.result_font_size.clamp(11, 32);
        if !matches!(self.ui.result_view.as_str(), "source" | "translation" | "both") {
            self.ui.result_view = "both".into();
        }
        self.history.limit = self.history.limit.clamp(0, 1000);
        if self.ocr.base_url.trim().is_empty() {
            self.ocr.base_url = DEFAULT_OPENAI_BASE_URL.into();
        }
        // Switching to the local service should not leave the OpenAI endpoint
        // behind just because it used to be the default.
        if self.ocr.provider == "local" && self.ocr.base_url == DEFAULT_OPENAI_BASE_URL {
            self.ocr.base_url = DEFAULT_LOCAL_OCR_URL.into();
        }
        self.ocr.base_url = self.ocr.base_url.trim_end_matches('/').to_string();
        // Configs written before per-engine profiles carried a single flat set of
        // credentials. Fold them into the profile of the engine that was selected
        // so nothing the user already typed is lost.
        let legacy = TranslateProfile {
            base_url: std::mem::take(&mut self.translate.base_url),
            api_key: std::mem::take(&mut self.translate.api_key),
            api_secret: std::mem::take(&mut self.translate.api_secret),
            region: std::mem::take(&mut self.translate.region),
            model: std::mem::take(&mut self.translate.model),
            model_tier: String::new(),
            auth: String::new(),
            bearer: String::new(),
        };
        if !legacy.is_blank() {
            let id = self.translate.provider.clone();
            let p = self.translate.profile_mut(&id);
            if p.base_url.trim().is_empty() {
                p.base_url = legacy.base_url;
            }
            if p.api_key.trim().is_empty() {
                p.api_key = legacy.api_key;
            }
            if p.api_secret.trim().is_empty() {
                p.api_secret = legacy.api_secret;
            }
            if p.region.trim().is_empty() {
                p.region = legacy.region;
            }
            if p.model.trim().is_empty() {
                p.model = legacy.model;
            }
        }
        // Only the OpenAI-compatible engine has a sensible default endpoint: every
        // other engine knows its own and reads an empty value as "use the built-in
        // one". The default model only makes sense for the OpenAI-compatible path.
        let openai = self.translate.profile_mut("openai");
        if openai.base_url.trim().is_empty() {
            openai.base_url = DEFAULT_OPENAI_BASE_URL.into();
        }
        if openai.model.trim().is_empty() {
            openai.model = DEFAULT_TEXT_MODEL.into();
        }
        for p in self.translate.profiles.values_mut() {
            p.base_url = p.base_url.trim().trim_end_matches('/').to_string();
            p.api_key = p.api_key.trim().to_string();
            p.api_secret = p.api_secret.trim().to_string();
            p.region = p.region.trim().to_string();
            p.model = p.model.trim().to_string();
            // Unknown tiers fall back to the engine's own default at use time.
            // `pro` no longer exists: it was the old Youdao LLM tier and is
            // rewritten to `llm` above.
            p.model_tier = match p.model_tier.trim() {
                "llm" | "lite" | "standard" => p.model_tier.trim().to_string(),
                _ => String::new(),
            };
            p.auth = match p.auth.trim() {
                "sign" | "key" => p.auth.trim().to_string(),
                _ => String::new(),
            };
            p.bearer = p.bearer.trim().to_string();
        }
        if self.translate.target_lang.trim().is_empty() {
            self.translate.target_lang = "zh-CN".into();
        }
    }

    /// The translation engine that reads the picture itself, when the OCR
    /// engine is set to `picture` and the selected service supports it.
    pub fn picture_provider(&self) -> Option<&'static str> {
        match self.translate.provider.as_str() {
            "youdao" => Some("youdao"),
            "baidu" => Some("baidu"),
            "tencent" => Some("tencent"),
            _ => None,
        }
    }

    /// True when OCR *and* translation are done by the translation service in a
    /// single request (no local OCR engine involved).
    pub fn uses_picture_translate(&self) -> bool {
        self.ocr.provider == "picture" && self.picture_provider().is_some()
    }

    /// True when a capture can produce something useful — i.e. the selected OCR
    /// engine is usable. A missing translation only fails the translation step,
    /// so it does not block taking the shot.
    pub fn is_ocr_ready(&self) -> bool {
        match self.ocr.provider.as_str() {
            // The picture translator reads the image itself, so it works
            // without any local OCR engine — but only with a service that can.
            "picture" => self.uses_picture_translate(),
            // Windows ships this engine; nothing to configure.
            "winocr" => true,
            // Local HTTP OCR service: the endpoint is all we need.
            "local" => !self.ocr.base_url.trim().is_empty(),
            // Multimodal model: needs credentials.
            _ => !self.ocr.api_key.trim().is_empty() && !self.ocr.model.trim().is_empty(),
        }
    }

    /// True when the configured pipeline can run end to end without further
    /// setup. Used for onboarding: the settings window opens when this is false.
    pub fn is_ready(&self) -> bool {
        let tr = &self.translate;
        let p = tr.active();
        let tr_ok = match tr.provider.as_str() {
            "none" => true,
            "deepl" | "microsoft" | "google" => !p.api_key.trim().is_empty(),
            // These engines sign the request with a second credential.
            "youdao" | "tencent" => {
                !p.api_key.trim().is_empty() && !p.api_secret.trim().is_empty()
            }
            // Baidu's LLM endpoint can also authenticate with a Bearer API Key,
            // in which case only the app id and that key are needed.
            "baidu" => {
                if p.tier("baidu") == "llm" && p.auth.trim() == "key" {
                    !p.api_key.trim().is_empty() && !p.bearer.trim().is_empty()
                } else {
                    !p.api_key.trim().is_empty() && !p.api_secret.trim().is_empty()
                }
            }
            _ => !p.api_key.trim().is_empty() && !p.model.trim().is_empty(),
        };
        self.is_ocr_ready() && tr_ok
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
    fn picture_translation_skips_ocr_requirements() {
        let mut cfg = AppConfig::default();
        // The multimodal engine needs a key and a model.
        cfg.ocr.provider = "vision".into();
        assert!(!cfg.is_ocr_ready());

        // Choosing the picture engine hands the work to the translation
        // service, so no local OCR engine is needed at all.
        cfg.translate.provider = "youdao".into();
        cfg.ocr.provider = "picture".into();
        cfg.sanitize();
        assert!(cfg.uses_picture_translate());
        assert!(cfg.is_ocr_ready());
        assert_eq!(cfg.picture_provider(), Some("youdao"));

        // An engine that cannot read a picture leaves the pipeline without OCR.
        cfg.translate.provider = "openai".into();
        assert!(!cfg.uses_picture_translate());
        assert!(!cfg.is_ocr_ready());

        // A local OCR service only needs its endpoint.
        cfg.ocr.provider = "local".into();
        cfg.sanitize();
        assert!(!cfg.uses_picture_translate());
        assert!(cfg.is_ocr_ready());
        assert_eq!(cfg.ocr.base_url, DEFAULT_LOCAL_OCR_URL);
    }

    #[test]
    fn tiers_resolve_to_engine_defaults() {
        let mut cfg = AppConfig::default();
        assert_eq!(cfg.translate.active().tier("openai"), "");
        cfg.translate.provider = "baidu".into();
        assert_eq!(cfg.translate.active().tier("baidu"), "standard");
        cfg.translate.provider = "tencent".into();
        assert_eq!(cfg.translate.active().tier("tencent"), "llm");

        cfg.translate.profile_mut("tencent").model_tier = "lite".into();
        assert_eq!(cfg.translate.active().tier("tencent"), "lite");
        cfg.sanitize();
        assert_eq!(cfg.translate.active().model_tier, "lite");

        cfg.translate.profile_mut("tencent").model_tier = "nonsense".into();
        cfg.sanitize();
        assert_eq!(cfg.translate.active().model_tier, "", "unknown tiers are dropped");
    }

    #[test]
    fn readiness_requires_keys_only_for_api_paths() {
        let mut cfg = AppConfig::default();
        // Offline OCR alone is enough to take a shot and recognise text; only
        // the translation step needs a service, and it reports its own error.
        assert!(cfg.is_ocr_ready(), "offline winocr alone should be usable");
        assert!(!cfg.is_ready(), "full readiness still needs a translation path");

        cfg.ocr.provider = "vision".into();
        assert!(!cfg.is_ocr_ready(), "vision without key/model must not be ready");

        cfg.ocr.api_key = "k".into();
        cfg.ocr.model = "m".into();
        assert!(cfg.is_ocr_ready());
        assert!(!cfg.is_ready(), "translation still needs a key");

        cfg.translate.provider = "none".into();
        assert!(cfg.is_ready());
    }
}
