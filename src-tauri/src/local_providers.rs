use base64::Engine;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{format_compact, now_millis, read_env_value, today_date_str};

pub(crate) fn fetch_claude_snapshot(account_id: &str, label: &str) -> Option<Value> {
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

    let plan_str = if plan_type.is_empty() {
        "unknown"
    } else {
        &plan_type
    };

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
        "tokenUsage": {
            "exactInput": token_usage.total_input,
            "exactOutput": token_usage.total_output,
            "exactTotal": token_usage.total_input + token_usage.total_output,
            "estimatedInput": 0,
            "estimatedOutput": 0,
            "estimatedTotal": 0,
            "source": "claude-jsonl"
        },
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
        return ClaudeTodayStats {
            message_count: 0,
            session_count: 0,
            tool_call_count: 0,
        };
    };
    let Ok(stats) = serde_json::from_str::<Value>(&content) else {
        return ClaudeTodayStats {
            message_count: 0,
            session_count: 0,
            tool_call_count: 0,
        };
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

    let mut files: Vec<_> = entries
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
        return TokenUsage {
            total_input: 0,
            total_output: 0,
        };
    }

    let Ok(entries) = fs::read_dir(&projects_dir) else {
        return TokenUsage {
            total_input: 0,
            total_output: 0,
        };
    };
    let mut total_input = 0u64;
    let mut total_output = 0u64;
    let today = today_date_str();

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
                if !entry_is_today(&entry, &today) {
                    continue;
                }
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

    TokenUsage {
        total_input,
        total_output,
    }
}

fn entry_is_today(entry: &Value, today: &str) -> bool {
    let timestamp = entry
        .get("timestamp")
        .or_else(|| entry.get("createdAt"))
        .or_else(|| entry.get("created_at"))
        .or_else(|| entry.get("ts"));

    if let Some(value) = timestamp.and_then(Value::as_str) {
        if value.starts_with(today) {
            return true;
        }
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
            return dt.format("%Y-%m-%d").to_string() == today;
        }
    }

    if let Some(value) = timestamp.and_then(Value::as_f64) {
        let millis = if value > 1_000_000_000_000.0 {
            value as i64
        } else {
            (value * 1000.0) as i64
        };
        let secs = millis / 1000;
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, 0) {
            return dt.format("%Y-%m-%d").to_string() == today;
        }
    }

    false
}

pub(crate) fn fetch_gemini_snapshot(account_id: &str, label: &str) -> Option<Value> {
    let home = dirs::home_dir()?;
    let antigravity_dirs = antigravity_data_dirs(&home);
    if !antigravity_dirs.is_empty() {
        return Some(fetch_antigravity_snapshot(
            account_id,
            label,
            &antigravity_dirs,
        ));
    }
    fetch_legacy_gemini_snapshot(account_id, label)
}

fn antigravity_data_dirs(home: &Path) -> Vec<PathBuf> {
    [
        home.join(".gemini").join("antigravity-cli"),
        home.join(".gemini").join("antigravity"),
        home.join(".gemini").join("antigravity-ide"),
    ]
    .into_iter()
    .filter(|path| path.exists())
    .collect()
}

fn fetch_antigravity_snapshot(account_id: &str, label: &str, dirs: &[PathBuf]) -> Value {
    let stats = read_antigravity_today_stats(dirs);
    let status = if stats.authenticated {
        "live-local"
    } else if stats.saw_login_error {
        "stale"
    } else {
        "setup"
    };
    let model = if stats.latest_model.is_empty() {
        "Auto".to_string()
    } else {
        stats.latest_model.clone()
    };
    let mut lines = vec![json!({
        "type": "text",
        "label": "Today",
        "value": format!("{} calls", format_compact(stats.model_requests as f64)),
        "subtitle": format!(
            "{} sessions · {} quota checks",
            format_compact(stats.session_count as f64),
            format_compact(stats.quota_refreshes as f64)
        )
    })];
    lines.push(json!({
        "type": "text",
        "label": "Model",
        "value": model,
        "subtitle": "Antigravity local logs"
    }));

    json!({
        "accountId": account_id,
        "provider": "gemini",
        "label": label,
        "status": status,
        "capturedAt": now_millis(),
        "source": "local_auth",
        "plan": "Antigravity",
        "balanceUsd": null,
        "creditTotalUsd": null,
        "creditUsedUsd": null,
        "usage": {
            "used": stats.model_requests,
            "total": 0,
            "remaining": 0,
            "remainingPercent": 0,
            "todayMessages": stats.model_requests,
            "todaySessions": stats.session_count,
            "todayTools": stats.tool_events,
            "tokens": {
                "input": 0,
                "output": 0,
            }
        },
        "tokenUsage": {
            "exactInput": 0,
            "exactOutput": 0,
            "exactTotal": 0,
            "estimatedInput": 0,
            "estimatedOutput": 0,
            "estimatedTotal": 0,
            "source": "antigravity-local"
        },
        "lines": lines,
        "meta": {
            "isStale": status != "live-local",
            "todayMessages": stats.model_requests,
            "todaySessions": stats.session_count,
            "todayTools": stats.tool_events,
            "modelRequests": stats.model_requests,
            "quotaRefreshes": stats.quota_refreshes,
            "latestModel": stats.latest_model,
        },
        "message": if status == "live-local" {
            "Antigravity local usage stats"
        } else {
            "Antigravity login required or stale"
        }
    })
}

