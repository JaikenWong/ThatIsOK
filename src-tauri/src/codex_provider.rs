use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{epoch_to_ms, iso_from_ms, now_millis, read_env_value, read_number_value};

pub(crate) async fn fetch_codex_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let home = dirs::home_dir()?;
    let codex_home = read_env_value(&["CODEX_HOME"])
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let auth_path = codex_home.join("auth.json");
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
            .is_ok_and(|d| (chrono::Utc::now() - d).num_hours() > 8 * 24)
    }) || last_refresh.is_none();

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
        usage_data = read_codex_session_usage(&codex_home);
    }

    let effective_stale = is_stale && usage_data.is_none();
    let lines = build_codex_lines(usage_data.as_ref());
    let message = usage_error.unwrap_or_else(|| format!("plan {display_plan}"));
    let status = if effective_stale {
        "stale"
    } else {
        "live-local"
    };
    let plan = if effective_stale {
        "Codex auth stale"
    } else {
        &display_plan
    };
    let source = usage_data
        .as_ref()
        .and_then(|d| d.get("source"))
        .and_then(Value::as_str)
        .unwrap_or("local_auth");

    Some(json!({
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
        "lines": lines,
        "meta": {
            "planType": plan_type,
            "displayPlan": display_plan,
            "isStale": effective_stale,
            "lastRefresh": last_refresh,
        },
        "message": message
    }))
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
    let headers = response.headers().clone();
    let mut data: Value = response.json().await.map_err(|e| e.to_string())?;
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "source".to_string(),
            Value::String("provider_api".to_string()),
        );
        if let Some(val) = headers
            .get("x-codex-primary-used-percent")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<f64>().ok())
        {
            obj.insert("header_primary_used_pct".to_string(), json!(val));
        }
        if let Some(val) = headers
            .get("x-codex-secondary-used-percent")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<f64>().ok())
        {
            obj.insert("header_secondary_used_pct".to_string(), json!(val));
        }
    }
    Ok(data)
}

fn read_codex_session_usage(codex_home: &Path) -> Option<Value> {
    let sessions_dir = codex_home.join("sessions");
    if !sessions_dir.exists() {
        return None;
    }
    let latest = find_latest_rate_limit_event(&sessions_dir)?;
    let limits = latest
        .get("rateLimits")
        .or_else(|| latest.get("rate_limits"))?;

    let primary = limits.get("primary").and_then(|p| {
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
                let ms = event.get("timeMs").and_then(Value::as_i64).unwrap_or(0);
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
                let rate_limits = parsed.get("payload").and_then(|p| p.get("rate_limits"));
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
            auth_val["last_refresh"] = Value::String(chrono::Utc::now().to_rfc3339());
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
    let auth = data.get("https://api.openai.com/auth");
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

fn build_codex_lines(usage: Option<&Value>) -> Vec<Value> {
    let data = match usage {
        Some(d) => d,
        None => return Vec::new(),
    };
    let mut lines = Vec::new();

    let header_primary = data
        .get("header_primary_used_pct")
        .and_then(read_number_value);
    let header_secondary = data
        .get("header_secondary_used_pct")
        .and_then(read_number_value);

    if let Some(pct) = header_primary {
        let remaining = (100.0 - pct).max(0.0);
        lines.push(json!({
            "type": "progress",
            "label": "Session",
            "used": remaining,
            "limit": 100.0,
            "format": { "kind": "percent", "mode": "remaining" },
            "subtitle": format!("{}% left", remaining.round()),
        }));
    }
    if let Some(pct) = header_secondary {
        let remaining = (100.0 - pct).max(0.0);
        lines.push(json!({
            "type": "progress",
            "label": "Weekly",
            "used": remaining,
            "limit": 100.0,
            "format": { "kind": "percent", "mode": "remaining" },
            "subtitle": format!("{}% left", remaining.round()),
        }));
    }

    let has_primary = header_primary.is_some();
    let has_secondary = header_secondary.is_some();

    if has_primary && has_secondary {
        return lines;
    }

    let rate_limit = data.get("rate_limit").or_else(|| data.get("rateLimits"));
    let primary = rate_limit
        .and_then(|r| r.get("primary_window"))
        .or_else(|| rate_limit.and_then(|r| r.get("primaryWindow")));
    let secondary = rate_limit
        .and_then(|r| r.get("secondary_window"))
        .or_else(|| rate_limit.and_then(|r| r.get("secondaryWindow")));

    if !has_primary {
        if let Some(pw) = primary {
            let used_pct = pw
                .get("used_percent")
                .or_else(|| pw.get("usedPercent"))
                .and_then(read_number_value)
                .unwrap_or(0.0);
            let remaining_pct = (100.0 - used_pct).max(0.0);
            let reset_at = pw
                .get("reset_at")
                .or_else(|| pw.get("resetAt"))
                .and_then(read_number_value)
                .and_then(epoch_to_ms)
                .map(iso_from_ms);
            lines.push(json!({
                "type": "progress",
                "label": "5h window",
                "used": remaining_pct,
                "limit": 100.0,
                "format": { "kind": "percent", "mode": "remaining" },
                "subtitle": format!("{}% left", remaining_pct.round()),
                "resetsAt": reset_at
            }));
        }
    }

    if !has_secondary {
        if let Some(sw) = secondary {
            let used_pct = sw
                .get("used_percent")
                .or_else(|| sw.get("usedPercent"))
                .and_then(read_number_value)
                .unwrap_or(0.0);
            let remaining_pct = (100.0 - used_pct).max(0.0);
            let reset_at = sw
                .get("reset_at")
                .or_else(|| sw.get("resetAt"))
                .and_then(read_number_value)
                .and_then(epoch_to_ms)
                .map(iso_from_ms);
            lines.push(json!({
                "type": "progress",
                "label": "7d window",
                "used": remaining_pct,
                "limit": 100.0,
                "format": { "kind": "percent", "mode": "remaining" },
                "subtitle": format!("{}% left", remaining_pct.round()),
                "resetsAt": reset_at
            }));
        }
    }

    if !lines.is_empty() {
        return lines;
    }

    if let Some(rl) = rate_limit {
        if let Some(primary) = rl.get("primary_window").or_else(|| rl.get("primaryWindow")) {
            let used_pct = primary
                .get("used_percent")
                .or_else(|| primary.get("usedPercent"))
                .and_then(read_number_value)
                .unwrap_or(0.0);
            let remaining_pct = (100.0 - used_pct).max(0.0);
            let reset_at = primary
                .get("resets_at")
                .or_else(|| primary.get("resetAt"))
                .or_else(|| primary.get("resets_at"))
                .and_then(Value::as_str)
                .map(String::from);
            lines.push(json!({
                "type": "progress",
                "label": "Rate limit",
                "used": remaining_pct,
                "limit": 100.0,
                "format": { "kind": "percent", "mode": "remaining" },
                "subtitle": format!("{}% left", remaining_pct.round()),
                "resetsAt": reset_at
            }));
        }
    }

    lines
}
