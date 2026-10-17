//! Model / API access: OpenAI-compatible chat completions and DeepL.
//!
//! Everything that talks to the network lives here so the pipeline stays a pure
//! orchestration layer. Requests carry an explicit timeout, retry on the
//! retryable class of failures (429 / 5xx / transport), and strip model
//! "thinking" leakage from the reply.

use crate::config::{OcrConfig, TranslateConfig};
use crate::model::ConnectionCheck;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_ATTEMPTS: u32 = 3;
/// Translate in slices of roughly this many characters.
const CHUNK_CHARS: usize = 2400;

#[derive(Clone)]
pub struct Providers {
    client: reqwest::Client,
}

impl Providers {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("SnapLingo/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("could not build the HTTP client")?;
        Ok(Self { client })
    }

    /* ------------------------------------------------------------- OCR */

    /// Vision OCR. When `one_shot` is set the model also translates, saving a
    /// whole round trip. Returns `(source_text, translation)`.
    pub async fn vision(
        &self,
        cfg: &OcrConfig,
        image_data_url: &str,
        target_lang: &str,
        one_shot: bool,
    ) -> Result<(String, Option<String>)> {
        require_key(&cfg.api_key, "OCR / 视觉模型")?;
        if cfg.model.trim().is_empty() {
            bail!("尚未配置 OCR 模型名称。");
        }

        let (system, user_text, json_mode) = if one_shot {
            (
                one_shot_system(target_lang),
                "请识别这张图片并翻译。".to_string(),
                true,
            )
        } else {
            (ocr_system(&cfg.prompt), "请识别这张图片中的文字。".to_string(), false)
        };

        let messages = vec![
            json!({ "role": "system", "content": system }),
            json!({
                "role": "user",
                "content": [
                    { "type": "text", "text": user_text },
                    { "type": "image_url", "image_url": { "url": image_data_url, "detail": "high" } }
                ]
            }),
        ];

        let raw = self
            .chat(&cfg.base_url, &cfg.api_key, &cfg.model, messages, 4096, json_mode)
            .await?;

        if one_shot {
            let reply = parse_one_shot(&raw);
            let source = reply.source.trim().to_string();
            let translation = reply.translation.trim().to_string();
            if source.is_empty() && translation.is_empty() {
                bail!("视觉模型没有返回可用内容。");
            }
            Ok((source, if translation.is_empty() { None } else { Some(translation) }))
        } else {
            let text = clean_model_text(&raw);
            if text.is_empty() {
                bail!("没有识别到文字。请尝试框选更清晰、更大的区域。");
            }
            Ok((text, None))
        }
    }

    /* ------------------------------------------------------- translate */

    pub async fn translate(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        match cfg.provider.as_str() {
            "none" => Ok(String::new()),
            "deepl" => self.deepl(cfg, text).await,
            _ => self.translate_chat(cfg, text).await,
        }
    }

