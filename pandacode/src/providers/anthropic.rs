#![allow(dead_code)]

use anyhow::{Context, Result, anyhow};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    client::{ChatMessage, ChatRequest, ChatResponse, ReasoningOptions, Role, Usage},
    config::{ProviderKind, ResolvedConfig},
};

const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    messages: Vec<AnthropicMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<AnthropicRequestContentBlock<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking<'a>>,
}

#[derive(Debug, Serialize)]
struct AnthropicThinking<'a> {
    #[serde(rename = "type")]
    thinking_type: &'a str,
    budget_tokens: u32,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage<'a> {
    role: &'a str,
    content: Vec<AnthropicRequestContentBlock<'a>>,
}

#[derive(Debug, Serialize)]
struct AnthropicRequestContentBlock<'a> {
    #[serde(rename = "type")]
    block_type: &'static str,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_control: Option<AnthropicCacheControl>,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct AnthropicCacheControl {
    #[serde(rename = "type")]
    cache_type: &'static str,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContentBlock>,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelInfo>,
}

#[derive(Debug, Deserialize)]
struct ModelInfo {
    id: String,
}

pub async fn complete(config: &ResolvedConfig, request: ChatRequest) -> Result<ChatResponse> {
    let url = messages_url(&config.base_url);
    let client = reqwest::Client::new();
    let body = request_body(config, &request);

    let response = client
        .post(url)
        .headers(headers(config)?)
        .json(&body)
        .send()
        .await
        .context("failed to send Anthropic-compatible request")?;

    let status = response.status();
    let raw: Value = response
        .json()
        .await
        .context("failed to parse provider JSON")?;
    if !status.is_success() {
        return Err(anyhow!(
            "provider returned {status}: {}",
            compact_json(&raw)
        ));
    }

    let parsed: AnthropicResponse =
        serde_json::from_value(raw.clone()).context("unexpected Anthropic response")?;
    let message = parsed
        .content
        .iter()
        .filter(|block| block.block_type == "text")
        .filter_map(|block| block.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    Ok(ChatResponse {
        provider: ProviderKind::Anthropic,
        model: config.model.clone(),
        message,
        tool_calls: Vec::new(),
        usage: parsed.usage.map_or_else(Usage::default, |usage| Usage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage
                .input_tokens
                .zip(usage.output_tokens)
                .map(|(a, b)| a + b),
            reasoning_tokens: None,
            cache_hit_tokens: usage.cache_read_input_tokens,
            cache_miss_tokens: usage.cache_creation_input_tokens.or_else(|| {
                usage
                    .input_tokens
                    .zip(usage.cache_read_input_tokens)
                    .map(|(input_tokens, cache_hit)| input_tokens.saturating_sub(cache_hit))
            }),
        }),
        raw: Some(raw),
    })
}

pub async fn list_models(config: &ResolvedConfig) -> Result<Vec<String>> {
    let url = models_url(&config.base_url);
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .headers(headers(config)?)
        .send()
        .await
        .context("failed to send models request")?;

    let status = response.status();
    let raw: Value = response
        .json()
        .await
        .context("failed to parse provider JSON")?;
    if !status.is_success() {
        return Err(anyhow!(
            "provider returned {status}: {}",
            compact_json(&raw)
        ));
    }

    let parsed: ModelsResponse =
        serde_json::from_value(raw).context("unexpected models response shape")?;
    Ok(parsed.data.into_iter().map(|model| model.id).collect())
}

fn request_body<'a>(config: &'a ResolvedConfig, request: &'a ChatRequest) -> AnthropicRequest<'a> {
    let cacheable = request
        .cache_key
        .as_deref()
        .is_some_and(|key| !key.trim().is_empty());

    AnthropicRequest {
        model: &config.model,
        max_tokens: config.max_tokens,
        messages: request
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| AnthropicMessage {
                role: role_name(message),
                content: text_blocks(&message.content, cacheable && index == 0),
            })
            .collect(),
        system: request
            .system
            .as_deref()
            .map(|system| text_blocks(system, cacheable)),
        temperature: request.temperature,
        top_p: request.params.top_p,
        stop_sequences: request.params.stop.clone(),
        thinking: thinking_body(config.max_tokens, request.reasoning.as_ref()),
    }
}

