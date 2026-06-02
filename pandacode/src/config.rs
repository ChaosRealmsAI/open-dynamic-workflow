use std::{env, fmt, path::PathBuf, str::FromStr};

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::{cli::ProviderOverrides, models};

const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_DEEPSEEK_BASE_URL: &str = "https://api.deepseek.com";
const DEFAULT_XIAOMI_BASE_URL: &str = "https://api.xiaomimimo.com/v1";
const DEFAULT_KIMI_BASE_URL: &str = "https://api.moonshot.cn/v1";
const DEFAULT_ZHIPU_BASE_URL: &str = "https://open.bigmodel.cn/api/paas/v4";
const DEFAULT_MINIMAX_BASE_URL: &str = "https://api.minimaxi.com/v1";
const DEFAULT_QWEN_BASE_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
const DEFAULT_STEPFUN_BASE_URL: &str = "https://api.stepfun.com/v1";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-3-5-haiku-latest";
const DEFAULT_DEEPSEEK_MODEL: &str = "deepseek-v4-pro";
const DEFAULT_XIAOMI_MODEL: &str = "mimo-v2.5-pro";
const DEFAULT_KIMI_MODEL: &str = "kimi-k2.6";
const DEFAULT_ZHIPU_MODEL: &str = "glm-5.1";
const DEFAULT_MINIMAX_MODEL: &str = "MiniMax-M3";
const DEFAULT_QWEN_MODEL: &str = "qwen3.7-max";
const DEFAULT_STEPFUN_MODEL: &str = "step-3.7-flash";
const DEFAULT_CODING_MAX_TOKENS: u32 = 16_384;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Openai,
    Anthropic,
    Deepseek,
    Xiaomi,
    Kimi,
    Zhipu,
    Minimax,
    Qwen,
    Stepfun,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderKind::Openai => write!(f, "openai"),
            ProviderKind::Anthropic => write!(f, "anthropic"),
            ProviderKind::Deepseek => write!(f, "deepseek"),
            ProviderKind::Xiaomi => write!(f, "xiaomi"),
            ProviderKind::Kimi => write!(f, "kimi"),
            ProviderKind::Zhipu => write!(f, "zhipu"),
            ProviderKind::Minimax => write!(f, "minimax"),
            ProviderKind::Qwen => write!(f, "qwen"),
            ProviderKind::Stepfun => write!(f, "stepfun"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "openai-compatible" | "chat-completions" => Ok(Self::Openai),
            "anthropic" | "anthropic-compatible" | "messages" | "claude" => Ok(Self::Anthropic),
            "deepseek" | "deepseek-v4" | "deepseek-v4-pro" | "deepseek-v4-flash" => {
                Ok(Self::Deepseek)
            }
            "xiaomi" | "mimo" | "xiaomi-mimo" | "mimo-v2.5" | "mimo-v2.5-pro" | "mimo-v2-flash" => {
                Ok(Self::Xiaomi)
            }
            "kimi" | "moonshot" | "moonshot-ai" | "kimi-k2.6" | "kimi-k2.5"
            | "kimi-k2-thinking" => Ok(Self::Kimi),
            "zhipu" | "bigmodel" | "glm" | "chatglm" | "glm-5.1" | "glm-5" | "glm-5-turbo"
            | "glm-4.7" => Ok(Self::Zhipu),
            "minimax" | "minimaxi" | "minimax-m3" | "m3" | "minimax-m2.7" | "m2.7" => {
                Ok(Self::Minimax)
            }
            "qwen" | "dashscope" | "aliyun" | "alibaba" | "bailian" | "tongyi" | "qwen3.7-max"
            | "qwen3.6-plus" | "qwen3.6-flash" => Ok(Self::Qwen),
            "stepfun" | "step" | "stepai" | "step-ai" | "step-3.7" | "step-3.7-flash"
            | "jieyue" | "jieyuexingchen" => Ok(Self::Stepfun),
            other => Err(anyhow!("unsupported provider: {other}")),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileConfig {
    pub provider: Option<ProviderKind>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub model: Option<String>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedConfig {
    pub provider: ProviderKind,
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
}

impl ResolvedConfig {
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.provider,
            "base_url": self.base_url,
            "api_key": if self.api_key.is_empty() { serde_json::Value::Null } else { serde_json::json!("***") },
            "model": self.model,
            "max_tokens": self.max_tokens,
        })
    }
}

