use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::{now_millis, read_env_value};

pub(crate) async fn fetch_cursor_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let token = read_cursor_token()?;

    let usage = match fetch_cursor_api(
        client,
        &token,
        "/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
    )
    .await
    {
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
        .args(["find-generic-password", "-s", "cursor-access-token", "-w"])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    let trimmed = token.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
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
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
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
