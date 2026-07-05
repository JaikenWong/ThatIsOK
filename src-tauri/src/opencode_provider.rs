use chrono::{Datelike, TimeZone};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::{debug_log, format_compact, iso_from_ms, now_millis, read_env_value, read_json_file};

#[derive(Default)]
pub(crate) struct LocalTokenUsage {
    pub(crate) input: u64,
    pub(crate) output: u64,
    pub(crate) reasoning: u64,
    pub(crate) cache_read: u64,
    pub(crate) cache_write: u64,
}

#[derive(Default)]
struct CostRow {
    created_ms: i64,
    cost: f64,
}

pub(crate) async fn fetch_opencode_snapshot(
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
        debug_log!("[opencode] cookie configured; trying web usage first");
        let cookie = normalize_cookie_header(&cookie_header);
        let workspace_id = match cookie {
            Some(ref c) => fetch_opencode_go_workspace_id(client, c).await,
            None => None,
        };
        debug_log!(
            "[opencode] workspace id {}",
            workspace_id.as_deref().unwrap_or("<missing>")
        );

        if is_go {
            if let (Some(ref c), Some(ref wid)) = (&cookie, &workspace_id) {
                if let Some(web_lines) = fetch_opencode_go_web_lines(client, c, wid).await {
                    debug_log!(
                        "[opencode] using web dashboard usage lines={}",
                        web_lines.len()
                    );
                    lines.extend(web_lines);
                    usage_source = "opencode-go-web".to_string();
                    dashboard_message = Some("official dashboard usage".to_string());
                } else {
                    debug_log!("[opencode] web dashboard usage unavailable");
                }
            }
            if lines.is_empty() {
                if let Some(ref wid) = workspace_id {
                    debug_log!("[opencode] trying subscription usage fallback");
                    if let Some(sub_lines) =
                        fetch_opencode_subscription_lines(client, &cookie_header, wid).await
                    {
                        debug_log!(
                            "[opencode] using subscription usage lines={}",
                            sub_lines.len()
                        );
                        lines.extend(sub_lines);
                        usage_source = "opencode-subscription".to_string();
                        dashboard_message = Some("subscription usage".to_string());
                    } else {
                        debug_log!("[opencode] subscription usage unavailable");
                    }
                } else {
                    debug_log!("[opencode] skip subscription usage: missing workspace id");
                }
            }
            if let Some(ref wid) = workspace_id {
                zen_balance_usd = fetch_opencode_zen_balance(client, &cookie_header, wid).await;
            }
        } else {
            if let Some(ref wid) = workspace_id {
                debug_log!("[opencode] Zen account; trying subscription usage");
                if let Some(sub_lines) =
                    fetch_opencode_subscription_lines(client, &cookie_header, wid).await
                {
                    debug_log!(
                        "[opencode] using subscription usage lines={}",
                        sub_lines.len()
                    );
                    lines.extend(sub_lines);
                    usage_source = "opencode-subscription".to_string();
                    dashboard_message = Some("subscription usage".to_string());
                } else {
                    debug_log!("[opencode] subscription usage unavailable");
                }
            } else {
                debug_log!("[opencode] skip subscription usage: missing workspace id");
            }
        }
    } else {
        debug_log!("[opencode] no web cookie configured; skipping web usage");
    }

    if lines.is_empty() && is_go {
        let db_path = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        if let Some(local_lines) = build_opencode_go_local_lines(&db_path) {
            debug_log!(
                "[opencode] using local SQLite usage lines={}",
                local_lines.len()
            );
            lines.extend(local_lines);
            usage_source = "opencode-go-local".to_string();
            dashboard_message = Some("local history estimate".to_string());
        } else {
            debug_log!("[opencode] local SQLite usage unavailable");
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

async fn fetch_opencode_go_web_lines(
    client: &reqwest::Client,
    cookie_header: &str,
    workspace_id: &str,
) -> Option<Vec<Value>> {
    let cookie = normalize_cookie_header(cookie_header)?;
    let url = format!("https://opencode.ai/workspace/{workspace_id}/go");
    debug_log!("[opencode] fetch web dashboard url={url}");
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
        .map_err(|error| debug_log!("[opencode] web dashboard request failed: {error}"))
        .ok()?;
    let status = response.status();
    debug_log!("[opencode] web dashboard status={status}");
    if !status.is_success() {
        return None;
    }
    let text = response
        .text()
        .await
        .map_err(|error| debug_log!("[opencode] web dashboard body failed: {error}"))
        .ok()?;
    debug_log!("[opencode] web dashboard body bytes={}", text.len());
    let lines = build_opencode_go_web_lines(&text);
    debug_log!(
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
        debug_log!("[opencode] workspace id from override");
        return Some(id);
    }
    let text = fetch_opencode_go_workspace_text(client, cookie_header, "GET", None).await?;
    if let Some(id) = parse_opencode_go_workspace_id(&text) {
        debug_log!("[opencode] workspace id from GET server response");
        return Some(id);
    }
    let fallback =
        fetch_opencode_go_workspace_text(client, cookie_header, "POST", Some("[]")).await?;
    let id = parse_opencode_go_workspace_id(&fallback);
    if id.is_some() {
        debug_log!("[opencode] workspace id from POST server response");
    } else {
        debug_log!("[opencode] workspace id parse failed");
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
    if let Some(lines) = parse_opencode_subscription_json(text) {
        return Some(lines);
    }
    parse_opencode_subscription_regex(text)
}

fn parse_opencode_subscription_json(text: &str) -> Option<Vec<Value>> {
    let val: Value = serde_json::from_str(text).ok()?;
    let dict = val.as_object()?;

    if let Some(lines) = parse_usage_dictionary(dict) {
        return Some(lines);
    }
    for key in ["data", "result", "usage", "billing", "payload"] {
        if let Some(nested) = dict.get(key).and_then(Value::as_object) {
            if let Some(lines) = parse_usage_dictionary(nested) {
                return Some(lines);
            }
        }
    }
    if let Some(lines) = parse_usage_nested(dict, 0) {
        return Some(lines);
    }
    parse_usage_from_candidates(&val)
}

fn parse_usage_dictionary(dict: &serde_json::Map<String, Value>) -> Option<Vec<Value>> {
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

    let mut percent: Option<f64> = None;
    for key in &percent_keys {
        if let Some(n) = dict.get(*key).and_then(|v| json_f64(v)) {
            percent = Some(n);
            break;
        }
    }

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

static OP_SUB_ROLLING_PCT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"rollingUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#).unwrap()
});
static OP_SUB_ROLLING_RST_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"rollingUsage[^}]*?resetInSec\s*:\s*([0-9]+)"#).unwrap()
});
static OP_SUB_WEEKLY_PCT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"weeklyUsage[^}]*?usagePercent\s*:\s*([0-9]+(?:\.[0-9]+)?)"#).unwrap()
});
static OP_SUB_WEEKLY_RST_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"weeklyUsage[^}]*?resetInSec\s*:\s*([0-9]+)"#).unwrap()
});

