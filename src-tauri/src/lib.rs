mod codex_provider;
mod cursor_provider;
mod hooks;
mod local_providers;
mod providers;
mod remote_providers;
mod shortcuts;

use rusqlite::Connection;
use chrono::{Datelike, TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use tokio::sync::{oneshot, Notify};

const PILL_WIDTH: u32 = 356;
const PILL_HEIGHT: u32 = 50;
const EXPANDED_WIDTH: u32 = 356;
const DEFAULT_EXPANDED_HEIGHT: u32 = 404;
const WINDOW_MARGIN: i32 = 12;
const MANAGED_KEY: &str = "ThatIsOk";
const DEFAULTS_JSON: &str = include_str!("../../config/defaults.json");
const PROVIDERS_JSON: &str = include_str!("../../config/providers.json");
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "PermissionRequest",
];

#[derive(Default)]
struct AppState {
    window: Mutex<IslandState>,
    usage: Mutex<UsageState>,
    intervention: Mutex<Option<PendingIntervention>>,
    sessions: Mutex<HashMap<String, SessionInfo>>,
    sync: SyncState,
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
    updated_at: u128,
    last_event: String,
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
    command: String,
    file_path: String,
    tool_name: String,
    raw: String,
    meta: Value,
    created_at: u128,
    responder: Option<oneshot::Sender<InterventionDecision>>,
}

#[derive(Clone, Copy)]
struct InterventionDecision {
    approved: bool,
    allow_persistent: bool,
}

#[derive(Clone, Deserialize, Serialize)]
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

fn app_config_dir() -> PathBuf {
    dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ThatIsOk")
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
        paths.push(home.join(".thatisok").join(".env"));
        paths.push(home.join(".config").join("thatisok").join(".env"));
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
    let without_export = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim();
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
    command.contains("ThatIsOk")
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
    let manual_plan = setting.and_then(|item| item.get("manualPlan")).and_then(Value::as_str);
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
        "message": "Tauri sync pending for this provider."
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

// ===== OPENCODE PROVIDER =====

async fn fetch_opencode_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let home = dirs::home_dir()?;
    let auth_path = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json");
    if !auth_path.exists() {
        return None;
    }

    let Ok(content) = fs::read_to_string(&auth_path) else {
        return None;
    };
    let Ok(auth) = serde_json::from_str::<Value>(&content) else {
        return None;
    };

    let go_key = auth
        .get("opencode-go")
        .and_then(|o| o.get("key"))
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty());

    let zen_key = auth
        .get("opencode")
        .and_then(|o| o.get("key"))
        .and_then(Value::as_str)
        .filter(|k| !k.is_empty());

    let is_go = go_key.is_some();
    let api_key = go_key.or(zen_key)?;

    let models_url = "https://opencode.ai/zen/go/v1/models";
    let model_count = match client
        .get(models_url)
        .bearer_auth(api_key)
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            match resp.json::<Value>().await {
                Ok(data) => {
                    data.get("data")
                        .and_then(Value::as_array)
                        .map(|models| models.len())
                        .unwrap_or(0)
                }
                Err(_) => 0,
            }
        }
        _ => 0,
    };

    let plan = if is_go { "Go Plan" } else { "Zen" };
    let status = if model_count > 0 { "live" } else { "warn" };
    let message = if model_count > 0 {
        format!("OpenCode {} · {} models available", plan, model_count)
    } else {
        format!("OpenCode {} · API unreachable", plan)
    };

    let mut lines = Vec::new();

    if is_go {
        let db_path = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        if db_path.exists() {
            if let Some(go_lines) = build_opencode_go_lines(&db_path) {
                lines.extend(go_lines);
            }
        }
    }

    if model_count > 0 {
        lines.push(json!({
            "type": "text",
            "label": "Models",
            "value": format_compact(model_count as f64),
            "subtitle": format!("{} models via {} API", model_count, if is_go { "Go" } else { "Zen" })
        }));
    }

    Some(json!({
        "accountId": account_id,
        "provider": "opencode",
        "label": label,
        "balanceUsd": null,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "status": status,
        "capturedAt": now_millis(),
        "source": "local_auth",
        "plan": plan,
        "lines": lines,
        "meta": {
            "modelCount": model_count,
            "apiEndpoint": models_url,
        },
        "message": message
    }))
}

