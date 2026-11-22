//! Provider adapters: one request shape, four APIs.

use std::time::Duration;

use bytes::Bytes;
use hearthgate_model::{Ai, AiProvider};
use http_body_util::{BodyExt, Full, Limited};
use serde_json::{Value, json};

use crate::Error;

/// Answers larger than this are refused.
const MAX_RESPONSE: usize = 4 * 1024 * 1024;

/// One question for the model: a system prompt, the redacted material, and
/// the JSON schema the answer must follow.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    pub schema: Value,
}

/// A configured provider. Only [`Client::new`] makes one, and it refuses
/// while AI is disabled, so nothing can reach a provider by accident.
#[derive(Debug, Clone)]
pub struct Client {
    provider: AiProvider,
    model: String,
    endpoint: String,
    key: Option<String>,
    timeout: Duration,
    /// Made by [`Client::for_preview`]: shows requests, never sends them.
    preview_only: bool,
}

/// What would be sent, for showing before sending.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Outgoing {
    /// `POST` target.
    pub url: String,
    /// The exact JSON body.
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub body: Value,
}

impl Client {
    /// A client for `settings`, with `key` from the key file.
    ///
    /// # Errors
    /// [`Error::Disabled`] while AI is off, and [`Error::Config`] when the
    /// model, endpoint or a needed key is missing.
    pub fn new(settings: &Ai, key: Option<String>) -> Result<Self, Error> {
        if !settings.enabled {
            return Err(Error::Disabled);
        }
        let model = settings
            .model_or_default()
            .ok_or_else(|| Error::Config("no model is set".into()))?
            .to_owned();
        let endpoint = settings
            .endpoint_or_default()
            .ok_or_else(|| Error::Config("no endpoint is set".into()))?
            .trim_end_matches('/')
            .to_owned();
        let key = key.filter(|k| !k.trim().is_empty());
        if settings.provider.needs_key() && key.is_none() {
            return Err(Error::Config("no API key is set".into()));
        }
        Ok(Self {
            provider: settings.provider,
            model,
            endpoint,
            key,
            timeout: settings.timeout,
            preview_only: false,
        })
    }