pub fn config_dir() -> Result<PathBuf> {
    if let Ok(dir) = env::var("PANDACODE_BAMBOO_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(dir) = env::var("BAMBOO_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }

    dirs::home_dir()
        .map(|home| home.join(".pandacode").join("bamboo"))
        .ok_or_else(|| anyhow!("could not determine home directory"))
}

pub fn config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("config.toml"))
}

pub fn load_file_config() -> Result<FileConfig> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(FileConfig::default());
    }

    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

#[allow(dead_code)]
pub fn save_file_config(config: &FileConfig) -> Result<PathBuf> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw = toml::to_string_pretty(config).context("failed to serialize config")?;
    std::fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

pub fn resolve(overrides: &ProviderOverrides, max_tokens: Option<u32>) -> Result<ResolvedConfig> {
    resolve_inner(overrides, max_tokens, true)
}

pub fn resolve_partial(
    overrides: &ProviderOverrides,
    max_tokens: Option<u32>,
) -> Result<ResolvedConfig> {
    resolve_inner(overrides, max_tokens, false)
}

fn resolve_inner(
    overrides: &ProviderOverrides,
    max_tokens: Option<u32>,
    require_api_key: bool,
) -> Result<ResolvedConfig> {
    let file = load_file_config()?;

    let provider = overrides
        .provider
        .or_else(env_provider)
        .or(file.provider)
        .unwrap_or(ProviderKind::Openai);

    let base_url = overrides
        .base_url
        .clone()
        .or_else(|| env_var("PANDACODE_BAMBOO_BASE_URL"))
        .or_else(|| env_var("BAMBOO_BASE_URL"))
        .or_else(|| match provider {
            ProviderKind::Openai => env_var("OPENAI_BASE_URL"),
            ProviderKind::Anthropic => env_var("ANTHROPIC_BASE_URL"),
            ProviderKind::Deepseek => env_var("DEEPSEEK_BASE_URL"),
            ProviderKind::Xiaomi => env_var("XIAOMI_BASE_URL").or_else(|| env_var("MIMO_BASE_URL")),
            ProviderKind::Kimi => env_var("KIMI_BASE_URL").or_else(|| env_var("MOONSHOT_BASE_URL")),
            ProviderKind::Zhipu => env_var("ZHIPU_BASE_URL")
                .or_else(|| env_var("BIGMODEL_BASE_URL"))
                .or_else(|| env_var("GLM_BASE_URL")),
            ProviderKind::Minimax => {
                env_var("MINIMAX_BASE_URL").or_else(|| env_var("MINIMAXI_BASE_URL"))
            }
            ProviderKind::Qwen => env_var("QWEN_BASE_URL")
                .or_else(|| env_var("DASHSCOPE_BASE_URL"))
                .or_else(|| env_var("BAILIAN_BASE_URL")),
            ProviderKind::Stepfun => env_var("STEPFUN_BASE_URL")
                .or_else(|| env_var("STEP_BASE_URL"))
                .or_else(|| env_var("STEP_PLAN_BASE_URL")),
        })
        .or(file.base_url)
        .unwrap_or_else(|| default_base_url(provider).to_string());

    let api_key = overrides
        .api_key
        .clone()
        .or_else(|| env_var("PANDACODE_BAMBOO_API_KEY"))
        .or_else(|| env_var("BAMBOO_API_KEY"))
        .or_else(|| match provider {
            ProviderKind::Openai => env_var("OPENAI_API_KEY"),
            ProviderKind::Anthropic => env_var("ANTHROPIC_API_KEY"),
            ProviderKind::Deepseek => env_var("DEEPSEEK_API_KEY"),
            ProviderKind::Xiaomi => env_var("XIAOMI_API_KEY").or_else(|| env_var("MIMO_API_KEY")),
            ProviderKind::Kimi => env_var("KIMI_API_KEY").or_else(|| env_var("MOONSHOT_API_KEY")),
            ProviderKind::Zhipu => env_var("ZHIPU_API_KEY")
                .or_else(|| env_var("BIGMODEL_API_KEY"))
                .or_else(|| env_var("GLM_API_KEY")),
            ProviderKind::Minimax => {
                env_var("MINIMAX_API_KEY").or_else(|| env_var("MINIMAXI_API_KEY"))
            }
            ProviderKind::Qwen => env_var("QWEN_API_KEY")
                .or_else(|| env_var("DASHSCOPE_API_KEY"))
                .or_else(|| env_var("BAILIAN_API_KEY"))
                .or_else(|| env_var("ALIBABA_API_KEY")),
            ProviderKind::Stepfun => env_var("STEPFUN_API_KEY")
                .or_else(|| env_var("STEP_API_KEY"))
                .or_else(|| env_var("STEP_PLAN_API_KEY")),
        })
        .or(file.api_key)
        .unwrap_or_default();

    if require_api_key && api_key.trim().is_empty() {
        return Err(anyhow!(
            "missing API key; set PANDACODE_BAMBOO_API_KEY, BAMBOO_API_KEY, or a provider-specific API key"
        ));
    }

    let model = overrides
        .model
        .clone()
        .or_else(|| env_var("PANDACODE_BAMBOO_MODEL"))
        .or_else(|| env_var("BAMBOO_MODEL"))
        .or_else(|| match provider {
            ProviderKind::Openai => env_var("OPENAI_MODEL"),
            ProviderKind::Anthropic => env_var("ANTHROPIC_MODEL"),
            ProviderKind::Deepseek => env_var("DEEPSEEK_MODEL"),
            ProviderKind::Xiaomi => env_var("XIAOMI_MODEL").or_else(|| env_var("MIMO_MODEL")),
            ProviderKind::Kimi => env_var("KIMI_MODEL").or_else(|| env_var("MOONSHOT_MODEL")),
            ProviderKind::Zhipu => env_var("ZHIPU_MODEL")
                .or_else(|| env_var("BIGMODEL_MODEL"))
                .or_else(|| env_var("GLM_MODEL")),
            ProviderKind::Minimax => env_var("MINIMAX_MODEL").or_else(|| env_var("MINIMAXI_MODEL")),
            ProviderKind::Qwen => env_var("QWEN_MODEL")
                .or_else(|| env_var("DASHSCOPE_MODEL"))
                .or_else(|| env_var("BAILIAN_MODEL")),
            ProviderKind::Stepfun => env_var("STEPFUN_MODEL")
                .or_else(|| env_var("STEP_MODEL"))
                .or_else(|| env_var("STEP_PLAN_MODEL")),
        })
        .or(file.model)
        .unwrap_or_else(|| default_model(provider).to_string());

    let resolved_max_tokens = max_tokens
        .or(file.max_tokens)
        .unwrap_or_else(|| default_max_tokens(provider, &model));

    Ok(ResolvedConfig {
        provider,
        base_url,
        api_key,
        model,
        max_tokens: resolved_max_tokens,
    })
}

