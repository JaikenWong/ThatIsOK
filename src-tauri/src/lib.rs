mod codex_provider;
mod cursor_provider;
mod hooks;
mod local_providers;
mod providers;
mod remote_providers;
mod shortcuts;

use chrono::{Datelike, TimeZone};
use rusqlite::Connection;
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
const EXPANDED_WIDTH: u32 = 560;
const DEFAULT_EXPANDED_HEIGHT: u32 = 600;
const WINDOW_MARGIN: i32 = 12;
const MANAGED_KEY: &str = "ThatIsOk";
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
    let command = command.to_ascii_lowercase();
    command.contains("thatisok")
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
        Ok(resp) if resp.status().is_success() => match resp.json::<Value>().await {
            Ok(data) => data
                .get("data")
                .and_then(Value::as_array)
                .map(|models| models.len())
                .unwrap_or(0),
            Err(_) => 0,
        },
        _ => 0,
    };

    let plan = if is_go { "OpenCode Go" } else { "OpenCode Zen" };
    let status = if model_count > 0 { "live" } else { "warn" };
    let message = if model_count > 0 {
        format!("{plan} · {} models available", model_count)
    } else {
        format!("{plan} · API unreachable")
    };

    let mut lines = Vec::new();
    let mut usage_source = "local_auth".to_string();
    let mut dashboard_message: Option<String> = None;
    let mut zen_balance_usd: Option<f64> = None;

    if let Some(cookie_header) = read_opencode_go_cookie_header() {
        eprintln!("[opencode] cookie configured; trying web usage first");
        let cookie = normalize_cookie_header(&cookie_header);
        let workspace_id = match cookie {
            Some(ref c) => fetch_opencode_go_workspace_id(client, c).await,
            None => None,
        };
        eprintln!(
            "[opencode] workspace id {}",
            workspace_id.as_deref().unwrap_or("<missing>")
        );

        if is_go {
            // Go: try web dashboard first, then subscription API fallback
            if let Some(ref c) = cookie {
                if let Some(web_lines) = fetch_opencode_go_web_lines(client, c).await {
                    eprintln!(
                        "[opencode] using web dashboard usage lines={}",
                        web_lines.len()
                    );
                    lines.extend(web_lines);
                    usage_source = "opencode-go-web".to_string();
                    dashboard_message = Some("official dashboard usage".to_string());
                } else {
                    eprintln!("[opencode] web dashboard usage unavailable");
                }
            }
            if lines.is_empty() {
                if let Some(ref wid) = workspace_id {
                    eprintln!("[opencode] trying subscription usage fallback");
                    if let Some(sub_lines) =
                        fetch_opencode_subscription_lines(client, &cookie_header, wid).await
                    {
                        eprintln!(
                            "[opencode] using subscription usage lines={}",
                            sub_lines.len()
                        );
                        lines.extend(sub_lines);
                        usage_source = "opencode-subscription".to_string();
                        dashboard_message = Some("subscription usage".to_string());
                    } else {
                        eprintln!("[opencode] subscription usage unavailable");
                    }
                } else {
                    eprintln!("[opencode] skip subscription usage: missing workspace id");
                }
            }
            // Zen balance (concurrent-ish, runs after usage lines)
            if let Some(ref wid) = workspace_id {
                zen_balance_usd = fetch_opencode_zen_balance(client, &cookie_header, wid).await;
            }
        } else {
            // Non-Go: subscription API only
            if let Some(ref wid) = workspace_id {
                eprintln!("[opencode] Zen account; trying subscription usage");
                if let Some(sub_lines) =
                    fetch_opencode_subscription_lines(client, &cookie_header, wid).await
                {
                    eprintln!(
                        "[opencode] using subscription usage lines={}",
                        sub_lines.len()
                    );
                    lines.extend(sub_lines);
                    usage_source = "opencode-subscription".to_string();
                    dashboard_message = Some("subscription usage".to_string());
                } else {
                    eprintln!("[opencode] subscription usage unavailable");
                }
            } else {
                eprintln!("[opencode] skip subscription usage: missing workspace id");
            }
        }
    } else {
        eprintln!("[opencode] no web cookie configured; skipping web usage");
    }

    if lines.is_empty() && is_go {
        let db_path = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        if let Some(local_lines) = build_opencode_go_local_lines(&db_path) {
            eprintln!(
                "[opencode] using local SQLite usage lines={}",
                local_lines.len()
            );
            lines.extend(local_lines);
            usage_source = "opencode-go-local".to_string();
            dashboard_message = Some("local history estimate".to_string());
        } else {
            eprintln!("[opencode] local SQLite usage unavailable");
        }
    }

    let token_usage = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("opencode.db");
    let token_usage = read_opencode_token_usage(&token_usage).unwrap_or_default();

    if model_count > 0 {
        lines.push(json!({
            "type": "text",
            "label": "Models",
            "value": format_compact(model_count as f64),
            "subtitle": format!("{} models via {} API", model_count, if is_go { "Go" } else { "Zen" })
        }));
    }

    if let Some(balance) = zen_balance_usd {
        lines.push(json!({
            "type": "text",
            "label": "Zen Balance",
            "value": format!("${:.2}", balance),
            "subtitle": "current Zen credit"
        }));
    }

    // Hint if no cookie configured
    if usage_source == "local_auth" && is_go {
        lines.push(json!({
            "type": "text",
            "label": "Cookie",
            "value": "not set",
            "subtitle": "set opencodeGoCookieHeader in defaults.json for accurate usage"
        }));
    }

    Some(json!({
        "accountId": account_id,
        "provider": "opencode",
        "label": label,
        "balanceUsd": zen_balance_usd,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "status": status,
        "capturedAt": now_millis(),
        "source": usage_source,
        "plan": plan,
        "lines": lines,
        "tokenUsage": {
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
        },
        "meta": {
            "modelCount": model_count,
            "apiEndpoint": models_url,
        },
        "message": dashboard_message.unwrap_or(message)
    }))
}