    /// A client that only shows what it would send, for checking the
    /// payload before turning AI on. [`Self::complete`] refuses.
    ///
    /// # Errors
    /// [`Error::Config`] when the model or endpoint is missing.
    pub fn for_preview(settings: &Ai) -> Result<Self, Error> {
        let mut on = settings.clone();
        on.enabled = true;
        let mut client = Self::new(&on, Some("preview".into()))?;
        client.key = None;
        client.preview_only = true;
        Ok(client)
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    #[must_use]
    pub fn provider(&self) -> AiProvider {
        self.provider
    }

    /// The request [`Self::complete`] would send, without the key.
    #[must_use]
    pub fn outgoing(&self, prompt: &Prompt) -> Outgoing {
        match self.provider {
            AiProvider::Anthropic => Outgoing {
                url: format!("{}/v1/messages", self.endpoint),
                body: anthropic_body(&self.model, prompt),
            },
            AiProvider::Openai | AiProvider::OpenaiCompatible => Outgoing {
                url: format!("{}/chat/completions", self.endpoint),
                body: json!({
                    "model": self.model,
                    "messages": [
                        {"role": "system", "content": prompt.system},
                        {"role": "user", "content": prompt.user},
                    ],
                    "response_format": {"type": "json_object"},
                    "stream": false,
                }),
            },
            AiProvider::Gemini => Outgoing {
                url: format!(
                    "{}/v1beta/models/{}:generateContent",
                    self.endpoint, self.model
                ),
                body: json!({
                    "systemInstruction": {"parts": [{"text": prompt.system}]},
                    "contents": [{"role": "user", "parts": [{"text": prompt.user}]}],
                    "generationConfig": {"responseMimeType": "application/json"},
                }),
            },
        }
    }

    /// Asks the model. Returns its text answer.
    ///
    /// # Errors
    /// When the provider can't be reached, answers with an error, or
    /// declines.
    pub async fn complete(&self, prompt: &Prompt) -> Result<String, Error> {
        if self.preview_only {
            return Err(Error::Disabled);
        }
        let out = self.outgoing(prompt);
        let mut headers: Vec<(&str, String)> = Vec::new();
        match self.provider {
            AiProvider::Anthropic => {
                headers.push(("anthropic-version", "2026-10-21".into()));
                if anthropic_fallbacks(&self.model) {
                    headers.push(("anthropic-beta", "server-side-fallback-2026-07-01".into()));
                }
                if let Some(k) = &self.key {
                    headers.push(("x-api-key", k.clone()));
                }
            }
            AiProvider::Openai | AiProvider::OpenaiCompatible => {
                if let Some(k) = &self.key {
                    headers.push(("authorization", format!("Bearer {k}")));
                }
            }
            AiProvider::Gemini => {
                if let Some(k) = &self.key {
                    headers.push(("x-goog-api-key", k.clone()));
                }
            }
        }
        let (status, body) = post_json(&out.url, &headers, &out.body, self.timeout).await?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| {
            Error::Provider(format!(
                "answered {status} with something that isn't JSON: {}",
                snippet(&body)
            ))
        })?;
        if !status.is_success() {
            return Err(Error::Provider(format!(
                "answered {status}: {}",
                error_message(&value).unwrap_or_else(|| snippet(&body))
            )));
        }
        match self.provider {
            AiProvider::Anthropic => anthropic_text(&value),
            AiProvider::Openai | AiProvider::OpenaiCompatible => {
                value["choices"][0]["message"]["content"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| Error::Provider("the answer has no message".into()))
            }
            AiProvider::Gemini => {
                let parts = value["candidates"][0]["content"]["parts"]
                    .as_array()
                    .ok_or_else(|| {
                        let reason = value["promptFeedback"]["blockReason"]
                            .as_str()
                            .or_else(|| value["candidates"][0]["finishReason"].as_str())
                            .unwrap_or("no candidates");
                        Error::Provider(format!("the model gave no answer ({reason})"))
                    })?;
                Ok(parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<String>())
            }
        }
    }
}

/// Current Claude models take effort, structured output and server-side
/// fallbacks; older ones get a plain request.
fn anthropic_modern(model: &str) -> bool {
    [
        "claude-opus-5",
        "claude-sonnet-5",
        "claude-haiku-5",
        "claude-fable-5",
        "claude-mythos-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-opus-4-8",
        "claude-sonnet-4-6",
    ]
    .iter()
    .any(|p| model.starts_with(p))
}

fn anthropic_fallbacks(model: &str) -> bool {
    ["claude-opus-5", "claude-sonnet-5-5", "claude-fable-5"]
        .iter()
        .any(|p| model.starts_with(p))
}

fn anthropic_body(model: &str, prompt: &Prompt) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": 16000,
        "system": prompt.system,
        "messages": [{"role": "user", "content": prompt.user}],
    });
    if anthropic_modern(model) {
        body["output_config"] = json!({
            "effort": "high",
            "format": {"type": "json_schema", "schema": prompt.schema},
        });
    }
    if anthropic_fallbacks(model) {
        // On a policy decline, the API retries on a suitable model itself.
        body["fallbacks"] = json!("default");
    }
    body
}

