//! GitHub Releases based updater.
//!
//! SnapLingo ships as an NSIS installer attached to a GitHub release, so
//! "checking for updates" is one unauthenticated HTTP call and "updating" is
//! "download that installer and run it". Deliberately *not* using
//! `tauri-plugin-updater`: it needs a signing key pair plus `latest.json` in
//! every release, which turns `npm run tauri build` into something that fails
//! whenever an environment variable is missing. The trade-off is that the
//! download is only verified by HTTPS, not by a signature — documented in the
//! README.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

const USER_AGENT: &str = concat!("SnapLingo/", env!("CARGO_PKG_VERSION"));
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

pub const DEFAULT_REPO: &str = "AlanNiew/snap-lingo";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub has_update: bool,
    pub published_at: String,
    pub notes: String,
    pub page_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Downloaded {
    pub path: String,
    pub name: String,
    pub bytes: u64,
}

impl UpdateInfo {
    fn failed(current: &str, message: impl Into<String>) -> Self {
        Self {
            current: current.to_string(),
            error: Some(message.into()),
            ..Self::default()
        }
    }
}

/// Version of the running build, as the installer names it (`tauri.conf.json`).
pub fn app_version(app: &tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Accepts `owner/name`, a GitHub URL, a trailing `.git`, stray whitespace —
/// and falls back to the default repository when nothing usable is left.
pub fn normalize_repo(repo: &str) -> String {
    let raw = repo.trim();
    let trimmed = raw
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .trim_end_matches('/');
    let slug = trimmed
        .rsplit_once("github.com/")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed)
        .trim_matches('/');
    let mut parts = slug.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(name), None) if !owner.is_empty() && !name.is_empty() => {
            format!("{owner}/{name}")
        }
        _ => DEFAULT_REPO.to_string(),
    }
}

fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
        .context("无法初始化网络连接")
}

/// Latest published (non-draft, non-prerelease) release of the repository.
pub async fn check(repo: &str, current: &str) -> UpdateInfo {
    let slug = normalize_repo(repo);
    let current = current.to_string();
    let http = match client(CHECK_TIMEOUT) {
        Ok(client) => client,
        Err(err) => return UpdateInfo::failed(&current, format!("{err}")),
    };
    let url = format!("https://api.github.com/repos/{slug}/releases/latest");
    log::info!("检查更新：GET {url}");
    let response = match http
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            return UpdateInfo::failed(
                &current,
                format!("连不上 GitHub（{err}）。如果你在中国大陆，可能需要代理或稍后再试。"),
            )
        }
    };
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return UpdateInfo::failed(
            &current,
            format!("仓库 {slug} 还没有发布任何版本，或者它是私有仓库（私有仓库需要先改成公开）。"),
        );
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return UpdateInfo::failed(&current, "GitHub 接口限流了，请过一会儿再试。");
    }
    if !status.is_success() {
        return UpdateInfo::failed(&current, format!("GitHub 返回 HTTP {status}。"));
    }
    let body: serde_json::Value = match response.json().await {
        Ok(body) => body,
        Err(err) => return UpdateInfo::failed(&current, format!("GitHub 返回的内容看不懂：{err}")),
    };

    let latest = body
        .get("tag_name")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim_start_matches(['v', 'V'])
        .to_string();
    if latest.is_empty() {
        return UpdateInfo::failed(&current, "最新版本没有版本号（tag_name 为空）。");
    }
    let (asset_name, asset_url, asset_size) = pick_asset(&body);
    UpdateInfo {
        has_update: compare_versions(&latest, &current) == Ordering::Greater,
        latest,
        current,
        published_at: body
            .get("published_at")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        notes: truncate(
            body.get("body").and_then(|value| value.as_str()).unwrap_or_default(),
            4000,
        ),
        page_url: body
            .get("html_url")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
        asset_name,
        asset_url,
        asset_size,
        error: None,
    }
}

/// Preference order: the NSIS installer we actually build, then any setup exe,
/// then any exe, then nothing (the UI falls back to the release page).
fn pick_asset(release: &serde_json::Value) -> (String, String, u64) {
    let empty = Vec::new();
    let assets = release
        .get("assets")
        .and_then(|value| value.as_array())
        .unwrap_or(&empty);
    let mut best: Option<(&serde_json::Value, u8)> = None;
    for asset in assets {
        let name = asset
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let rank = if name.ends_with("_x64-setup.exe") {
            3
        } else if name.ends_with(".exe") && name.contains("setup") {
            2
        } else if name.ends_with(".exe") {
            1
        } else {
            0
        };
        if rank > 0 && best.map(|(_, best_rank)| rank > best_rank).unwrap_or(true) {
            best = Some((asset, rank));
        }
    }
    match best {
        Some((asset, _)) => (
            asset
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            asset
                .get("browser_download_url")
                .and_then(|value| value.as_str())
                .unwrap_or_default()
                .to_string(),
            asset
                .get("size")
                .and_then(|value| value.as_u64())
                .unwrap_or(0),
        ),
        None => (String::new(), String::new(), 0),
    }
}