#[derive(Default)]
struct LocalTokenUsage {
    input: u64,
    output: u64,
    reasoning: u64,
    cache_read: u64,
    cache_write: u64,
}

#[derive(Default)]
struct CostRow {
    created_ms: i64,
    cost: f64,
}

async fn fetch_opencode_go_web_lines(
    client: &reqwest::Client,
    cookie_header: &str,
) -> Option<Vec<Value>> {
    let cookie = normalize_cookie_header(cookie_header)?;
    let workspace_id = fetch_opencode_go_workspace_id(client, &cookie).await?;
    let url = format!("https://opencode.ai/workspace/{workspace_id}/go");
    eprintln!("[opencode] fetch web dashboard url={url}");
    let response = client
        .get(&url)
        .header("Cookie", cookie)
        .header("User-Agent", OPENCODE_GO_USER_AGENT)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
        .map_err(|error| eprintln!("[opencode] web dashboard request failed: {error}"))
        .ok()?;
    let status = response.status();
    eprintln!("[opencode] web dashboard status={status}");
    if !status.is_success() {
        return None;
    }
    let text = response
        .text()
        .await
        .map_err(|error| eprintln!("[opencode] web dashboard body failed: {error}"))
        .ok()?;
    eprintln!("[opencode] web dashboard body bytes={}", text.len());
    let lines = build_opencode_go_web_lines(&text);
    eprintln!(
        "[opencode] web dashboard parse {}",
        if lines.is_some() { "ok" } else { "failed" }
    );
    lines
}

async fn fetch_opencode_go_workspace_id(
    client: &reqwest::Client,
    cookie_header: &str,
) -> Option<String> {
    if let Some(id) = read_opencode_go_workspace_override() {
        eprintln!("[opencode] workspace id from override");
        return Some(id);
    }
    let text = fetch_opencode_go_workspace_text(client, cookie_header, "GET", None).await?;
    if let Some(id) = parse_opencode_go_workspace_id(&text) {
        eprintln!("[opencode] workspace id from GET server response");
        return Some(id);
    }
    let fallback =
        fetch_opencode_go_workspace_text(client, cookie_header, "POST", Some("[]")).await?;
    let id = parse_opencode_go_workspace_id(&fallback);
    if id.is_some() {
        eprintln!("[opencode] workspace id from POST server response");
    } else {
        eprintln!("[opencode] workspace id parse failed");
    }
    id
}

async fn fetch_opencode_server_text(
    client: &reqwest::Client,
    cookie_header: &str,
    server_id: &str,
    args: Option<&str>,
    method: &str,
    referer: &str,
) -> Option<String> {
    let request = if method == "POST" {
        client.post("https://opencode.ai/_server")
    } else {
        client.get("https://opencode.ai/_server")
    };
    let request = request
        .query(&[("id", server_id)])
        .header("Cookie", cookie_header)
        .header("X-Server-Id", server_id)
        .header("X-Server-Instance", format!("server-fn:{}", now_millis()))
        .header("User-Agent", OPENCODE_GO_USER_AGENT)
        .header("Origin", "https://opencode.ai")
        .header("Referer", referer)
        .header(
            "Accept",
            "text/javascript, application/json;q=0.9, */*;q=0.8",
        );
    let request = if method == "POST" {
        request
            .header("Content-Type", "text/plain;charset=UTF-8")
            .body(args.unwrap_or("[]").to_string())
    } else {
        request
    };
    request.send().await.ok()?.text().await.ok()
}

async fn fetch_opencode_go_workspace_text(
    client: &reqwest::Client,
    cookie_header: &str,
    method: &str,
    body: Option<&str>,
) -> Option<String> {
    fetch_opencode_server_text(
        client,
        cookie_header,
        OPENCODE_GO_WORKSPACES_SERVER_ID,
        body,
        method,
        "https://opencode.ai",
    )
    .await
}

fn build_opencode_go_web_lines(text: &str) -> Option<Vec<Value>> {
    // Layer 1: try full JSON parse (CodexBar: parseSubscriptionJSON)
    if let Some(lines) = parse_opencode_subscription_json(text) {
        return Some(lines);
    }
    // Layer 2: regex fallback on raw text (CodexBar: parseSubscription regex)
    parse_opencode_subscription_regex(text)
}

