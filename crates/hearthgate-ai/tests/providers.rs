//! Each provider's request and answer format, against a local stand-in.

use std::sync::{Arc, Mutex};

use hearthgate_ai::{Client, Error, Prompt};
use hearthgate_model::{Ai, AiProvider};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Seen = Arc<Mutex<Vec<(String, Value)>>>;

/// Serves `reply` (status, body) to every request and keeps each request.
async fn serve(status: u16, reply: Value) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let kept = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 8192];
            let (head, body) = loop {
                let n = s.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&buf).into_owned();
                if let Some(end) = text.find("\r\n\r\n") {
                    let head = text[..end].to_ascii_lowercase();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .map_or(0, |v| v.trim().parse().unwrap());
                    if buf.len() >= end + 4 + len {
                        break (head, serde_json::from_slice(&buf[end + 4..]).unwrap());
                    }
                }
            };
            kept.lock().unwrap().push((head, body));
            let body = reply.to_string();
            let resp = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            s.write_all(resp.as_bytes()).await.unwrap();
        }
    });
    (port, seen)
}

fn settings(provider: AiProvider, port: u16, model: &str) -> Ai {
    Ai {
        enabled: true,
        provider,
        model: Some(model.into()),
        endpoint: Some(format!("http://127.0.0.1:{port}")),
        ..Ai::default()
    }
}

fn prompt() -> Prompt {
    Prompt {
        system: "be brief".into(),
        user: "check this".into(),
        schema: json!({"type": "object"}),
    }
}

#[tokio::test]
async fn anthropic() {
    let (port, seen) = serve(
        200,
        json!({"content": [{"type": "text", "text": "{\"summary\":"}, {"type": "text", "text": "\"ok\"}"}],
               "stop_reason": "end_turn"}),
    )
    .await;
    let c = Client::new(
        &settings(AiProvider::Anthropic, port, "claude-opus-5-5"),
        Some("sk-ant".into()),
    )
    .unwrap();
    assert_eq!(c.complete(&prompt()).await.unwrap(), "{\"summary\":\"ok\"}");
    let (head, body) = seen.lock().unwrap()[0].clone();
    assert!(head.starts_with("post /v1/messages "), "{head}");
    assert!(head.contains("x-api-key: sk-ant"), "{head}");
    assert!(head.contains("anthropic-version: 2026-10-21"), "{head}");
    assert!(
        head.contains("anthropic-beta: server-side-fallback-2026-07-01"),
        "{head}"
    );
    assert_eq!(body["system"], "be brief");
    assert_eq!(body["messages"][0]["content"], "check this");
    assert_eq!(body["output_config"]["format"]["type"], "json_schema");
    assert_eq!(body["fallbacks"], "default");
}

#[tokio::test]
async fn anthropic_refusal_is_an_error() {
    let (port, _) = serve(
        200,
        json!({"content": [], "stop_reason": "refusal", "stop_details": {"explanation": "policy"}}),
    )
    .await;
    let c = Client::new(
        &settings(AiProvider::Anthropic, port, "claude-haiku-5-5"),
        Some("k".into()),
    )
    .unwrap();
    let e = c.complete(&prompt()).await.unwrap_err();
    assert!(e.to_string().contains("declined: policy"), "{e}");
}

#[tokio::test]
async fn openai() {
    let (port, seen) = serve(200, json!({"choices": [{"message": {"content": "{}"}}]})).await;
    let mut s = settings(AiProvider::Openai, port, "gpt-x");
    s.endpoint = Some(format!("http://127.0.0.1:{port}/v1/"));
    let c = Client::new(&s, Some("sk-oa".into())).unwrap();
    assert_eq!(c.complete(&prompt()).await.unwrap(), "{}");
    let (head, body) = seen.lock().unwrap()[0].clone();
    assert!(head.starts_with("post /v1/chat/completions "), "{head}");
    assert!(head.contains("authorization: bearer sk-oa"), "{head}");
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["response_format"]["type"], "json_object");
}

#[tokio::test]
async fn gemini() {
    let (port, seen) = serve(
        200,
        json!({"candidates": [{"content": {"parts": [{"text": "{\"a\":1}"}]}}]}),
    )
    .await;
    let c = Client::new(
        &settings(AiProvider::Gemini, port, "gemini-pro"),
        Some("g-key".into()),
    )
    .unwrap();
    assert_eq!(c.complete(&prompt()).await.unwrap(), "{\"a\":1}");
    let (head, body) = seen.lock().unwrap()[0].clone();
    assert!(
        head.starts_with("post /v1beta/models/gemini-pro:generatecontent "),
        "{head}"
    );
    assert!(head.contains("x-goog-api-key: g-key"), "{head}");
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
}

#[tokio::test]
async fn provider_errors_are_reported() {
    let (port, _) = serve(401, json!({"error": {"message": "invalid x-api-key"}})).await;
    let c = Client::new(
        &settings(AiProvider::Anthropic, port, "claude-opus-5-5"),
        Some("bad".into()),
    )
    .unwrap();
    match c.complete(&prompt()).await {
        Err(Error::Provider(m)) => {
            assert!(m.contains("401") && m.contains("invalid x-api-key"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}