struct AntigravityTodayStats {
    session_count: u32,
    model_requests: u32,
    quota_refreshes: u32,
    tool_events: u32,
    authenticated: bool,
    saw_login_error: bool,
    latest_model: String,
}

fn read_antigravity_today_stats(dirs: &[PathBuf]) -> AntigravityTodayStats {
    let today_ymd = today_date_str();
    let today = today_ymd.replace('-', "");
    let mut stats = AntigravityTodayStats {
        session_count: 0,
        model_requests: 0,
        quota_refreshes: 0,
        tool_events: 0,
        authenticated: false,
        saw_login_error: false,
        latest_model: String::new(),
    };

    for dir in dirs {
        let log_dir = dir.join("log");
        if log_dir.exists() {
            let Ok(files) = fs::read_dir(&log_dir) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
                    continue;
                }
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if !name.contains(&today) {
                    continue;
                }
                stats.session_count += 1;
                read_antigravity_log_file(&path, &mut stats);
            }
            continue;
        }

        let conversations_dir = dir.join("conversations");
        if conversations_dir.exists() {
            stats.session_count +=
                count_today_files_in_dir(&conversations_dir, &["db", "pb"], &today_ymd) as u32;
        }
    }

    stats
}

fn read_antigravity_log_file(path: &Path, stats: &mut AntigravityTodayStats) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        if line.contains("v1internal:loadCodeAssist") {
            stats.model_requests += 1;
        }
        if line.contains("quotaRefreshLoop: starting reload")
            || line.contains("retrieveUserQuotaSummary")
        {
            stats.quota_refreshes += 1;
        }
        if line.contains("PreToolUse") || line.contains("PostToolUse") {
            stats.tool_events += 1;
        }
        if line.contains("authenticated successfully") || line.contains("authenticated via keyring")
        {
            stats.authenticated = true;
        }
        if line.contains("You are not logged into Antigravity") {
            stats.saw_login_error = true;
        }
        if let Some(model) = extract_quoted_value(line, "label=\"") {
            stats.latest_model = model;
        }
    }
}

fn extract_quoted_value(line: &str, marker: &str) -> Option<String> {
    let start = line.find(marker)? + marker.len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn count_today_files_in_dir(dir: &Path, extensions: &[&str], today: &str) -> usize {
    let Ok(files) = fs::read_dir(dir) else {
        return 0;
    };
    files
        .flatten()
        .filter(|file| {
            let path = file.path();
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| extensions.contains(&ext))
                && file
                    .metadata()
                    .ok()
                    .and_then(|meta| meta.modified().ok())
                    .is_some_and(|modified| {
                        let modified = chrono::DateTime::<chrono::Utc>::from(modified);
                        modified.format("%Y-%m-%d").to_string() == today
                    })
        })
        .count()
}

