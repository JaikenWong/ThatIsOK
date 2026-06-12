use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

const PILL_WIDTH: u32 = 248;
const PILL_HEIGHT: u32 = 56;
const EXPANDED_WIDTH: u32 = 356;
const DEFAULT_EXPANDED_HEIGHT: u32 = 404;
const WINDOW_MARGIN: i32 = 12;
const MINIMAX_MODEL_CALLS_PER_PROMPT: f64 = 15.0;
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
}

#[derive(Clone)]
struct IslandState {
    mode: String,
    expanded_height: u32,
    drag_start_bounds: Option<WindowBounds>,
    drag_start_mouse: Option<MousePoint>,
}

impl Default for IslandState {
    fn default() -> Self {
        Self {
            mode: "pill".to_string(),
            expanded_height: DEFAULT_EXPANDED_HEIGHT,
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

pub fn run_hook_bridge_from_args() -> bool {
    let args = env::args().collect::<Vec<_>>();
    if !args.iter().any(|arg| arg == "--hook-source") {
        return false;
    }

    let source = get_cli_arg(&args, "--hook-source").unwrap_or_else(|| "unknown".to_string());
    let event_name = get_cli_arg(&args, "--hook-event").unwrap_or_else(|| "unknown".to_string());
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);

    if let Err(error) = run_hook_bridge(&source, &event_name, &input) {
        eprintln!("ThatIsOk hook bridge failed: {error}");
        process::exit(0);
    }
    true
}

fn get_cli_arg(args: &[String], name: &str) -> Option<String> {
    args.windows(2)
        .find(|items| items.first().is_some_and(|item| item == name))
        .and_then(|items| items.get(1))
        .cloned()
}

fn run_hook_bridge(source: &str, event_name: &str, input: &str) -> Result<(), String> {
    let payload = json!({
        "event": "hook-event",
        "data": {
            "source": source,
            "event": event_name,
            "raw": input,
            "payload": serde_json::from_str::<Value>(input).ok()
        }
    });
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", 45873)).map_err(|err| err.to_string())?;
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .map_err(|err| err.to_string())?;
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|err| err.to_string())?;
    let response = serde_json::from_str::<Value>(&line).map_err(|err| err.to_string())?;
    if event_name.eq_ignore_ascii_case("PermissionRequest") {
        write_permission_output(source, &response)?;
    }
    Ok(())
}

fn write_permission_output(_source: &str, response: &Value) -> Result<(), String> {
    if response.get("requiresDecision").and_then(Value::as_bool) != Some(true) {
        return Ok(());
    }
    let approved = response
        .get("approved")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut decision = serde_json::Map::new();
    decision.insert(
        "behavior".to_string(),
        Value::String(if approved { "allow" } else { "deny" }.to_string()),
    );
    if !approved {
        decision.insert(
            "message".to_string(),
            Value::String("Denied from ThatIsOk".to_string()),
        );
    }
    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": Value::Object(decision)
        }
    });
    println!(
        "{}",
        serde_json::to_string(&output).map_err(|err| err.to_string())?
    );
    Ok(())
}

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

fn read_env_value(names: &[&str]) -> Option<String> {
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

fn inject_agent_hooks() {
    let exe_path = match env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to resolve current exe for hooks: {error}");
            return;
        }
    };
    inject_codex_hooks(&exe_path);
    inject_claude_hooks(&exe_path);
}

fn inject_codex_hooks(exe_path: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let hooks_path = home.join(".codex").join("hooks.json");
    let mut config = fs::read_to_string(&hooks_path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or_else(|| json!({ "hooks": {} }));
    if !config.get("hooks").is_some_and(Value::is_object) {
        config["hooks"] = json!({});
    }
    for event_name in HOOK_EVENTS {
        let existing = config["hooks"]
            .get(*event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let filtered = existing
            .into_iter()
            .filter(|entry| !is_managed_hook_value(entry))
            .collect::<Vec<_>>();
        let mut next_entries = filtered;
        next_entries.push(json!({
            "hooks": [{
                "type": "command",
                "command": build_tauri_hook_command(exe_path, "codex", event_name),
                "timeout": if *event_name == "PermissionRequest" { 86400 } else { 10 }
            }],
            "_managedBy": MANAGED_KEY
        }));
        config["hooks"][*event_name] = Value::Array(next_entries);
    }
    if let Some(parent) = hooks_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string_pretty(&config) {
        let _ = fs::write(hooks_path, format!("{content}\n"));
    }
}

fn inject_claude_hooks(exe_path: &Path) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let settings_path = home.join(".claude").join("settings.json");
    let Ok(content) = fs::read_to_string(&settings_path) else {
        return;
    };
    let Ok(mut config) = serde_json::from_str::<Value>(&content) else {
        return;
    };
    if !config.get("hooks").is_some_and(Value::is_object) {
        config["hooks"] = json!({});
    }
    for event_name in HOOK_EVENTS {
        let existing = config["hooks"]
            .get(*event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let filtered = existing
            .into_iter()
            .filter(|entry| !is_managed_hook_value(entry))
            .collect::<Vec<_>>();
        let mut next_entries = filtered;
        next_entries.push(json!({
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": build_tauri_hook_command(exe_path, "claude", event_name),
                "timeout": if *event_name == "PermissionRequest" { 86400 } else { 10 }
            }],
            "_managedBy": MANAGED_KEY
        }));
        config["hooks"][*event_name] = Value::Array(next_entries);
    }
    if let Ok(content) = serde_json::to_string_pretty(&config) {
        let _ = fs::write(settings_path, format!("{content}\n"));
    }
}

fn build_tauri_hook_command(exe_path: &Path, source: &str, event_name: &str) -> String {
    let escaped = exe_path.display().to_string().replace('"', "\\\"");
    if cfg!(target_os = "windows") {
        format!("\"{escaped}\" --hook-source {source} --hook-event {event_name}")
    } else {
        format!("\"{escaped}\" --hook-source {source} --hook-event {event_name}")
    }
}

fn is_managed_hook_value(value: &Value) -> bool {
    if value.get("_managedBy").and_then(Value::as_str) == Some(MANAGED_KEY) {
        return true;
    }
    if value
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_managed_command)
    {
        return true;
    }
    value
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(is_managed_hook_value))
}

fn is_managed_command(command: &str) -> bool {
    command.contains("ThatIsOk")
        || command.contains("hook-bridge.js")
        || command.contains("--hook-source")
}