pub fn default_base_url(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Openai => DEFAULT_OPENAI_BASE_URL,
        ProviderKind::Anthropic => DEFAULT_ANTHROPIC_BASE_URL,
        ProviderKind::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
        ProviderKind::Xiaomi => DEFAULT_XIAOMI_BASE_URL,
        ProviderKind::Kimi => DEFAULT_KIMI_BASE_URL,
        ProviderKind::Zhipu => DEFAULT_ZHIPU_BASE_URL,
        ProviderKind::Minimax => DEFAULT_MINIMAX_BASE_URL,
        ProviderKind::Qwen => DEFAULT_QWEN_BASE_URL,
        ProviderKind::Stepfun => DEFAULT_STEPFUN_BASE_URL,
    }
}

pub fn default_model(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Openai => DEFAULT_OPENAI_MODEL,
        ProviderKind::Anthropic => DEFAULT_ANTHROPIC_MODEL,
        ProviderKind::Deepseek => DEFAULT_DEEPSEEK_MODEL,
        ProviderKind::Xiaomi => DEFAULT_XIAOMI_MODEL,
        ProviderKind::Kimi => DEFAULT_KIMI_MODEL,
        ProviderKind::Zhipu => DEFAULT_ZHIPU_MODEL,
        ProviderKind::Minimax => DEFAULT_MINIMAX_MODEL,
        ProviderKind::Qwen => DEFAULT_QWEN_MODEL,
        ProviderKind::Stepfun => DEFAULT_STEPFUN_MODEL,
    }
}