    async fn translate_chat(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "翻译")?;
        if cfg.model.trim().is_empty() {
            bail!("尚未配置翻译模型名称。");
        }
        let target = language_name(&cfg.target_lang);
        let system = translate_system(&cfg.prompt, &target);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let messages = vec![
                json!({ "role": "system", "content": system }),
                json!({ "role": "user", "content": chunk }),
            ];
            let reply = self
                .chat(&cfg.base_url, &cfg.api_key, &cfg.model, messages, 4096, false)
                .await?;
            let cleaned = clean_model_text(&reply);
            if cleaned.is_empty() {
                bail!("模型返回了空译文。");
            }
            out.push(cleaned);
        }
        Ok(out.join("\n"))
    }

    async fn deepl(&self, cfg: &TranslateConfig, text: &str) -> Result<String> {
        require_key(&cfg.api_key, "DeepL")?;
        let endpoint = if cfg.deepl_plan == "pro" {
            "https://api.deepl.com/v2/translate"
        } else {
            "https://api-free.deepl.com/v2/translate"
        };
        let target = deepl_target(&cfg.target_lang);

        let mut out = Vec::new();
        for chunk in chunk_text(text, CHUNK_CHARS) {
            let body = json!({
                "text": [chunk],
                "target_lang": target,
                "preserve_formatting": true,
            });
            let resp = self
                .send_with_retry(
                    || {
                        self.client
                            .post(endpoint)
                            .header("Authorization", format!("DeepL-Auth-Key {}", cfg.api_key.trim()))
                            .json(&body)
                    },
                    "DeepL",
                )
                .await?;
            let parsed: Value = serde_json::from_str(&resp)?;
            let translated = parsed
                .get("translations")
                .and_then(|t| t.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|t| t.get("text").and_then(|s| s.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if translated.trim().is_empty() {
                bail!("DeepL 返回了空译文（请检查目标语言代码与套餐类型）。");
            }
            out.push(translated);
        }
        Ok(out.join("\n"))
    }

    /* ------------------------------------------------------------ misc */

    pub async fn list_models(&self, base_url: &str, api_key: &str) -> Result<Vec<String>> {
        let url = format!("{}/models", base_url.trim_end_matches('/'));
        let resp = self
            .send_with_retry(
                || {
                    let mut req = self.client.get(&url);
                    if !api_key.trim().is_empty() {
                        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
                    }
                    req
                },
                "模型列表",
            )
            .await?;
        let parsed: Value = serde_json::from_str(&resp)?;
        let mut models: Vec<String> = parsed
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if models.is_empty() {
            bail!("该接口没有返回任何模型。请确认 Base URL 是否正确（应指向 /v1）。");
        }
        models.sort();
        Ok(models)
    }

    /// Prove the endpoint + key + model work. Never panics; the failure text is
    /// what the settings UI shows.
    pub async fn test(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        kind: &str,
    ) -> ConnectionCheck {
        let started = Instant::now();
        let messages = vec![json!({ "role": "user", "content": "Reply with the single word: ok" })];
        let result = self.chat(base_url, api_key, model, messages, 16, false).await;
        let elapsed_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(text) => ConnectionCheck {
                ok: true,
                message: if kind == "translate" {
                    "翻译接口连接正常".to_string()
                } else {
                    "OCR 接口连接正常".to_string()
                },
                elapsed_ms,
                sample: Some(text.chars().take(120).collect()),
            },
            Err(err) => ConnectionCheck {
                ok: false,
                message: format!("{err:#}"),
                elapsed_ms,
                sample: None,
            },
        }
    }

    /* -------------------------------------------------------- internals */

    /// One chat completion. `json_mode` asks for a JSON object when the provider
    /// supports it, and is dropped silently when it does not.
    async fn chat(
        &self,
        base_url: &str,
        api_key: &str,
        model: &str,
        messages: Vec<Value>,
        max_tokens: u32,
        json_mode: bool,
    ) -> Result<String> {
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));

        let build = |with_json_mode: bool, with_max_tokens: bool| {
            let mut body = json!({
                "model": model,
                "messages": messages,
                "temperature": 0.2,
                "stream": false,
            });
            if with_max_tokens {
                body["max_tokens"] = json!(max_tokens);
            }
            if with_json_mode {
                body["response_format"] = json!({ "type": "json_object" });
            }
            body
        };

        let raw = match self
            .send_with_retry(
                || {
                    self.client
                        .post(&url)
                        .header("Authorization", format!("Bearer {}", api_key.trim()))
                        .json(&build(json_mode, true))
                },
                "聊天补全",
            )
            .await
        {
            Ok(v) => v,
            Err(err) if is_param_rejection(&err) => {
                // Provider dislikes response_format -> retry as plain text.
                log::warn!("provider rejected optional params, retrying plain: {err:#}");
                match self
                    .send_with_retry(
                        || {
                            self.client
                                .post(&url)
                                .header("Authorization", format!("Bearer {}", api_key.trim()))
                                .json(&build(false, false))
                        },
                        "聊天补全",
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(_) => return Err(err),
                }
            }
            Err(err) => return Err(err),
        };

        let parsed: Value = serde_json::from_str(&raw).context("响应不是合法 JSON")?;
        let message = parsed
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .ok_or_else(|| anyhow!("响应缺少 choices[0].message：{}", truncate(&raw, 400)))?;

        // `content` may be a string or an array of parts depending on provider.
        let content = match message.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(parts)) => parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        };

        if content.trim().is_empty() {
            // Reasoning models sometimes only fill `reasoning_content`.
            if let Some(rc) = message.get("reasoning_content").and_then(|r| r.as_str()) {
                if !rc.trim().is_empty() {
                    return Ok(rc.to_string());
                }
            }
            let finish = parsed
                .get("choices")
                .and_then(|c| c.get(0))
                .and_then(|c| c.get("finish_reason"))
                .and_then(|f| f.as_str())
                .unwrap_or("unknown");
            bail!("模型返回了空内容（finish_reason={finish}）。");
        }
        Ok(content)
    }

    /// POST with bounded exponential backoff on transient failures.
    async fn send_with_retry<F>(&self, build: F, what: &str) -> Result<String>
    where
        F: Fn() -> reqwest::RequestBuilder,
    {
        let mut last: Option<anyhow::Error> = None;
        for attempt in 1..=MAX_ATTEMPTS {
            let outcome = match build().send().await {
                Err(err) => Err(anyhow!(classify_transport(&err))),
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.text().await.unwrap_or_default();
                    if status.is_success() {
                        return Ok(body);
                    }
                    Err(anyhow!(explain_http_error(status.as_u16(), &body)))
                }
            };

            match outcome {
                Ok(v) => return Ok(v),
                Err(err) => {
                    let retryable = is_retryable(&err);
                    log::debug!("{what} attempt {attempt}/{MAX_ATTEMPTS} failed: {err}");
                    last = Some(err);
                    if !retryable || attempt == MAX_ATTEMPTS {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(400 * 2u64.pow(attempt - 1))).await;
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("{what} 失败")))
    }
}

