#![allow(dead_code)]

use anyhow::{Context, Result, anyhow};
use reqwest::{
    StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::collections::BTreeMap;

use crate::{
    client::{
        ChatMessage, ChatRequest, ChatResponse, ReasoningOptions, Role, ToolCall, ToolDefinition,
        Usage,
    },
    config::{ProviderKind, ResolvedConfig},
};

#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enable_thinking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_split: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_cache_retention: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    #[serde(flatten)]
    extra_params: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OpenAiRequestToolCall>,
}

#[derive(Debug, Serialize)]
struct OpenAiTool {
    #[serde(rename = "type")]
    tool_type: &'static str,
    function: OpenAiFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiFunction {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize)]
struct OpenAiRequestToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: &'static str,
    function: OpenAiRequestToolCallFunction,
}

#[derive(Debug, Serialize)]
struct OpenAiRequestToolCallFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiResponseMessage>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiResponseToolCall>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseToolCall {
    id: Option<String>,
    function: Option<OpenAiResponseToolCallFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponseToolCallFunction {
    name: Option<String>,
    arguments: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    prompt_cache_hit_tokens: Option<u64>,
    prompt_cache_miss_tokens: Option<u64>,
    completion_tokens_details: Option<CompletionTokenDetails>,
    prompt_tokens_details: Option<PromptTokenDetails>,
}

#[derive(Debug, Deserialize)]
struct CompletionTokenDetails {
    reasoning_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct PromptTokenDetails {
    cached_tokens: Option<u64>,
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
    let url = chat_completions_url(config);
    let client = reqwest::Client::new();
    let preferred_tool_choice = (!request.tools.is_empty()).then_some("required");
    let body = request_body(config, &request, preferred_tool_choice);

    let (mut status, mut raw) = send_chat_request(&client, &url, headers(config)?, body).await?;
    if !status.is_success()
        && preferred_tool_choice == Some("required")
        && should_retry_tool_choice_auto(status, &raw)
    {
        let fallback_body = request_body(config, &request, Some("auto"));
        (status, raw) = send_chat_request(&client, &url, headers(config)?, fallback_body).await?;
    }
    if !status.is_success() {
        return Err(anyhow!(
            "provider returned {status}: {}",
            compact_json(&raw)
        ));
    }

    let parsed: OpenAiResponse =
        serde_json::from_value(raw.clone()).context("unexpected OpenAI-compatible response")?;
    let message = parsed
        .choices
        .first()
        .and_then(|choice| {
            choice
                .message
                .as_ref()
                .and_then(|message| message.content.clone())
                .or_else(|| choice.text.clone())
        })
        .unwrap_or_default();
    let tool_calls = parsed
        .choices
        .first()
        .map(tool_calls_from_choice)
        .transpose()?
        .unwrap_or_default();

    Ok(ChatResponse {
        provider: config.provider,
        model: config.model.clone(),
        message,
        tool_calls,
        usage: parsed.usage.map_or_else(Usage::default, usage_from_openai),
        raw: Some(raw),
    })
}

async fn send_chat_request(
    client: &reqwest::Client,
    url: &str,
    headers: HeaderMap,
    body: OpenAiRequest,
) -> Result<(StatusCode, Value)> {
    let response = client
        .post(url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .context("failed to send OpenAI-compatible request")?;

    let status = response.status();
    let raw: Value = response
        .json()
        .await
        .context("failed to parse provider JSON")?;
    Ok((status, raw))
}

fn should_retry_tool_choice_auto(status: StatusCode, raw: &Value) -> bool {
    if status != StatusCode::BAD_REQUEST {
        return false;
    }
    let message = compact_json(raw).to_ascii_lowercase();
    message.contains("tool_choice")
        || message.contains("tool choice")
        || message.contains("required")
        || message.contains("unsupported")
}

pub async fn list_models(config: &ResolvedConfig) -> Result<Vec<String>> {
    let url = models_url(config);
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

fn request_body(
    config: &ResolvedConfig,
    request: &ChatRequest,
    tool_choice: Option<&'static str>,
) -> OpenAiRequest {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(OpenAiMessage {
            role: "system".to_string(),
            content: Some(system.clone()),
            tool_call_id: None,
            reasoning_content: None,
            tool_calls: Vec::new(),
        });
    }
    for message in &request.messages {
        messages.push(openai_message(config.provider, message));
    }

    let reasoning = request.reasoning.as_ref();
    let use_completion_tokens = uses_max_completion_tokens(config);
    let temperature = match (config.provider, reasoning) {
        (ProviderKind::Xiaomi, Some(options)) if options.thinking_type == "enabled" => None,
        _ => request.temperature,
    };

    OpenAiRequest {
        model: config.model.clone(),
        messages,
        tool_choice,
        tools: request.tools.iter().map(openai_tool).collect(),
        thinking: thinking_body(config.provider, reasoning),
        enable_thinking: enable_thinking(config.provider, reasoning),
        reasoning_split: (config.provider == ProviderKind::Minimax).then_some(true),
        prompt_cache_key: matches!(config.provider, ProviderKind::Openai | ProviderKind::Kimi)
            .then_some(request.cache_key.clone())
            .flatten(),
        prompt_cache_retention: (config.provider == ProviderKind::Openai)
            .then_some(request.cache_retention.clone())
            .flatten(),
        reasoning_effort: reasoning_effort_body(config.provider, reasoning).map(str::to_string),
        temperature,
        top_p: request.params.top_p,
        presence_penalty: request.params.presence_penalty,
        frequency_penalty: request.params.frequency_penalty,
        stop: request.params.stop.clone(),
        max_tokens: (!use_completion_tokens).then_some(config.max_tokens),
        max_completion_tokens: use_completion_tokens.then_some(config.max_tokens),
        extra_params: request.params.extra.clone(),
    }
}

fn openai_message(provider: ProviderKind, message: &ChatMessage) -> OpenAiMessage {
    let content = match message.role {
        Role::Assistant if !message.tool_calls.is_empty() && message.content.trim().is_empty() => {
            None
        }
        _ => Some(message.content.clone()),
    };
    let reasoning_content = (provider == ProviderKind::Kimi
        && matches!(message.role, Role::Assistant)
        && !message.tool_calls.is_empty())
    .then(String::new);

    OpenAiMessage {
        role: role_name(message).to_string(),
        content,
        tool_call_id: message.tool_call_id.clone(),
        reasoning_content,
        tool_calls: message
            .tool_calls
            .iter()
            .map(|call| OpenAiRequestToolCall {
                id: call.id.clone(),
                call_type: "function",
                function: OpenAiRequestToolCallFunction {
                    name: call.name.clone(),
                    arguments: compact_json(&call.arguments),
                },
            })
            .collect(),
    }
}

fn openai_tool(tool: &ToolDefinition) -> OpenAiTool {
    OpenAiTool {
        tool_type: "function",
        function: OpenAiFunction {
            name: tool.name.clone(),
            description: tool.description.clone(),
            parameters: tool.parameters.clone(),
        },
    }
}

fn tool_calls_from_choice(choice: &OpenAiChoice) -> Result<Vec<ToolCall>> {
    let Some(message) = &choice.message else {
        return Ok(Vec::new());
    };
    let Some(calls) = &message.tool_calls else {
        return Ok(Vec::new());
    };

    let parsed_calls = calls
        .iter()
        .enumerate()
        .filter_map(|(index, call)| {
            let function = call.function.as_ref()?;
            let name = function.name.as_ref()?.trim();
            if name.is_empty() {
                return None;
            }
            Some((index, call, name.to_string(), function.arguments.clone()))
        })
        .map(|(index, call, name, arguments)| ToolCall {
            id: call
                .id
                .clone()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("call-{index}")),
            name,
            arguments: parse_tool_arguments(arguments),
        })
        .collect::<Vec<_>>();
    Ok(parsed_calls)
}

fn parse_tool_arguments(arguments: Option<Value>) -> Value {
    match arguments.unwrap_or(Value::Object(Default::default())) {
        Value::String(text) => {
            if text.trim().is_empty() {
                Value::Object(Default::default())
            } else {
                serde_json::from_str(&text).unwrap_or_else(|_| {
                    serde_json::json!({
                        "__raw_arguments": text,
                        "__parse_error": "function.arguments was not valid JSON; retry with a valid JSON object. For large file content, use smaller append_file chunks instead of one giant argument."
                    })
                })
            }
        }
        value => value,
    }
}

fn usage_from_openai(usage: OpenAiUsage) -> Usage {
    let reasoning_tokens = usage
        .completion_tokens_details
        .and_then(|details| details.reasoning_tokens);
    let cache_hit = usage
        .prompt_cache_hit_tokens
        .or(usage.cached_tokens)
        .or_else(|| {
            usage
                .prompt_tokens_details
                .and_then(|details| details.cached_tokens)
        });
    let cache_miss = usage.prompt_cache_miss_tokens.or_else(|| {
        usage
            .prompt_tokens
            .zip(cache_hit)
            .map(|(prompt_tokens, cache_hit)| prompt_tokens.saturating_sub(cache_hit))
    });

    Usage {
        input_tokens: usage.prompt_tokens,
        output_tokens: usage.completion_tokens,
        total_tokens: usage.total_tokens,
        reasoning_tokens,
        cache_hit_tokens: cache_hit,
        cache_miss_tokens: cache_miss,
    }
}

fn role_name(message: &ChatMessage) -> &'static str {
    match message.role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

fn thinking_body(provider: ProviderKind, reasoning: Option<&ReasoningOptions>) -> Option<Value> {
    match provider {
        ProviderKind::Deepseek
        | ProviderKind::Xiaomi
        | ProviderKind::Kimi
        | ProviderKind::Zhipu => {
            reasoning.map(|options| serde_json::json!({ "type": options.thinking_type }))
        }
        ProviderKind::Openai
        | ProviderKind::Anthropic
        | ProviderKind::Minimax
        | ProviderKind::Qwen
        | ProviderKind::Stepfun => None,
    }
}

fn enable_thinking(provider: ProviderKind, reasoning: Option<&ReasoningOptions>) -> Option<bool> {
    (provider == ProviderKind::Qwen)
        .then(|| reasoning.map(|options| options.thinking_type == "enabled"))
        .flatten()
}

fn reasoning_effort_body(
    provider: ProviderKind,
    reasoning: Option<&ReasoningOptions>,
) -> Option<&str> {
    let effort = reasoning.and_then(|options| options.reasoning_effort.as_deref())?;
    match provider {
        ProviderKind::Deepseek => Some(effort),
        ProviderKind::Stepfun => Some(match effort {
            "minimal" | "low" | "none" => "low",
            "medium" => "medium",
            "high" | "max" | "xhigh" => "high",
            _ => "medium",
        }),
        ProviderKind::Openai => Some(match effort {
            "max" => "xhigh",
            other => other,
        }),
        ProviderKind::Anthropic
        | ProviderKind::Xiaomi
        | ProviderKind::Kimi
        | ProviderKind::Zhipu
        | ProviderKind::Minimax
        | ProviderKind::Qwen => None,
    }
}

fn headers(config: &ResolvedConfig) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let bearer = HeaderValue::from_str(&format!("Bearer {}", config.api_key))
        .context("invalid API key for Authorization header")?;
    headers.insert(AUTHORIZATION, bearer);
    if config.provider == ProviderKind::Xiaomi {
        headers.insert(
            HeaderName::from_static("api-key"),
            HeaderValue::from_str(&config.api_key).context("invalid API key for api-key header")?,
        );
    }
    Ok(headers)
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

pub fn chat_completions_url(config: &ResolvedConfig) -> String {
    let base = config.base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else if config.provider == ProviderKind::Deepseek
        || base.ends_with("/v1")
        || base.ends_with("/v4")
    {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

pub fn models_url(config: &ResolvedConfig) -> String {
    let base = config.base_url.trim_end_matches('/');
    if let Some(prefix) = base.strip_suffix("/chat/completions") {
        format!("{prefix}/models")
    } else if config.provider == ProviderKind::Deepseek
        || base.ends_with("/v1")
        || base.ends_with("/v4")
    {
        format!("{base}/models")
    } else {
        format!("{base}/v1/models")
    }
}

pub fn requires_max_completion_tokens(model: &str) -> bool {
    let model = model.to_ascii_lowercase();
    model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
        || model.starts_with("gpt-5")
        || model.starts_with("mimo-")
        || model.starts_with("kimi-")
        || model.starts_with("minimax-")
}

fn uses_max_completion_tokens(config: &ResolvedConfig) -> bool {
    matches!(
        config.provider,
        ProviderKind::Xiaomi | ProviderKind::Kimi | ProviderKind::Minimax
    ) || requires_max_completion_tokens(&config.model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_chat_urls() {
        let config = test_config(ProviderKind::Openai, "https://api.example.com");
        assert_eq!(
            chat_completions_url(&config),
            "https://api.example.com/v1/chat/completions"
        );
        let config = test_config(ProviderKind::Openai, "https://api.example.com/v1");
        assert_eq!(
            chat_completions_url(&config),
            "https://api.example.com/v1/chat/completions"
        );
        let config = test_config(
            ProviderKind::Openai,
            "https://api.example.com/v1/chat/completions",
        );
        assert_eq!(
            chat_completions_url(&config),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalizes_models_urls() {
        let config = test_config(ProviderKind::Openai, "https://api.example.com");
        assert_eq!(models_url(&config), "https://api.example.com/v1/models");
        let config = test_config(ProviderKind::Openai, "https://api.example.com/v1");
        assert_eq!(models_url(&config), "https://api.example.com/v1/models");
        let config = test_config(
            ProviderKind::Openai,
            "https://api.example.com/v1/chat/completions",
        );
        assert_eq!(models_url(&config), "https://api.example.com/v1/models");
    }

    #[test]
    fn normalizes_deepseek_urls() {
        let config = test_config(ProviderKind::Deepseek, "https://api.deepseek.com");
        assert_eq!(
            chat_completions_url(&config),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(models_url(&config), "https://api.deepseek.com/models");

        let beta = test_config(ProviderKind::Deepseek, "https://api.deepseek.com/beta");
        assert_eq!(
            chat_completions_url(&beta),
            "https://api.deepseek.com/beta/chat/completions"
        );
    }

    #[test]
    fn normalizes_xiaomi_urls() {
        let config = test_config(ProviderKind::Xiaomi, "https://api.xiaomimimo.com");
        assert_eq!(
            chat_completions_url(&config),
            "https://api.xiaomimimo.com/v1/chat/completions"
        );
        assert_eq!(models_url(&config), "https://api.xiaomimimo.com/v1/models");

        let v1 = test_config(ProviderKind::Xiaomi, "https://api.xiaomimimo.com/v1");
        assert_eq!(
            chat_completions_url(&v1),
            "https://api.xiaomimimo.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalizes_openai_compatible_provider_urls() {
        let kimi = test_config(ProviderKind::Kimi, "https://api.moonshot.ai/v1");
        assert_eq!(
            chat_completions_url(&kimi),
            "https://api.moonshot.ai/v1/chat/completions"
        );

        let zhipu = test_config(ProviderKind::Zhipu, "https://open.bigmodel.cn/api/paas/v4");
        assert_eq!(
            chat_completions_url(&zhipu),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            models_url(&zhipu),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );

        let minimax = test_config(ProviderKind::Minimax, "https://api.minimax.io/v1");
        assert_eq!(
            chat_completions_url(&minimax),
            "https://api.minimax.io/v1/chat/completions"
        );

        let qwen = test_config(
            ProviderKind::Qwen,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
        );
        assert_eq!(
            chat_completions_url(&qwen),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );

        let stepfun = test_config(ProviderKind::Stepfun, "https://api.stepfun.com/v1");
        assert_eq!(
            chat_completions_url(&stepfun),
            "https://api.stepfun.com/v1/chat/completions"
        );
    }

    #[test]
    fn provider_specific_request_body_fields() {
        let openai_config =
            test_config_with_model(ProviderKind::Openai, "https://api.openai.com/v1", "gpt-5.2");
        let openai_body = serde_json::to_value(request_body(
            &openai_config,
            &test_request(
                Some(ReasoningOptions {
                    thinking_type: "enabled".to_string(),
                    reasoning_effort: Some("max".to_string()),
                }),
                Some("openai-cache-key".to_string()),
            ),
            None,
        ))
        .unwrap();
        assert_eq!(openai_body["reasoning_effort"], "xhigh");
        assert_eq!(openai_body["prompt_cache_key"], "openai-cache-key");
        assert!(openai_body.get("prompt_cache_retention").is_none());

        let openai_extended_cache_body = serde_json::to_value(request_body(
            &openai_config,
            &test_request_with_cache_retention(
                None,
                Some("openai-cache-key".to_string()),
                Some("24h".to_string()),
            ),
            None,
        ))
        .unwrap();
        assert_eq!(openai_extended_cache_body["prompt_cache_retention"], "24h");

        let openai_disabled_body = serde_json::to_value(request_body(
            &openai_config,
            &test_request(
                Some(ReasoningOptions {
                    thinking_type: "disabled".to_string(),
                    reasoning_effort: Some("none".to_string()),
                }),
                None,
            ),
            None,
        ))
        .unwrap();
        assert_eq!(openai_disabled_body["reasoning_effort"], "none");

        let kimi_config = test_config_with_model(
            ProviderKind::Kimi,
            "https://api.moonshot.ai/v1",
            "kimi-k2.6",
        );
        let kimi_body = serde_json::to_value(request_body(
            &kimi_config,
            &test_request(
                Some(ReasoningOptions {
                    thinking_type: "enabled".to_string(),
                    reasoning_effort: None,
                }),
                Some("session-a".to_string()),
            ),
            None,
        ))
        .unwrap();
        assert_eq!(kimi_body["max_completion_tokens"], 16);
        assert_eq!(kimi_body["thinking"]["type"], "enabled");
        assert_eq!(kimi_body["prompt_cache_key"], "session-a");
        assert!(kimi_body.get("max_tokens").is_none());

        let zhipu_config = test_config_with_model(
            ProviderKind::Zhipu,
            "https://open.bigmodel.cn/api/paas/v4",
            "glm-5.1",
        );
        let zhipu_body = serde_json::to_value(request_body(
            &zhipu_config,
            &test_request(
                Some(ReasoningOptions {
                    thinking_type: "disabled".to_string(),
                    reasoning_effort: None,
                }),
                None,
            ),
            None,
        ))
        .unwrap();
        assert_eq!(zhipu_body["max_tokens"], 16);
        assert_eq!(zhipu_body["thinking"]["type"], "disabled");

        let minimax_config = test_config_with_model(
            ProviderKind::Minimax,
            "https://api.minimax.io/v1",
            "MiniMax-M2.7",
        );
        let minimax_body = serde_json::to_value(request_body(
            &minimax_config,
            &test_request(None, None),
            None,
        ))
        .unwrap();
        assert_eq!(minimax_body["max_completion_tokens"], 16);
        assert_eq!(minimax_body["reasoning_split"], true);
        assert!(minimax_body.get("max_tokens").is_none());

        let qwen_config = test_config_with_model(
            ProviderKind::Qwen,
            "https://dashscope.aliyuncs.com/compatible-mode/v1",
            "qwen3.6-plus",
        );
        let qwen_body = serde_json::to_value(request_body(
            &qwen_config,
            &test_request(
                Some(ReasoningOptions {
                    thinking_type: "enabled".to_string(),
                    reasoning_effort: None,
                }),
                None,
            ),
            None,
        ))
        .unwrap();
        assert_eq!(qwen_body["max_tokens"], 16);
        assert_eq!(qwen_body["enable_thinking"], true);
        assert!(qwen_body.get("thinking").is_none());

        let stepfun_config = test_config_with_model(
            ProviderKind::Stepfun,
            "https://api.stepfun.com/v1",
            "step-3.7-flash",
        );
        let stepfun_body = serde_json::to_value(request_body(
            &stepfun_config,
            &test_request(
                Some(ReasoningOptions {
                    thinking_type: "enabled".to_string(),
                    reasoning_effort: Some("xhigh".to_string()),
                }),
                None,
            ),
            None,
        ))
        .unwrap();
        assert_eq!(stepfun_body["max_tokens"], 16);
        assert_eq!(stepfun_body["reasoning_effort"], "high");
        assert!(stepfun_body.get("thinking").is_none());
        assert!(stepfun_body.get("enable_thinking").is_none());
    }

    #[test]
    fn serializes_native_tool_definitions_and_history() {
        let config = test_config(ProviderKind::Deepseek, "https://api.deepseek.com");
        let request = ChatRequest {
            system: None,
            messages: vec![
                ChatMessage::assistant_with_tool_calls(
                    "",
                    vec![ToolCall {
                        id: "call-1".to_string(),
                        name: "git_status".to_string(),
                        arguments: serde_json::json!({}),
                    }],
                ),
                ChatMessage::tool("call-1", r#"{"ok":true}"#),
            ],
            tools: vec![ToolDefinition {
                name: "git_status".to_string(),
                description: "Return git status".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": [],
                    "additionalProperties": false
                }),
            }],
            temperature: None,
            params: Default::default(),
            reasoning: None,
            cache_key: None,
            cache_retention: None,
        };

        let body = serde_json::to_value(request_body(&config, &request, Some("required"))).unwrap();
        assert_eq!(body["tool_choice"], "required");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "git_status");
        assert_eq!(body["messages"][0]["role"], "assistant");
        assert!(body["messages"][0].get("content").is_none());
        assert!(body["messages"][0].get("reasoning_content").is_none());
        assert_eq!(body["messages"][0]["tool_calls"][0]["id"], "call-1");
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
        assert_eq!(body["messages"][1]["role"], "tool");
        assert_eq!(body["messages"][1]["tool_call_id"], "call-1");

        let kimi_config = test_config_with_model(
            ProviderKind::Kimi,
            "https://api.moonshot.cn/v1",
            "kimi-k2.6",
        );
        let kimi_body =
            serde_json::to_value(request_body(&kimi_config, &request, Some("required"))).unwrap();
        assert_eq!(kimi_body["messages"][0]["reasoning_content"], "");
    }

    #[test]
    fn parses_native_tool_calls_from_response() {
        let parsed: OpenAiResponse = serde_json::from_value(serde_json::json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": "{\"path\":\"src/main.rs\",\"limit\":20}"
                        }
                    }]
                }
            }]
        }))
        .unwrap();

