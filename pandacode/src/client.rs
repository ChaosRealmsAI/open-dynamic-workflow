use std::collections::BTreeMap;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    config::{ProviderKind, ResolvedConfig},
    providers::{anthropic, openai},
};

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls: Vec::new(),
        }
    }

    pub fn assistant_with_tool_calls(
        content: impl Into<String>,
        tool_calls: Vec<ToolCall>,
    ) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call_id: None,
            tool_calls,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolDefinition>,
    pub temperature: Option<f32>,
    pub params: RequestParams,
    pub reasoning: Option<ReasoningOptions>,
    pub cache_key: Option<String>,
    pub cache_retention: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RequestParams {
    pub top_p: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub stop: Vec<String>,
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReasoningOptions {
    pub thinking_type: String,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_hit_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_miss_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatResponse {
    pub provider: ProviderKind,
    pub model: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

pub async fn complete(config: &ResolvedConfig, request: ChatRequest) -> Result<ChatResponse> {
    match config.provider {
        ProviderKind::Openai
        | ProviderKind::Deepseek
        | ProviderKind::Xiaomi
        | ProviderKind::Kimi
        | ProviderKind::Zhipu
        | ProviderKind::Minimax
        | ProviderKind::Qwen
        | ProviderKind::Stepfun => openai::complete(config, request).await,
        ProviderKind::Anthropic => anthropic::complete(config, request).await,
    }
}

#[allow(dead_code)]
pub async fn list_models(config: &ResolvedConfig) -> Result<Vec<String>> {
    match config.provider {
        ProviderKind::Openai
        | ProviderKind::Deepseek
        | ProviderKind::Xiaomi
        | ProviderKind::Kimi
        | ProviderKind::Zhipu
        | ProviderKind::Minimax
        | ProviderKind::Qwen
        | ProviderKind::Stepfun => openai::list_models(config).await,
        ProviderKind::Anthropic => anthropic::list_models(config).await,
    }
}