fn provider_visibility() -> HashMap<String, ProviderVisibility> {
    let providers = read_json_file("providers.json");
    let defaults = read_json_file("defaults.json");
    let accounts = providers
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let default_providers = defaults.get("providers").and_then(Value::as_object);

    let mut result = HashMap::new();
    for account in accounts {
        let Some(provider) = account.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let setting = default_providers.and_then(|items| items.get(provider));
        let visible = setting
            .and_then(|item| item.get("visible"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let label = setting
            .and_then(|item| item.get("label"))
            .and_then(Value::as_str)
            .or_else(|| account.get("label").and_then(Value::as_str))
            .unwrap_or(provider)
            .to_string();
        result.insert(provider.to_string(), ProviderVisibility { visible, label });
    }

    result
}

fn compute_overview(accounts: &[Value]) -> Value {
    let mut total_balance = 0.0;
    let mut tracked_used = 0.0;
    let mut tracked_budget = 0.0;
    let mut has_any_balance = false;

    for account in accounts {
        let provider = account
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("");

        if let Some(balance) = account.get("balanceUsd").and_then(Value::as_f64) {
            total_balance += balance;
            has_any_balance = true;
        }

        if provider == "deepseek" {
            if let Some(usage) = account.get("usage") {
                let currency = usage
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("CNY");
                if currency == "CNY" {
                    if let Some(balance) = usage.get("totalBalance").and_then(Value::as_f64) {
                        total_balance += balance / 7.2;
                        has_any_balance = true;
                    }
                }
            }
        }

        if let Some(used) = account.get("creditUsedUsd").and_then(Value::as_f64) {
            tracked_used += used;
        }
        if let Some(total) = account.get("creditTotalUsd").and_then(Value::as_f64) {
            tracked_budget += total;
        }
    }

    let quota_percent = if tracked_budget > 0.0 {
        Some(((tracked_budget - tracked_used) / tracked_budget * 100.0).clamp(0.0, 100.0))
    } else {
        None
    };

    let runway_days: Option<f64> = None;

    let total_balance_val = if has_any_balance { json!(total_balance) } else { Value::Null };
    let tracked_budget_val = if tracked_budget > 0.0 { json!(tracked_budget) } else { Value::Null };
    let tracked_used_val = if tracked_used > 0.0 { json!(tracked_used) } else { Value::Null };

    json!({
        "totalBalanceUsd": total_balance_val,
        "trackedBudgetUsd": tracked_budget_val,
        "trackedUsedUsd": tracked_used_val,
        "quotaPercent": quota_percent,
        "todayCostUsd": 0,
        "monthCostUsd": 0,
        "runwayDays": runway_days,
        "runwayDaysLabel": "--"
    })
}

fn get_dashboard_data(state: &AppState) -> Value {
    let stored_accounts = state
        .usage
        .lock()
        .map(|usage| usage.balances.clone())
        .unwrap_or_default();
    let accounts = if stored_accounts.is_empty() {
        build_config_accounts()
    } else {
        stored_accounts
    };
    let overview = compute_overview(&accounts);
    let sessions: Vec<Value> = state
        .sessions
        .lock()
        .map(|s| {
            let mut entries: Vec<&SessionInfo> = s.values().collect();
            entries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            entries.truncate(5);
            entries
                .iter()
                .map(|info| {
                    json!({
                        "source": info.source,
                        "status": info.status,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    json!({
        "overview": overview,
        "accounts": accounts,
        "dailySeries": [],
        "recentEvents": [],
        "sessions": sessions
    })
}

async fn sync_provider_accounts() -> Vec<Value> {
    let providers = read_json_file("providers.json");
    let defaults = read_json_file("defaults.json");
    let accounts = providers
        .get("accounts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let default_providers = defaults.get("providers").and_then(Value::as_object);
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(_) => return build_config_accounts(),
    };

    let mut snapshots = Vec::new();
    for account in accounts {
        let Some(provider) = account.get("provider").and_then(Value::as_str) else {
            continue;
        };
        let account_id = account
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(provider);
        let setting = default_providers.and_then(|items| items.get(provider));
        let label = setting
            .and_then(|item| item.get("label"))
            .and_then(Value::as_str)
            .or_else(|| account.get("label").and_then(Value::as_str))
            .unwrap_or(provider);

        let snapshot = match provider {
            "deepseek" => fetch_deepseek_snapshot(&client, account_id, label).await,
            "minimax" => fetch_minimax_snapshot(&client, account_id, label).await,
            "claude" => fetch_claude_snapshot(account_id, label),
            "codex" => fetch_codex_snapshot(&client, account_id, label).await,
            "cursor" => fetch_cursor_snapshot(&client, account_id, label).await,
            "gemini" => fetch_gemini_snapshot(account_id, label),
            _ => None,
        }
        .unwrap_or_else(|| build_config_account(&account, setting));
        snapshots.push(snapshot);
    }
    snapshots
}

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

// ===== CLAUDE PROVIDER =====

fn fetch_claude_snapshot(account_id: &str, label: &str) -> Option<Value> {
    let home = dirs::home_dir()?;
    let claude_dir = home.join(".claude");
    if !claude_dir.exists() {
        return None;
    }

    let today_stats = read_claude_today_stats(&claude_dir);
    let plan_type = read_claude_plan_type(&claude_dir);
    let token_usage = read_claude_token_usage(&claude_dir);

    let mut lines = Vec::new();
    lines.push(json!({
        "type": "text",
        "label": "Today",
        "value": format!("{} msg", format_compact(today_stats.message_count as f64)),
        "subtitle": format!("{} tools · {} sess",
            format_compact(today_stats.tool_call_count as f64),
            format_compact(today_stats.session_count as f64))
    }));

    if token_usage.total_input > 0 || token_usage.total_output > 0 {
        let total = token_usage.total_input + token_usage.total_output;
        lines.push(json!({
            "type": "text",
            "label": "Tokens",
            "value": format_compact(total as f64),
            "subtitle": format!("in {} · out {}",
                format_compact(token_usage.total_input as f64),
                format_compact(token_usage.total_output as f64))
        }));
    }

    let plan_str = if plan_type.is_empty() { "unknown" } else { &plan_type };

    Some(json!({
        "accountId": account_id,
        "provider": "claude",
        "label": label,
        "balanceUsd": null,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "status": "live-local",
        "capturedAt": now_millis(),
        "source": "local_auth",
        "plan": format!("{plan_str} plan"),
        "lines": lines,
        "meta": {
            "planType": plan_str,
            "todayMessages": today_stats.message_count,
            "todaySessions": today_stats.session_count,
            "todayTools": today_stats.tool_call_count,
            "tokensInput": token_usage.total_input,
            "tokensOutput": token_usage.total_output,
        },
        "message": format!("plan {plan_str} · local usage stats")
    }))
}

struct ClaudeTodayStats {
    message_count: u32,
    session_count: u32,
    tool_call_count: u32,
}

struct TokenUsage {
    total_input: u64,
    total_output: u64,
}

fn read_claude_today_stats(claude_dir: &Path) -> ClaudeTodayStats {
    let stats_path = claude_dir.join("stats-cache.json");
    let Ok(content) = fs::read_to_string(&stats_path) else {
        return ClaudeTodayStats { message_count: 0, session_count: 0, tool_call_count: 0 };
    };
    let Ok(stats) = serde_json::from_str::<Value>(&content) else {
        return ClaudeTodayStats { message_count: 0, session_count: 0, tool_call_count: 0 };
    };

    let daily = stats
        .get("dailyActivity")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let today_key = today_date_str();
    let today = daily
        .iter()
        .find(|d| d.get("date").and_then(Value::as_str) == Some(&today_key));

    ClaudeTodayStats {
        message_count: today
            .and_then(|d| d.get("messageCount").and_then(Value::as_u64))
            .unwrap_or(0) as u32,
        session_count: today
            .and_then(|d| d.get("sessionCount").and_then(Value::as_u64))
            .unwrap_or(0) as u32,
        tool_call_count: today
            .and_then(|d| d.get("toolCallCount").and_then(Value::as_u64))
            .unwrap_or(0) as u32,
    }
}

fn read_claude_plan_type(claude_dir: &Path) -> String {
    let telemetry_dir = claude_dir.join("telemetry");
    if !telemetry_dir.exists() {
        let creds_path = claude_dir.join(".credentials.json");
        if creds_path.exists() {
            return "Subscription".to_string();
        }
        return String::new();
    }

    let Ok(entries) = fs::read_dir(&telemetry_dir) else {
        return String::new();
    };

    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.path())
        .collect();

    files.sort_by(|a, b| {
        let am = fs::metadata(a).ok().and_then(|m| m.modified().ok());
        let bm = fs::metadata(b).ok().and_then(|m| m.modified().ok());
        bm.cmp(&am)
    });

    for file in files.iter().take(8) {
        let Ok(content) = fs::read_to_string(file) else {
            continue;
        };
        for line in content.lines().rev() {
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let encoded = entry
                .get("event_data")
                .and_then(|d| d.get("additional_metadata"))
                .and_then(Value::as_str);

            if let Some(encoded) = encoded {
                if let Ok(decoded) =
                    base64::engine::general_purpose::STANDARD.decode(encoded.as_bytes())
                {
                    if let Ok(meta) = serde_json::from_slice::<Value>(&decoded) {
                        if let Some(pt) = meta
                            .get("subscription_type")
                            .or_else(|| meta.get("billingType"))
                            .and_then(Value::as_str)
                        {
                            return pt.to_string();
                        }
                    }
                }
            }
        }
    }

    String::new()
}

fn read_claude_token_usage(claude_dir: &Path) -> TokenUsage {
    let projects_dir = claude_dir.join("projects");
    if !projects_dir.exists() {
        return TokenUsage { total_input: 0, total_output: 0 };
    }

    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return TokenUsage { total_input: 0, total_output: 0 };
    };
    let mut total_input = 0u64;
    let mut total_output = 0u64;

    for entry in entries.flatten().take(10) {
        let project_path = entry.path();
        if !project_path.is_dir() {
            continue;
        }
        let Ok(jsonl_files) = fs::read_dir(&project_path) else {
            continue;
        };
        for f in jsonl_files.flatten().take(5) {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&fp) else {
                continue;
            };
            for line in content.lines() {
                let Ok(entry) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let usage = entry
                    .get("usage")
                    .or_else(|| entry.get("token_usage"))
                    .or_else(|| entry.get("tokens"));
                if let Some(usage) = usage {
                    total_input += usage
                        .get("input_tokens")
                        .or_else(|| usage.get("input"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    total_output += usage
                        .get("output_tokens")
                        .or_else(|| usage.get("output"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                }
            }
        }
    }

    TokenUsage { total_input, total_output }
}

// ===== CODEX PROVIDER =====

async fn fetch_codex_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let home = dirs::home_dir()?;
    let auth_path = home.join(".codex").join("auth.json");
    if !auth_path.exists() {
        return None;
    }

    let auth_content = fs::read_to_string(&auth_path).ok()?;
    let auth: Value = serde_json::from_str(&auth_content).ok()?;

    let access_token = auth
        .get("tokens")
        .and_then(|t| t.get("access_token"))
        .and_then(Value::as_str);
    let account_id_codex = auth
        .get("tokens")
        .and_then(|t| t.get("account_id"))
        .and_then(Value::as_str);
    let id_token = auth
        .get("tokens")
        .and_then(|t| t.get("id_token"))
        .and_then(Value::as_str);
    let last_refresh = auth.get("last_refresh").and_then(Value::as_str);
    let is_stale = last_refresh.is_some_and(|r| {
        r.parse::<chrono::DateTime<chrono::Utc>>()
            .is_ok_and(|d| {
                (chrono::Utc::now() - d).num_hours() > 8 * 24
            })
    }) || last_refresh.is_none();

    // Decode JWT for plan info
    let (plan_type, _subscription_until) = if let Some(token) = id_token {
        decode_codex_jwt(token)
    } else {
        (String::new(), None)
    };

    let display_plan = format_codex_plan(&plan_type);

    let mut usage_data: Option<Value> = None;
    let mut usage_error: Option<String> = None;
    let effective_token = access_token.map(String::from);

    if let Some(token) = &effective_token {
        match fetch_codex_usage_api(client, token, account_id_codex).await {
            Ok(data) => usage_data = Some(data),
            Err(err) => {
                let is_expired = err.contains("401") || err.contains("expired");
                if is_expired {
                    if let Some(refreshed) = refresh_codex_token(client, &auth, &auth_path).await {
                        match fetch_codex_usage_api(client, &refreshed, account_id_codex).await {
                            Ok(data) => usage_data = Some(data),
                            Err(e) => usage_error = Some(e),
                        }
                    }
                } else {
                    usage_error = Some(err);
                }
            }
        }
    }

    if usage_data.is_none() {
        usage_data = read_codex_session_usage(&home);
    }

    let effective_stale = is_stale && usage_data.is_none();

    // Codex doesn't generate progress lines — usage data is rate-limit info.
    // The Electron version only stores raw usage; no progress bars.

    let message = usage_error.unwrap_or_else(|| format!("plan {display_plan}"));
    let status = if effective_stale { "stale" } else { "live-local" };
    let plan = if effective_stale { "Codex auth stale" } else { &display_plan };
    let source = usage_data
        .as_ref()
        .and_then(|d| d.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("local_auth");

    let snapshot = json!({
        "accountId": account_id,
        "provider": "codex",
        "label": label,
        "balanceUsd": null,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "status": status,
        "capturedAt": now_millis(),
        "source": source,
        "plan": plan,
        "usage": usage_data,
        "lines": json!([]),
        "meta": {
            "planType": plan_type,
            "displayPlan": display_plan,
            "isStale": effective_stale,
            "lastRefresh": last_refresh,
        },
        "message": message
    });
    Some(snapshot)
}

async fn fetch_codex_usage_api(
    client: &reqwest::Client,
    token: &str,
    account_id: Option<&str>,
) -> Result<Value, String> {
    let mut req = client
        .get("https://chatgpt.com/backend-api/wham/usage")
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("User-Agent", "ThatIsOk");

    if let Some(id) = account_id {
        req = req.header("ChatGPT-Account-Id", id);
    }

    let response = req.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let mut data: Value = response.json().await.map_err(|e| e.to_string())?;
    if let Some(obj) = data.as_object_mut() {
        obj.insert("source".to_string(), Value::String("provider_api".to_string()));
    }
    Ok(data)
}

fn read_codex_session_usage(home: &Path) -> Option<Value> {
    let sessions_dir = home.join(".codex").join("sessions");
    if !sessions_dir.exists() {
        return None;
    }
    let latest = find_latest_rate_limit_event(&sessions_dir)?;
    let limits = latest.get("rateLimits").or_else(|| latest.get("rate_limits"))?;

    let primary = limits
        .get("primary")
        .and_then(|p| {
    Some::<Value>(json!({
                "used_percent": p.get("used_percent").or_else(|| p.get("usedPercent")).and_then(Value::as_f64).unwrap_or(0.0),
                "resets_at": p.get("resets_at").or_else(|| p.get("reset_at")).or_else(|| p.get("resetAt")).and_then(Value::as_str),
                "window_minutes": p.get("window_minutes").or_else(|| p.get("windowMinutes")).and_then(Value::as_f64),
            }))
        });

    Some(json!({
        "plan_type": limits.get("plan_type").or_else(|| limits.get("planType")).and_then(Value::as_str),
        "source": "local_sessions",
        "captured_at": latest.get("timestamp").and_then(Value::as_str),
        "rate_limit": {
            "primary_window": primary,
        }
    }))
}

fn find_latest_rate_limit_event(dir: &Path) -> Option<Value> {
    let mut latest: Option<Value> = None;
    let mut latest_ms: i64 = 0;

    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(event) = find_latest_rate_limit_event(&path) {
                let ms = event
                    .get("timeMs")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if ms > latest_ms {
                    latest_ms = ms;
                    latest = Some(event);
                }
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            for line in content.lines() {
                if !line.contains("rate_limits") {
                    continue;
                }
                let Ok(parsed) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let rate_limits = parsed
                    .get("payload")
                    .and_then(|p| p.get("rate_limits"));
                if rate_limits.is_none() {
                    continue;
                }
                let timestamp = parsed.get("timestamp").and_then(Value::as_str);
                let time_ms = timestamp
                    .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                    .map(|d| d.timestamp_millis())
                    .unwrap_or(0);
                if time_ms > latest_ms {
                    latest_ms = time_ms;
                    latest = Some(json!({
                        "timestamp": timestamp,
                        "timeMs": time_ms,
                        "rateLimits": rate_limits,
                    }));
                }
            }
        }
    }

    latest
}

async fn refresh_codex_token(
    client: &reqwest::Client,
    auth: &Value,
    auth_path: &Path,
) -> Option<String> {
    let refresh_token = auth
        .get("tokens")
        .and_then(|t| t.get("refresh_token"))
        .and_then(Value::as_str)?;

    let resp = client
        .post("https://auth.openai.com/oauth/token")
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", "app_EMoamEEZ73f0CkXaXp7hrann"),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let data: Value = resp.json().await.ok()?;
    let new_access = data.get("access_token").and_then(Value::as_str)?;
    let new_refresh = data.get("refresh_token").and_then(Value::as_str);

    if let Ok(content) = fs::read_to_string(auth_path) {
        if let Ok(mut auth_val) = serde_json::from_str::<Value>(&content) {
            if let Some(tokens) = auth_val.get_mut("tokens") {
                tokens["access_token"] = Value::String(new_access.to_string());
                if let Some(rt) = new_refresh {
                    tokens["refresh_token"] = Value::String(rt.to_string());
                }
                if let Some(idt) = data.get("id_token").and_then(Value::as_str) {
                    tokens["id_token"] = Value::String(idt.to_string());
                }
            }
            auth_val["last_refresh"] =
                Value::String(chrono::Utc::now().to_rfc3339());
            let _ = fs::write(auth_path, serde_json::to_string_pretty(&auth_val).ok()?);
        }
    }

    Some(new_access.to_string())
}

fn decode_codex_jwt(token: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return (String::new(), None);
    }
    let payload = parts[1].replace('-', "+").replace('_', "/");
    let normalized = format!("{}===", payload.trim_end_matches('='));
    let Ok(decoded) = base64::engine::general_purpose::STANDARD
        .decode(normalized.trim_end_matches('=').as_bytes())
    else {
        return (String::new(), None);
    };
    let Ok(data): Result<Value, _> = serde_json::from_slice(&decoded) else {
        return (String::new(), None);
    };
    let auth = data
        .get("https://api.openai.com/auth")
        .or_else(|| data.get("https://api.openai.com/auth"));
    let plan = auth
        .and_then(|a| a.get("chatgpt_plan_type"))
        .or_else(|| data.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let until = auth
        .and_then(|a| a.get("chatgpt_subscription_active_until"))
        .or_else(|| data.get("chatgpt_subscription_active_until"))
        .and_then(Value::as_str)
        .map(String::from);
    (plan, until)
}

fn format_codex_plan(raw: &str) -> String {
    match raw.to_lowercase().as_str() {
        "prolite" => "Pro 5x".to_string(),
        "pro" => "Pro 20x".to_string(),
        "plus" => "Plus".to_string(),
        "" | "unknown" => "ChatGPT login".to_string(),
        other => {
            let mut c = other.chars();
            match c.next() {
                None => "ChatGPT login".to_string(),
                Some(first) => format!("{}{}", first.to_uppercase(), c.collect::<String>()),
            }
        }
    }
}

// ===== CURSOR PROVIDER =====

async fn fetch_cursor_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let token = read_cursor_token()?;

    let usage = match fetch_cursor_api(client, &token, "/aiserver.v1.DashboardService/GetCurrentPeriodUsage").await {
        Ok(v) => v,
        Err(err) => {
            return Some(json!({
                "accountId": account_id,
                "provider": "cursor",
                "label": label,
                "status": "error",
                "message": format!("Cursor fetch failed: {err}"),
                "capturedAt": now_millis(),
                "source": "local_auth",
                "lines": []
            }))
        }
    };

    let plan = fetch_cursor_api(client, &token, "/aiserver.v1.DashboardService/GetPlanInfo")
        .await
        .ok();

    Some(build_cursor_snapshot(account_id, label, &usage, plan.as_ref()))
}

fn read_cursor_token() -> Option<String> {
    if let Some(token) = read_env_value(&["CURSOR_ACCESS_TOKEN", "CURSOR_API_TOKEN"]) {
        return Some(token);
    }

    if cfg!(target_os = "macos") {
        if let Some(token) = read_keychain_token() {
            return Some(token);
        }
    }

    read_cursor_db_token()
}

fn read_keychain_token() -> Option<String> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-s",
            "cursor-access-token",
            "-w",
        ])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let trimmed = token.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

fn read_cursor_db_token() -> Option<String> {
    let db_path = cursor_db_path()?;
    if !db_path.exists() {
        return None;
    }
    let output = Command::new("sqlite3")
        .arg(db_path.to_string_lossy().as_ref())
        .arg("SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let trimmed = token.trim().to_string();
    if trimmed.is_empty() { None } else { Some(trimmed) }
}

fn cursor_db_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    if cfg!(target_os = "macos") {
        Some(
            home.join("Library")
                .join("Application Support")
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
        )
    } else if cfg!(target_os = "windows") {
        Some(
            dirs::data_dir()
                .or_else(|| Some(home.join("AppData").join("Roaming")))?
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
        )
    } else {
        Some(
            dirs::config_dir()
                .unwrap_or_else(|| home.join(".config"))
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb"),
        )
    }
}

async fn fetch_cursor_api(
    client: &reqwest::Client,
    token: &str,
    path: &str,
) -> Result<Value, String> {
    let url = format!("https://api2.cursor.sh{path}");
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .header("Content-Type", "application/json")
        .header("Connect-Protocol-Version", "1")
        .body("{}")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    resp.json().await.map_err(|e| e.to_string())
}

fn build_cursor_snapshot(account_id: &str, label: &str, usage: &Value, plan: Option<&Value>) -> Value {
    let plan_usage = usage.get("planUsage").or_else(|| usage.get("plan_usage"));
    let spend_limit = usage.get("spendLimitUsage").or_else(|| usage.get("spend_limit_usage"));

    let limit_cents = plan_usage
        .and_then(|p| p.get("limit"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let remaining_cents = plan_usage
        .and_then(|p| p.get("remaining"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let total_spend = plan_usage
        .and_then(|p| p.get("totalSpend"))
        .and_then(Value::as_f64);
    let total_percent = plan_usage
        .and_then(|p| p.get("totalPercentUsed"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let auto_percent = plan_usage
        .and_then(|p| p.get("autoPercentUsed"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let api_percent = plan_usage
        .and_then(|p| p.get("apiPercentUsed"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);

    let limit_dollars = if limit_cents > 0.0 { Some(limit_cents / 100.0) } else { None };
    let remaining_dollars = if limit_cents > 0.0 { Some(remaining_cents / 100.0) } else { None };
    let used_cents = total_spend.unwrap_or_else(|| (limit_cents - remaining_cents).max(0.0));
    let used_dollars = if limit_cents > 0.0 { Some(used_cents / 100.0) } else { None };

    let plan_name = plan
        .and_then(|p| p.get("planInfo").or_else(|| p.get("plan_info")))
        .and_then(|i| i.get("planName").or_else(|| i.get("plan_name")))
        .and_then(Value::as_str)
        .unwrap_or("Cursor");

    let mut lines = Vec::new();
    if total_percent > 0.0 || (limit_dollars.is_some() && used_dollars.is_some()) {
        let remaining = (100.0 - total_percent).max(0.0);
        lines.push(json!({
            "type": "progress",
            "label": "Total",
            "used": remaining,
            "limit": 100.0,
            "format": { "kind": "percent", "mode": "remaining" },
            "subtitle": format!("{}% left", remaining.round())
        }));
    }
    if auto_percent > 0.0 {
        let remaining = (100.0 - auto_percent).max(0.0);
        lines.push(json!({
            "type": "progress",
            "label": "Auto",
            "used": remaining,
            "limit": 100.0,
            "format": { "kind": "percent", "mode": "remaining" },
            "subtitle": format!("{}% left", remaining.round())
        }));
    }
    if api_percent > 0.0 {
        let remaining = (100.0 - api_percent).max(0.0);
        lines.push(json!({
            "type": "progress",
            "label": "API",
            "used": remaining,
            "limit": 100.0,
            "format": { "kind": "percent", "mode": "remaining" },
            "subtitle": format!("{}% left", remaining.round())
        }));
    }
    if let (Some(limit), Some(used)) = (limit_dollars, used_dollars) {
        lines.push(json!({
            "type": "progress",
            "label": "Budget",
            "used": used,
            "limit": limit,
            "format": { "kind": "currency", "currency": "USD" },
            "subtitle": format!("${:.1} left", remaining_dollars.unwrap_or(0.0))
        }));
    }

    json!({
        "accountId": account_id,
        "provider": "cursor",
        "label": label,
        "balanceUsd": remaining_dollars,
        "creditTotalUsd": limit_dollars,
        "creditUsedUsd": used_dollars,
        "status": "live-local",
        "capturedAt": now_millis(),
        "source": "local_auth",
        "plan": plan_name,
        "usage": {
            "totalPercent": total_percent,
            "autoPercent": auto_percent,
            "apiPercent": api_percent,
            "spendLimit": if let Some(sl) = spend_limit { sl.clone() } else { Value::Null }
        },
        "lines": lines,
        "meta": {
            "planName": plan_name,
        }
    })
}

// ===== GEMINI PROVIDER =====

fn fetch_gemini_snapshot(account_id: &str, label: &str) -> Option<Value> {
    let home = dirs::home_dir()?;
    let gemini_dir = home.join(".gemini");
    if !gemini_dir.exists() {
        return None;
    }

    let creds_path = gemini_dir.join("oauth_creds.json");
    let (is_stale, has_creds) = if creds_path.exists() {
        let Ok(content) = fs::read_to_string(&creds_path) else {
            return None;
        };
        let Ok(creds) = serde_json::from_str::<Value>(&content) else {
            return None;
        };
        let expiry = creds.get("expiry_date").and_then(Value::as_f64);
        let stale = expiry.is_some_and(|e| (e as i64) < chrono::Utc::now().timestamp_millis())
            || expiry.is_none();
        (stale, true)
    } else {
        (false, false)
    };

    if !has_creds {
        return None;
    }

    let daily_limit = infer_gemini_daily_limit(&gemini_dir);
    let today_stats = read_gemini_today_stats(&gemini_dir);
    let used = today_stats.model_requests;
    let remaining = (daily_limit as i64 - used as i64).max(0) as u32;
    let remaining_pct = if daily_limit > 0 {
        (remaining as f64 / daily_limit as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };

    let plan_name = if daily_limit >= 1000 { "Gemini Code Assist" } else { "Gemini CLI" };

    let mut lines = Vec::new();
    if daily_limit > 0 {
        lines.push(json!({
            "type": "progress",
            "label": "Today",
            "used": remaining,
            "limit": daily_limit,
            "format": { "kind": "count", "mode": "remaining", "suffix": "requests" },
            "subtitle": format!("{} left · {} sessions", remaining, today_stats.session_count)
        }));
    }
    if today_stats.tokens_input > 0 || today_stats.tokens_output > 0 {
        let total = today_stats.tokens_input + today_stats.tokens_output;
        lines.push(json!({
            "type": "text",
            "label": "Tokens",
            "value": format_compact(total as f64),
            "subtitle": format!("in {} · out {}",
                format_compact(today_stats.tokens_input as f64),
                format_compact(today_stats.tokens_output as f64))
        }));
    }

    Some(json!({
        "accountId": account_id,
        "provider": "gemini",
        "label": label,
        "status": if is_stale { "stale" } else { "live-local" },
        "capturedAt": now_millis(),
        "source": "local_auth",
        "plan": plan_name,
        "usage": {
            "used": used,
            "total": daily_limit,
            "remaining": remaining,
            "remainingPercent": remaining_pct,
            "todayMessages": today_stats.message_count,
            "todaySessions": today_stats.session_count,
            "tokens": {
                "input": today_stats.tokens_input,
                "output": today_stats.tokens_output,
            }
        },
        "lines": lines,
        "meta": {
            "isStale": is_stale,
            "dailyLimit": daily_limit,
            "modelRequests": used,
        }
    }))
}

struct GeminiTodayStats {
    message_count: u32,
    session_count: u32,
    model_requests: u32,
    tokens_input: u64,
    tokens_output: u64,
}

fn infer_gemini_daily_limit(gemini_dir: &Path) -> u32 {
    let creds_path = gemini_dir.join("oauth_creds.json");
    if let Ok(content) = fs::read_to_string(&creds_path) {
        if let Ok(creds) = serde_json::from_str::<Value>(&content) {
            if creds.get("id_token").is_some() {
                return 1000;
            }
        }
    }
    if read_env_value(&["GEMINI_API_KEY"]).is_some() {
        return 250;
    }
    1000
}

fn read_gemini_today_stats(gemini_dir: &Path) -> GeminiTodayStats {
    let tmp_dir = gemini_dir.join("tmp");
    if !tmp_dir.exists() {
        return GeminiTodayStats {
            message_count: 0,
            session_count: 0,
            model_requests: 0,
            tokens_input: 0,
            tokens_output: 0,
        };
    }

    let mut stats = GeminiTodayStats {
        message_count: 0,
        session_count: 0,
        model_requests: 0,
        tokens_input: 0,
        tokens_output: 0,
    };

    let today = today_date_str();
    let Ok(projects) = fs::read_dir(&tmp_dir) else { return stats };

    for project in projects.flatten() {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }
        let chats_path = project_path.join("chats");
        if !chats_path.exists() {
            continue;
        }
        let Ok(files) = fs::read_dir(&chats_path) else { continue };
        for file in files.flatten() {
            let fp = file.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&fp) else { continue };
            let mut is_today = false;
            let mut msg_count: u32 = 0;
            let mut model_reqs: u32 = 0;

            for line in content.lines() {
                let Ok(entry) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                if let Some(ts) = entry
                    .get("timestamp")
                    .or_else(|| entry.get("ts"))
                    .and_then(Value::as_str)
                {
                    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
                        if dt.format("%Y-%m-%d").to_string() == today {
                            is_today = true;
                        }
                    }
                }
                if entry.get("type").and_then(Value::as_str) == Some("user") {
                    if let Some(content_arr) = entry.get("content").and_then(Value::as_array) {
                        let has_text = content_arr.iter().any(|c| {
                            c.get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|t| !t.starts_with("<function_response"))
                        });
                        if has_text {
                            msg_count += 1;
                        }
                    }
                }
                if entry.get("type").and_then(Value::as_str) == Some("gemini") {
                    model_reqs += 1;
                }
                if let Some(set) = entry.get("$set") {
                    if let Some(messages) = set.get("messages").and_then(Value::as_array) {
                        for msg in messages {
                            if msg.get("type").and_then(Value::as_str) == Some("gemini") {
                                model_reqs += 1;
                            }
                        }
                    }
                }
                if let Some(tokens) = entry.get("tokens") {
                    stats.tokens_input += tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
                    stats.tokens_output += tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
                }
            }

            if is_today {
                stats.session_count += 1;
                stats.message_count += msg_count;
                stats.model_requests += model_reqs;
            }
        }
    }

    stats
}

// ===== DEEPSEEK & MINIMAX (existing) =====

async fn fetch_deepseek_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let api_key = match read_env_value(&["DEEPSEEK_API_KEY", "DEEPSEEK_API_TOKEN"]) {
        Some(api_key) => api_key,
        None => return None,
    };
    let base_url = read_env_value(&["DEEPSEEK_BASE_URL"])
        .unwrap_or_else(|| "https://api.deepseek.com".to_string());
    let response = client
        .get(format!("{base_url}/user/balance"))
        .bearer_auth(api_key)
        .header("Accept", "application/json")
        .send()
        .await;
    let data = match response {
        Ok(response) => match response.json::<Value>().await {
            Ok(data) => data,
            Err(error) => {
                return Some(error_snapshot(
                    account_id,
                    "deepseek",
                    label,
                    format!("DeepSeek fetch failed: {error}"),
                ))
            }
        },
        Err(error) => {
            return Some(error_snapshot(
                account_id,
                "deepseek",
                label,
                format!("DeepSeek fetch failed: {error}"),
            ))
        }
    };
    let infos = data
        .get("balance_infos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let primary = infos
        .iter()
        .find(|item| item.get("currency").and_then(Value::as_str) == Some("CNY"))
        .or_else(|| infos.first());
    let currency = primary
        .and_then(|item| item.get("currency"))
        .and_then(Value::as_str)
        .unwrap_or("CNY");
    let total = primary
        .and_then(|item| item.get("total_balance"))
        .and_then(read_number_value);
    let granted = primary
        .and_then(|item| item.get("granted_balance"))
        .and_then(read_number_value);
    let topped_up = primary
        .and_then(|item| item.get("topped_up_balance"))
        .and_then(read_number_value);
    let mut lines = Vec::new();
    if let Some(total) = total {
        lines.push(json!({
            "type": "text",
            "label": "Balance",
            "value": format_money(total, currency),
            "subtitle": "available"
        }));
        lines.push(json!({
            "type": "progress",
            "used": 0,
            "limit": 1,
            "label": "Balance",
            "value": format_money(total, currency),
            "format": { "ringText": format_money(total, currency) }
        }));
    }
    if granted.is_some() || topped_up.is_some() {
        let parts = [
            granted.map(|value| format!("grant {}", format_money(value, currency))),
            topped_up.map(|value| format!("topup {}", format_money(value, currency))),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        lines.push(json!({
            "type": "text",
            "label": "Split",
            "value": parts.join(" · "),
            "subtitle": "DeepSeek API balance"
        }));
    }

    Some(json!({
        "accountId": account_id,
        "provider": "deepseek",
        "label": label,
        "balanceUsd": if currency == "USD" { json!(total) } else { Value::Null },
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "status": if data.get("is_available").and_then(Value::as_bool) == Some(false) { "warn" } else { "live" },
        "capturedAt": now_millis(),
        "source": "provider_api",
        "plan": currency,
        "usage": {
            "currency": currency,
            "totalBalance": total,
            "grantedBalance": granted,
            "toppedUpBalance": topped_up,
            "isAvailable": data.get("is_available").and_then(Value::as_bool).unwrap_or(true)
        },
        "lines": lines,
        "meta": {
            "balanceInfos": infos,
            "baseUrl": base_url
        }
    }))
}

async fn fetch_minimax_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    for region in minimax_regions() {
        let Some((api_key, base_url)) = minimax_key_for_region(&region) else {
            continue;
        };
        let response = client
            .get(format!("{base_url}/v1/token_plan/remains"))
            .bearer_auth(api_key)
            .header("Accept", "application/json")
            .send()
            .await;
        let data = match response {
            Ok(response) => {
                if !response.status().is_success() {
                    continue;
                }
                match response.json::<Value>().await {
                    Ok(data) => data,
                    Err(_) => continue,
                }
            }
            Err(_) => continue,
        };
        if let Some(snapshot) = build_minimax_snapshot(account_id, label, &region, &base_url, data)
        {
            return Some(snapshot);
        }
    }

    if read_env_value(&["MINIMAX_API_KEY", "MINIMAX_API_TOKEN", "MINIMAX_CN_API_KEY"]).is_some() {
        Some(error_snapshot(
            account_id,
            "minimax",
            label,
            "MiniMax fetch failed. Check API key or endpoint.".to_string(),
        ))
    } else {
        None
    }
}

fn minimax_regions() -> Vec<String> {
    if read_env_value(&["MINIMAX_CN_API_KEY"]).is_some() {
        vec!["CN".to_string(), "GLOBAL".to_string()]
    } else {
        vec!["GLOBAL".to_string(), "CN".to_string()]
    }
}

fn minimax_key_for_region(region: &str) -> Option<(String, String)> {
    let custom_base = read_env_value(&["MINIMAX_BASE_URL", "MINIMAX_API_HOST"]);
    let names = if region == "CN" {
        ["MINIMAX_CN_API_KEY", "MINIMAX_API_KEY", "MINIMAX_API_TOKEN"].as_slice()
    } else {
        ["MINIMAX_API_KEY", "MINIMAX_API_TOKEN"].as_slice()
    };
    let key = read_env_value(names)?;
    let base_url = custom_base.unwrap_or_else(|| {
        if region == "CN" {
            "https://api.minimaxi.com".to_string()
        } else {
            "https://www.minimax.io".to_string()
        }
    });
    Some((key, base_url))
}

fn build_minimax_snapshot(
    account_id: &str,
    label: &str,
    region: &str,
    base_url: &str,
    payload: Value,
) -> Option<Value> {
    let data = payload
        .get("data")
        .filter(|value| value.is_object())
        .unwrap_or(&payload);
    let remains = data
        .get("model_remains")
        .or_else(|| data.get("modelRemains"))
        .or_else(|| payload.get("model_remains"))
        .or_else(|| payload.get("modelRemains"))
        .and_then(Value::as_array)?;
    let chosen = pick_minimax_remain(remains, region)?;
    let total = read_number_field(
        chosen,
        &["current_interval_total_count", "currentIntervalTotalCount"],
    )
    .unwrap_or(0.0);
    let remaining_percent = read_number_field(
        chosen,
        &[
            "current_interval_remaining_percent",
            "currentIntervalRemainingPercent",
        ],
    );
    let display_multiplier = if region == "CN" {
        1.0 / MINIMAX_MODEL_CALLS_PER_PROMPT
    } else {
        1.0
    };
    let has_count = total > 0.0 && (total * display_multiplier).round() > 0.0;
    let (used, total_out, remaining, remaining_percent_out, is_percent_mode) = if !has_count {
        let percent = remaining_percent?;
        (100.0 - percent, 100.0, percent, Some(percent), true)
    } else {
        let usage_field = read_number_field(
            chosen,
            &["current_interval_usage_count", "currentIntervalUsageCount"],
        );
        let remaining_count = read_number_field(
            chosen,
            &[
                "current_interval_remaining_count",
                "currentIntervalRemainingCount",
                "current_interval_remains_count",
                "currentIntervalRemainsCount",
                "remaining_count",
                "remainingCount",
                "remaining",
                "remains",
            ],
        );
        let explicit_used = read_number_field(
            chosen,
            &[
                "current_interval_used_count",
                "currentIntervalUsedCount",
                "used_count",
                "used",
            ],
        );
        let inferred_remaining = remaining_count.or(usage_field);
        let raw_used = explicit_used
            .unwrap_or_else(|| (total - inferred_remaining.unwrap_or(total)).clamp(0.0, total));
        let raw_remaining = inferred_remaining.unwrap_or(total - raw_used).max(0.0);
        let used = (raw_used * display_multiplier).round();
        let total_out = (total * display_multiplier).round();
        let remaining = (raw_remaining * display_multiplier).round();
        let percent = if total_out > 0.0 {
            Some((remaining / total_out * 100.0).clamp(0.0, 100.0))
        } else {
            None
        };
        (used, total_out, remaining, percent, false)
    };
    let resets_at = read_number_field(chosen, &["end_time", "endTime"])
        .and_then(epoch_to_ms)
        .map(iso_from_ms);
    let plan = infer_minimax_plan(total, region).unwrap_or_else(|| "MiniMax".to_string());
    let plan_with_region = format!("{plan} ({region})");
    let line = if is_percent_mode {
        let pct = remaining_percent_out.unwrap_or(0.0).clamp(0.0, 100.0);
        json!({
            "type": "progress",
            "label": "Session",
            "used": pct,
            "limit": 100,
            "format": { "kind": "percent", "mode": "remaining" },
            "subtitle": format!("{}% left", pct.round()),
            "resetsAt": resets_at
        })
    } else {
        json!({
            "type": "progress",
            "label": "Session",
            "used": remaining,
            "limit": total_out,
            "format": { "kind": "count", "mode": "remaining", "suffix": "prompts" },
            "subtitle": format!("{} left", remaining.round()),
            "resetsAt": resets_at
        })
    };

    Some(json!({
        "accountId": account_id,
        "provider": "minimax",
        "label": label,
        "balanceUsd": null,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "status": "live-local",
        "capturedAt": now_millis(),
        "source": "local_auth",
        "plan": plan_with_region,
        "usage": {
            "used": used,
            "total": total_out,
            "remaining": remaining,
            "remainingPercent": remaining_percent_out,
            "isPercentMode": is_percent_mode,
            "resetsAt": resets_at
        },
        "lines": [line],
        "meta": {
            "region": region,
            "baseUrl": base_url,
            "isPercentMode": is_percent_mode
        }
    }))
}

fn pick_minimax_remain<'a>(remains: &'a [Value], region: &str) -> Option<&'a Value> {
    let multiplier = if region == "CN" {
        1.0 / MINIMAX_MODEL_CALLS_PER_PROMPT
    } else {
        1.0
    };
    let mut percent_any = None;
    let mut percent_general = None;
    for item in remains {
        let total = read_number_field(
            item,
            &["current_interval_total_count", "currentIntervalTotalCount"],
        );
        if total.is_some_and(|total| total > 0.0 && (total * multiplier).round() > 0.0) {
            return Some(item);
        }
        let percent = read_number_field(
            item,
            &[
                "current_interval_remaining_percent",
                "currentIntervalRemainingPercent",
            ],
        );
        if percent.is_some_and(|percent| (0.0..=100.0).contains(&percent)) {
            if percent_any.is_none() {
                percent_any = Some(item);
            }
            let model = item
                .get("model_name")
                .or_else(|| item.get("modelName"))
                .and_then(Value::as_str);
            if percent_general.is_none() && model == Some("general") {
                percent_general = Some(item);
            }
        }
    }
    percent_general.or(percent_any)
}

fn error_snapshot(account_id: &str, provider: &str, label: &str, message: String) -> Value {
    json!({
        "accountId": account_id,
        "provider": provider,
        "label": label,
        "status": "error",
        "message": message,
        "capturedAt": now_millis(),
        "source": "provider_api",
        "lines": []
    })
}

fn read_number_value(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|raw| raw.trim().parse::<f64>().ok())
    })
}

fn read_number_field(item: &Value, names: &[&str]) -> Option<f64> {
    names
        .iter()
        .find_map(|name| item.get(*name).and_then(read_number_value))
}

fn format_money(value: f64, currency: &str) -> String {
    let symbol = match currency {
        "USD" => "$",
        "CNY" => "¥",
        _ => currency,
    };
    if symbol == currency && currency != "USD" && currency != "CNY" {
        format!("{symbol} {value:.2}")
    } else {
        format!("{symbol}{value:.2}")
    }
}

fn format_compact(value: f64) -> String {
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

fn today_date_str() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn epoch_to_ms(value: f64) -> Option<u128> {
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

fn iso_from_ms(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let millis = (ms % 1000) as u32;
    let datetime = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, millis * 1_000_000)
        .unwrap_or_else(chrono::Utc::now);
    datetime.to_rfc3339()
}

fn infer_minimax_plan(total: f64, region: &str) -> Option<String> {
    let prompts = if region == "CN" {
        total / MINIMAX_MODEL_CALLS_PER_PROMPT
    } else {
        total
    };
    if prompts >= 1000.0 {
        Some("Unlimited".to_string())
    } else if prompts >= 100.0 {
        Some("Pro".to_string())
    } else if prompts > 0.0 {
        Some("Standard".to_string())
    } else {
        None
    }
}

// ===== WINDOW MANAGEMENT =====

fn main_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())
}

fn now_millis() -> u128 {
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

fn map_session_status(event_name: &str) -> String {
    match event_name.to_lowercase().as_str() {
        "sessionstart" => "active".to_string(),
        "stop" | "sessionend" => "done".to_string(),
        "permissionrequest" => "waiting".to_string(),
        "posttooluse" | "userpromptsubmit" => "active".to_string(),
        _ => "idle".to_string(),
    }
}

async fn start_hook_server(app: AppHandle) {
    let listener = match TcpListener::bind(("127.0.0.1", 45873)).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("failed to bind hook server: {error}");
            return;
        }
    };

    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = handle_hook_socket(app_handle, stream).await;
                });
            }
            Err(error) => eprintln!("failed to accept hook client: {error}"),
        }
    }
}

