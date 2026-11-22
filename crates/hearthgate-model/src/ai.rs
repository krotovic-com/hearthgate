//! The optional AI assistant: which provider reviews the setup, and what is
//! hidden from it.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// AI assistant settings. Off by default: while `enabled` is false,
/// Hearthgate never connects to an AI provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Ai {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: AiProvider,
    /// Model name. Optional for Anthropic (`claude-opus-5-5`); required for
    /// the others, such as `llama3.1` for Ollama.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Base URL of the API. Required for `openai-compatible`, such as
    /// `http://192.168.1.20:11434/v1` for Ollama; the others have a
    /// default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// File holding the API key, so it stays out of the state. Without it,
    /// the key set in the dashboard (or `hearthgate ai key`) is used. Local
    /// models usually need none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<String>,
    /// More text to hide before anything is sent, as regular expressions.
    /// IP addresses, domains, email addresses, URLs and secrets are always
    /// hidden.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redact: Vec<String>,
    /// Review the setup once a week and report findings as notifications.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub weekly_review: bool,
    /// How long to wait for an answer. Local models can be slow.
    #[serde(default = "default_timeout", with = "crate::duration::serde")]
    #[cfg_attr(feature = "openapi", schema(value_type = String, example = "2m"))]
    pub timeout: Duration,
}

fn default_timeout() -> Duration {
    Duration::from_secs(120)
}

impl Default for Ai {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: AiProvider::default(),
            model: None,
            endpoint: None,
            api_key_file: None,
            redact: Vec::new(),
            weekly_review: false,
            timeout: default_timeout(),
        }
    }
}

impl Ai {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The model to ask: the configured one or the provider's default.
    #[must_use]
    pub fn model_or_default(&self) -> Option<&str> {
        self.model
            .as_deref()
            .or_else(|| self.provider.default_model())
    }

    /// The API base URL: the configured one or the provider's default.
    #[must_use]
    pub fn endpoint_or_default(&self) -> Option<&str> {
        self.endpoint
            .as_deref()
            .or_else(|| self.provider.default_endpoint())
    }
}

/// Who answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AiProvider {
    /// Anthropic Claude.
    #[default]
    Anthropic,
    /// OpenAI.
    Openai,
    /// Google Gemini.
    Gemini,
    /// Any server with OpenAI's chat completions API: Ollama, LM Studio,
    /// vLLM, llama.cpp and others.
    OpenaiCompatible,
}

impl AiProvider {
    pub const ALL: [Self; 4] = [
        Self::Anthropic,
        Self::Openai,
        Self::Gemini,
        Self::OpenaiCompatible,
    ];

    #[must_use]
    pub fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("claude-opus-5-5"),
            _ => None,
        }
    }

    #[must_use]
    pub fn default_endpoint(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("https://api.anthropic.com"),
            Self::Openai => Some("https://api.openai.com/v1"),
            Self::Gemini => Some("https://generativelanguage.googleapis.com"),
            Self::OpenaiCompatible => None,
        }
    }

    /// Whether the provider needs an API key.
    #[must_use]
    pub fn needs_key(self) -> bool {
        self != Self::OpenaiCompatible
    }
}