fn parse_opencode_subscription_regex(text: &str) -> Option<Vec<Value>> {
    let rolling_pct = OP_SUB_ROLLING_PCT_RE
        .captures(text)?
        .get(1)
        .and_then(|m| m.as_str().parse::<f64>().ok())?;
    let rolling_rst = OP_SUB_ROLLING_RST_RE
        .captures(text)?
        .get(1)
        .and_then(|m| m.as_str().parse::<i64>().ok())?;
    let weekly_pct = OP_SUB_WEEKLY_PCT_RE
        .captures(text)?
        .get(1)
        .and_then(|m| m.as_str().parse::<f64>().ok())?;
    let weekly_rst = OP_SUB_WEEKLY_RST_RE
        .captures(text)?
        .get(1)
        .and_then(|m| m.as_str().parse::<i64>().ok())?;

    let mut lines = Vec::new();
    lines.push(opencode_go_web_line("5-hour", rolling_pct, rolling_rst));
    lines.push(opencode_go_web_line("Weekly", weekly_pct, weekly_rst));
    Some(lines)
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

static ZEN_BALANCE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r#"(?i)(?:current\s+balance|zen\s+balance|現在の残高)[^$]{0,80}\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)"#,
    )
    .unwrap()
});
static ZEN_BALANCE_BROAD_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?i)(?:balance|残高)[\s\S]{0,120}?\$\s*([0-9][0-9,]*(?:\.[0-9]+)?)"#)
        .unwrap()
});

