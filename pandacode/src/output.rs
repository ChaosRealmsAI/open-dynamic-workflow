use anyhow::Result;

use crate::{cli::OutputFormat, client::ChatResponse};

#[allow(dead_code)]
pub fn print_response(response: &ChatResponse, format: &OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Text => {
            println!("{}", response.message);
        }
        OutputFormat::Json => {
            let value = serde_json::json!({
                "provider": response.provider,
                "model": response.model,
                "message": response.message,
                "usage": response.usage,
            });
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        OutputFormat::Raw => {
            if let Some(raw) = &response.raw {
                println!("{}", serde_json::to_string_pretty(raw)?);
            } else {
                println!("{}", serde_json::to_string_pretty(response)?);
            }
        }
    }
    Ok(())
}

pub fn print_cache_report(response: &ChatResponse) {
    let usage = &response.usage;
    let input = format_optional(usage.input_tokens);
    let output = format_optional(usage.output_tokens);
    let total = format_optional(usage.total_tokens);
    let cache_hit = format_optional(usage.cache_hit_tokens);
    let cache_miss = format_optional(usage.cache_miss_tokens);
    let reasoning = format_optional(usage.reasoning_tokens);
    let hit_rate = cache_hit_rate(usage.cache_hit_tokens, usage.cache_miss_tokens)
        .map(|rate| format!("{:.1}%", rate * 100.0))
        .unwrap_or_else(|| "n/a".to_string());

    eprintln!(
        "usage provider={} model={} input={} output={} reasoning={} total={} cache_hit={} cache_miss={} cache_hit_rate={}",
        response.provider,
        response.model,
        input,
        output,
        reasoning,
        total,
        cache_hit,
        cache_miss,
        hit_rate
    );
}

fn format_optional(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn cache_hit_rate(cache_hit: Option<u64>, cache_miss: Option<u64>) -> Option<f64> {
    let hit = cache_hit?;
    let miss = cache_miss?;
    let total = hit + miss;
    (total > 0).then_some(hit as f64 / total as f64)
}