fn text_blocks(text: &str, cacheable: bool) -> Vec<AnthropicRequestContentBlock<'_>> {
    vec![AnthropicRequestContentBlock {
        block_type: "text",
        text,
        cache_control: cacheable.then_some(AnthropicCacheControl {
            cache_type: "ephemeral",
        }),
    }]
}

fn role_name(message: &ChatMessage) -> &'static str {
    match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "user",
    }
}

fn headers(config: &ResolvedConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        HeaderName::from_static("x-api-key"),
        HeaderValue::from_str(&config.api_key).context("invalid API key for x-api-key header")?,
    );
    headers.insert(
        HeaderName::from_static("anthropic-version"),
        HeaderValue::from_static(ANTHROPIC_VERSION),
    );
    Ok(headers)
}

fn thinking_body(
    max_tokens: u32,
    reasoning: Option<&ReasoningOptions>,
) -> Option<AnthropicThinking<'static>> {
    let reasoning = reasoning?;
    if reasoning.thinking_type != "enabled" {
        return None;
    }

    Some(AnthropicThinking {
        thinking_type: "enabled",
        budget_tokens: thinking_budget(max_tokens, reasoning.reasoning_effort.as_deref()),
    })
}

fn thinking_budget(max_tokens: u32, effort: Option<&str>) -> u32 {
    let requested = match effort {
        Some("minimal" | "low") | None => 1_024,
        Some("medium") => 2_048,
        Some("high") => 4_096,
        Some("max" | "xhigh") => 8_192,
        Some(_) => 2_048,
    };

    if max_tokens > 1_024 {
        requested.min(max_tokens.saturating_sub(1)).max(1_024)
    } else {
        1_024
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

pub fn messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/messages") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

pub fn models_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if let Some(prefix) = base.strip_suffix("/messages") {
        format!("{prefix}/models")
    } else if base.ends_with("/v1") {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_messages_urls() {
        assert_eq!(
            messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://proxy.example.com/v1/messages"),
            "https://proxy.example.com/v1/messages"
        );
    }

    #[test]
    fn normalizes_models_urls() {
        assert_eq!(
            models_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_url("https://api.anthropic.com/v1"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            models_url("https://proxy.example.com/v1/messages"),
            "https://proxy.example.com/v1/models"
        );
    }

    #[test]
    fn maps_extended_thinking_budget() {
        let config = ResolvedConfig {
            provider: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "test".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
        };
        let body = serde_json::to_value(request_body(
            &config,
            &ChatRequest {
                system: None,
                messages: vec![ChatMessage::user("hello")],
                tools: Vec::new(),
                temperature: None,
                params: Default::default(),
                reasoning: Some(ReasoningOptions {
                    thinking_type: "enabled".to_string(),
                    reasoning_effort: Some("high".to_string()),
                }),
                cache_key: None,
                cache_retention: None,
            },
        ))
        .unwrap();

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 4095);
    }

    #[test]
    fn marks_stable_anthropic_blocks_for_prompt_caching() {
        let config = ResolvedConfig {
            provider: ProviderKind::Anthropic,
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "test".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            max_tokens: 4096,
        };
        let body = serde_json::to_value(request_body(
            &config,
            &ChatRequest {
                system: Some("stable tool protocol".to_string()),
                messages: vec![
                    ChatMessage::user("stable repo context"),
                    ChatMessage::user("volatile task"),
                ],
                tools: Vec::new(),
                temperature: None,
                params: Default::default(),
                reasoning: None,
                cache_key: Some("stable-key".to_string()),
                cache_retention: None,
            },
        ))
        .unwrap();

        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(
            body["messages"][0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
        assert!(
            body["messages"][1]["content"][0]
                .get("cache_control")
                .is_none()
        );
    }
}
