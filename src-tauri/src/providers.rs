use serde_json::{json, Value};
use std::collections::HashMap;

use crate::codex_provider::fetch_codex_snapshot;
use crate::cursor_provider::fetch_cursor_snapshot;
use crate::local_providers::{fetch_claude_snapshot, fetch_gemini_snapshot};
use crate::remote_providers::{fetch_deepseek_snapshot, fetch_minimax_snapshot};
use crate::{
    build_config_account, build_config_accounts, fetch_opencode_snapshot, read_json_file,
    AppState, ProviderVisibility, SessionInfo,
};

pub(crate) fn provider_visibility() -> HashMap<String, ProviderVisibility> {
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

pub(crate) fn get_dashboard_data(state: &AppState) -> Value {
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

pub(crate) async fn sync_provider_accounts() -> Vec<Value> {
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
            "opencode" => fetch_opencode_snapshot(&client, account_id, label).await,
            _ => None,
        }
        .unwrap_or_else(|| build_config_account(&account, setting));
        snapshots.push(snapshot);
    }
    snapshots
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