async fn handle_hook_socket(app: AppHandle, stream: TcpStream) -> Result<(), String> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .await
        .map_err(|err| err.to_string())?;
    if read == 0 || line.trim().is_empty() {
        return Ok(());
    }

    let response = handle_hook_message(
        app,
        serde_json::from_str(&line).map_err(|err| err.to_string())?,
    )
    .await;
    let payload = serde_json::to_string(&response).map_err(|err| err.to_string())?;
    let mut stream = reader.into_inner();
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .await
        .map_err(|err| err.to_string())
}

async fn handle_hook_message(app: AppHandle, message: Value) -> Value {
    if message.get("event").and_then(Value::as_str) != Some("hook-event") {
        return json!({ "ok": true });
    }

    let data = message.get("data").cloned().unwrap_or_else(|| json!({}));
    let source = data
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let event_name = data
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let raw = data.get("raw").and_then(Value::as_str).unwrap_or("");
    let payload = data.get("payload").cloned().unwrap_or(Value::Null);

    // Record session for all events
    {
        let app_state = app.state::<AppState>();
        let mut sessions = match app_state.sessions.lock() {
            Ok(g) => g,
            Err(_) => return json!({ "ok": true, "recorded": false }),
        };
        let session_id = payload
            .get("session_id")
            .or_else(|| payload.get("sessionId"))
            .or_else(|| data.get("session_id"))
            .or_else(|| data.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or(source);
        let entry = sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionInfo::default);
        entry.id = session_id.to_string();
        entry.source = source.to_string();
        entry.status = map_session_status(event_name);
        entry.updated_at = now_millis();
        entry.last_event = event_name.to_string();
    }

    if !event_name.eq_ignore_ascii_case("PermissionRequest") {
        return json!({ "ok": true, "recorded": true });
    }

    let mut intervention = build_intervention(source, event_name, raw, payload);
    if is_persistently_allowed(&intervention) {
        return json!({
            "ok": true,
            "requiresDecision": true,
            "approved": true,
            "allowPersistent": true
        });
    }

    let (tx, rx) = oneshot::channel();
    intervention.responder = Some(tx);
    let pending_json = pending_intervention_json(&intervention);
    {
        let app_state = app.state::<AppState>();
        let mut pending = match app_state.intervention.lock() {
            Ok(pending) => pending,
            Err(_) => return json!({ "ok": true, "requiresDecision": true, "approved": false }),
        };
        if pending.is_some() {
            return json!({
                "ok": true,
                "requiresDecision": true,
                "approved": false,
                "allowPersistent": false,
                "message": "ThatIsOk already has a pending approval."
            });
        }
        *pending = Some(intervention);
    }

    let _ = app.emit("intervention-state", pending_json);
    let _ = app.emit("island-force-expand", ());
    match rx.await {
        Ok(decision) => json!({
            "ok": true,
            "requiresDecision": true,
            "approved": decision.approved,
            "allowPersistent": decision.allow_persistent
        }),
        Err(_) => json!({
            "ok": true,
            "requiresDecision": true,
            "approved": false,
            "allowPersistent": false
        }),
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
        PILL_WIDTH
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

fn position_initial(window: &WebviewWindow) -> Result<(), String> {
    let (area_x, area_y, area_w, _) = primary_work_area(window)?;
    let x = area_x + ((area_w as i32 - PILL_WIDTH as i32) / 2);
    window
        .set_position(PhysicalPosition::new(x, area_y + WINDOW_MARGIN))
        .map_err(|err| err.to_string())
}

fn register_shortcuts(app: &AppHandle) {
    let app_handle = app.clone();
    let toggle = Shortcut::new(
        Some(Modifiers::CONTROL | Modifiers::SHIFT),
        Code::Space,
    );
    let _ = app.global_shortcut().on_shortcut(toggle, move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            let _ = set_mode(&app_handle, "expanded");
            if let Ok(window) = main_window(&app_handle) {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
    });

    // Decision shortcuts
    let app_handle = app.clone();
    let approve = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyA);
    let _ = app.global_shortcut().on_shortcut(approve, move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            respond_to_intervention(&app_handle, "approve");
        }
    });

    let app_handle = app.clone();
    let always = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyL);
    let _ = app.global_shortcut().on_shortcut(always, move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            respond_to_intervention(&app_handle, "approve_always");
        }
    });

    let app_handle = app.clone();
    let deny = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyD);
    let _ = app.global_shortcut().on_shortcut(deny, move |_app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            respond_to_intervention(&app_handle, "deny");
        }
    });
}

