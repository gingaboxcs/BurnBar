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

// Pricing per 1M tokens (USD): (input, output, cache read). Best-effort
// defaults taken from the providers' public price pages (checked 2026-11-26);
// users can ignore if inaccurate. Rows are checked top to bottom, so more
// specific model names must come before the shorter names they contain
// (e.g. "opus-5-5" before "opus-5", "gpt-5.4-mini" before "gpt-5.4").
// A cache read price of `None` means "10% of input", which is the usual rate.
// Cache writes are approximated as input * 1.25 below.
const ANTHROPIC_PRICES: &[(&str, f64, f64, Option<f64>)] = &[
    // Fable / Mythos
    ("fable-5-2", 8.0, 40.0, Some(0.20)),
    ("mythos-5-2", 8.0, 40.0, Some(0.20)),
    ("fable", 10.0, 50.0, None),
    ("mythos", 10.0, 50.0, None),
    // Opus
    ("opus-5-5", 4.0, 20.0, Some(0.20)),
    ("opus-5", 5.0, 25.0, None),
    ("opus-4-8", 5.0, 25.0, None),
    ("opus-4-7", 5.0, 25.0, None),
    ("opus-4-6", 5.0, 25.0, None),
    ("opus-4-5", 5.0, 25.0, None),
    ("opus", 15.0, 75.0, None), // Opus 4.1 / 4 / 3
    // Sonnet
    ("sonnet-5-5", 2.0, 10.0, Some(0.10)),
    ("sonnet-5", 2.0, 10.0, None),
    ("sonnet", 3.0, 15.0, None), // Sonnet 4.x / 3.x
    // Haiku
    ("haiku-5-5", 0.10, 0.50, None), // prompts up to 100K tokens
    ("haiku-4", 1.0, 5.0, None),
    ("3-5-haiku", 0.80, 4.0, None),
    ("haiku", 0.25, 1.25, None), // Claude 3 Haiku
];

// OpenAI model ids are matched as prefixes, so dated snapshots like
// "gpt-4o-2024-08-06" still resolve. Short-context (<272K) rates.
const OPENAI_PRICES: &[(&str, f64, f64, Option<f64>)] = &[
    // GPT-6.x
    ("gpt-6.2-astra", 8.0, 40.0, None),
    ("gpt-6.2-sol", 2.0, 10.0, None),
    ("gpt-6.1-sol", 2.0, 10.0, None),
    ("gpt-6.1-terra", 1.0, 5.0, None),
    ("gpt-6-astra", 10.0, 50.0, None),
    ("gpt-6-sol", 2.0, 10.0, None),
    ("gpt-6-luna", 0.10, 0.50, None),
    ("gpt-6-cyber", 20.0, 100.0, None),
    ("gpt-6-rosalind", 10.0, 50.0, None),
    ("gpt-rosalind", 5.0, 25.0, None),
    // GPT-5.x
    ("gpt-5.6-cyber", 12.5, 75.0, None),
    ("gpt-5.6-sol", 4.0, 20.0, None),
    ("gpt-5.6-terra", 2.0, 12.0, None),
    ("gpt-5.6-luna", 0.20, 1.20, None),
    ("gpt-5.5-cyber", 12.5, 75.0, None),
    ("gpt-5.5-pro", 30.0, 180.0, None),
    ("gpt-5.5", 5.0, 30.0, None),
    ("gpt-5.4-pro", 30.0, 180.0, None),
    ("gpt-5.4-mini", 0.75, 4.50, None),
    ("gpt-5.4-nano", 0.20, 1.25, None),
    ("gpt-5.4", 2.50, 15.0, None),
    ("gpt-5.3", 1.75, 14.0, None),
    ("gpt-5.2-pro", 21.0, 168.0, None),
    ("gpt-5.2", 1.75, 14.0, None),
    ("gpt-5.1", 1.25, 10.0, None),
    ("gpt-5-pro", 15.0, 120.0, None),
    ("gpt-5-mini", 0.25, 2.0, None),
    ("gpt-5-nano", 0.05, 0.40, None),
    ("gpt-5", 1.25, 10.0, None),
    ("chat-latest", 5.0, 30.0, None),
    // GPT-4.x
    ("gpt-4.1-mini", 0.40, 1.60, None),
    ("gpt-4.1-nano", 0.10, 0.40, None),
    ("gpt-4.1", 2.0, 8.0, None),
    ("gpt-4o-mini", 0.15, 0.60, None),
    ("gpt-4o", 2.50, 10.0, None),
    ("gpt-4", 10.0, 30.0, None),
    ("gpt-3.5", 0.50, 1.50, None),
    ("gpt-35", 0.50, 1.50, None),
    // o-series
    ("o4-mini-deep-research", 2.0, 8.0, None),
    ("o4-mini", 1.10, 4.40, None),
    ("o3-deep-research", 10.0, 40.0, None),
    ("o3-pro", 20.0, 80.0, None),
    ("o3-mini", 1.10, 4.40, None),
    ("o3", 2.0, 8.0, None),
    ("o1-pro", 150.0, 600.0, None),
    ("o1-mini", 1.10, 4.40, None),
    ("o1", 15.0, 60.0, None),
];