/* --------------------------------------------------------------- helpers */

fn require_key(key: &str, what: &str) -> Result<()> {
    if key.trim().is_empty() {
        bail!("{what} 尚未配置 API Key。请打开设置填写。");
    }
    Ok(())
}

fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = format!("{err}");
    msg.starts_with("RETRY:")
}

fn classify_transport(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "RETRY: 请求超时（网络较慢或模型响应过久），请重试。".to_string()
    } else if err.is_connect() {
        "RETRY: 无法连接到服务端点，请检查网络或 Base URL。".to_string()
    } else {
        format!("RETRY: 网络请求失败：{err}")
    }
}

fn explain_http_error(status: u16, body: &str) -> String {
    let detail = extract_error_message(body).unwrap_or_else(|| truncate(body, 300));

    let prefix = match status {
        401 | 403 => "RETRY: 鉴权失败（401/403）——请检查 API Key 是否正确、是否有该模型权限。",
        429 => "RETRY: 触发限流（429）——稍后自动重试。",
        500..=599 => "RETRY: 服务端错误——稍后自动重试。",
        _ => "请求被拒绝：",
    };
    format!("{prefix} HTTP {status}: {detail}")
}

fn extract_error_message(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let err = v.get("error")?;
    if let Some(s) = err.as_str() {
        return Some(s.to_string());
    }
    err.get("message")
        .and_then(|m| m.as_str())
        .map(str::to_string)
        .or_else(|| Some(err.to_string()))
}

/// True when the error looks like "you passed an unsupported parameter".
fn is_param_rejection(err: &anyhow::Error) -> bool {
    let msg = format!("{err}").to_lowercase();
    !msg.starts_with("retry: ")
        && (msg.contains("response_format")
            || msg.contains("max_tokens")
            || msg.contains("unsupported parameter")
            || msg.contains("unrecognized"))
}

fn truncate(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

/// Strip thinking blocks, code fences and "译文：" style prefixes that models
/// like to add even when told not to.
pub fn clean_model_text(raw: &str) -> String {
    let mut s = raw.trim().to_string();

    for (open, close) in [
        ("<think>", "</think>"),
        ("<thinking>", "</thinking>"),
        ("<reasoning>", "</reasoning>"),
    ] {
        while let Some(a) = s.find(open) {
            match s[a..].find(close) {
                Some(b) => {
                    let end = a + b + close.len();
                    s.replace_range(a..end, "");
                }
                None => {
                    s.truncate(a);
                    break;
                }
            }
        }
    }

    // ``` … ``` fences, possibly with a language tag.
    if s.starts_with("```") {
        let rest = &s[3..];
        let rest = match rest.find('\n') {
            Some(nl) if !rest[..nl].contains('`') => &rest[nl + 1..],
            _ => rest,
        };
        s = rest.trim_end().to_string();
        if let Some(pos) = s.rfind("```") {
            s.truncate(pos);
        }
    }

    // Leading labels such as "译文：" / "Translation:".
    for label in ["译文：", "译文:", "翻译：", "翻译:", "Translation:", "translation:"] {
        if let Some(rest) = s.strip_prefix(label) {
            s = rest.trim_start().to_string();
            break;
        }
    }

    // Whole-answer quotes.
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') && !t[1..t.len() - 1].contains('"') {
        s = t[1..t.len() - 1].to_string();
    }

    s.trim().to_string()
}