fn build_opencode_go_lines(db_path: &Path) -> Option<Vec<Value>> {
    let conn = Connection::open(db_path).ok()?;
    let sql = "SELECT data FROM message WHERE json_valid(data) AND json_extract(data, '$.providerID') = 'opencode-go' AND json_extract(data, '$.role') = 'assistant' AND json_type(data, '$.cost') IN ('integer', 'real')";
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    #[derive(Default)]
    struct CostRow {
        created_ms: i64,
        cost: f64,
    }

    let mut costs: Vec<CostRow> = Vec::new();
    for row in rows {
        let Ok(data_str) = row else { continue };
        let Ok(data) = serde_json::from_str::<Value>(&data_str) else {
            continue;
        };
        let cost = data.get("cost").and_then(read_number_value).unwrap_or(0.0);
        if cost <= 0.0 {
            continue;
        }
        let created_ms = data
            .get("time")
            .and_then(|t| t.get("created"))
            .and_then(read_number_value)
            .map(|v| v as i64)
            .unwrap_or(now_ms);
        costs.push(CostRow { created_ms, cost });
    }
    drop(stmt);
    drop(conn);

    if costs.is_empty() {
        return None;
    }

    costs.sort_by_key(|r| r.created_ms);

    let mut lines = Vec::new();

    let sum_in_window = |start: i64, end: i64| -> f64 {
        let total: f64 = costs
            .iter()
            .filter(|r| r.created_ms >= start && r.created_ms < end)
            .map(|r| r.cost)
            .sum();
        (total * 10000.0).round() / 10000.0
    };

    fn clamp_pct(used: f64, limit: f64) -> f64 {
        if limit <= 0.0 {
            return 0.0;
        }
        ((used / limit * 100.0 * 10.0).round() / 10.0).clamp(0.0, 100.0)
    }

    fn start_of_utc_week(now_ms: i64) -> i64 {
        let secs = now_ms / 1000;
        let days_since_epoch = secs / 86400;
        let day_of_week = ((days_since_epoch + 4) % 7 + 7) % 7;
        let monday_start = (days_since_epoch - day_of_week) as i64 * 86400 * 1000;
        monday_start
    }

    fn start_of_utc_month(now_ms: i64) -> i64 {
        let secs = now_ms / 1000;
        let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_else(chrono::Utc::now);
        match chrono::Utc.with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0) {
            chrono::LocalResult::Single(t) => t.timestamp_millis(),
            _ => now_ms,
        }
    }

    // Compute monthly bounds anchored to the day-of-month of the earliest usage row
    fn anchored_month(now_ms: i64, anchor_ms: i64) -> (i64, i64) {
        let now_secs = now_ms / 1000;
        let now_dt =
            chrono::DateTime::from_timestamp(now_secs, 0).unwrap_or_else(chrono::Utc::now);
        let anchor_secs = anchor_ms / 1000;
        let anchor_dt =
            chrono::DateTime::from_timestamp(anchor_secs, 0).unwrap_or_else(chrono::Utc::now);
        let day = anchor_dt.day();

        let build = |year: i32, month: u32| -> Option<i64> {
            let days_in_month = chrono::NaiveDate::from_ymd_opt(year, month, 1)
                .and_then(|d| d.pred_opt())
                .map(|d| d.day())
                .unwrap_or(30);
            let d = std::cmp::min(day, days_in_month);
            match chrono::Utc.with_ymd_and_hms(year, month, d, 0, 0, 0) {
                chrono::LocalResult::Single(t) => Some(t.timestamp_millis()),
                _ => None,
            }
        };

        let start = build(now_dt.year(), now_dt.month()).unwrap_or(now_ms);
        let start = if start > now_ms {
            let (y, m) = if now_dt.month() == 1 {
                (now_dt.year() - 1, 12)
            } else {
                (now_dt.year(), now_dt.month() - 1)
            };
            build(y, m).unwrap_or(start)
        } else {
            start
        };

        let end = {
            let (y, m) = if now_dt.month() == 12 {
                (now_dt.year() + 1, 1)
            } else {
                (now_dt.year(), now_dt.month() + 1)
            };
            build(y, m).unwrap_or(start + 30 * 86400 * 1000)
        };

        (start, end)
    }

    let session_limit = 12.0;
    let weekly_limit = 30.0;
    let monthly_limit = 60.0;
    let five_hours_ms: i64 = 5 * 60 * 60 * 1000;

    let session_start = now_ms - five_hours_ms;
    let session_cost = sum_in_window(session_start, now_ms);
    let session_pct = clamp_pct(session_cost, session_limit);

    let week_start = start_of_utc_week(now_ms);
    let week_end = week_start + 7 * 24 * 60 * 60 * 1000;
    let weekly_cost = sum_in_window(week_start, week_end);
    let weekly_pct = clamp_pct(weekly_cost, weekly_limit);

    let earliest_ms = costs.first().map(|r| r.created_ms).unwrap_or(now_ms);
    let (month_start, month_end) = if earliest_ms < now_ms {
        anchored_month(now_ms, earliest_ms)
    } else {
        let s = start_of_utc_month(now_ms);
        let next = {
            let secs = now_ms / 1000;
            let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_else(chrono::Utc::now);
            let (y, m) = if dt.month() == 12 {
                (dt.year() + 1, 1)
            } else {
                (dt.year(), dt.month() + 1)
            };
            match chrono::Utc.with_ymd_and_hms(y, m, 1, 0, 0, 0) {
                chrono::LocalResult::Single(t) => t.timestamp_millis(),
                _ => s + 30 * 86400 * 1000,
            }
        };
        (s, next)
    };

    let monthly_cost = sum_in_window(month_start, month_end);
    let monthly_pct = clamp_pct(monthly_cost, monthly_limit);

    let ms_to_iso = |ms: i64| -> String {
        let secs = ms / 1000;
        let millis = (ms % 1000) as u32;
        chrono::DateTime::from_timestamp(secs, millis * 1_000_000)
            .unwrap_or_else(chrono::Utc::now)
            .to_rfc3339()
    };

    let session_reset: i64 = {
        let oldest_in_window = costs
            .iter()
            .filter(|r| r.created_ms >= session_start && r.created_ms < now_ms)
            .map(|r| r.created_ms)
            .min()
            .unwrap_or(now_ms);
        oldest_in_window + five_hours_ms
    };

    lines.push(json!({
        "type": "progress",
        "label": "Session",
        "used": session_pct,
        "limit": 100.0,
        "format": { "kind": "percent" },
        "subtitle": format!("${:.2} / ${:.0}", session_cost, session_limit),
        "resetsAt": ms_to_iso(session_reset)
    }));

    lines.push(json!({
        "type": "progress",
        "label": "Weekly",
        "used": weekly_pct,
        "limit": 100.0,
        "format": { "kind": "percent" },
        "subtitle": format!("${:.2} / ${:.0}", weekly_cost, weekly_limit),
        "resetsAt": ms_to_iso(week_end)
    }));

    lines.push(json!({
        "type": "progress",
        "label": "Monthly",
        "used": monthly_pct,
        "limit": 100.0,
        "format": { "kind": "percent" },
        "subtitle": format!("${:.2} / ${:.0}", monthly_cost, monthly_limit),
        "resetsAt": ms_to_iso(month_end)
    }));

    Some(lines)
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
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, millis * 1_000_000)
        .unwrap_or_else(chrono::Utc::now);
    datetime.to_rfc3339()
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
        "command": pending.command,
        "filePath": pending.file_path,
        "toolName": pending.tool_name,
        "raw": pending.raw,
        "meta": pending.meta,
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

    let suffix = if failed.len() >= 3 {
        " Check provider credentials or network access."
    } else {
        ""
    };
    emit_runtime_warning(
        app,
        &format!("Sync completed with issues: {}.{}", failed.join(", "), suffix),
    );
}

