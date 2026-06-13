use serde_json::{json, Value};

use crate::{epoch_to_ms, iso_from_ms, now_millis, read_env_value, read_number_value};

const MINIMAX_MODEL_CALLS_PER_PROMPT: f64 = 15.0;

pub(crate) async fn fetch_deepseek_snapshot(
    client: &reqwest::Client,
    account_id: &str,
    label: &str,
) -> Option<Value> {
    let api_key = match read_env_value(&["DEEPSEEK_API_KEY", "DEEPSEEK_API_TOKEN"]) {
        Some(api_key) => api_key,
        None => return None,
    };
    let base_url =
        read_env_value(&["DEEPSEEK_BASE_URL"]).unwrap_or_else(|| "https://api.deepseek.com".to_string());
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

pub(crate) async fn fetch_minimax_snapshot(
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
        &["current_interval_remaining_percent", "currentIntervalRemainingPercent"],
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
            &["current_interval_remaining_percent", "currentIntervalRemainingPercent"],
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
