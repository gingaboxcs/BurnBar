// Polls Anthropic Admin + OpenAI Admin usage endpoints. Both require
// "admin" keys, not regular API keys. We aggregate the day's tokens per
// provider and estimate cost from a built-in price table.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

const ANTHROPIC_URL: &str =
    "https://api.anthropic.com/v1/organizations/usage_report/messages";
const OPENAI_URL: &str = "https://api.openai.com/v1/organization/usage/completions";

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub estimated_cost_usd: f64,
    pub by_model: Vec<ModelLine>,
    pub fetched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ModelLine {
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ApiSnapshot {
    pub anthropic: Option<ProviderUsage>,
    pub openai: Option<ProviderUsage>,
    pub anthropic_error: Option<String>,
    pub openai_error: Option<String>,
}

// Pricing per 1M tokens (USD). Best-effort defaults; users can ignore if
// inaccurate. Keys matched as substring against the returned `model` field.
fn price_per_million(model: &str) -> (f64, f64) {
    let m = model.to_lowercase();
    // (input, output) — cache prices approximated as input * 1.25 / 0.1 below.
    if m.contains("opus-4-7") {
        (15.0, 75.0)
    } else if m.contains("opus") {
        (15.0, 75.0)
    } else if m.contains("sonnet-4") {
        (3.0, 15.0)
    } else if m.contains("haiku-4") {
        (1.0, 5.0)
    } else if m.contains("gpt-5") {
        (5.0, 20.0)
    } else if m.contains("gpt-4.1") || m.contains("gpt-4-1") {
        (2.0, 8.0)
    } else if m.contains("gpt-4o") {
        (2.5, 10.0)
    } else if m.contains("o3") || m.contains("o4") {
        (10.0, 40.0)
    } else if m.contains("gpt-4") {
        (10.0, 30.0)
    } else if m.contains("gpt-3.5") || m.contains("gpt-35") {
        (0.5, 1.5)
    } else {
        (3.0, 15.0)
    }
}

fn estimate_cost(
    model: &str,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
) -> f64 {
    let (in_p, out_p) = price_per_million(model);
    let cc_p = in_p * 1.25;
    let cr_p = in_p * 0.10;
    (input as f64 / 1_000_000.0) * in_p
        + (output as f64 / 1_000_000.0) * out_p
        + (cache_creation as f64 / 1_000_000.0) * cc_p
        + (cache_read as f64 / 1_000_000.0) * cr_p
}

fn day_start_utc() -> DateTime<Utc> {
    let now = Utc::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
}

// ---------- Anthropic ----------

#[derive(Debug, Deserialize)]
struct AnthropicReport {
    data: Vec<AnthropicBucket>,
}
#[derive(Debug, Deserialize)]
struct AnthropicBucket {
    results: Vec<AnthropicRow>,
}
#[derive(Debug, Deserialize, Default)]
struct AnthropicRow {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    uncached_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation: AnthropicCacheCreation,
}
#[derive(Debug, Deserialize, Default)]
struct AnthropicCacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

pub async fn fetch_anthropic(admin_key: &str) -> Result<ProviderUsage, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let starting_at = day_start_utc()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let resp = client
        .get(ANTHROPIC_URL)
        .header("X-Api-Key", admin_key)
        .header("anthropic-version", "2023-06-01")
        .query(&[
            ("starting_at", starting_at.as_str()),
            ("bucket_width", "1d"),
            ("limit", "2"),
            ("group_by[]", "model"),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, truncate(&body, 200)));
    }

    let report: AnthropicReport = resp.json().await.map_err(|e| e.to_string())?;
    let mut usage = ProviderUsage::default();
    let mut model_acc: std::collections::HashMap<String, ModelLine> = Default::default();

    for bucket in report.data {
        for row in bucket.results {
            let model = row.model.clone().unwrap_or_else(|| "unknown".to_string());
            let cc = row.cache_creation.ephemeral_5m_input_tokens
                + row.cache_creation.ephemeral_1h_input_tokens;
            let cost = estimate_cost(
                &model,
                row.uncached_input_tokens,
                row.output_tokens,
                cc,
                row.cache_read_input_tokens,
            );
            usage.input_tokens += row.uncached_input_tokens;
            usage.output_tokens += row.output_tokens;
            usage.cache_creation += cc;
            usage.cache_read += row.cache_read_input_tokens;
            usage.estimated_cost_usd += cost;

            let entry = model_acc
                .entry(model.clone())
                .or_insert_with(|| ModelLine {
                    model,
                    ..Default::default()
                });
            entry.input_tokens += row.uncached_input_tokens;
            entry.output_tokens += row.output_tokens;
            entry.estimated_cost_usd += cost;
        }
    }
    let mut by_model: Vec<ModelLine> = model_acc.into_values().collect();
    by_model.sort_by(|a, b| {
        b.estimated_cost_usd
            .partial_cmp(&a.estimated_cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    usage.by_model = by_model;
    usage.fetched_at = Some(Utc::now());
    Ok(usage)
}

// ---------- OpenAI ----------

#[derive(Debug, Deserialize)]
struct OpenAIReport {
    data: Vec<OpenAIBucket>,
}
#[derive(Debug, Deserialize)]
struct OpenAIBucket {
    #[serde(default)]
    results: Vec<OpenAIRow>,
}
#[derive(Debug, Deserialize, Default)]
struct OpenAIRow {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    input_cached_tokens: u64,
}

pub async fn fetch_openai(admin_key: &str) -> Result<ProviderUsage, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())?;

    let start_unix = day_start_utc().timestamp();
    let end_unix = (Utc::now() + Duration::minutes(1)).timestamp();

    let resp = client
        .get(OPENAI_URL)
        .bearer_auth(admin_key)
        .query(&[
            ("start_time", start_unix.to_string()),
            ("end_time", end_unix.to_string()),
            ("bucket_width", "1d".into()),
            ("group_by[]", "model".into()),
            ("limit", "2".into()),
        ])
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, truncate(&body, 200)));
    }

    let report: OpenAIReport = resp.json().await.map_err(|e| e.to_string())?;
    let mut usage = ProviderUsage::default();
    let mut model_acc: std::collections::HashMap<String, ModelLine> = Default::default();

    for bucket in report.data {
        for row in bucket.results {
            let model = row.model.clone().unwrap_or_else(|| "unknown".to_string());
            let uncached_in = row.input_tokens.saturating_sub(row.input_cached_tokens);
            let cost = estimate_cost(
                &model,
                uncached_in,
                row.output_tokens,
                0,
                row.input_cached_tokens,
            );
            usage.input_tokens += uncached_in;
            usage.output_tokens += row.output_tokens;
            usage.cache_read += row.input_cached_tokens;
            usage.estimated_cost_usd += cost;

            let entry = model_acc
                .entry(model.clone())
                .or_insert_with(|| ModelLine {
                    model,
                    ..Default::default()
                });
            entry.input_tokens += uncached_in;
            entry.output_tokens += row.output_tokens;
            entry.estimated_cost_usd += cost;
        }
    }
    let mut by_model: Vec<ModelLine> = model_acc.into_values().collect();
    by_model.sort_by(|a, b| {
        b.estimated_cost_usd
            .partial_cmp(&a.estimated_cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    usage.by_model = by_model;
    usage.fetched_at = Some(Utc::now());
    Ok(usage)
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}
