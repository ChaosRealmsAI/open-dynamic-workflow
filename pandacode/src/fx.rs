use std::{
    collections::BTreeMap,
    env,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const DEFAULT_FX_URL: &str = "https://open.er-api.com/v6/latest/USD";

#[derive(Debug, Clone, Default, Serialize)]
pub struct FxContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rates: Option<FxRateSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FxRateSnapshot {
    pub source: String,
    pub provider: String,
    pub base: String,
    pub quote: String,
    pub usd_to_cny: f64,
    pub cny_to_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_last_update_utc: Option<String>,
    pub fetched_at_unix_ms: u128,
}

#[derive(Debug, Deserialize)]
struct ExchangeRateResponse {
    result: Option<String>,
    provider: Option<String>,
    time_last_update_utc: Option<String>,
    base_code: Option<String>,
    rates: BTreeMap<String, f64>,
}

pub async fn resolve_if_needed(price_file: Option<&PathBuf>, max_cost: Option<f64>) -> FxContext {
    if !should_resolve(price_file, max_cost) {
        return FxContext::default();
    }
    if is_truthy_env("PANDACODE_BAMBOO_DISABLE_FX") || is_truthy_env("BAMBOO_DISABLE_FX") {
        return FxContext {
            rates: None,
            error: Some("disabled by PANDACODE_BAMBOO_DISABLE_FX or BAMBOO_DISABLE_FX".to_string()),
        };
    }
    let manual_rate = env::var("PANDACODE_BAMBOO_USD_CNY_RATE")
        .or_else(|_| env::var("BAMBOO_USD_CNY_RATE"))
        .ok()
        .filter(|raw| !raw.trim().is_empty());
    if let Some(raw) = manual_rate {
        return match raw.trim().parse::<f64>() {
            Ok(rate) if rate > 0.0 => FxContext {
                rates: Some(snapshot(
                    "env:PANDACODE_BAMBOO_USD_CNY_RATE",
                    "manual",
                    rate,
                    None,
                )),
                error: None,
            },
            Ok(_) => FxContext {
                rates: None,
                error: Some(
                    "PANDACODE_BAMBOO_USD_CNY_RATE/BAMBOO_USD_CNY_RATE must be positive"
                        .to_string(),
                ),
            },
            Err(err) => FxContext {
                rates: None,
                error: Some(format!(
                    "invalid PANDACODE_BAMBOO_USD_CNY_RATE/BAMBOO_USD_CNY_RATE: {err}"
                )),
            },
        };
    }

    match fetch_live().await {
        Ok(snapshot) => FxContext {
            rates: Some(snapshot),
            error: None,
        },
        Err(err) => FxContext {
            rates: None,
            error: Some(err),
        },
    }
}

fn should_resolve(price_file: Option<&PathBuf>, max_cost: Option<f64>) -> bool {
    if max_cost.is_some() {
        return true;
    }
    if price_file.is_some() {
        return true;
    }
    if env::var("PANDACODE_BAMBOO_PRICE_FILE")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    if env::var("BAMBOO_PRICE_FILE")
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    PathBuf::from(".pandacode/bamboo/pricing.cn.json").is_file()
        || PathBuf::from(".bamboo/pricing.cn.json").is_file()
}

async fn fetch_live() -> Result<FxRateSnapshot, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|err| format!("failed to build FX client: {err}"))?;
    let response = client
        .get(
            env::var("PANDACODE_BAMBOO_FX_URL")
                .or_else(|_| env::var("BAMBOO_FX_URL"))
                .unwrap_or_else(|_| DEFAULT_FX_URL.to_string()),
        )
        .send()
        .await
        .map_err(|err| format!("failed to fetch live FX rate: {err}"))?;
    let status = response.status();
    let raw = response
        .json::<ExchangeRateResponse>()
        .await
        .map_err(|err| format!("failed to parse live FX response: {err}"))?;
    if !status.is_success() {
        return Err(format!("FX provider returned {status}"));
    }
    if raw
        .result
        .as_deref()
        .is_some_and(|result| result != "success")
    {
        return Err(format!(
            "FX provider result was {}",
            raw.result.unwrap_or_else(|| "unknown".to_string())
        ));
    }
    if raw.base_code.as_deref() != Some("USD") {
        return Err(format!(
            "FX provider base_code was {}, expected USD",
            raw.base_code.unwrap_or_else(|| "unknown".to_string())
        ));
    }
    let Some(usd_to_cny) = raw.rates.get("CNY").copied() else {
        return Err("FX response did not contain CNY rate".to_string());
    };
    if usd_to_cny <= 0.0 {
        return Err("FX response CNY rate must be positive".to_string());
    }
    Ok(snapshot(
        DEFAULT_FX_URL,
        raw.provider.as_deref().unwrap_or("open.er-api.com"),
        usd_to_cny,
        raw.time_last_update_utc,
    ))
}

fn snapshot(
    source: &str,
    provider: &str,
    usd_to_cny: f64,
    time_last_update_utc: Option<String>,
) -> FxRateSnapshot {
    FxRateSnapshot {
        source: source.to_string(),
        provider: provider.to_string(),
        base: "USD".to_string(),
        quote: "CNY".to_string(),
        usd_to_cny: round_rate(usd_to_cny),
        cny_to_usd: round_rate(1.0 / usd_to_cny),
        time_last_update_utc,
        fetched_at_unix_ms: unix_millis(),
    }
}

fn is_truthy_env(key: &str) -> bool {
    env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes"
            )
        })
        .unwrap_or(false)
}

fn round_rate(value: f64) -> f64 {
    (value * 1_000_000_000.0).round() / 1_000_000_000.0
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}