fn fetch_legacy_gemini_snapshot(account_id: &str, label: &str) -> Option<Value> {
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

    let plan_name = if daily_limit >= 1000 {
        "Gemini Code Assist (legacy)"
    } else {
        "Gemini CLI (legacy)"
    };

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
        "tokenUsage": {
            "exactInput": today_stats.tokens_input,
            "exactOutput": today_stats.tokens_output,
            "exactTotal": today_stats.tokens_input + today_stats.tokens_output,
            "estimatedInput": 0,
            "estimatedOutput": 0,
            "estimatedTotal": 0,
            "source": "gemini-jsonl"
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
    let Ok(projects) = fs::read_dir(&tmp_dir) else {
        return stats;
    };

    for project in projects.flatten() {
        let project_path = project.path();
        if !project_path.is_dir() {
            continue;
        }
        let chats_path = project_path.join("chats");
        if !chats_path.exists() {
            continue;
        }
        let Ok(files) = fs::read_dir(&chats_path) else {
            continue;
        };
        for file in files.flatten() {
            let fp = file.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(content) = fs::read_to_string(&fp) else {
                continue;
            };
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
                    if is_today {
                        stats.tokens_input +=
                            tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
                        stats.tokens_output +=
                            tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
                    }
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

pub(crate) fn fetch_kiro_snapshot(account_id: &str, label: &str) -> Option<Value> {
    let home = dirs::home_dir()?;
    let db_path = if cfg!(target_os = "macos") {
        home.join("Library")
            .join("Application Support")
            .join("Kiro")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
    } else if cfg!(target_os = "windows") {
        dirs::data_dir()
            .or_else(|| Some(home.join("AppData").join("Roaming")))?
            .join("Kiro")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
    } else {
        dirs::config_dir()
            .unwrap_or_else(|| home.join(".config"))
            .join("Kiro")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb")
    };
    if !db_path.exists() {
        return None;
    }

    let conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = 'kiro.kiroAgent' LIMIT 1")
        .ok()?;
    let raw: Option<String> = stmt.query_row([], |row| row.get(0)).ok();
    drop(stmt);
    drop(conn);

    let raw = raw?;
    let state: Value = serde_json::from_str(&raw).ok()?;
    let usage_state = state.get("kiro.resourceNotifications.usageState")?;
    let breakdowns = usage_state
        .get("usageBreakdowns")
        .and_then(Value::as_array)?;

    let primary = breakdowns
        .iter()
        .find(|b| {
            b.get("type")
                .or_else(|| b.get("resourceType"))
                .and_then(Value::as_str)
                == Some("CREDIT")
        })
        .or_else(|| breakdowns.first())?;

    let used = primary
        .get("currentUsageWithPrecision")
        .or_else(|| primary.get("currentUsage"))
        .and_then(crate::read_number_value)
        .unwrap_or(0.0);
    let limit = primary
        .get("usageLimitWithPrecision")
        .or_else(|| primary.get("usageLimit"))
        .and_then(crate::read_number_value)
        .unwrap_or(0.0);
    let reset_at = primary
        .get("resetDate")
        .or_else(|| primary.get("nextDateReset"))
        .or_else(|| primary.get("resetAt"))
        .and_then(Value::as_str)
        .map(String::from);

    if limit <= 0.0 {
        return None;
    }

    let remaining = (limit - used).max(0.0);
    let mut lines = Vec::new();
    let mut progress = json!({
        "type": "progress",
        "label": "Credits",
        "used": remaining,
        "limit": limit,
        "format": { "kind": "count", "mode": "remaining", "suffix": "credits" },
        "subtitle": format!("{} left of {}", remaining.round(), limit.round())
    });
    if let Some(ref r) = reset_at {
        progress["resetsAt"] = json!(r);
    }
    lines.push(progress);

    let bonus = primary
        .get("freeTrialInfo")
        .or_else(|| primary.get("freeTrialUsage"));
    if let Some(b) = bonus {
        let b_used = b
            .get("currentUsageWithPrecision")
            .or_else(|| b.get("currentUsage"))
            .and_then(crate::read_number_value)
            .unwrap_or(0.0);
        let b_limit = b
            .get("usageLimitWithPrecision")
            .or_else(|| b.get("usageLimit"))
            .and_then(crate::read_number_value)
            .unwrap_or(0.0);
        if b_limit > 0.0 {
            let b_remaining = (b_limit - b_used).max(0.0);
            lines.push(json!({
                "type": "progress",
                "label": "Bonus Credits",
                "used": b_remaining,
                "limit": b_limit,
                "format": { "kind": "count", "mode": "remaining", "suffix": "credits" },
                "subtitle": format!("{} left of {}", b_remaining.round(), b_limit.round())
            }));
        }
    }

    let timestamp = usage_state
        .get("timestamp")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let plan = state
        .get("subscriptionInfo")
        .and_then(|s| s.get("subscriptionTitle"))
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    Some(json!({
        "accountId": account_id,
        "provider": "kiro",
        "label": label,
        "balanceUsd": null,
        "creditTotalUsd": limit,
        "creditUsedUsd": used,
        "status": "live-local",
        "capturedAt": timestamp.max(0) as u128,
        "source": "local_db",
        "plan": plan.unwrap_or_else(|| "Kiro".to_string()),
        "lines": lines,
        "meta": {
            "timestamp": timestamp,
            "hasBonus": bonus.is_some()
        }
    }))
}
