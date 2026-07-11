mod codex_provider;
mod cursor_provider;
mod hooks;
mod local_providers;
mod opencode_provider;
mod providers;
mod remote_providers;
mod shortcuts;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use tokio::sync::{oneshot, Notify};

use opencode_provider::{build_opencode_go_local_lines, read_opencode_token_usage};

#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if cfg!(debug_assertions) {
            eprintln!($($arg)*);
        }
    };
}

const PILL_WIDTH: u32 = 480;
const PILL_HEIGHT: u32 = 50;
const EXPANDED_WIDTH: u32 = 480;
const DEFAULT_EXPANDED_HEIGHT: u32 = 600;
const WINDOW_MARGIN: i32 = 12;
const MANAGED_KEY: &str = "AgentIsOk";
const DEFAULTS_JSON: &str = include_str!("../../config/defaults.json");
const PROVIDERS_JSON: &str = include_str!("../../config/providers.json");
const ALL_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "PermissionRequest",
];
const CODEX_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
];
const CLAUDE_HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "SessionStart",
    "SessionEnd",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "Notification",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionDenied",
    "PreCompact",
];

#[derive(Default)]
struct AppState {
    window: Mutex<IslandState>,
    usage: Mutex<UsageState>,
    intervention: Mutex<Option<PendingIntervention>>,
    sessions: Mutex<HashMap<String, SessionInfo>>,
    sync: SyncState,
    opencode_refresh_abort: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[derive(Clone)]
struct IslandState {
    mode: String,
    expanded_height: u32,
    pill_width: u32,
    drag_start_bounds: Option<WindowBounds>,
    drag_start_mouse: Option<MousePoint>,
}

impl Default for IslandState {
    fn default() -> Self {
        Self {
            mode: "pill".to_string(),
            expanded_height: DEFAULT_EXPANDED_HEIGHT,
            pill_width: PILL_WIDTH,
            drag_start_bounds: None,
            drag_start_mouse: None,
        }
    }
}

#[derive(Clone, Default)]
struct SessionInfo {
    id: String,
    source: String,
    status: String,
    activity: String,
    activity_detail: String,
    tool_name: String,
    command: String,
    file_path: String,
    events: Vec<SessionEvent>,
    updated_at: u128,
    last_event: String,
    jump_target: Option<Value>,
}

#[derive(Clone, Default)]
struct SessionEvent {
    event: String,
    summary: String,
    detail: String,
    created_at: u128,
}

#[derive(Clone, Copy, Deserialize)]
struct MousePoint {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy)]
struct WindowBounds {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Default)]
struct UsageState {
    balances: Vec<Value>,
    synced_at: u128,
}

struct SyncState {
    interval_minutes: Mutex<u64>,
    notify: Arc<Notify>,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            interval_minutes: Mutex::new(10),
            notify: Arc::new(Notify::new()),
        }
    }
}

struct PendingIntervention {
    id: String,
    source: String,
    event: String,
    title: String,
    detail: String,
    explanation: String,
    thinking: String,
    command: String,
    file_path: String,
    tool_name: String,
    raw: String,
    meta: Value,
    jump_target: Option<Value>,
    created_at: u128,
    responder: Option<oneshot::Sender<InterventionDecision>>,
}

#[derive(Clone)]
struct InterventionDecision {
    approved: bool,
    allow_persistent: bool,
    answer: Option<String>,
}

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApprovalRule {
    source: String,
    tool_name: String,
    command: String,
    file_path: String,
    prefix_rule: String,
    created_at: u128,
}

pub use hooks::run_hook_bridge_from_args;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderVisibility {
    visible: bool,
    label: String,
}

pub(crate) fn app_config_dir() -> PathBuf {
    dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("AgentIsOK")
}

fn read_json_file(name: &str) -> Value {
    let embedded = match name {
        "defaults.json" => {
            serde_json::from_str::<Value>(DEFAULTS_JSON).unwrap_or_else(|_| json!({}))
        }
        "providers.json" => {
            serde_json::from_str::<Value>(PROVIDERS_JSON).unwrap_or_else(|_| json!({}))
        }
        _ => json!({}),
    };
    if name != "defaults.json" {
        return embedded;
    }
    let path = app_config_dir().join(name);
    let Some(mut user_value) = fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
    else {
        return embedded;
    };
    let mut merged = embedded;
    merge_json(&mut merged, user_value.take());
    merged
}

fn merge_json(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (key, value) in overlay_map {
                merge_json(base_map.entry(key).or_insert(Value::Null), value);
            }
        }
        (base_slot, value) => *base_slot = value,
    }
}

pub(crate) fn read_env_value(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(value) = env::var(name) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    for path in env_file_candidates() {
        if let Some(value) = read_env_file_value(&path, names) {
            return Some(value);
        }
    }

    None
}

fn env_file_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = env::current_dir() {
        paths.push(cwd.join(".env"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".agentisok").join(".env"));
        paths.push(home.join(".config").join("agentisok").join(".env"));
        for profile in [
            ".zshenv",
            ".zprofile",
            ".zshrc",
            ".bash_profile",
            ".bashrc",
            ".profile",
        ] {
            paths.push(home.join(profile));
        }
    }
    paths
}

fn read_env_file_value(path: &Path, names: &[&str]) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let Some((key, value)) = parse_env_line(line) else {
            continue;
        };
        if names.iter().any(|name| *name == key) {
            return Some(value);
        }
    }
    None
}

fn parse_env_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let without_export = if let Some(rest) = trimmed.strip_prefix("export") {
        if rest.starts_with(char::is_whitespace) {
            rest.trim_start()
        } else {
            return None;
        }
    } else {
        trimmed
    };
    let (key, value) = without_export.split_once('=')?;
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    let mut value = strip_inline_comment(value.trim()).trim().to_string();
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        value = value[1..value.len().saturating_sub(1)].to_string();
    }
    if value.is_empty() {
        return None;
    }
    Some((key.to_string(), value))
}

fn strip_inline_comment(value: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, ch) in value.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &value[..index],
            _ => {}
        }
    }
    value
}

fn write_defaults(defaults: &Value) -> Result<(), String> {
    let dir = app_config_dir();
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let path = dir.join("defaults.json");
    let content = serde_json::to_string_pretty(defaults).map_err(|err| err.to_string())?;
    fs::write(path, format!("{content}\n")).map_err(|err| err.to_string())
}

fn approval_rules_path() -> PathBuf {
    app_config_dir().join("approval-rules.json")
}

fn read_approval_rules() -> Vec<ApprovalRule> {
    fs::read_to_string(approval_rules_path())
        .ok()
        .and_then(|content| serde_json::from_str::<Vec<ApprovalRule>>(&content).ok())
        .unwrap_or_default()
}

fn write_approval_rules(rules: &[ApprovalRule]) -> Result<(), String> {
    let dir = app_config_dir();
    fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let content = serde_json::to_string_pretty(rules).map_err(|err| err.to_string())?;
    fs::write(approval_rules_path(), format!("{content}\n")).map_err(|err| err.to_string())
}

