//! The IPC surface is written down three times: on the Rust function
//! (`#[tauri::command]`), in the `generate_handler!` list, and in `src/lib/api.ts`
//! (`invoke("snake_case_name")`). A name that drifts in one of the three does not
//! fail to compile — it fails when a user presses that button.
//!
//! This test pins the three lists together by reading the sources, so the drift
//! becomes a red `cargo test` instead of a runtime "command not found".

const COMMANDS_RS: &str = include_str!("../src/commands.rs");
const LIB_RS: &str = include_str!("../src/lib.rs");
const PIPELINE_RS: &str = include_str!("../src/pipeline.rs");
const UPDATE_RS: &str = include_str!("../src/update.rs");
const API_TS: &str = include_str!("../../src/lib/api.ts");

fn first_ident(text: &str) -> String {
    text.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// Names carrying `#[tauri::command]`, read from the function signature below
/// each attribute.
fn declared_commands() -> Vec<String> {
    let mut out = Vec::new();
    let mut lines = COMMANDS_RS.lines();
    while let Some(line) = lines.next() {
        if line.trim() != "#[tauri::command]" {
            continue;
        }
        // The signature may sit one or two lines below the attribute (docs or a
        // multi-line `pub async fn`); either way the name ends at the `(`.
        for sig in lines.by_ref().take(3) {
            let sig = sig.trim();
            let name = sig
                .strip_prefix("pub async fn ")
                .or_else(|| sig.strip_prefix("pub fn "));
            if let Some(rest) = name {
                out.push(first_ident(rest));
                break;
            }
        }
    }
    out
}

/// Names listed in `tauri::generate_handler![...]`.
fn registered_commands() -> Vec<String> {
    const OPEN: &str = "invoke_handler(tauri::generate_handler![";
    let start = LIB_RS.find(OPEN).expect("generate_handler block not found") + OPEN.len();
    let rest = &LIB_RS[start..];
    let end = rest.find("]);").expect("generate_handler block is not closed");
    rest[..end]
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            entry
                .strip_prefix("commands::")
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
        })
        .collect()
}

/// Command names handed to `invoke(...)` from the frontend wrapper.
fn api_commands() -> Vec<String> {
    let bytes = API_TS.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(pos) = API_TS[i..].find("invoke") {
        let mut j = i + pos + "invoke".len();
        // Skip the optional generic arguments: `invoke<SessionImage | null>(`.
        if bytes.get(j) == Some(&b'<') {
            let mut depth = 0usize;
            while j < bytes.len() {
                match bytes[j] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        if depth == 0 {
                            j += 1;
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
        }
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if bytes.get(j) != Some(&b'(') {
            // A mention in prose or the `import { invoke }` line, not a call.
            i = j;
            continue;
        }
        j += 1;
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if bytes.get(j) != Some(&b'"') {
            i = j;
            continue;
        }
        j += 1;
        let start = j;
        while bytes.get(j).is_some_and(|c| *c != b'"') {
            j += 1;
        }
        out.push(API_TS[start..j].to_string());
        i = j;
    }
    out
}

/// Event names in the `EVENTS` map the frontend listens on.
fn api_events() -> Vec<String> {
    let start = API_TS.find("const EVENTS = {").expect("EVENTS map not found");
    let rest = &API_TS[start..];
    let end = rest.find("} as const").expect("EVENTS map is not closed");
    let block = &rest[..end];
    block
        .split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, value)| value.to_string())
        .collect()
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_declared_command_is_registered() {
    let declared = sorted(declared_commands());
    let registered = sorted(registered_commands());
    assert_eq!(
        declared, registered,
        "命令函数与 generate_handler! 列表不一致（左=函数，右=注册表）"
    );
}

#[test]
fn every_registered_command_has_a_frontend_wrapper() {
    let registered = sorted(registered_commands());
    let api = sorted(api_commands());
    assert_eq!(
        registered, api,
        "generate_handler! 与 src/lib/api.ts 的命令名不一致（左=后端，右=前端）"
    );
}

#[test]
fn every_frontend_event_exists_in_rust() {
    let rust = [PIPELINE_RS, LIB_RS, COMMANDS_RS, UPDATE_RS].concat();
    for event in api_events() {
        assert!(
            rust.contains(&format!("\"{event}\"")),
            "前端监听的事件 `{event}` 在 Rust 侧找不到同名字符串常量"
        );
    }
}
