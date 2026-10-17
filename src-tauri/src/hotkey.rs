//! Global shortcut registration.
//!
//! Registering is all-or-nothing per shortcut, so a failed re-register (the
//! classic case: another app already owns the combination) must not silently
//! leave the app without a hotkey. Callers revert to the previous accelerator.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

/// Replace whatever is registered with `accelerator`.
pub fn apply(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let manager = app.global_shortcut();
    // Clearing first is what makes "change my shortcut" work; without it the
    // old combination keeps firing.
    let _ = manager.unregister_all();

    let shortcut: Shortcut = accelerator
        .trim()
        .parse()
        .map_err(|_| format!("无法解析快捷键「{accelerator}」，示例：Ctrl+Alt+Z、CommandOrControl+Shift+T"))?;

    manager
        .register(shortcut)
        .map_err(|err| format!("注册快捷键「{accelerator}」失败：{err}（可能已被其他程序占用）"))
}