fn build_intervention(
    source: &str,
    event_name: &str,
    raw: &str,
    payload: Value,
) -> PendingIntervention {
    let tool_name = payload
        .get("tool_name")
        .or_else(|| payload.get("toolName"))
        .or_else(|| payload.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or("permission")
        .to_string();
    let command = payload
        .get("command")
        .or_else(|| payload.get("cmd"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            payload
                .get("tool_input")
                .and_then(Value::as_object)
                .and_then(|input| {
                    input
                        .get("command")
                        .or_else(|| input.get("cmd"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        })
        .unwrap_or_default();
    let file_path = payload
        .get("file_path")
        .or_else(|| payload.get("filePath"))
        .or_else(|| payload.get("path"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let detail = payload
        .get("reason")
        .or_else(|| payload.get("message"))
        .or_else(|| payload.get("prompt"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if command.is_empty() {
                raw.chars().take(240).collect()
            } else {
                command.clone()
            }
        });
    let source_label = if source == "codex" { "Codex" } else { "Agent" };
    let title = if !file_path.is_empty() {
        format!("{source_label} wants to edit {file_path}")
    } else if !command.is_empty() {
        format!("{source_label} wants to run a command")
    } else {
        format!("{source_label} needs approval")
    };

    PendingIntervention {
        id: format!("req_{}", now_millis()),
        source: source.to_string(),
        event: event_name.to_string(),
        title,
        detail,
        command,
        file_path,
        tool_name,
        raw: raw.to_string(),
        meta: payload,
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
    }
    let data = {
        let state = app.state::<AppState>();
        get_dashboard_data(&state)
    };
    emit_sync_warning_if_needed(&app, data.get("accounts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
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
        .min((area_h as f64 * 0.8).max(80.0)) as u32;
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
    state.drag_start_mouse = None;
    Ok(())
}

#[tauri::command]
fn intervention_respond(app: AppHandle, decision: String) -> bool {
    let app_state = app.state::<AppState>();
    let mut pending = match app_state.intervention.lock() {
        Ok(pending) => pending,
        Err(_) => return false,
    };
    let Some(mut current) = pending.take() else {
        return false;
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
        });
    }
    let _ = app.emit("intervention-state", Option::<Value>::None);
    let _ = set_mode(&app, "pill");
    true
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
    json!({
        "syncIntervalMinutes": current_sync_interval(&app)
    })
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

fn position_initial(window: &WebviewWindow) -> Result<(), String> {
    let (area_x, area_y, area_w, _) = primary_work_area(window)?;
    let x = area_x + ((area_w as i32 - PILL_WIDTH as i32) / 2);
    window
        .set_position(PhysicalPosition::new(x, area_y + WINDOW_MARGIN))
        .map_err(|err| err.to_string())
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

async fn perform_initial_sync(app: AppHandle) {
    let accounts = sync_provider_accounts().await;
    let _ = replace_usage_balances(&app, accounts);
    let data = {
        let state = app.state::<AppState>();
        get_dashboard_data(&state)
    };
    emit_sync_warning_if_needed(&app, data.get("accounts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
    let _ = app.emit("island-data", data);
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
            emit_sync_warning_if_needed(&app, data.get("accounts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]));
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
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
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
            settings_set_sync_interval
        ])
        .setup(|app| {
            let window = main_window(app.handle()).map_err(Box::<dyn std::error::Error>::from)?;
            position_initial(&window).map_err(Box::<dyn std::error::Error>::from)?;
            window.show().map_err(Box::<dyn std::error::Error>::from)?;
            initialize_sync_interval(app.handle());
            shortcuts::register_shortcuts(app.handle());

            // Tray icon
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
                .map_err(|e| format!("tray icon: {e}")).ok();
            if let Some(icon) = tray_icon {
                let open = MenuItemBuilder::with_id("open", "Open")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let sync_item = MenuItemBuilder::with_id("sync", "Sync Now")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let sep = PredefinedMenuItem::separator(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();
                let quit_item = MenuItemBuilder::with_id("quit", "Quit")
                    .build(app)
                    .map_err(|e| format!("tray menu: {e}")).ok();

                if let (Some(open), Some(sync_item), Some(sep), Some(quit_item)) =
                    (open, sync_item, sep, quit_item)
                {
                    let menu = MenuBuilder::new(app)
                        .items(&[&open, &sync_item, &sep, &quit_item])
                        .build()
                        .map_err(|e| format!("tray menu: {e}")).ok();

                    if let Some(menu) = menu {
                        let _tray = TrayIconBuilder::new()
                            .icon(icon)
                            .menu(&menu)
                            .on_menu_event(move |app_handle_inner, event| {
                                match event.id().as_ref() {
                                    "open" => {
                                        if let Ok(window) = main_window(&app_handle_inner) {
                                            let _ = window.show();
                                            let _ = window.set_focus();
                                        }
                                        let _ =
                                            set_mode(&app_handle_inner, "expanded");
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
            hooks::inject_agent_hooks(app.handle());

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

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