/// JSON-based parsing: try top-level dict, nested keys, recursive search, then candidate search
fn parse_opencode_subscription_json(text: &str) -> Option<Vec<Value>> {
    let val: Value = serde_json::from_str(text).ok()?;
    let dict = val.as_object()?;

    // Try direct usage dict
    if let Some(lines) = parse_usage_dictionary(dict) {
        return Some(lines);
    }
    // Try nested keys
    for key in ["data", "result", "usage", "billing", "payload"] {
        if let Some(nested) = dict.get(key).and_then(Value::as_object) {
            if let Some(lines) = parse_usage_dictionary(nested) {
                return Some(lines);
            }
        }
    }
    // Recursive nested search
    if let Some(lines) = parse_usage_nested(dict, 0) {
        return Some(lines);
    }
    // Candidate-based search
    parse_usage_from_candidates(&val)
}

fn parse_usage_dictionary(dict: &serde_json::Map<String, Value>) -> Option<Vec<Value>> {
    // Recurse into nested "usage" key
    if let Some(usage) = dict.get("usage").and_then(Value::as_object) {
        if let Some(lines) = parse_usage_dictionary(usage) {
            return Some(lines);
        }
    }

    let rolling_keys = [
        "rollingUsage",
        "rolling",
        "rolling_usage",
        "rollingWindow",
        "rolling_window",
    ];
    let weekly_keys = [
        "weeklyUsage",
        "weekly",
        "weekly_usage",
        "weeklyWindow",
        "weekly_window",
    ];

    let rolling = first_dict_from(dict, &rolling_keys);
    let weekly = first_dict_from(dict, &weekly_keys);

    let (Some(rolling), Some(weekly)) = (rolling, weekly) else {
        return None;
    };

    build_lines_from_windows(&rolling, &weekly)
}

fn parse_usage_nested(dict: &serde_json::Map<String, Value>, depth: usize) -> Option<Vec<Value>> {
    if depth > 3 {
        return None;
    }
    let mut rolling: Option<&serde_json::Map<String, Value>> = None;
    let mut weekly: Option<&serde_json::Map<String, Value>> = None;

    for (key, value) in dict {
        let sub = match value.as_object() {
            Some(s) => s,
            None => continue,
        };
        let lower = key.to_lowercase();
        if lower.contains("rolling")
            || lower.contains("hour")
            || lower.contains("5h")
            || lower.contains("5-hour")
        {
            rolling = Some(sub);
        } else if lower.contains("weekly") || lower.contains("week") {
            weekly = Some(sub);
        }
    }

    if let (Some(r), Some(w)) = (rolling, weekly) {
        if let Some(lines) = build_lines_from_windows(r, w) {
            return Some(lines);
        }
    }

    for value in dict.values() {
        if let Some(sub) = value.as_object() {
            if let Some(lines) = parse_usage_nested(sub, depth + 1) {
                return Some(lines);
            }
        }
    }
    None
}

struct WindowCandidate {
    percent: f64,
    reset_in_sec: i64,
    path_lower: String,
}

fn parse_usage_from_candidates(val: &Value) -> Option<Vec<Value>> {
    let mut candidates: Vec<WindowCandidate> = Vec::new();
    collect_window_candidates(val, &mut candidates, "");
    if candidates.is_empty() {
        return None;
    }

    let rolling = candidates
        .iter()
        .find(|c| {
            c.path_lower.contains("rolling")
                || c.path_lower.contains("hour")
                || c.path_lower.contains("5h")
                || c.path_lower.contains("5-hour")
        })
        .or_else(|| candidates.first())?;
    let weekly = candidates.iter().find(|c| {
        (c.path_lower.contains("weekly") || c.path_lower.contains("week"))
            && c.path_lower != rolling.path_lower
    })?;

    let mut lines = Vec::new();
    lines.push(opencode_go_web_line(
        "5-hour",
        rolling.percent,
        rolling.reset_in_sec,
    ));
    lines.push(opencode_go_web_line(
        "Weekly",
        weekly.percent,
        weekly.reset_in_sec,
    ));
    Some(lines)
}

fn collect_window_candidates(val: &Value, out: &mut Vec<WindowCandidate>, path: &str) {
    if let Some(dict) = val.as_object() {
        if let Some((percent, reset)) = try_parse_window_dict(dict) {
            out.push(WindowCandidate {
                percent,
                reset_in_sec: reset,
                path_lower: path.to_lowercase(),
            });
        }
        for (key, value) in dict {
            let child_path = if path.is_empty() {
                key.clone()
            } else {
                format!("{}/{}", path, key)
            };
            collect_window_candidates(value, out, &child_path);
        }
    } else if let Some(arr) = val.as_array() {
        for (i, item) in arr.iter().enumerate() {
            collect_window_candidates(item, out, &format!("{}/{}", path, i));
        }
    }
}