/// Lenient semver comparison: numeric segments only, so `v1.2` < `1.10.0` and
/// `1.0.0-beta.2` == `1.0.0` (a local build ahead of the tag still counts as
/// "not older").
pub fn compare_versions(left: &str, right: &str) -> Ordering {
    let a = numeric_segments(left);
    let b = numeric_segments(right);
    let len = a.len().max(b.len());
    for index in 0..len {
        let x = a.get(index).copied().unwrap_or(0);
        let y = b.get(index).copied().unwrap_or(0);
        match x.cmp(&y) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

fn numeric_segments(version: &str) -> Vec<u64> {
    // Only the release part counts: `1.0.0-beta.2` is the same *version* as
    // `1.0.0` for update purposes, and a local build must never be told that a
    // pre-release of its own version is "newer".
    let release = version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+', ' '])
        .next()
        .unwrap_or_default();
    release
        .split('.')
        .filter_map(|part| {
            let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse().ok()
        })
        .collect()
}

/// Byte counter for the download UI. `total` is 0 when the server does not send
/// a `Content-Length` (rare for GitHub assets, but handled).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProgress {
    pub received: u64,
    pub total: u64,
}

/// Downloads the installer of the newest release into the temp directory.
///
/// `on_progress` is called while the body streams in (throttled) and once more
/// with the final sizes, so the UI can show a real progress bar instead of a
/// spinner that looks stuck on a slow connection.
pub async fn download(
    repo: &str,
    current: &str,
    mirror: &str,
    on_progress: impl Fn(UpdateProgress),
) -> Result<Downloaded> {
    let info = check(repo, current).await;
    if let Some(error) = info.error {
        bail!("{error}");
    }
    if info.asset_url.is_empty() {
        bail!("最新版本 {} 没有附带安装包，请打开 Release 页面手动下载。", info.latest);
    }
    let http = client(DOWNLOAD_TIMEOUT)?;
    // A mirror is prefixed verbatim (`https://ghproxy.net/` + the GitHub URL),
    // which is the shape every accelerator in the wild expects.
    let url = if mirror.trim().is_empty() {
        info.asset_url.clone()
    } else {
        format!("{}{}", mirror.trim(), info.asset_url)
    };
    log::info!("下载更新：GET {url}");
    let response = http
        .get(&url)
        .header("Accept", "application/octet-stream")
        .send()
        .await
        .with_context(|| {
            format!(
                "下载 {} 失败（{url}）。如果这里连不上 GitHub，可在「更新来源」里填一个下载加速前缀，或点「用浏览器下载」。",
                info.asset_name
            )
        })?;
    let status = response.status();
    if !status.is_success() {
        bail!("下载安装包失败：HTTP {status}。");
    }
    let total = response.content_length().unwrap_or(0);
    log::info!("开始下载更新包：{}（{} 字节）", info.asset_name, total);
    on_progress(UpdateProgress { received: 0, total });

    let mut payload: Vec<u8> = Vec::with_capacity(total as usize);
    let mut received: u64 = 0;
    let mut last_report = std::time::Instant::now();
    let mut response = response;
    while let Some(part) = response
        .chunk()
        .await
        .context("下载安装包时连接中断")?
    {
        received += part.len() as u64;
        payload.extend_from_slice(&part);
        // ~8 events/second is plenty for a progress bar and keeps the IPC quiet.
        if last_report.elapsed() >= std::time::Duration::from_millis(120) {
            on_progress(UpdateProgress { received, total });
            last_report = std::time::Instant::now();
        }
    }
    if payload.is_empty() {
        bail!("下载到的安装包是空的，请稍后重试或到 Release 页面手动下载。");
    }
    on_progress(UpdateProgress {
        received,
        total: total.max(received),
    });

    let dir = std::env::temp_dir().join("SnapLingo");
    std::fs::create_dir_all(&dir).context("无法创建临时目录")?;
    let path = dir.join(safe_file_name(&info.asset_name));
    std::fs::write(&path, &payload).context("无法写入安装包")?;
    log::info!("更新包已保存：{}（{} 字节）", path.display(), payload.len());
    Ok(Downloaded {
        path: path.to_string_lossy().to_string(),
        name: safe_file_name(&info.asset_name),
        bytes: payload.len() as u64,
    })
}

fn safe_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    if cleaned.to_ascii_lowercase().ends_with(".exe") {
        cleaned
    } else {
        "SnapLingo-setup.exe".to_string()
    }
}