/// Parse the one-shot JSON reply, tolerating surrounding prose and fences.
pub fn parse_one_shot(raw: &str) -> crate::model::OneShotReply {
    let cleaned = clean_model_text(raw);
    if let Ok(v) = serde_json::from_str::<crate::model::OneShotReply>(&cleaned) {
        return v;
    }
    // Sometimes the object is embedded in prose; grab the outermost braces.
    if let (Some(a), Some(b)) = (cleaned.find('{'), cleaned.rfind('}')) {
        if b > a {
            if let Ok(v) = serde_json::from_str::<crate::model::OneShotReply>(&cleaned[a..=b]) {
                return v;
            }
        }
    }
    crate::model::OneShotReply { source: cleaned, translation: String::new() }
}

/// Split on paragraph / sentence boundaries so each API call stays small.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut count = 0usize;

    for line in text.split_inclusive('\n') {
        let line_len = line.chars().count();
        if count > 0 && count + line_len > limit {
            chunks.push(std::mem::take(&mut current).trim_end().to_string());
            count = 0;
        }
        if line_len > limit {
            // A single monster line: hard-split it.
            let mut piece = String::new();
            for ch in line.chars() {
                piece.push(ch);
                if piece.chars().count() >= limit {
                    if !current.is_empty() {
                        chunks.push(std::mem::take(&mut current).trim_end().to_string());
                    }
                    chunks.push(std::mem::take(&mut piece));
                }
            }
            current.push_str(&piece);
            count = current.chars().count();
            continue;
        }
        current.push_str(line);
        count += line_len;
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim_end().to_string());
    }
    chunks.retain(|c| !c.trim().is_empty());
    chunks
}

/* -------------------------------------------------------------- prompts */

fn ocr_system(custom: &str) -> String {
    if !custom.trim().is_empty() {
        return custom.to_string();
    }
    "你是一个高精度的 OCR 引擎。请提取图片中的所有可见文字。\n\
     规则：\n\
     1. 原样输出文字，不要翻译、不要解释、不要总结。\n\
     2. 保留原有的换行与分段；表格用制表符分隔列。\n\
     3. 忽略纯装饰性元素（图标、边框、背景图案）。\n\
     4. 若图片中没有文字，只输出：[NO_TEXT]\n\
     5. 直接输出文字，不要使用代码块包裹。"
        .to_string()
}

fn one_shot_system(target: &str) -> String {
    format!(
        "你是一个截图翻译引擎。请识别图片中的文字，并翻译成{target}。\n\
         只输出一个 JSON 对象，不要输出任何其他内容，格式为：\n\
         {{\"source\":\"识别出的原文\",\"translation\":\"译文\"}}\n\
         要求：原文保留换行；译文准确、自然、符合目标语言习惯；\
         专有名词、代码、命令、URL、变量名保持原样不翻译。"
    )
}

fn translate_system(custom: &str, target: &str) -> String {
    if !custom.trim().is_empty() {
        return format!("{custom}\n\n目标语言：{target}");
    }
    format!(
        "你是一个专业的翻译引擎。请将用户提供的内容翻译成{target}。\n\
         规则：\n\
         1. 只输出译文，不要输出原文、解释、注释或任何前言后语。\n\
         2. 保留原有的换行、缩进与段落结构。\n\
         3. 代码、命令、文件路径、URL、变量名、专有名词保持原样。\n\
         4. 若输入已经是{target}，原样输出即可。\n\
         5. 不要使用代码块包裹译文。"
    )
}