fn try_parse_window_dict(dict: &serde_json::Map<String, Value>) -> Option<(f64, i64)> {
    let percent_keys = [
        "usagePercent",
        "usedPercent",
        "percentUsed",
        "percent",
        "usage_percent",
        "used_percent",
        "utilization",
        "utilizationPercent",
        "utilization_percent",
        "usage",
    ];
    let reset_in_keys = [
        "resetInSec",
        "resetInSeconds",
        "resetSeconds",
        "reset_sec",
        "reset_in_sec",
        "resetsInSec",
        "resetsInSeconds",
        "resetIn",
        "resetSec",
    ];
    let reset_at_keys = [
        "resetAt",
        "resetsAt",
        "reset_at",
        "resets_at",
        "nextReset",
        "next_reset",
        "renewAt",
        "renew_at",
    ];
    let used_keys = ["used", "usage", "consumed", "count", "usedTokens"];
    let limit_keys = ["limit", "total", "quota", "max", "cap", "tokenLimit"];

    // Try percent keys
    let mut percent: Option<f64> = None;
    for key in &percent_keys {
        if let Some(n) = dict.get(*key).and_then(|v| json_f64(v)) {
            percent = Some(n);
            break;
        }
    }

    // Fallback: used/limit ratio
    if percent.is_none() {
        let used = used_keys
            .iter()
            .find_map(|k| dict.get(*k).and_then(|v| json_f64(v)));
        let limit = limit_keys
            .iter()
            .find_map(|k| dict.get(*k).and_then(|v| json_f64(v)));
        if let (Some(u), Some(l)) = (used, limit) {
            if l > 0.0 {
                percent = Some((u / l) * 100.0);
            }
        }
    }

    let mut resolved = percent?;
    if (0.0..=1.0).contains(&resolved) {
        resolved *= 100.0;
    }
    resolved = resolved.clamp(0.0, 100.0);

    let mut reset: Option<i64> = None;
    for key in &reset_in_keys {
        if let Some(n) = dict.get(*key).and_then(|v| json_i64(v)) {
            reset = Some(n);
            break;
        }
    }
    if reset.is_none() {
        for key in &reset_at_keys {
            if let Some(s) = dict.get(*key).and_then(Value::as_str) {
                if let Some(secs) = reset_seconds_from_iso(s) {
                    reset = Some(secs);
                    break;
                }
            }
        }
    }

    Some((resolved, reset.unwrap_or(0)))
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| {
        v.as_str()
            .and_then(|s| s.replace(',', "").parse::<f64>().ok())
    })
}

fn json_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn first_dict_from<'a>(
    dict: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<&'a serde_json::Map<String, Value>> {
    for key in keys {
        if let Some(obj) = dict.get(*key).and_then(Value::as_object) {
            return Some(obj);
        }
    }
    None
}

fn build_lines_from_windows(
    rolling: &serde_json::Map<String, Value>,
    weekly: &serde_json::Map<String, Value>,
) -> Option<Vec<Value>> {
    let r = try_parse_window_dict(rolling)?;
    let w = try_parse_window_dict(weekly)?;

    let mut lines = Vec::new();
    lines.push(opencode_go_web_line("5-hour", r.0, r.1));
    lines.push(opencode_go_web_line("Weekly", w.0, w.1));
    Some(lines)
}