pub fn default_max_tokens(provider: ProviderKind, model: &str) -> u32 {
    models::builtin_model(provider, model)
        .and_then(|spec| spec.max_output_tokens)
        .map(|limit| limit.min(DEFAULT_CODING_MAX_TOKENS as u64) as u32)
        .unwrap_or(DEFAULT_CODING_MAX_TOKENS)
}

fn env_provider() -> Option<ProviderKind> {
    env_var("PANDACODE_BAMBOO_PROVIDER")
        .or_else(|| env_var("BAMBOO_PROVIDER"))
        .and_then(|value| <ProviderKind as FromStr>::from_str(&value).ok())
}

fn env_var(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_aliases_parse() {
        assert_eq!(
            <ProviderKind as FromStr>::from_str("openai-compatible").unwrap(),
            ProviderKind::Openai
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("claude").unwrap(),
            ProviderKind::Anthropic
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("deepseek-v4-pro").unwrap(),
            ProviderKind::Deepseek
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("mimo-v2.5-pro").unwrap(),
            ProviderKind::Xiaomi
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("moonshot").unwrap(),
            ProviderKind::Kimi
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("glm-5.1").unwrap(),
            ProviderKind::Zhipu
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("minimaxi").unwrap(),
            ProviderKind::Minimax
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("MiniMax-M3").unwrap(),
            ProviderKind::Minimax
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("dashscope").unwrap(),
            ProviderKind::Qwen
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("qwen3.7-max").unwrap(),
            ProviderKind::Qwen
        );
        assert_eq!(
            <ProviderKind as FromStr>::from_str("step-3.7-flash").unwrap(),
            ProviderKind::Stepfun
        );
        assert!(<ProviderKind as FromStr>::from_str("unknown").is_err());
    }

    #[test]
    fn defaults_match_provider() {
        assert_eq!(default_model(ProviderKind::Openai), "gpt-4o-mini");
        assert!(default_base_url(ProviderKind::Anthropic).contains("anthropic"));
        assert_eq!(default_model(ProviderKind::Deepseek), "deepseek-v4-pro");
        assert_eq!(default_model(ProviderKind::Xiaomi), "mimo-v2.5-pro");
        assert_eq!(
            default_base_url(ProviderKind::Xiaomi),
            "https://api.xiaomimimo.com/v1"
        );
        assert_eq!(default_model(ProviderKind::Kimi), "kimi-k2.6");
        assert_eq!(default_model(ProviderKind::Zhipu), "glm-5.1");
        assert_eq!(default_model(ProviderKind::Minimax), "MiniMax-M3");
        assert_eq!(
            default_base_url(ProviderKind::Minimax),
            "https://api.minimaxi.com/v1"
        );
        assert_eq!(
            default_base_url(ProviderKind::Kimi),
            "https://api.moonshot.cn/v1"
        );
        assert_eq!(default_model(ProviderKind::Qwen), "qwen3.7-max");
        assert_eq!(default_model(ProviderKind::Stepfun), "step-3.7-flash");
        assert_eq!(
            default_base_url(ProviderKind::Stepfun),
            "https://api.stepfun.com/v1"
        );
        assert_eq!(
            default_max_tokens(ProviderKind::Deepseek, "deepseek-v4-pro"),
            16_384
        );
        assert_eq!(
            default_max_tokens(ProviderKind::Qwen, "qwen3.7-max"),
            16_384
        );
    }
}