fn parse_opencode_zen_balance_from_text(text: &str) -> Option<f64> {
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        if let Some(balance) = find_zen_balance_in_json(&val) {
            return Some(balance);
        }
    }
    if let Some(caps) = ZEN_BALANCE_RE.captures(text) {
        if let Some(m) = caps.get(1) {
            if let Ok(v) = m.as_str().replace(',', "").parse::<f64>() {
                return Some(v);
            }
        }
    }
    if let Some(caps) = ZEN_BALANCE_BROAD_RE.captures(text) {
        if let Some(m) = caps.get(1) {
            if let Ok(v) = m.as_str().replace(',', "").parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

static BILLING_CUSTOMER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"(?:\"customerID\"|customerID)"#).unwrap());
static BILLING_BALANCE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?:"balance"|balance)\s*:\s*(?:\$R\[\d+\]\s*=\s*)?(-?[0-9]+(?:\.[0-9]+)?)"#)
        .unwrap()
});

fn parse_opencode_zen_balance_from_billing(text: &str) -> Option<f64> {
    if let Ok(val) = serde_json::from_str::<Value>(text) {
        if let Some(raw) = find_raw_billing_balance(&val) {
            return Some(raw / 100_000_000.0);
        }
    }
    if !BILLING_CUSTOMER_RE.is_match(text) {
        return None;
    }
    if let Some(caps) = BILLING_BALANCE_RE.captures(text) {
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
        "resetsAt": iso_from_ms(reset_at_ms as u128)
    })
}

pub(crate) fn build_opencode_go_local_lines(db_path: &Path) -> Option<Vec<Value>> {
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
    use rusqlite::{Connection, OpenFlags};
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
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

fn sqlite_has_table(conn: &rusqlite::Connection, name: &str) -> bool {
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
        "resetsAt": iso_from_ms(reset_at_ms as u128)
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
        if cookie.is_empty() {
            return None;
        }
        return Some(cookie.to_string());
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
    debug_log!("[opencode] cookie from CodexBar legacy cache");
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
        now_millis(),
    ));
    // Remove stale temp file from previous sync
    let _ = fs::remove_file(&temp_path);
    fs::copy(db_path, &temp_path).ok()?;
    let result = read_chromium_opencodego_cookie_from_copied_db(&temp_path, db_path);
    let _ = fs::remove_file(temp_path);
    result
}

fn read_chromium_opencodego_cookie_from_copied_db(
    temp_path: &Path,
    source_path: &Path,
) -> Option<String> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(temp_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
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
        debug_log!(
            "[opencode] cookie from Chromium profile {} names={}",
            source_path.display(),
            cookies.len()
        );
        return Some(cookies.join("; "));
    }
    if encrypted_count > 0 {
        debug_log!(
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

const OPENCODE_GO_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const OPENCODE_GO_WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";
const OPENCODE_SUBSCRIPTION_SERVER_ID: &str =
    "7abeebee372f304e050aaaf92be863f4a86490e382f8c79db68fd94040d691b4";
const OPENCODE_GO_BILLING_SERVER_ID: &str =
    "c83b78a614689c38ebee981f9b39a8b377716db85c1fd7dbab604adc02d3313d";

pub(crate) fn read_opencode_token_usage(db_path: &Path) -> Option<LocalTokenUsage> {
    use rusqlite::{Connection, OpenFlags};
    if !db_path.exists() {
        return None;
    }
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
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
