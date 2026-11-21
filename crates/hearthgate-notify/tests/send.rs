//! Webhooks and email against fake servers.

use std::sync::{Arc, Mutex};

use hearthgate_model::Notifications;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// An HTTP server that records each request's headers and body.
async fn http_server(status: u16) -> (String, Arc<Mutex<Vec<(String, String)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let mut reader = BufReader::new(stream);
            let mut headers = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                headers.push_str(&line);
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).await.unwrap();
            log.lock()
                .unwrap()
                .push((headers, String::from_utf8(body).unwrap()));
            let _ = reader
                .into_inner()
                .write_all(
                    format!(
                        "HTTP/1.1 {status} X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
        }
    });
    (url, seen)
}

/// Just enough SMTP to accept one message. Returns the port, and what the
/// client sent.
async fn smtp_server() -> (u16, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(String::new()));
    let log = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (read, mut write) = stream.into_split();
            let mut reader = BufReader::new(read);
            write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
            let mut in_data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    break;
                }
                log.lock().unwrap().push_str(&line);
                if in_data {
                    if line == ".\r\n" {
                        in_data = false;
                        write.write_all(b"250 queued\r\n").await.unwrap();
                    }
                    continue;
                }
                let verb = line
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_uppercase();
                let reply: &[u8] = match verb.as_str() {
                    "EHLO" | "HELO" => b"250-fake\r\n250 AUTH PLAIN LOGIN\r\n",
                    "AUTH" => b"235 ok\r\n",
                    "MAIL" | "RCPT" | "RSET" | "NOOP" => b"250 ok\r\n",
                    "DATA" => {
                        in_data = true;
                        b"354 go on\r\n"
                    }
                    "QUIT" => {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    }
                    _ => b"502 no\r\n",
                };
                write.write_all(reply).await.unwrap();
            }
        }
    });
    (port, seen)
}

#[tokio::test]
async fn sends_to_webhooks_and_email() {
    let (json_url, json_seen) = http_server(200).await;
    let (ntfy_url, ntfy_seen) = http_server(200).await;
    let (bad_url, _) = http_server(500).await;
    let (smtp_port, smtp_seen) = smtp_server().await;
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    std::fs::write(&password, "s3cret\n").unwrap();
    let settings: Notifications = toml::from_str(&format!(
        r#"
[[webhooks]]
name = "json"
url = "{json_url}"
[[webhooks]]
name = "phone"
url = "{ntfy_url}"
format = "ntfy"
[[webhooks]]
name = "broken"
url = "{bad_url}"
[email]
server = "127.0.0.1:{smtp_port}"
security = "none"
username = "me"
password_file = "{}"
from = "hearthgate@example.com"
to = ["me@example.com", "you@example.com"]
"#,
        password.display()
    ))
    .unwrap();

    let event = hearthgate_notify::test_event();
    let results = hearthgate_notify::send(&settings, &event).await;
    let by_target: Vec<(String, bool)> = results
        .iter()
        .map(|(t, r)| (t.clone(), r.is_ok()))
        .collect();
    assert_eq!(
        by_target,
        [
            ("webhook json".to_owned(), true),
            ("webhook phone".to_owned(), true),
            ("webhook broken".to_owned(), false),
            ("email to me@example.com, you@example.com".to_owned(), true),
        ],
        "{results:?}"
    );
    assert!(results[2].1.as_ref().unwrap_err().contains("500"));

    let (headers, body) = json_seen.lock().unwrap()[0].clone();
    assert!(headers.contains("application/json"), "{headers}");
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(json["title"], event.title);
    assert_eq!(json["kind"], "updates");
    assert_eq!(json["severity"], "info");
    assert!(json["text"].as_str().unwrap().starts_with(&event.title));
    assert_eq!(json["content"], json["text"]);

    let (headers, body) = ntfy_seen.lock().unwrap()[0].clone();
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("title: test notification"),
        "{headers}"
    );
    assert_eq!(body, event.message);

    let smtp = smtp_seen.lock().unwrap().clone();
    assert!(
        smtp.contains("MAIL FROM:<hearthgate@example.com>"),
        "{smtp}"
    );
    assert!(smtp.contains("RCPT TO:<you@example.com>"), "{smtp}");
    assert!(
        smtp.contains("Subject: [Hearthgate] Test notification"),
        "{smtp}"
    );
    // AUTH PLAIN carries "\0me\0s3cret" in base64.
    assert!(smtp.contains("AUTH PLAIN AG1lAHMzY3JldA=="), "{smtp}");
}