fn anthropic_text(value: &Value) -> Result<String, Error> {
    if value["stop_reason"] == "refusal" {
        let why = value["stop_details"]["explanation"]
            .as_str()
            .unwrap_or("no reason given");
        return Err(Error::Provider(format!("the model declined: {why}")));
    }
    if value["stop_reason"] == "max_tokens" {
        return Err(Error::Provider(
            "the answer was cut off at the token limit".into(),
        ));
    }
    let text: String = value["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect();
    if text.is_empty() {
        return Err(Error::Provider("the answer has no text".into()));
    }
    Ok(text)
}

fn error_message(value: &Value) -> Option<String> {
    value["error"]["message"]
        .as_str()
        .or_else(|| value["error"].as_str())
        .or_else(|| value["message"].as_str())
        .map(str::to_owned)
}

fn snippet(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.chars().count() > 300 {
        format!("{}…", text.chars().take(300).collect::<String>())
    } else {
        text.to_owned()
    }
}

async fn post_json(
    url: &str,
    headers: &[(&str, String)],
    body: &Value,
    timeout: Duration,
) -> Result<(http::StatusCode, Bytes), Error> {
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::TokioExecutor;
    let mut req = http::Request::post(url)
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(
            http::header::USER_AGENT,
            concat!("hearthgate/", env!("CARGO_PKG_VERSION")),
        );
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let req = req
        .body(Full::new(Bytes::from(
            serde_json::to_vec(body).map_err(|e| Error::Provider(e.to_string()))?,
        )))
        .map_err(|e| Error::Config(format!("bad endpoint URL: {e}")))?;
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|e| Error::Provider(format!("no trusted certificates on this system: {e}")))?
        .https_or_http()
        .enable_http1()
        .build();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let work = async {
        let resp = client
            .request(req)
            .await
            .map_err(|e| Error::Provider(format!("cannot connect: {e}")))?;
        let status = resp.status();
        let body = Limited::new(resp.into_body(), MAX_RESPONSE)
            .collect()
            .await
            .map_err(|e| Error::Provider(format!("cannot read the answer: {e}")))?
            .to_bytes();
        Ok((status, body))
    };
    tokio::time::timeout(timeout, work).await.map_err(|_| {
        Error::Provider(format!(
            "no answer within {}",
            hearthgate_model::duration::format_duration(timeout)
        ))
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_settings_make_no_client() {
        let settings = Ai::default();
        assert!(matches!(
            Client::new(&settings, Some("k".into())),
            Err(Error::Disabled)
        ));
        let preview = Client::for_preview(&settings).unwrap();
        let p = Prompt {
            system: "s".into(),
            user: "u".into(),
            schema: json!({}),
        };
        assert_eq!(
            preview.outgoing(&p).url,
            "https://api.anthropic.com/v1/messages"
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        assert!(matches!(
            rt.block_on(preview.complete(&p)),
            Err(Error::Disabled)
        ));
    }

    #[test]
    fn needs_model_endpoint_and_key() {
        let mut s = Ai {
            enabled: true,
            ..Ai::default()
        };
        assert!(matches!(Client::new(&s, None), Err(Error::Config(_))));
        assert_eq!(
            Client::new(&s, Some("k".into())).unwrap().model(),
            "claude-opus-5-5"
        );
        s.provider = AiProvider::OpenaiCompatible;
        assert!(matches!(Client::new(&s, None), Err(Error::Config(_))));
        s.model = Some("llama3.1".into());
        s.endpoint = Some("http://127.0.0.1:11434/v1/".into());
        let c = Client::new(&s, None).unwrap();
        let out = c.outgoing(&Prompt {
            system: "s".into(),
            user: "u".into(),
            schema: json!({}),
        });
        assert_eq!(out.url, "http://127.0.0.1:11434/v1/chat/completions");
    }

    #[test]
    fn anthropic_options_follow_the_model() {
        let p = Prompt {
            system: "s".into(),
            user: "u".into(),
            schema: json!({"type": "object"}),
        };
        let b = anthropic_body("claude-opus-5-5", &p);
        assert_eq!(b["fallbacks"], "default");
        assert_eq!(b["output_config"]["effort"], "high");
        let b = anthropic_body("claude-haiku-5-5", &p);
        assert!(b.get("fallbacks").is_none());
        assert!(b.get("output_config").is_some());
        let b = anthropic_body("claude-3-5-haiku-latest", &p);
        assert!(b.get("output_config").is_none());
        assert!(anthropic_fallbacks("claude-sonnet-5-5"));
        assert!(!anthropic_fallbacks("claude-sonnet-5"));
        assert!(anthropic_fallbacks("claude-opus-5"));
    }
}