const DEFAULT_PRICE: (f64, f64, f64) = (3.0, 15.0, 0.30);

fn price_per_million(model: &str) -> (f64, f64, f64) {
    let m = model.to_lowercase();
    let row = ANTHROPIC_PRICES
        .iter()
        .find(|(k, ..)| m.contains(k))
        .or_else(|| OPENAI_PRICES.iter().find(|(k, ..)| m.starts_with(k)));
    match row {
        Some((_, i, o, cr)) => (*i, *o, cr.unwrap_or(i * 0.10)),
        None => DEFAULT_PRICE,
    }
}

fn estimate_cost(
    model: &str,
    input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
) -> f64 {
    let (in_p, out_p, cr_p) = price_per_million(model);
    let cc_p = in_p * 1.25;
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

#[cfg(test)]
mod tests {
    use super::price_per_million;

    fn io(model: &str) -> (f64, f64) {
        let (i, o, _) = price_per_million(model);
        (i, o)
    }

    #[test]
    fn anthropic_models_match_most_specific_row() {
        assert_eq!(io("claude-fable-5-2"), (8.0, 40.0));
        assert_eq!(io("claude-fable-5-1"), (10.0, 50.0));
        assert_eq!(io("claude-opus-5-5"), (4.0, 20.0));
        assert_eq!(io("claude-opus-5"), (5.0, 25.0));
        assert_eq!(io("claude-opus-4-8"), (5.0, 25.0));
        assert_eq!(io("claude-opus-4-5-20251101"), (5.0, 25.0));
        assert_eq!(io("claude-opus-4-1-20250805"), (15.0, 75.0));
        assert_eq!(io("claude-sonnet-5-5"), (2.0, 10.0));
        assert_eq!(io("claude-sonnet-4-6"), (3.0, 15.0));
        assert_eq!(io("claude-3-5-sonnet-20241022"), (3.0, 15.0));
        assert_eq!(io("claude-haiku-5-5"), (0.10, 0.50));
        assert_eq!(io("claude-haiku-4-5"), (1.0, 5.0));
        assert_eq!(price_per_million("claude-opus-5-5").2, 0.20);
        assert_eq!(price_per_million("claude-opus-4-8").2, 0.50);
    }

    #[test]
    fn openai_models_match_most_specific_row() {
        assert_eq!(io("gpt-6.2-astra"), (8.0, 40.0));
        assert_eq!(io("gpt-6.2-sol-healthcare"), (2.0, 10.0));
        assert_eq!(io("gpt-6-luna"), (0.10, 0.50));
        assert_eq!(io("gpt-5.5-pro"), (30.0, 180.0));
        assert_eq!(io("gpt-5.4-mini-2026-03-17"), (0.75, 4.50));
        assert_eq!(io("gpt-5.2-codex"), (1.75, 14.0));
        assert_eq!(io("gpt-5-mini-2025-08-07"), (0.25, 2.0));
        assert_eq!(io("gpt-5"), (1.25, 10.0));
        assert_eq!(io("gpt-4o-mini-2024-07-18"), (0.15, 0.60));
        assert_eq!(io("gpt-4o-2024-08-06"), (2.50, 10.0));
        assert_eq!(io("o4-mini"), (1.10, 4.40));
        assert_eq!(io("o3"), (2.0, 8.0));
        assert_eq!(io("something-new"), (3.0, 15.0));
    }
}