/// Launches the downloaded installer right away and returns; this process then
/// exits a moment later (the callers do that on a 400ms timer) so the locked
/// executable is free by the time the installer copies its files.
///
/// Flags: `/S` = silent (no wizard), `/UPDATE` = skip the WebView2 section, and
/// `/R` = relaunch the app once the install has finished.
///
/// This used to run `cmd /C timeout /t 2 … & start "" …` instead: it needed a
/// console (which our spawned child never has — `timeout` bails with
/// "Input redirection is not supported") and its quoting broke into pieces
/// once Rust's `\"` escaping met cmd's quote stripping — machines that were not
/// this dev box saw Windows complain about a missing file called `\\`. CreateProcess
/// on the installer itself has no such rapids to fall into.
pub fn install(path: &Path) -> Result<()> {
    if !path.is_file() {
        bail!("安装包不存在：{}", path.display());
    }
    std::process::Command::new(path)
        .args(["/S", "/UPDATE", "/R"])
        .spawn()
        .context("无法启动安装程序")?;
    log::info!("已启动安装程序：{}，本进程即将退出", path.display());
    Ok(())
}

/// Path of a previously downloaded installer, if it is still around.
pub fn downloaded_path(name: &str) -> Option<PathBuf> {
    let path = std::env::temp_dir().join("SnapLingo").join(safe_file_name(name));
    path.is_file().then_some(path)
}

/// An installer an earlier session already downloaded. Reusing it means clicking
/// the button twice does not pull the same 2.6 MB again.
pub fn cached(asset_name: &str) -> Option<Downloaded> {
    if asset_name.trim().is_empty() {
        return None;
    }
    let name = safe_file_name(asset_name);
    let path = downloaded_path(&name)?;
    let bytes = std::fs::metadata(&path).ok()?.len();
    (bytes > 0).then(|| Downloaded {
        path: path.to_string_lossy().to_string(),
        name,
        bytes,
    })
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(limit).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repositories_are_normalised() {
        assert_eq!(normalize_repo("AlanNiew/snap-lingo"), "AlanNiew/snap-lingo");
        assert_eq!(
            normalize_repo(" https://github.com/AlanNiew/snap-lingo.git/ "),
            "AlanNiew/snap-lingo"
        );
        assert_eq!(normalize_repo(""), DEFAULT_REPO);
        assert_eq!(normalize_repo("not-a-repo"), DEFAULT_REPO);
        assert_eq!(normalize_repo("a/b/c"), DEFAULT_REPO);
    }

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("v1.0.1", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0.0-beta.2", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("0.9", "1.0.0"), Ordering::Less);
    }

    #[test]
    fn the_nsis_installer_is_preferred() {
        let release = serde_json::json!({
            "assets": [
                { "name": "SnapLingo_1.2.0_x64_en-US.msi", "browser_download_url": "https://x/msi", "size": 10 },
                { "name": "portable.exe", "browser_download_url": "https://x/plain", "size": 20 },
                { "name": "SnapLingo_1.2.0_x64-setup.exe", "browser_download_url": "https://x/nsis", "size": 30 }
            ]
        });
        let (name, url, size) = pick_asset(&release);
        assert_eq!(name, "SnapLingo_1.2.0_x64-setup.exe");
        assert_eq!(url, "https://x/nsis");
        assert_eq!(size, 30);
    }

    #[test]
    fn a_release_without_binaries_falls_back_to_the_page() {
        let release = serde_json::json!({ "assets": [{ "name": "notes.txt" }] });
        let (name, url, size) = pick_asset(&release);
        assert!(name.is_empty() && url.is_empty() && size == 0);
    }

    #[test]
    fn asset_names_are_sanitised() {
        assert_eq!(safe_file_name("SnapLingo_1.2.0_x64-setup.exe"), "SnapLingo_1.2.0_x64-setup.exe");
        assert_eq!(safe_file_name("a/b.exe"), "a_b.exe");
        assert_eq!(safe_file_name("payload.zip"), "SnapLingo-setup.exe");
    }
}