/// Human-readable target language name used inside prompts.
pub fn language_name(tag: &str) -> String {
    match tag.trim() {
        "zh-CN" | "zh" | "zh-Hans" | "zh-SG" => "简体中文",
        "zh-TW" | "zh-Hant" | "zh-HK" => "繁体中文",
        "en" | "en-US" | "en-GB" => "英文",
        "ja" | "ja-JP" => "日文",
        "ko" | "ko-KR" => "韩文",
        "fr" => "法文",
        "de" => "德文",
        "es" => "西班牙文",
        "ru" => "俄文",
        "pt" | "pt-BR" => "葡萄牙文",
        "it" => "意大利文",
        "ar" => "阿拉伯文",
        "th" => "泰文",
        "vi" => "越南文",
        other => other,
    }
    .to_string()
}

/// DeepL expects upper-cased language codes with its own quirks.
pub fn deepl_target(tag: &str) -> String {
    match tag.trim() {
        "zh-CN" | "zh" | "zh-Hans" | "zh-SG" => "ZH".to_string(),
        "zh-TW" | "zh-Hant" | "zh-HK" => "ZH-HANT".to_string(),
        "en" | "en-US" | "en-GB" => "EN-US".to_string(),
        "pt" | "pt-BR" => "PT-BR".to_string(),
        // Anything else: DeepL wants the bare, upper-cased primary subtag.
        other => other.split('-').next().unwrap_or(other).to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_thinking_and_fences() {
        assert_eq!(clean_model_text("<think>hmm</think>你好"), "你好");
        assert_eq!(clean_model_text("```\n你好\n```"), "你好");
        assert_eq!(clean_model_text("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(clean_model_text("译文： 你好"), "你好");
        assert_eq!(clean_model_text("\"你好\""), "你好");
        // An unclosed thinking block must not leak its contents.
        assert_eq!(clean_model_text("<think>secret 你好"), "");
    }

    #[test]
    fn one_shot_tolerates_prose_wrappers() {
        let r = parse_one_shot("好的：\n{\"source\":\"Hello\",\"translation\":\"你好\"}");
        assert_eq!(r.source, "Hello");
        assert_eq!(r.translation, "你好");

        // A non-JSON answer degrades to "source only" rather than failing.
        let r = parse_one_shot("Hello world");
        assert_eq!(r.source, "Hello world");
        assert!(r.translation.is_empty());
    }

    #[test]
    fn chunking_respects_limit_and_loses_nothing() {
        let text = "line one\nline two\nline three\nline four";
        let chunks = chunk_text(text, 20);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(c.chars().count() <= 20, "chunk too long: {c:?}");
        }
        let joined: String = chunks.join("\n");
        assert!(joined.contains("line one") && joined.contains("line four"));

        // A single oversized line is hard-split, not dropped.
        let long = "x".repeat(50);
        let chunks = chunk_text(&long, 10);
        assert_eq!(chunks.iter().map(|c| c.chars().count()).sum::<usize>(), 50);
    }

    #[test]
    fn short_text_is_a_single_chunk() {
        assert_eq!(chunk_text("  hi  ", 100), vec!["hi".to_string()]);
        assert!(chunk_text("   ", 100).is_empty());
    }

    #[test]
    fn http_errors_classify_retryability() {
        let e = anyhow!(explain_http_error(429, r#"{"error":{"message":"slow down"}}"#));
        assert!(is_retryable(&e));
        assert!(format!("{e}").contains("slow down"));

        let e = anyhow!(explain_http_error(400, r#"{"error":{"message":"bad model"}}"#));
        assert!(!is_retryable(&e));
    }

    #[test]
    fn language_and_deepl_mapping() {
        assert_eq!(language_name("zh-CN"), "简体中文");
        assert_eq!(deepl_target("zh-CN"), "ZH");
        assert_eq!(deepl_target("zh-TW"), "ZH-HANT");
        assert_eq!(deepl_target("en-US"), "EN-US");
        assert_eq!(deepl_target("ja"), "JA");
    }

    #[test]
    fn param_rejection_detection_ignores_retryable() {
        assert!(is_param_rejection(&anyhow!(
            "请求被拒绝： HTTP 400: unsupported parameter response_format"
        )));
        assert!(!is_param_rejection(&anyhow!("RETRY: 触发限流（429）")));
    }
}