fn respond_to_intervention(app: &AppHandle, decision: &str) {
    let app_state = app.state::<AppState>();
    let mut pending = match app_state.intervention.lock() {
        Ok(pending) => pending,
        Err(_) => return,
    };
    let Some(mut current) = pending.take() else {
        return;
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
    let _ = set_mode(app, "pill");
}

async fn perform_initial_sync(app: AppHandle) {
    let accounts = sync_provider_accounts().await;
    {
        let state = app.state::<AppState>();
        let mut usage = state.usage.lock().unwrap();
        usage.balances = accounts;
    }
    let data = {
        let state = app.state::<AppState>();
        get_dashboard_data(&state)
    };
    let _ = app.emit("island-data", data);
}

fn schedule_periodic_sync(app: AppHandle) {
    let defaults = read_json_file("defaults.json");
    let interval_minutes = defaults
        .get("syncIntervalMinutes")
        .and_then(Value::as_u64)
        .unwrap_or(10);
    let duration = std::time::Duration::from_secs(interval_minutes * 60);

    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(duration);
        // Skip first tick (already did initial sync)
        interval.tick().await;
        loop {
            interval.tick().await;
            let accounts = sync_provider_accounts().await;
            {
                let state = app.state::<AppState>();
                let mut usage = state.usage.lock().unwrap();
                usage.balances = accounts;
            }
            let data = {
                let state = app.state::<AppState>();
                get_dashboard_data(&state)
            };
            let _ = app.emit("island-data", data);
        }
    });
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
            island_drag_start,
            island_drag_move,
            island_drag_end,
            intervention_respond,
            providers_get_visibility,
            providers_set_visibility
        ])
        .setup(|app| {
            let window = main_window(app.handle()).map_err(Box::<dyn std::error::Error>::from)?;
            position_initial(&window).map_err(Box::<dyn std::error::Error>::from)?;
            window.show().map_err(Box::<dyn std::error::Error>::from)?;
            register_shortcuts(app.handle());

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
                                            let accounts =
                                                sync_provider_accounts().await;
                                            {
                                                let state = h.state::<AppState>();
                                                let mut usage = state.usage.lock().unwrap();
                                                usage.balances = accounts;
                                            }
                                            let data = {
                                                let state = h.state::<AppState>();
                                                get_dashboard_data(&state)
                                            };
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
                start_hook_server(app_handle).await;
            });

            // Hook injection
            inject_agent_hooks();

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