fn is_managed_command(command: &str) -> bool {
    let command = command.to_ascii_lowercase();
    command.contains("agentisok")
        || command.contains("thatisok")
        || command.contains("hook-bridge.js")
        || command.contains("--hook-source")
}

use providers::{get_dashboard_data, provider_visibility, sync_provider_accounts};

fn build_config_account(account: &Value, setting: Option<&Value>) -> Value {
    let provider = account
        .get("provider")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let account_id = account
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(provider);
    let label = setting
        .and_then(|item| item.get("label"))
        .and_then(Value::as_str)
        .or_else(|| account.get("label").and_then(Value::as_str))
        .unwrap_or(provider);
    let manual_plan = setting
        .and_then(|item| item.get("manualPlan"))
        .and_then(Value::as_str);
    let plan = manual_plan.unwrap_or("Local setup");
    let mut lines = vec![json!({
        "type": "text",
        "label": "Runtime",
        "value": "Tauri"
    })];
    if let Some(plan) = manual_plan {
        lines.push(json!({
            "type": "text",
            "label": "Plan",
            "value": plan
        }));
    }

    json!({
        "accountId": account_id,
        "provider": provider,
        "label": label,
        "status": "setup",
        "plan": plan,
        "balanceUsd": null,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "lines": lines,
        "meta": {
            "manualPlan": manual_plan
        },
        "message": "Not connected — run Sync to check usage"
    })
}

fn build_config_accounts() -> Vec<Value> {
    let providers = read_json_file("providers.json");
    let defaults = read_json_file("defaults.json");
    let accounts = providers
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let default_providers = defaults.get("providers").and_then(Value::as_object);

    accounts
        .into_iter()
        .filter_map(|account| {
            let provider = account.get("provider")?.as_str()?.to_string();
            let setting = default_providers.and_then(|items| items.get(&provider));
            Some(build_config_account(&account, setting))
        })
        .collect()
}

pub(crate) fn refresh_opencode_local_usage(app: &AppHandle) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let db_path = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db");
    if !db_path.exists() {
        return;
    }

    let state = app.state::<AppState>();
    let mut usage = match state.usage.lock() {
        Ok(usage) => usage,
        Err(_) => return,
    };

    if usage.balances.is_empty() {
        usage.balances = build_config_accounts();
    }

    let Some(account) = usage
        .balances
        .iter_mut()
        .find(|account| account.get("provider").and_then(Value::as_str) == Some("opencode"))
    else {
        return;
    };

    if account.get("source").and_then(Value::as_str) == Some("opencode-go-local") {
        if let Some(new_usage_lines) = build_opencode_go_local_lines(&db_path) {
            let mut lines = account
                .get("lines")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|line| {
                    let label = line.get("label").and_then(Value::as_str).unwrap_or("");
                    !(line.get("type").and_then(Value::as_str) == Some("progress")
                        && matches!(label, "Session" | "5-hour" | "Weekly"))
                })
                .collect::<Vec<_>>();
            lines.extend(new_usage_lines);
            account["lines"] = Value::Array(lines);
        }
    }

    if let Some(token_usage) = read_opencode_token_usage(&db_path) {
        account["tokenUsage"] = json!({
            "exactInput": token_usage.input,
            "exactOutput": token_usage.output,
            "exactReasoning": token_usage.reasoning,
            "exactCacheRead": token_usage.cache_read,
            "exactCacheWrite": token_usage.cache_write,
            "exactTotal": token_usage.input + token_usage.output + token_usage.reasoning,
            "estimatedInput": 0,
            "estimatedOutput": 0,
            "estimatedTotal": 0,
            "source": "opencode-sqlite"
        });
    }
    account["capturedAt"] = json!(now_millis());
    usage.synced_at = now_millis();
    drop(usage);

    let data = get_dashboard_data(&state);
    let _ = app.emit("island-data", data);
}

pub(crate) fn read_number_value(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
    })
}

pub(crate) fn format_compact(value: f64) -> String {
    if !value.is_finite() {
        return "--".to_string();
    }
    if value >= 1_000_000.0 {
        format!("{:.1}M", value / 1_000_000.0)
    } else if value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else {
        format!("{}", value.round() as u64)
    }
}

pub(crate) fn today_date_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

pub(crate) fn epoch_to_ms(value: f64) -> Option<u128> {
    if value <= 0.0 {
        return None;
    }
    let ms = if value > 1_000_000_000_000.0 {
        value
    } else {
        value * 1000.0
    };
    Some(ms as u128)
}

pub(crate) fn iso_from_ms(ms: u128) -> String {
    let ms_i64 = i64::try_from(ms).unwrap_or(i64::MAX);
    let secs = ms_i64 / 1000;
    let millis = (ms_i64.rem_euclid(1000)) as u32;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, millis * 1_000_000)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

// ===== WINDOW MANAGEMENT =====

fn main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())
}

pub(crate) fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn pending_intervention_json(pending: &PendingIntervention) -> Value {
    json!({
        "id": pending.id,
        "source": pending.source,
        "event": pending.event,
        "title": pending.title,
        "detail": pending.detail,
        "explanation": pending.explanation,
        "thinking": pending.thinking,
        "command": pending.command,
        "filePath": pending.file_path,
        "toolName": pending.tool_name,
        "raw": pending.raw,
        "meta": pending.meta,
        "jumpTarget": pending.jump_target,
        "createdAt": pending.created_at
    })
}