/// Regex fallback: extract rollingUsage/weeklyUsage from raw text
fn parse_opencode_subscription_regex(text: &str) -> Option<Vec<Value>> {
    let rolling_pct = extract_double_regex(
        r#"rollingUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#,
        text,
    )?;
    let rolling_rst = extract_int_regex(r#"rollingUsage[^}]*?resetInSec\s*:\s*([0-9]+)"#, text)?;
    let weekly_pct = extract_double_regex(
        r#"weeklyUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#,
        text,
    )?;
    let weekly_rst = extract_int_regex(r#"weeklyUsage[^}]*?resetInSec\s*:\s*([0-9]+)"#, text)?;

    let mut lines = Vec::new();
    lines.push(opencode_go_web_line("5-hour", rolling_pct, rolling_rst));
    lines.push(opencode_go_web_line("Weekly", weekly_pct, weekly_rst));
    Some(lines)
}

fn extract_double_regex(pattern: &str, text: &str) -> Option<f64> {
    let re = regex::Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    caps.get(1).and_then(|m| m.as_str().parse::<f64>().ok())
}

fn extract_int_regex(pattern: &str, text: &str) -> Option<i64> {
    let re = regex::Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    caps.get(1).and_then(|m| m.as_str().parse::<i64>().ok())
}

async fn fetch_opencode_subscription_lines(
    client: &reqwest::Client,
    cookie_header: &str,
    workspace_id: &str,
) -> Option<Vec<Value>> {
    let referer = format!("https://opencode.ai/workspace/{workspace_id}/billing");
    let args = format!("[\"{workspace_id}\"]");
    let text = fetch_opencode_server_text(
        client,
        cookie_header,
        OPENCODE_SUBSCRIPTION_SERVER_ID,
        Some(&args),
        "GET",
        &referer,
    )
    .await;
    let text = match text {
        Some(t) => t,
        None => {
            fetch_opencode_server_text(
                client,
                cookie_header,
                OPENCODE_SUBSCRIPTION_SERVER_ID,
                Some(&args),
                "POST",
                &referer,
            )
            .await?
        }
    };
    build_opencode_go_web_lines(&text)
}

async fn fetch_opencode_zen_balance(
    client: &reqwest::Client,
    cookie_header: &str,
    workspace_id: &str,
) -> Option<f64> {
    let cookie = normalize_cookie_header(cookie_header)?;
    // Try HTML page first
    let url = format!("https://opencode.ai/workspace/{workspace_id}");
    if let Ok(resp) = client
        .get(&url)
        .header("Cookie", &cookie)
        .header("User-Agent", OPENCODE_GO_USER_AGENT)
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .send()
        .await
    {
        if let Ok(text) = resp.text().await {
            if let Some(balance) = parse_opencode_zen_balance_from_text(&text) {
                return Some(balance);
            }
        }
    }
    // Fallback: billing server
    let args = format!("[\"{workspace_id}\"]");
    let referer = format!("https://opencode.ai/workspace/{workspace_id}");
    if let Some(text) = fetch_opencode_server_text(
        client,
        cookie_header,
        OPENCODE_GO_BILLING_SERVER_ID,
        Some(&args),
        "GET",
        &referer,
    )
    .await
    {
        if let Some(balance) = parse_opencode_zen_balance_from_billing(&text) {
            return Some(balance);
        }
    }
    None
}

fn parse_opencode_zen_balance_from_text(text: &str) -> Option<f64> {
    // Try JSON recursive walk first
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        if let Some(balance) = find_zen_balance_in_json(&val) {
            return Some(balance);
        }
    }
    // Regex: "current balance" / "zen balance" followed by $ amount
    let re = regex::Regex::new(
        r#"(?i)(?:current\s+balance|zen\s+balance|現在の残高)[^$]{0,80}\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)"#,
    )
    .ok()?;
    if let Some(caps) = re.captures(text) {
        if let Some(m) = caps.get(1) {
            if let Ok(v) = m.as_str().replace(',', "").parse::<f64>() {
                return Some(v);
            }
        }
    }
    // Broader: "balance" within 120 chars of $ amount
    let re2 =
        regex::Regex::new(r#"(?i)(?:balance|残高)[\s\S]{0,120}?\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)"#)
            .ok()?;
    if let Some(caps) = re2.captures(text) {
        if let Some(m) = caps.get(1) {
            if let Ok(v) = m.as_str().replace(',', "").parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

fn parse_opencode_zen_balance_from_billing(text: &str) -> Option<f64> {
    // Try JSON: find customerID + balance, divide by 100_000_000
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        if let Some(raw) = find_raw_billing_balance(&val) {
            return Some(raw / 100_000_000.0);
        }
    }
    // Regex fallback for RSC patterns
    let has_customer = regex::Regex::new(r#"(?:\"customerID\"|customerID)"#)
        .ok()?
        .is_match(text);
    if !has_customer {
        return None;
    }
    let re = regex::Regex::new(
        r#"(?:"balance"|balance)\s*:\s*(?:\$R\[\d+\]\s*=\s*)?(-?[0-9]+(?:\.[0-9]+)?)"#,
    )
    .ok()?;
    if let Some(caps) = re.captures(text) {
        if let Some(m) = caps.get(1) {
            if let Ok(raw) = m.as_str().parse::<f64>() {
                return Some(raw / 100_000_000.0);
            }
        }
    }
    None
}

fn find_zen_balance_in_json(val: &Value) -> Option<f64> {
    match val {
        Value::Object(map) => {
            for (key, value) in map {
                let normalized = key
                    .chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
                    .to_lowercase();
                if [
                    "zenbalance",
                    "zencurrentbalance",
                    "currentbalance",
                    "currentbalanceusd",
                    "balanceusd",
                    "usdbalance",
                ]
                .contains(&normalized.as_str())
                {
                    if let Some(n) = value.as_f64() {
                        return Some(n);
                    }
                    if let Some(s) = value.as_str() {
                        if let Ok(n) = s.replace(',', "").parse::<f64>() {
                            return Some(n);
                        }
                    }
                }
                if let Some(found) = find_zen_balance_in_json(value) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(find_zen_balance_in_json),
        _ => None,
    }
}

fn find_raw_billing_balance(val: &Value) -> Option<f64> {
    match val {
        Value::Object(map) => {
            if map.contains_key("balance") && map.contains_key("customerID") {
                let cid = map.get("customerID").and_then(Value::as_str).unwrap_or("");
                if !cid.is_empty() {
                    if let Some(n) = map.get("balance").and_then(|v| v.as_f64()) {
                        return Some(n);
                    }
                }
            }
            for value in map.values() {
                if let Some(found) = find_raw_billing_balance(value) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(arr) => arr.iter().find_map(find_raw_billing_balance),
        _ => None,
    }
}

fn opencode_go_web_line(label: &str, used_percent: f64, reset_in_sec: i64) -> Value {
    let remaining = (100.0 - used_percent).clamp(0.0, 100.0);
    let reset_at_ms = chrono::Utc::now().timestamp_millis() + reset_in_sec.max(0) * 1000;
    json!({
        "type": "progress",
        "label": label,
        "used": remaining,
        "limit": 100.0,
        "format": { "kind": "percent", "mode": "remaining" },
        "subtitle": format!("{:.0}% used", used_percent.clamp(0.0, 100.0)),
        "resetsAt": millis_to_iso(reset_at_ms)
    })
}

fn build_opencode_go_local_lines(db_path: &Path) -> Option<Vec<Value>> {
    let rows = read_opencode_go_cost_rows(db_path)?;
    if rows.is_empty() {
        return None;
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let five_hours_ms: i64 = 5 * 60 * 60 * 1000;
    let week_ms: i64 = 7 * 24 * 60 * 60 * 1000;
    let session_start = now_ms - five_hours_ms;
    let week_start = start_of_utc_week_ms(now_ms);
    let week_end = week_start + week_ms;

    let session_cost = sum_opencode_go_costs(&rows, session_start, now_ms);
    let weekly_cost = sum_opencode_go_costs(&rows, week_start, week_end);

    let session_reset = rows
        .iter()
        .filter(|row| row.created_ms >= session_start && row.created_ms < now_ms)
        .map(|row| row.created_ms)
        .min()
        .unwrap_or(now_ms)
        + five_hours_ms;

    Some(vec![
        opencode_go_local_line("5-hour", session_cost, 12.0, session_reset),
        opencode_go_local_line("Weekly", weekly_cost, 30.0, week_end),
    ])
}

fn read_opencode_go_cost_rows(db_path: &Path) -> Option<Vec<CostRow>> {
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open(db_path).ok()?;
    let sql = if sqlite_has_table(&conn, "part") {
        "WITH message_costs AS (
            SELECT
              id AS messageID,
              CAST(COALESCE(json_extract(data, '$.time.created'), time_created) AS INTEGER) AS createdMs,
              CAST(json_extract(data, '$.cost') AS REAL) AS cost
            FROM message
            WHERE json_valid(data)
              AND json_extract(data, '$.providerID') = 'opencode-go'
              AND json_extract(data, '$.role') = 'assistant'
              AND json_type(data, '$.cost') IN ('integer', 'real')
          )
          SELECT createdMs, cost
          FROM message_costs
          UNION ALL
          SELECT
            CAST(COALESCE(json_extract(p.data, '$.time.created'), p.time_created, m.time_created) AS INTEGER) AS createdMs,
            CAST(json_extract(p.data, '$.cost') AS REAL) AS cost
          FROM part p
          JOIN message m ON m.id = p.message_id
          WHERE json_valid(p.data)
            AND json_valid(m.data)
            AND json_extract(p.data, '$.type') = 'step-finish'
            AND json_type(p.data, '$.cost') IN ('integer', 'real')
            AND json_extract(m.data, '$.providerID') = 'opencode-go'
            AND json_extract(m.data, '$.role') = 'assistant'
            AND NOT EXISTS (
              SELECT 1
              FROM message_costs
              WHERE message_costs.messageID = p.message_id
            )"
    } else {
        "SELECT
            CAST(COALESCE(json_extract(data, '$.time.created'), time_created) AS INTEGER) AS createdMs,
            CAST(json_extract(data, '$.cost') AS REAL) AS cost
         FROM message
         WHERE json_valid(data)
           AND json_extract(data, '$.providerID') = 'opencode-go'
           AND json_extract(data, '$.role') = 'assistant'
           AND json_type(data, '$.cost') IN ('integer', 'real')"
    };
    let mut stmt = conn.prepare(sql).ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok(CostRow {
                created_ms: row.get::<_, Option<i64>>(0)?.unwrap_or_default(),
                cost: row.get::<_, f64>(1)?,
            })
        })
        .ok()?;
    let mut costs = Vec::new();
    for row in rows {
        let Ok(item) = row else { continue };
        if item.created_ms > 0 && item.cost >= 0.0 && item.cost.is_finite() {
            costs.push(item);
        }
    }
    Some(costs)
}

fn sqlite_has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

fn opencode_go_local_line(label: &str, used_cost: f64, limit: f64, reset_at_ms: i64) -> Value {
    let used_percent = opencode_go_percent(used_cost, limit);
    let remaining = (100.0 - used_percent).clamp(0.0, 100.0);
    json!({
        "type": "progress",
        "label": label,
        "used": remaining,
        "limit": 100.0,
        "format": { "kind": "percent", "mode": "remaining" },
        "subtitle": format!("${:.2} / ${:.0} local history", used_cost, limit),
        "resetsAt": millis_to_iso(reset_at_ms)
    })
}

fn opencode_go_percent(used: f64, limit: f64) -> f64 {
    if !used.is_finite() || limit <= 0.0 {
        return 0.0;
    }
    ((used / limit * 100.0) * 10.0).round() / 10.0
}

fn sum_opencode_go_costs(rows: &[CostRow], start_ms: i64, end_ms: i64) -> f64 {
    rows.iter()
        .filter(|row| row.created_ms >= start_ms && row.created_ms < end_ms)
        .map(|row| row.cost)
        .sum()
}

fn start_of_utc_week_ms(now_ms: i64) -> i64 {
    let secs = now_ms / 1000;
    let dt = chrono::DateTime::from_timestamp(secs, 0).unwrap_or_else(chrono::Utc::now);
    let mut days_to_monday = dt.weekday().num_days_from_monday() as i64;
    if days_to_monday < 0 {
        days_to_monday = 0;
    }
    let date = dt.date_naive() - chrono::Duration::days(days_to_monday);
    match chrono::Utc.with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0) {
        chrono::LocalResult::Single(t) => t.timestamp_millis(),
        _ => now_ms,
    }
}

fn reset_seconds_from_iso(value: &str) -> Option<i64> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value).ok()?;
    Some(((parsed.timestamp_millis() - chrono::Utc::now().timestamp_millis()) / 1000).max(0))
}

fn parse_opencode_go_workspace_id(text: &str) -> Option<String> {
    text.match_indices("wrk_").find_map(|(idx, _)| {
        let id = text[idx..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<String>();
        (id.len() > 4).then_some(id)
    })
}

fn normalize_cookie_header(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("Cookie:") {
        let cookie = rest.trim();
        if !cookie.is_empty() {
            return Some(cookie.to_string());
        }
    }
    Some(trimmed.to_string())
}

fn read_opencode_go_cookie_header() -> Option<String> {
    read_env_value(&[
        "OPENCODEGO_COOKIE",
        "OPENCODE_GO_COOKIE",
        "OPENCODEGO_COOKIE_HEADER",
        "OPENCODE_GO_COOKIE_HEADER",
    ])
    .or_else(read_config_opencodego_cookie)
    .or_else(read_codexbar_legacy_opencodego_cookie)
    .or_else(read_browser_opencodego_cookie)
    .and_then(|cookie| normalize_cookie_header(&cookie))
}

fn read_opencode_go_workspace_override() -> Option<String> {
    read_env_value(&["OPENCODEGO_WORKSPACE_ID", "OPENCODE_GO_WORKSPACE_ID"])
        .or_else(read_config_opencodego_workspace_id)
        .and_then(|raw| {
            parse_opencode_go_workspace_id(&raw).or_else(|| {
                let trimmed = raw.trim().to_string();
                (trimmed.starts_with("wrk_") && trimmed.len() > 4).then_some(trimmed)
            })
        })
}

fn read_config_opencodego_cookie() -> Option<String> {
    let defaults = read_json_file("defaults.json");
    let opencode = defaults.get("providers")?.get("opencode")?;
    opencode
        .get("opencodeGoCookieHeader")
        .or_else(|| opencode.get("opencodegoCookieHeader"))
        .or_else(|| opencode.get("cookieHeader"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn read_codexbar_legacy_opencodego_cookie() -> Option<String> {
    let path = dirs::home_dir()?
        .join("Library")
        .join("Application Support")
        .join("CodexBar")
        .join("opencodego-cookie.json");
    let data = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&data).ok()?;
    let cookie = value
        .get("cookieHeader")
        .and_then(Value::as_str)
        .or_else(|| value.get("cookie_header").and_then(Value::as_str))?;
    eprintln!("[opencode] cookie from CodexBar legacy cache");
    Some(cookie.to_string())
}

fn read_browser_opencodego_cookie() -> Option<String> {
    read_chromium_opencodego_cookie()
}

fn read_chromium_opencodego_cookie() -> Option<String> {
    let home = dirs::home_dir()?;
    let chrome_root = home
        .join("Library")
        .join("Application Support")
        .join("Google")
        .join("Chrome");
    let mut candidates = Vec::new();
    collect_chromium_cookie_db_paths(&chrome_root, &mut candidates);

    for path in candidates {
        if let Some(cookie) = read_chromium_opencodego_cookie_from_db(&path) {
            return Some(cookie);
        }
    }
    None
}

fn collect_chromium_cookie_db_paths(root: &Path, out: &mut Vec<PathBuf>) {
    if !root.exists() {
        return;
    }
    if root.join("Cookies").exists() {
        out.push(root.join("Cookies"));
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path().join("Cookies");
        if path.exists() {
            out.push(path);
        }
    }
}

fn read_chromium_opencodego_cookie_from_db(db_path: &Path) -> Option<String> {
    let temp_path = std::env::temp_dir().join(format!(
        "thatisok-opencode-cookies-{}-{}.sqlite",
        std::process::id(),
        now_millis()
    ));
    fs::copy(db_path, &temp_path).ok()?;
    let result = read_chromium_opencodego_cookie_from_copied_db(&temp_path, db_path);
    let _ = fs::remove_file(temp_path);
    result
}

fn read_chromium_opencodego_cookie_from_copied_db(
    temp_path: &Path,
    source_path: &Path,
) -> Option<String> {
    let conn = Connection::open(temp_path).ok()?;
    let mut stmt = conn
        .prepare(
            "SELECT name, value, length(encrypted_value)
             FROM cookies
             WHERE host_key LIKE '%opencode.ai%'
               AND name IN ('auth', '__Host-auth')",
        )
        .ok()?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        })
        .ok()?;

    let mut cookies = Vec::new();
    let mut encrypted_count = 0;
    for row in rows.flatten() {
        let (name, value, encrypted_len) = row;
        if !value.is_empty() {
            cookies.push(format!("{name}={value}"));
        } else if encrypted_len > 0 {
            encrypted_count += 1;
        }
    }

    if !cookies.is_empty() {
        eprintln!(
            "[opencode] cookie from Chromium profile {} names={}",
            source_path.display(),
            cookies.len()
        );
        return Some(cookies.join("; "));
    }
    if encrypted_count > 0 {
        eprintln!(
            "[opencode] Chromium profile {} has encrypted OpenCode cookie(s); decryption not enabled",
            source_path.display()
        );
    }
    None
}

fn read_config_opencodego_workspace_id() -> Option<String> {
    let defaults = read_json_file("defaults.json");
    let opencode = defaults.get("providers")?.get("opencode")?;
    opencode
        .get("opencodeGoWorkspaceId")
        .or_else(|| opencode.get("opencodegoWorkspaceId"))
        .or_else(|| opencode.get("workspaceId"))
        .or_else(|| opencode.get("workspaceID"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn millis_to_iso(ms: i64) -> String {
    let secs = ms / 1000;
    let millis = (ms % 1000).max(0) as u32;
    chrono::DateTime::from_timestamp(secs, millis * 1_000_000)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339()
}

const OPENCODE_GO_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const OPENCODE_GO_WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const OPENCODE_SUBSCRIPTION_SERVER_ID: &str =
    "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";
const OPENCODE_GO_BILLING_SERVER_ID: &str =
    "c83b78a614689c38ebee981f9b39a8b377716db85c1fd7dbab604adc02d3313d";

fn read_opencode_token_usage(db_path: &Path) -> Option<LocalTokenUsage> {
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open(db_path).ok()?;
    let today_start_ms = chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)?
        .and_utc()
        .timestamp_millis();

    let mut usage = LocalTokenUsage::default();
    if let Ok(mut stmt) = conn.prepare(
        "SELECT
            COALESCE(SUM(tokens_input), 0),
            COALESCE(SUM(tokens_output), 0),
            COALESCE(SUM(tokens_reasoning), 0),
            COALESCE(SUM(tokens_cache_read), 0),
            COALESCE(SUM(tokens_cache_write), 0)
         FROM session
         WHERE time_updated >= ?1",
    ) {
        let row = stmt.query_row([today_start_ms], |row| {
            Ok(LocalTokenUsage {
                input: row.get::<_, i64>(0)?.max(0) as u64,
                output: row.get::<_, i64>(1)?.max(0) as u64,
                reasoning: row.get::<_, i64>(2)?.max(0) as u64,
                cache_read: row.get::<_, i64>(3)?.max(0) as u64,
                cache_write: row.get::<_, i64>(4)?.max(0) as u64,
            })
        });
        if let Ok(row_usage) = row {
            usage = row_usage;
        }
    }

    if usage.input + usage.output + usage.reasoning + usage.cache_read + usage.cache_write > 0 {
        return Some(usage);
    }

    let mut stmt = conn
        .prepare(
            "SELECT data FROM message
             WHERE json_valid(data)
               AND time_created >= ?1
               AND json_type(data, '$.tokens') = 'object'",
        )
        .ok()?;
    let rows = stmt
        .query_map([today_start_ms], |row| row.get::<_, String>(0))
        .ok()?;
    for row in rows {
        let Ok(data_str) = row else { continue };
        let Ok(data) = serde_json::from_str::<Value>(&data_str) else {
            continue;
        };
        let Some(tokens) = data.get("tokens") else {
            continue;
        };
        usage.input += tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
        usage.output += tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
        usage.reasoning += tokens.get("reasoning").and_then(Value::as_u64).unwrap_or(0);
        if let Some(cache) = tokens.get("cache") {
            usage.cache_read += cache.get("read").and_then(Value::as_u64).unwrap_or(0);
            usage.cache_write += cache.get("write").and_then(Value::as_u64).unwrap_or(0);
        }
    }
    Some(usage)
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
        .and_then(Value::as_str)
        .unwrap_or("permission")
        .to_string();
    let command = string_field(&payload, &["command", "cmd"]);
    let file_path = string_field(
        &payload,
        &["file_path", "filePath", "path", "notebook_path"],
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
    let source_label = if source == "codex" {
        "Codex"
    } else if source == "claude" {
        "Claude"
    } else {
        "Agent"
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
            _ => {
                if !cwd.is_empty() {
                    return std::process::Command::new("open")
                        .arg(cwd)
                        .status()
                        .map(|status| status.success())
                        .map_err(|e| e.to_string());
                } else {
                    return Ok(false);
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
        "Agent Gate hooks removed. Agent integrations are disabled until hooks are installed again.",
    );
    Ok(())
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
            settings_set_sync_interval,
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
                    MenuItemBuilder::with_id("update", format!("Agent Gate v{version}"))
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

                    if let Some(menu) = menu {
                        let _tray = TrayIconBuilder::new()
                            .icon(icon)
                            .menu(&menu)
                            .on_menu_event(move |app_handle_inner, event| {
                                match event.id().as_ref() {
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
                                                "Agent Gate hooks installed.",
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