        let calls = tool_calls_from_choice(parsed.choices.first().unwrap()).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call-1");
        assert_eq!(calls[0].name, "read_file");
        assert_eq!(calls[0].arguments["path"], "src/main.rs");
        assert_eq!(calls[0].arguments["limit"], 20);
    }

    #[test]
    fn keeps_malformed_tool_arguments_as_parse_error_payload() {
        let parsed: OpenAiResponse = serde_json::from_value(serde_json::json!({
            "choices": [{
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "write_file",
                            "arguments": "{\"path\":\"index.html\",\"content\":\"line one\nline two\"}"
                        }
                    }]
                }
            }]
        }))
        .unwrap();

        let calls = tool_calls_from_choice(parsed.choices.first().unwrap()).unwrap();
        assert_eq!(calls[0].name, "write_file");
        assert!(calls[0].arguments["__raw_arguments"].is_string());
        assert!(calls[0].arguments["__parse_error"].is_string());
    }

    #[test]
    fn detects_reasoning_token_param() {
        assert!(requires_max_completion_tokens("gpt-5-mini"));
        assert!(requires_max_completion_tokens("o3"));
        assert!(requires_max_completion_tokens("mimo-v2.5-pro"));
        assert!(requires_max_completion_tokens("kimi-k2.6"));
        assert!(requires_max_completion_tokens("MiniMax-M2.7"));
        assert!(requires_max_completion_tokens("MiniMax-M3"));
        assert!(!requires_max_completion_tokens("gpt-4o-mini"));
    }

    #[test]
    fn merges_sampling_and_extra_request_params() {
        let config =
            test_config_with_model(ProviderKind::Qwen, "https://example.com/v1", "qwen3.7-max");
        let mut request = test_request(None, None);
        request.params.top_p = Some(0.8);
        request.params.presence_penalty = Some(0.1);
        request.params.frequency_penalty = Some(0.2);
        request.params.stop = vec!["END".to_string()];
        request.params.extra.insert(
            "response_format".to_string(),
            serde_json::json!({"type": "json_object"}),
        );

        let body = serde_json::to_value(request_body(&config, &request, None)).unwrap();

        assert_float_eq(&body["top_p"], 0.8);
        assert_float_eq(&body["presence_penalty"], 0.1);
        assert_float_eq(&body["frequency_penalty"], 0.2);
        assert_eq!(body["stop"][0], "END");
        assert_eq!(body["response_format"]["type"], "json_object");
    }

    fn assert_float_eq(value: &serde_json::Value, expected: f64) {
        let actual = value.as_f64().unwrap();
        assert!((actual - expected).abs() < 0.000_001);
    }

    fn test_config(provider: ProviderKind, base_url: &str) -> ResolvedConfig {
        test_config_with_model(provider, base_url, "model")
    }

    fn test_config_with_model(
        provider: ProviderKind,
        base_url: &str,
        model: &str,
    ) -> ResolvedConfig {
        ResolvedConfig {
            provider,
            base_url: base_url.to_string(),
            api_key: "test".to_string(),
            model: model.to_string(),
            max_tokens: 16,
        }
    }

    fn test_request(reasoning: Option<ReasoningOptions>, cache_key: Option<String>) -> ChatRequest {
        test_request_with_cache_retention(reasoning, cache_key, None)
    }

    fn test_request_with_cache_retention(
        reasoning: Option<ReasoningOptions>,
        cache_key: Option<String>,
        cache_retention: Option<String>,
    ) -> ChatRequest {
        ChatRequest {
            system: None,
            messages: vec![ChatMessage::user("hello")],
            tools: Vec::new(),
            temperature: None,
            params: Default::default(),
            reasoning,
            cache_key,
            cache_retention,
        }
    }
}