fn prefix_rule_from_meta(meta: &Value) -> String {
    meta.get("prefixRule")
        .or_else(|| meta.get("prefix_rule"))
        .and_then(|value| {
            if let Some(items) = value.as_array() {
                let parts = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("\u{1f}"))
                }
            } else {
                value.as_str().map(str::to_string)
            }
        })
        .or_else(|| {
            meta.get("sandbox_permissions")
                .or_else(|| meta.get("sandboxPermissions"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn approval_rule_for(pending: &PendingIntervention) -> ApprovalRule {
    ApprovalRule {
        source: pending.source.clone(),
        tool_name: pending.tool_name.clone(),
        command: pending.command.clone(),
        file_path: pending.file_path.clone(),
        prefix_rule: prefix_rule_from_meta(&pending.meta),
        created_at: now_millis(),
    }
}

fn rule_matches(rule: &ApprovalRule, pending: &PendingIntervention) -> bool {
    if rule.source != pending.source || rule.tool_name != pending.tool_name {
        return false;
    }
    let prefix_rule = prefix_rule_from_meta(&pending.meta);
    if !rule.prefix_rule.is_empty() || !prefix_rule.is_empty() {
        return rule.prefix_rule == prefix_rule;
    }
    if !rule.command.is_empty() || !pending.command.is_empty() {
        return rule.command == pending.command;
    }
    if !rule.file_path.is_empty() || !pending.file_path.is_empty() {
        return rule.file_path == pending.file_path;
    }
    false
}

fn is_persistently_allowed(pending: &PendingIntervention) -> bool {
    read_approval_rules()
        .iter()
        .any(|rule| rule_matches(rule, pending))
}

fn persist_approval_rule(pending: &PendingIntervention) {
    let rule = approval_rule_for(pending);
    let mut rules = read_approval_rules();
    if !rules.iter().any(|item| {
        item.source == rule.source
            && item.tool_name == rule.tool_name
            && item.command == rule.command
            && item.file_path == rule.file_path
            && item.prefix_rule == rule.prefix_rule
    }) {
        rules.push(rule);
        let _ = write_approval_rules(&rules);
    }
}

fn get_pending_intervention(state: &AppState) -> Option<Value> {
    state
        .intervention
        .lock()
        .ok()
        .and_then(|pending| pending.as_ref().map(pending_intervention_json))
}

fn nested_tool_input(payload: &Value) -> Option<&serde_json::Map<String, Value>> {
    payload
        .get("tool_input")
        .or_else(|| payload.get("toolInput"))
        .or_else(|| payload.get("input"))
        .or_else(|| payload.get("parameters"))
        .or_else(|| payload.get("arguments"))
        .or_else(|| payload.get("toolCall").and_then(|tool| tool.get("args")))
        .and_then(Value::as_object)
}

fn string_field(payload: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
        .or_else(|| {
            nested_tool_input(payload).and_then(|input| {
                keys.iter()
                    .find_map(|key| input.get(*key).and_then(Value::as_str))
            })
        })
        .unwrap_or("")
        .to_string()
}

fn char_preview(value: &str, limit: usize) -> String {
    let mut preview = value.replace('\n', "\\n");
    if preview.chars().count() > limit {
        preview = preview.chars().take(limit).collect::<String>();
        preview.push('…');
    }
    preview
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', " ")
}

fn tool_input_summary(
    tool_name: &str,
    input: Option<&serde_json::Map<String, Value>>,
    file_path: &str,
    command: &str,
) -> Option<String> {
    let tool = tool_name.to_ascii_lowercase();
    if tool == "bash" || !command.is_empty() {
        return Some(format!("Command: {}", char_preview(command, 180)));
    }

    let Some(input) = input else {
        return None;
    };

    if tool == "write" || tool == "notebookwrite" || tool == "edit" {
        let content = input
            .get("content")
            .or_else(|| input.get("new_string"))
            .or_else(|| input.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let line_count = content.lines().count();
        let target = if !file_path.is_empty() {
            file_path
        } else {
            "file"
        };
        if tool == "edit" {
            let old = input
                .get("old_string")
                .and_then(Value::as_str)
                .unwrap_or("");
            let new_value = input
                .get("new_string")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !old.is_empty() {
                if !new_value.is_empty() {
                    return Some(format!(
                        "Edit {target}: replace \"{}\" with \"{}\" ({} lines)",
                        char_preview(old, 40),
                        char_preview(new_value, 40),
                        line_count
                    ));
                }
                return Some(format!(
                    "Edit {target}: replace \"{}\" ({} lines)",
                    char_preview(old, 40),
                    line_count
                ));
            }
        }
        return Some(format!("Write {target}: writing {line_count} lines"));
    }

    if tool == "grep" || tool == "search" || tool == "glob" {
        let pattern = input
            .get("pattern")
            .or_else(|| input.get("query"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let path = input
            .get("path")
            .or_else(|| input.get("dir_path"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !pattern.is_empty() {
            return Some(format!(
                "Search \"{pattern}\" in {}",
                if path.is_empty() { "current dir" } else { path }
            ));
        }
    }

    if tool == "read" || tool == "read_file" {
        if !file_path.is_empty() {
            return Some(format!("Read file: {file_path}"));
        }
    }

    if tool == "multiedit" {
        let edits = input
            .get("edits")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if !file_path.is_empty() {
            return Some(format!("File: {file_path} · {edits} edits"));
        }
        return Some(format!("{edits} edits"));
    }

    if !file_path.is_empty() {
        return Some(format!("File: {file_path}"));
    }

    None
}

fn emit_runtime_warning(app: &AppHandle, message: &str) {
    let _ = app.emit(
        "runtime-warning",
        json!({
            "message": message
        }),
    );
}

fn emit_sync_warning_if_needed(app: &AppHandle, accounts: &[Value]) {
    let failed = accounts
        .iter()
        .filter(|account| account.get("status").and_then(Value::as_str) == Some("error"))
        .filter_map(|account| account.get("label").and_then(Value::as_str))
        .take(3)
        .map(str::to_string)
        .collect::<Vec<_>>();

    if failed.is_empty() {
        return;
    }

    emit_runtime_warning(
        app,
        &format!(
            "Sync issues: {}. Check provider credentials or network access.",
            failed.join(", ")
        ),
    );
}

fn build_intervention(
    source: &str,
    event_name: &str,
    raw: &str,
    payload: Value,
) -> PendingIntervention {
    let input = nested_tool_input(&payload);
    let tool_name = payload
        .get("tool_name")
        .or_else(|| payload.get("toolName"))
        .or_else(|| payload.get("tool"))
        .or_else(|| payload.get("toolCall").and_then(|tool| tool.get("name")))
        .and_then(Value::as_str)
        .unwrap_or("permission")
        .to_string();
    let command = string_field(&payload, &["command", "cmd", "CommandLine", "commandLine"]);
    let file_path = string_field(
        &payload,
        &[
            "file_path",
            "filePath",
            "path",
            "Path",
            "FilePath",
            "DirectoryPath",
            "notebook_path",
        ],
    );

    let explanation = payload
        .get("explanation_zh")
        .or_else(|| payload.get("explanation"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let thinking = payload
        .get("thinking")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let detail = payload
        .get("reason")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("prompt"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() < 300)
        .map(str::to_string)
        .or_else(|| {
            if !explanation.is_empty() {
                Some(explanation.clone())
            } else {
                None
            }
        })
        .or_else(|| tool_input_summary(&tool_name, input, &file_path, &command))
        .or_else(|| {
            if !tool_name.is_empty() && tool_name != "permission" {
                Some(format!("Tool: {}", tool_name))
            } else if !command.is_empty() {
                Some(command.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "Confirm this action".to_string());
    let source_label = match source {
        "codex" => "Codex",
        "claude" => "Claude",
        "antigravity" | "gemini" => "Antigravity",
        _ => "Agent",
    };
    let prompt_text = payload
        .get("prompt")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("reason"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(180).collect::<String>());
    let tool_key = tool_name.to_ascii_lowercase();
    let title = if !file_path.is_empty() && (tool_key == "write" || tool_key == "notebookwrite") {
        format!("{source_label} wants to write {file_path}")
    } else if !file_path.is_empty() && (tool_key == "edit" || tool_key == "multiedit") {
        format!("{source_label} wants to edit {file_path}")
    } else if !file_path.is_empty() {
        format!("{source_label} wants to use {tool_name} on {file_path}")
    } else if !command.is_empty() {
        format!(
            "{source_label} wants to run: {}",
            command.chars().take(80).collect::<String>()
        )
    } else if !tool_name.is_empty() && tool_name != "permission" {
        format!("{source_label} wants to use {tool_name}")
    } else if let Some(ref p) = prompt_text {
        p.clone()
    } else {
        format!("{source_label} needs approval")
    };

    let jump_target = payload
        .get("jump_target")
        .or_else(|| payload.get("jumpTarget"))
        .cloned();

    PendingIntervention {
        id: format!("req_{}", now_millis()),
        source: source.to_string(),
        event: event_name.to_string(),
        title,
        detail,
        explanation,
        thinking,
        command,
        file_path,
        tool_name,
        raw: raw.to_string(),
        meta: payload,
        jump_target,
        created_at: now_millis(),
        responder: None,
    }
}

fn primary_work_area(window: &WebviewWindow) -> Result<(i32, i32, u32, u32), String> {
    let monitor = window
        .current_monitor()
        .map_err(|err| err.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "monitor not found".to_string())?;
    let pos = monitor.position();
    let size = monitor.size();
    Ok((pos.x, pos.y, size.width, size.height))
}

fn clamp(value: i32, min: i32, max: i32) -> i32 {
    value.max(min).min(max)
}

fn apply_bounds(window: &WebviewWindow, bounds: WindowBounds) -> Result<(), String> {
    window
        .set_size(PhysicalSize::new(bounds.width, bounds.height))
        .map_err(|err| err.to_string())?;
    window
        .set_position(PhysicalPosition::new(bounds.x, bounds.y))
        .map_err(|err| err.to_string())
}

fn set_mode(app: &AppHandle, mode: &str) -> Result<(), String> {
    let window = main_window(app)?;
    let (area_x, area_y, area_w, area_h) = primary_work_area(&window)?;
    let app_state = app.state::<AppState>();
    let mut state = app_state.window.lock().map_err(|err| err.to_string())?;
    state.mode = mode.to_string();
    let width = if mode == "expanded" {
        EXPANDED_WIDTH
    } else {
        state.pill_width
    };
    let height = if mode == "expanded" {
        state.expanded_height
    } else {
        PILL_HEIGHT
    };
    let pos = window.outer_position().map_err(|err| err.to_string())?;
    let max_x = area_x + area_w as i32 - width as i32 - WINDOW_MARGIN;
    let max_y = area_y + area_h as i32 - height as i32 - WINDOW_MARGIN;
    apply_bounds(
        &window,
        WindowBounds {
            x: clamp(pos.x, area_x + WINDOW_MARGIN, max_x),
            y: clamp(pos.y, area_y + WINDOW_MARGIN, max_y),
            width,
            height,
        },
    )?;
    let _ = window.emit("island-window-state", json!({ "mode": mode }));
    Ok(())
}

// ===== TAURI COMMANDS =====

#[tauri::command]
fn island_get_data(state: tauri::State<'_, AppState>) -> Value {
    get_dashboard_data(&state)
}

#[tauri::command]
fn island_get_intervention(state: tauri::State<'_, AppState>) -> Option<Value> {
    get_pending_intervention(&state)
}

#[tauri::command]
async fn island_sync_now(app: AppHandle) -> Result<Value, String> {
    let accounts = sync_provider_accounts().await;
    {
        let state = app.state::<AppState>();
        let mut usage = state.usage.lock().map_err(|e| e.to_string())?;
        usage.balances = accounts;
        usage.synced_at = now_millis();
    }
    let data = {
        let state = app.state::<AppState>();
        get_dashboard_data(&state)
    };
    emit_sync_warning_if_needed(
        &app,
        data.get("accounts")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    );
    let _ = app.emit("island-data", data.clone());
    Ok(data)
}

#[tauri::command]
fn island_set_mode(app: AppHandle, mode: String) -> Result<(), String> {
    set_mode(&app, &mode)
}

#[tauri::command]
fn island_set_expanded_height(app: AppHandle, height: f64) -> Result<(), String> {
    let window = main_window(&app)?;
    let (_, _, _, area_h) = primary_work_area(&window)?;
    let next_height = height
        .round()
        .max(80.0)
        .min((area_h as f64 * 0.85).max(80.0)) as u32;
    {
        let app_state = app.state::<AppState>();
        let mut state = app_state.window.lock().map_err(|err| err.to_string())?;
        state.expanded_height = next_height;
        if state.mode != "expanded" {
            return Ok(());
        }
    }
    set_mode(&app, "expanded")
}

#[tauri::command]
fn island_set_pill_width(app: AppHandle, width: f64) -> Result<(), String> {
    let window = main_window(&app)?;
    let (_, _, area_w, _) = primary_work_area(&window)?;
    let next_width = width
        .round()
        .max(120.0)
        .min((area_w as f64 * 0.9).max(120.0)) as u32;
    {
        let app_state = app.state::<AppState>();
        let mut state = app_state.window.lock().map_err(|err| err.to_string())?;
        if state.pill_width == next_width {
            return Ok(());
        }
        state.pill_width = next_width;
        if state.mode != "pill" {
            return Ok(());
        }
    }
    set_mode(&app, "pill")
}

#[tauri::command]
fn app_restart(app: AppHandle) -> Result<(), String> {
    app.restart();
    #[allow(unreachable_code)]
    Ok(())
}

#[tauri::command]
fn island_drag_start(app: AppHandle, mouse: MousePoint) -> Result<(), String> {
    let window = main_window(&app)?;
    let pos = window.outer_position().map_err(|err| err.to_string())?;
    let size = window.outer_size().map_err(|err| err.to_string())?;
    let app_state = app.state::<AppState>();
    let mut state = app_state.window.lock().map_err(|err| err.to_string())?;
    state.drag_start_bounds = Some(WindowBounds {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    });
    state.drag_start_mouse = Some(mouse);
    Ok(())
}

#[tauri::command]
fn island_drag_move(app: AppHandle, mouse: MousePoint) -> Result<(), String> {
    let window = main_window(&app)?;
    let (area_x, area_y, area_w, area_h) = primary_work_area(&window)?;
    let app_state = app.state::<AppState>();
    let state = app_state.window.lock().map_err(|err| err.to_string())?;
    let Some(start_bounds) = state.drag_start_bounds else {
        return Ok(());
    };
    let Some(start_mouse) = state.drag_start_mouse else {
        return Ok(());
    };
    let x = start_bounds.x + (mouse.x - start_mouse.x).round() as i32;
    let y = start_bounds.y + (mouse.y - start_mouse.y).round() as i32;
    let max_x = area_x + area_w as i32 - start_bounds.width as i32 - WINDOW_MARGIN;
    let max_y = area_y + area_h as i32 - start_bounds.height as i32 - WINDOW_MARGIN;
    window
        .set_position(PhysicalPosition::new(
            clamp(x, area_x + WINDOW_MARGIN, max_x),
            clamp(y, area_y + WINDOW_MARGIN, max_y),
        ))
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn island_drag_end(app: AppHandle) -> Result<(), String> {
    let app_state = app.state::<AppState>();
    let mut state = app_state.window.lock().map_err(|err| err.to_string())?;
    state.drag_start_bounds = None;
    state.drag_start_mouse = {
        if let Ok(window) = main_window(&app) {
            if let Ok(pos) = window.outer_position() {
                let mut defaults = read_json_file("defaults.json");
                defaults["windowX"] = json!(pos.x);
                defaults["windowY"] = json!(pos.y);
                let _ = write_defaults(&defaults);
            }
        }
        None
    };
    Ok(())
}

#[tauri::command]
async fn intervention_respond(
    app: AppHandle,
    decision: Option<String>,
    answer: Option<String>,
) -> Result<bool, String> {
    let decision = decision.unwrap_or_default();
    if decision != "approve" && decision != "approve_always" && decision != "deny" {
        return Err("invalid approval decision".to_string());
    }
    let app_state = app.state::<AppState>();
    let mut pending = match app_state.intervention.try_lock() {
        Ok(pending) => pending,
        Err(_) => return Err("approval state is busy".to_string()),
    };
    let Some(mut current) = pending.take() else {
        return Ok(false);
    };
    let approved = decision == "approve" || decision == "approve_always";
    let allow_persistent = decision == "approve_always";
    if approved && allow_persistent {
        persist_approval_rule(&current);
    }
    if let Some(responder) = current.responder.take() {
        let _ = responder.send(InterventionDecision {
            approved,
            allow_persistent,
            answer: answer.and_then(|value| {
                let trimmed = value.trim().to_string();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed)
                }
            }),
        });
    }
    let _ = app.emit("intervention-state", Option::<Value>::None);
    let _ = set_mode(&app, "pill");
    Ok(true)
}

#[tauri::command]
async fn jump_to_terminal(target: Value) -> Result<bool, String> {
    let terminal_app = target
        .get("terminalApp")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let tty = target
        .get("terminalTTY")
        .and_then(Value::as_str)
        .unwrap_or("");
    let cwd = target
        .get("workingDirectory")
        .and_then(Value::as_str)
        .unwrap_or("");

    #[cfg(target_os = "macos")]
    {
        let tty = applescript_string(tty);
        let script = match terminal_app.to_lowercase().as_str() {
            "iterm" | "iterm2" => format!(
                "tell application \"iTerm2\"\n\
                    activate\n\
                    repeat with aWindow in windows\n\
                        repeat with aTab in tabs of aWindow\n\
                            repeat with aSession in sessions of aTab\n\
                                if (tty of aSession as text) is \"{}\" then\n\
                                    select aSession\n\
                                    return true\n\
                                end if\n\
                            end repeat\n\
                        end repeat\n\
                    end repeat\n\
                end tell",
                tty
            ),
            "terminal" => format!(
                "tell application \"Terminal\"\n\
                    activate\n\
                    repeat with aWindow in windows\n\
                        repeat with aTab in tabs of aWindow\n\
                            if (tty of aTab as text) is \"{}\" then\n\
                                set selected of aTab to true\n\
                                set frontmost of aWindow to true\n\
                                return true\n\
                            end if\n\
                        end repeat\n\
                    end repeat\n\
                end tell",
                tty
            ),
            "ghostty" => {
                let tty_match = if !tty.is_empty() {
                    format!(
                        "tell application \"Ghostty\"\n\
                            activate\n\
                            repeat with aWindow in windows\n\
                                repeat with aTab in tabs of aWindow\n\
                                    if name of aTab contains \"{}\" then\n\
                                        set selected of aTab to true\n\
                                        return true\n\
                                    end if\n\
                                end repeat\n\
                            end repeat\n\
                        end tell",
                        tty
                    )
                } else if !cwd.is_empty() {
                    format!(
                        "tell application \"Ghostty\"\n\
                            activate\n\
                            repeat with aWindow in windows\n\
                                repeat with aTab in tabs of aWindow\n\
                                    if name of aTab contains \"{}\" then\n\
                                        set selected of aTab to true\n\
                                        return true\n\
                                    end if\n\
                                end repeat\n\
                            end repeat\n\
                        end tell",
                        applescript_string(cwd)
                    )
                } else {
                    "tell application \"Ghostty\" to activate".to_string()
                };
                tty_match
            }
            "cmux" => format!(
                "tell application \"Ghostty\"\n\
                    activate\n\
                    repeat with aWindow in windows\n\
                        repeat with aTab in tabs of aWindow\n\
                            if name of aTab contains \"{}\" then\n\
                                set selected of aTab to true\n\
                                return true\n\
                            end if\n\
                        end repeat\n\
                    end repeat\n\
                end tell",
                if !tty.is_empty() {
                    tty.clone()
                } else {
                    applescript_string(cwd)
                }
            ),
            _ => {
                let app_name = terminal_app;
                let activate_script = format!(
                    "tell application \"{}\" to activate",
                    applescript_string(app_name)
                );
                let _ = std::process::Command::new("osascript")
                    .arg("-e")
                    .arg(&activate_script)
                    .output();

                if !cwd.is_empty() {
                    return std::process::Command::new("open")
                        .arg(cwd)
                        .status()
                        .map(|status| status.success())
                        .map_err(|e| e.to_string());
                } else {
                    return Ok(true);
                }
            }
        };

        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .map_err(|e| e.to_string())?;

        return Ok(output.status.success());
    }

    #[cfg(target_os = "windows")]
    {
        // On Windows, focusing a specific tab in Windows Terminal is harder without specialized APIs.
        // We'll try to use 'wt' CLI to at least open a new tab in that CWD as a fallback,
        // or just use explorer to open the path.
        if !cwd.is_empty() {
            let _ = std::process::Command::new("explorer").arg(cwd).spawn();
            return Ok(true);
        }
        return Ok(false);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Ok(false)
    }
}

#[tauri::command]
fn providers_get_visibility() -> HashMap<String, ProviderVisibility> {
    provider_visibility()
}

#[tauri::command]
fn providers_set_visibility(app: AppHandle, provider: String, visible: bool) -> Result<(), String> {
    let mut defaults = read_json_file("defaults.json");
    if !defaults.get("providers").is_some_and(Value::is_object) {
        defaults["providers"] = json!({});
    }
    if !defaults["providers"]
        .get(&provider)
        .is_some_and(Value::is_object)
    {
        defaults["providers"][&provider] = json!({ "label": provider });
    }
    defaults["providers"][&provider]["visible"] = json!(visible);
    write_defaults(&defaults)?;
    let data = get_dashboard_data(&app.state::<AppState>());
    let _ = app.emit("island-data", data);
    Ok(())
}

#[tauri::command]
fn settings_get(app: AppHandle) -> Value {
    let defaults = read_json_file("defaults.json");
    let shortcuts = defaults
        .get("shortcuts")
        .cloned()
        .unwrap_or_else(shortcuts::default_shortcuts_json);
    let shortcut_display = shortcuts
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), Value::String(shortcuts::display_shortcut(v.as_str().unwrap_or("")))))
                .collect::<serde_json::Map<String, Value>>()
        })
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));
    json!({
        "syncIntervalMinutes": current_sync_interval(&app),
        "shortcuts": shortcuts,
        "shortcutDisplay": shortcut_display,
        "trayMenu": tray_menu_config(&defaults)
    })
}

#[tauri::command]
fn settings_open(app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("settings").ok_or("settings window not found")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

const TRAY_MENU_IDS: &[&str] = &[
    "open-activity", "open-agents", "open-usage", "open-rules", "open-settings",
    "sync", "update",
];

fn tray_menu_config(defaults: &Value) -> Vec<String> {
    defaults.get("trayMenu").and_then(Value::as_array).map(|items| {
        items.iter().filter_map(Value::as_str).filter(|id| {
            TRAY_MENU_IDS.contains(id) || id.starts_with("sep-")
        }).map(str::to_string).collect()
    }).unwrap_or_else(|| TRAY_MENU_IDS.iter().map(|id| (*id).to_string()).collect())
}

fn build_tray_menu(app: &AppHandle) -> Result<Menu<tauri::Wry>, String> {
    let menu = Menu::new(app).map_err(|error| error.to_string())?;
    let version = app.package_info().version.to_string();
    for id in tray_menu_config(&read_json_file("defaults.json")) {
        if id.starts_with("sep-") {
            let item = PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?;
            menu.append(&item).map_err(|error| error.to_string())?;
            continue;
        }
        let label = match id.as_str() {
            "open-activity" => "Open Home".to_string(),
            "open-agents" => "Running Agents".to_string(),
            "open-usage" => "Usage & Providers".to_string(),
            "open-rules" => "Approval Rules".to_string(),
            "open-settings" => "Settings".to_string(),
            "sync" => "Sync Now".to_string(),
            "update" => format!("AgentIsOK v{version}"),
            _ => continue,
        };
        let item = MenuItemBuilder::with_id(id, label).build(app).map_err(|error| error.to_string())?;
        menu.append(&item).map_err(|error| error.to_string())?;
    }
    menu.append(&PredefinedMenuItem::separator(app).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    menu.append(&MenuItemBuilder::with_id("quit", "Quit").build(app).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    Ok(menu)
}

#[tauri::command]
fn settings_set_tray_menu(app: AppHandle, items: Vec<String>) -> Result<Value, String> {
    let mut unique = Vec::new();
    for id in items {
        if !TRAY_MENU_IDS.contains(&id.as_str()) {
            return Err(format!("unknown tray menu item: {id}"));
        }
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    let mut defaults = read_json_file("defaults.json");
    defaults["trayMenu"] = json!(unique);
    write_defaults(&defaults)?;
    let menu = build_tray_menu(&app)?;
    if let Some(tray) = app.tray_by_id("main-tray") {
        tray.set_menu(Some(menu)).map_err(|error| error.to_string())?;
    }
    Ok(json!({ "trayMenu": tray_menu_config(&defaults) }))
}

#[tauri::command]
fn settings_set_sync_interval(app: AppHandle, minutes: u64) -> Result<Value, String> {
    let clamped = minutes.clamp(5, 60);
    let mut defaults = read_json_file("defaults.json");
    defaults["syncIntervalMinutes"] = json!(clamped);
    write_defaults(&defaults)?;
    set_sync_interval(&app, clamped);
    Ok(json!({
        "syncIntervalMinutes": clamped
    }))
}

#[tauri::command]
fn settings_set_shortcuts(app: AppHandle, shortcuts: Value) -> Result<Value, String> {
    // Validate each shortcut parses correctly
    for (action, val) in shortcuts.as_object().ok_or("invalid shortcuts format")? {
        let s = val.as_str().ok_or(format!("shortcut '{action}' must be a string"))?;
        if shortcuts::parse_code(s).is_none() && shortcuts::parse_shortcut(s).is_none() {
            return Err(format!("invalid shortcut for '{action}': '{s}'"));
        }
    }
    let mut defaults = read_json_file("defaults.json");
    defaults["shortcuts"] = shortcuts;
    write_defaults(&defaults)?;
    shortcuts::reload_shortcuts(&app);

    // Build response with display strings
    let shortcuts = defaults.get("shortcuts").cloned().unwrap_or_else(|| json!({}));
    let shortcut_display = shortcuts
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), Value::String(shortcuts::display_shortcut(v.as_str().unwrap_or("")))))
                .collect::<serde_json::Map<String, Value>>()
        })
        .map(Value::Object)
        .unwrap_or_else(|| json!({}));
    Ok(json!({
        "shortcuts": shortcuts,
        "shortcutDisplay": shortcut_display
    }))
}

#[tauri::command]
fn approval_rules_list() -> Vec<ApprovalRule> {
    read_approval_rules()
}

#[tauri::command]
fn approval_rules_delete(index: usize) -> Result<Vec<ApprovalRule>, String> {
    let mut rules = read_approval_rules();
    if index >= rules.len() {
        return Err("approval rule not found".to_string());
    }
    rules.remove(index);
    write_approval_rules(&rules)?;
    Ok(rules)
}

#[tauri::command]
fn approval_rules_restore(index: usize, rule: ApprovalRule) -> Result<Vec<ApprovalRule>, String> {
    let mut rules = read_approval_rules();
    if !rules.iter().any(|item| item == &rule) {
        let insert_index = index.min(rules.len());
        rules.insert(insert_index, rule);
        write_approval_rules(&rules)?;
    }
    Ok(rules)
}

fn position_initial(window: &WebviewWindow) -> Result<(), String> {
    let defaults = read_json_file("defaults.json");
    if let (Some(x), Some(y)) = (
        defaults.get("windowX").and_then(Value::as_i64),
        defaults.get("windowY").and_then(Value::as_i64),
    ) {
        window
            .set_position(PhysicalPosition::new(x as i32, y as i32))
            .map_err(|err| err.to_string())
    } else {
        let (area_x, area_y, area_w, _) = primary_work_area(window)?;
        let x = area_x + ((area_w as i32 - PILL_WIDTH as i32) / 2);
        window
            .set_position(PhysicalPosition::new(x, area_y + WINDOW_MARGIN))
            .map_err(|err| err.to_string())
    }
}

fn configured_sync_interval() -> u64 {
    read_json_file("defaults.json")
        .get("syncIntervalMinutes")
        .and_then(Value::as_u64)
        .unwrap_or(10)
        .clamp(5, 60)
}

fn current_sync_interval(app: &AppHandle) -> u64 {
    let state = app.state::<AppState>();
    state
        .sync
        .interval_minutes
        .lock()
        .map(|minutes| *minutes)
        .unwrap_or_else(|_| configured_sync_interval())
}

fn set_sync_interval(app: &AppHandle, minutes: u64) {
    let state = app.state::<AppState>();
    let lock_result = state.sync.interval_minutes.lock();
    if let Ok(mut current) = lock_result {
        *current = minutes;
    }
    state.sync.notify.notify_waiters();
}

fn initialize_sync_interval(app: &AppHandle) {
    let configured = configured_sync_interval();
    let state = app.state::<AppState>();
    let lock_result = state.sync.interval_minutes.lock();
    if let Ok(mut current) = lock_result {
        *current = configured;
    }
}

fn hooks_enabled() -> bool {
    read_json_file("defaults.json")
        .get("hooksEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn set_hooks_enabled(enabled: bool) -> Result<(), String> {
    let mut defaults = read_json_file("defaults.json");
    defaults["hooksEnabled"] = json!(enabled);
    write_defaults(&defaults)
}

fn install_hooks(app: &AppHandle) -> Result<(), String> {
    set_hooks_enabled(true)?;
    hooks::inject_agent_hooks(app);
    Ok(())
}

fn uninstall_hooks(app: &AppHandle) -> Result<(), String> {
    set_hooks_enabled(false)?;
    hooks::remove_agent_hooks()?;
    emit_runtime_warning(
        app,
        "AgentIsOK hooks removed. Agent integrations are disabled until hooks are installed again.",
    );
    Ok(())
}

#[tauri::command]
fn integrations_get() -> Value {
    let enabled = hooks_enabled();
    json!({
        "enabled": enabled,
        "healthy": enabled && hooks::hooks_intact()
    })
}

#[tauri::command]
fn integrations_enable(app: AppHandle) -> Result<Value, String> {
    install_hooks(&app)?;
    Ok(integrations_get())
}

#[tauri::command]
fn integrations_disable(app: AppHandle) -> Result<Value, String> {
    uninstall_hooks(&app)?;
    Ok(integrations_get())
}

async fn perform_initial_sync(app: AppHandle) {
    let accounts = sync_provider_accounts().await;
    let _ = replace_usage_balances(&app, accounts);
    let data = {
        let state = app.state::<AppState>();
        get_dashboard_data(&state)
    };
    emit_sync_warning_if_needed(
        &app,
        data.get("accounts")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
    );
    let _ = app.emit("island-data", data);
}

fn schedule_hooks_check(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            if !hooks_enabled() {
                continue;
            }
            if hooks::hooks_intact() {
                continue;
            }
            eprintln!("hooks missing, re-injecting");
            hooks::inject_agent_hooks(&app);
        }
    });
}

fn schedule_periodic_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            let duration = std::time::Duration::from_secs(current_sync_interval(&app) * 60);
            let notify = {
                let state = app.state::<AppState>();
                state.sync.notify.clone()
            };
            tokio::select! {
                _ = tokio::time::sleep(duration) => {}
                _ = notify.notified() => {
                    continue;
                }
            }
            let accounts = sync_provider_accounts().await;
            let _ = replace_usage_balances(&app, accounts);
            let data = {
                let state = app.state::<AppState>();
                get_dashboard_data(&state)
            };
            emit_sync_warning_if_needed(
                &app,
                data.get("accounts")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]),
            );
            let _ = app.emit("island-data", data);
        }
    });
}

fn replace_usage_balances(app: &AppHandle, accounts: Vec<Value>) -> bool {
    let state = app.state::<AppState>();
    let lock_result = state.usage.lock();
    match lock_result {
        Ok(mut usage) => {
            usage.balances = accounts;
            usage.synced_at = now_millis();
            true
        }
        Err(error) => {
            eprintln!("failed to lock usage state: {error}");
            false
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .on_window_event(|window, event| {
            if window.label() == "settings" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            island_get_data,
            island_get_intervention,
            island_sync_now,
            island_set_mode,
            island_set_expanded_height,
            island_set_pill_width,
            island_drag_start,
            island_drag_move,
            island_drag_end,
            intervention_respond,
            providers_get_visibility,
            providers_set_visibility,
            settings_get,
            settings_open,
            settings_set_sync_interval,
            settings_set_shortcuts,
            settings_set_tray_menu,
            integrations_get,
            integrations_enable,
            integrations_disable,
            approval_rules_list,
            approval_rules_delete,
            approval_rules_restore,
            app_restart,
            jump_to_terminal
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::ActivationPolicy;
                let _ = app.set_activation_policy(ActivationPolicy::Accessory);
            }
            let window = main_window(app.handle()).map_err(Box::<dyn std::error::Error>::from)?;
            position_initial(&window).map_err(Box::<dyn std::error::Error>::from)?;
            window.show().map_err(Box::<dyn std::error::Error>::from)?;
            initialize_sync_interval(app.handle());

            // Auto-check for updates on startup
            {
                use tauri_plugin_updater::UpdaterExt;
                let h = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    match h.updater() {
                        Ok(updater) => match updater.check().await {
                            Ok(Some(update)) => {
                                let _ = h.emit("update-status", json!({"status": "available", "version": update.version.to_string()}));
                            }
                            Ok(None) => {}
                            Err(_) => {}
                        },
                        Err(_) => {}
                    }
                });
            }

            shortcuts::register_shortcuts(app.handle());

            // Tray icon
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
                .map_err(|e| format!("tray icon: {e}")).ok();
            if let Some(icon) = tray_icon {
                let open_activity = MenuItemBuilder::with_id("open-activity", "Open Home")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}"))
                    .ok();
                let open_agents = MenuItemBuilder::with_id("open-agents", "Running Agents")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}"))
                    .ok();
                let open_usage = MenuItemBuilder::with_id("open-usage", "Usage & Providers")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}"))
                    .ok();
                let open_rules = MenuItemBuilder::with_id("open-rules", "Approval Rules")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let open_settings = MenuItemBuilder::with_id("open-settings", "Settings")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let sync_item = MenuItemBuilder::with_id("sync", "Sync Now")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let install_hooks_item = MenuItemBuilder::with_id("install-hooks", "Install Hooks")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let remove_hooks_item = MenuItemBuilder::with_id("remove-hooks", "Remove Hooks")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let version = app.package_info().version.to_string();
                let update_item =
                    MenuItemBuilder::with_id("update", format!("AgentIsOK v{version}"))
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let sep_view = PredefinedMenuItem::separator(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let sep_hooks = PredefinedMenuItem::separator(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let sep_app = PredefinedMenuItem::separator(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let quit_item = MenuItemBuilder::with_id("quit", "Quit")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();

                if let (
                    Some(open_activity),
                    Some(open_agents),
                    Some(open_usage),
                    Some(open_rules),
                    Some(open_settings),
                    Some(sync_item),
                    Some(install_hooks_item),
                    Some(remove_hooks_item),
                    Some(update_item),
                    Some(sep_view),
                    Some(sep_hooks),
                    Some(sep_app),
                    Some(quit_item),
                ) = (
                    open_activity,
                    open_agents,
                    open_usage,
                    open_rules,
                    open_settings,
                    sync_item,
                    install_hooks_item,
                    remove_hooks_item,
                    update_item,
                    sep_view,
                    sep_hooks,
                    sep_app,
                    quit_item,
                )
                {
                    let menu = MenuBuilder::new(app)
                        .items(&[
                            &open_activity,
                            &open_agents,
                            &open_usage,
                            &open_rules,
                            &open_settings,
                            &sep_view,
                            &sync_item,
                            &sep_hooks,
                            &install_hooks_item,
                            &remove_hooks_item,
                            &sep_app,
                            &update_item,
                            &quit_item,
                        ])
                        .build()
                        .map_err(|e| format!("tray menu: {e}")).ok();

                    if let Some(menu) = build_tray_menu(app.handle()).ok().or(menu) {
                        let _tray = TrayIconBuilder::with_id("main-tray")
                            .icon(icon)
                            .menu(&menu)
                            .on_menu_event(move |app_handle_inner, event| {
                                match event.id().as_ref() {
                                    "open-settings" => {
                                        let _ = settings_open(app_handle_inner.clone());
                                    }
                                    "open-activity" | "open-agents" | "open-usage" | "open-rules" => {
                                        if let Ok(window) = main_window(&app_handle_inner) {
                                            let _ = window.show();
                                            let _ = window.set_focus();
                                        }
                                        let view = match event.id().as_ref() {
                                            "open-agents" => "agents",
                                            "open-usage" => "usage",
                                            "open-rules" => "rules",
                                            _ => "home",
                                        };
                                        let _ = app_handle_inner.emit("island-open-view", view);
                                        let _ = set_mode(&app_handle_inner, "expanded");
                                    }
                                    "sync" => {
                                        let h = app_handle_inner.clone();
                                        tauri::async_runtime::spawn(async move {
                                            let accounts = sync_provider_accounts().await;
                                            let _ = replace_usage_balances(&h, accounts);
                                            let data = {
                                                let state = h.state::<AppState>();
                                                get_dashboard_data(&state)
                                            };
                                            emit_sync_warning_if_needed(&h, data.get("accounts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
                                            let _ = h.emit("island-data", data);
                                        });
                                    }
                                    "install-hooks" => {
                                        if let Err(error) = install_hooks(&app_handle_inner) {
                                            emit_runtime_warning(
                                                &app_handle_inner,
                                                &format!("Hook install failed: {error}"),
                                            );
                                        } else {
                                            emit_runtime_warning(
                                                &app_handle_inner,
                                                "AgentIsOK hooks installed.",
                                            );
                                        }
                                    }
                                    "remove-hooks" => {
                                        if let Err(error) = uninstall_hooks(&app_handle_inner) {
                                            emit_runtime_warning(
                                                &app_handle_inner,
                                                &format!("Hook removal failed: {error}"),
                                            );
                                        }
                                    }
                                    "update" => {
                                        use tauri_plugin_updater::UpdaterExt;
                                        let h = app_handle_inner.clone();
                                        let _ = h.emit("island-force-expand", ());
                                        let _ = h.emit("update-status", json!({"status": "checking"}));
                                        tauri::async_runtime::spawn(async move {
                                            match h.updater() {
                                                Ok(updater) => match updater.check().await {
                                                    Ok(Some(update)) => {
                                                        let _ = h.emit("update-status", json!({"status": "downloading", "version": update.version.to_string()}));
                                                        let result = update.download_and_install(
                                                            |_chunk_length, _content_length| {},
                                                            || {},
                                                        )
                                                        .await;
                                                        match result {
                                                            Ok(()) => {
                                                                let _ = h.emit("update-status", json!({"status": "installed"}));
                                                            }
                                                            Err(e) => {
                                                                let _ = h.emit("update-status", json!({"status": "error", "message": e.to_string()}));
                                                            }
                                                        }
                                                    }
                                                    Ok(None) => {
                                                        let _ = h.emit("update-status", json!({"status": "up-to-date"}));
                                                    }
                                                    Err(e) => {
                                                        let _ = h.emit("update-status", json!({"status": "error", "message": e.to_string()}));
                                                    }
                                                },
                                                Err(e) => {
                                                    let _ = h.emit("update-status", json!({"status": "error", "message": e.to_string()}));
                                                }
                                            }
                                        });
                                    }
                                    "quit" => {
                                        app_handle_inner.exit(0);
                                    }
                                    _ => {}
                                }
                            })
                            .on_tray_icon_event(|tray_icon, event| {
                                if let TrayIconEvent::Click {
                                    button: MouseButton::Left,
                                    button_state: MouseButtonState::Up,
                                    ..
                                } = event
                                {
                                    let _ = set_mode(tray_icon.app_handle(), "expanded");
                                    if let Ok(window) =
                                        main_window(tray_icon.app_handle())
                                    {
                                        let _ = window.show();
                                        let _ = window.set_focus();
                                    }
                                }
                            })
                            .build(app);
                    }
                }
            }

            // Hook server
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                hooks::start_hook_server(app_handle).await;
            });

            // Hook injection
            if hooks_enabled() {
                hooks::inject_agent_hooks(app.handle());
            }

            // Initial data emission
            let data = get_dashboard_data(&app.state::<AppState>());
            let _ = window.emit("island-data", data);
            let _ = window.emit("intervention-state", Option::<Value>::None);
            let _ = window.emit("island-window-state", json!({ "mode": "pill" }));

            // Startup sync
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                perform_initial_sync(app_handle).await;
            });

            // Periodic sync
            schedule_periodic_sync(app.handle().clone());

            // Periodic hooks integrity check
            schedule_hooks_check(app.handle().clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_write_intervention_shows_file_and_lines() {
        let item = build_intervention(
            "claude",
            "PermissionRequest",
            "{}",
            json!({
                "tool_name": "Write",
                "tool_input": {
                    "file_path": "/tmp/example.txt",
                    "content": "one\ntwo\nthree"
                }
            }),
        );

        assert!(item.title.contains("write /tmp/example.txt"));
        assert!(item.detail.contains("writing 3 lines"));
        assert_eq!(item.file_path, "/tmp/example.txt");
    }

    #[test]
    fn claude_edit_intervention_shows_replacement() {
        let item = build_intervention(
            "claude",
            "PermissionRequest",
            "{}",
            json!({
                "tool_name": "Edit",
                "tool_input": {
                    "file_path": "/tmp/example.txt",
                    "old_string": "old value",
                    "new_string": "new value"
                }
            }),
        );

        assert!(item.title.contains("edit /tmp/example.txt"));
        assert!(item.detail.contains("old value"));
        assert!(item.detail.contains("new value"));
    }

    #[test]
    fn claude_multiedit_intervention_shows_edit_count() {
        let item = build_intervention(
            "claude",
            "PermissionRequest",
            "{}",
            json!({
                "tool_name": "MultiEdit",
                "tool_input": {
                    "file_path": "/tmp/example.txt",
                    "edits": [
                        { "old_string": "a", "new_string": "b" },
                        { "old_string": "c", "new_string": "d" }
                    ]
                }
            }),
        );

        assert!(item.title.contains("edit /tmp/example.txt"));
        assert!(item.detail.contains("2 edits"));
    }
}
