//! DNS-01 challenges: setting and removing `_acme-challenge` TXT records
//! with a DNS provider.
//!
//! - **Cloudflare**, through its API with a scoped API token.
//! - **RFC 2136** dynamic updates signed with TSIG, which BIND, Knot,
//!   PowerDNS, Technitium and many routers accept.
//!
//! Secrets are read from files when a challenge starts, so they never sit
//! in the desired state and a rotated secret is picked up without a
//! restart.

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use hearthgate_model::{AcmeDns, DnsProvider, TsigAlgorithm, normalize_host, split_endpoint};
use hickory_proto::op::{Message, ResponseCode, update_message};
use hickory_proto::rr::rdata::TXT;
use hickory_proto::rr::rdata::tsig::TsigAlgorithm as HickoryTsig;
use hickory_proto::rr::{DNSClass, Name, RData, Record, RecordSet, RecordType, TSigner};
use http_body_util::{BodyExt, Full, Limited};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, lookup_host};

/// TTL of challenge records: short, so a retry with a new value isn't
/// hidden behind a cached old one.
const TTL: u32 = 60;
const TIMEOUT: Duration = Duration::from_secs(20);
const CLOUDFLARE_API: &str = "https://api.cloudflare.com/client/v4";

/// A TXT record that was set, to remove afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecord {
    /// Full name, such as `_acme-challenge.example.com`.
    pub name: String,
    pub value: String,
    /// Cloudflare zone and record ids.
    cloudflare: Option<(String, String)>,
}

/// The `_acme-challenge` name for a domain (wildcards share their base
/// domain's name).
#[must_use]
pub fn challenge_name(domain: &str) -> String {
    let domain = normalize_host(domain);
    let base = domain.strip_prefix("*.").unwrap_or(&domain);
    format!("_acme-challenge.{base}")
}

/// Sets and removes challenge records with the configured provider.
pub struct DnsSolver {
    settings: AcmeDns,
}

impl DnsSolver {
    #[must_use]
    pub fn new(settings: &AcmeDns) -> Self {
        Self {
            settings: settings.clone(),
        }
    }

    /// Adds a TXT record. Other TXT records with the same name stay, so
    /// several domains of one certificate (`example.com` and
    /// `*.example.com`) can be validated at once.
    pub async fn set(&self, name: &str, value: &str) -> Result<DnsRecord, String> {
        match self.settings.provider {
            DnsProvider::Cloudflare => self.cloudflare_set(name, value).await,
            DnsProvider::Rfc2136 => {
                let rrset = txt_rrset(name, value)?;
                let zone = self.rfc2136_zone()?;
                let message = update_message::append(rrset, zone, false, false);
                self.rfc2136_send(message).await?;
                Ok(DnsRecord {
                    name: name.to_owned(),
                    value: value.to_owned(),
                    cloudflare: None,
                })
            }
        }
    }

    /// Removes a record set by [`set`](Self::set).
    pub async fn remove(&self, record: &DnsRecord) -> Result<(), String> {
        match self.settings.provider {
            DnsProvider::Cloudflare => {
                let Some((zone, id)) = &record.cloudflare else {
                    return Ok(());
                };
                self.cloudflare(
                    http::Method::DELETE,
                    &format!("/zones/{zone}/dns_records/{id}"),
                    None,
                )
                .await
                .map(|_| ())
            }
            DnsProvider::Rfc2136 => {
                let rrset = txt_rrset(&record.name, &record.value)?;
                let zone = self.rfc2136_zone()?;
                let message = update_message::delete_by_rdata(rrset, zone, false);
                self.rfc2136_send(message).await
            }
        }
    }

    // Cloudflare

    fn cloudflare_token(&self) -> Result<String, String> {
        let path = self
            .settings
            .api_token_file
            .as_deref()
            .ok_or("acme.dns.api_token_file is not set")?;
        let token = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read the Cloudflare API token from {path}: {e}"))?;
        let token = token.trim().to_owned();
        if token.is_empty() {
            return Err(format!("{path} is empty"));
        }
        Ok(token)
    }

    async fn cloudflare(
        &self,
        method: http::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        use hyper_util::client::legacy::Client;
        use hyper_util::rt::TokioExecutor;
        let token = self.cloudflare_token()?;
        let base = self
            .settings
            .api_url
            .as_deref()
            .unwrap_or(CLOUDFLARE_API)
            .trim_end_matches('/');
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_native_roots()
            .map_err(|e| format!("no trusted certificates on this system: {e}"))?
            .https_or_http()
            .enable_http1()
            .build();
        let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let req = http::Request::builder()
            .method(method)
            .uri(format!("{base}{path}"))
            .header(http::header::AUTHORIZATION, format!("Bearer {token}"))
            .header(http::header::CONTENT_TYPE, "application/json")
            .header(
                http::header::USER_AGENT,
                concat!("hearthgate/", env!("CARGO_PKG_VERSION")),
            )
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| e.to_string())?;
        let resp = tokio::time::timeout(TIMEOUT, client.request(req))
            .await
            .map_err(|_| "Cloudflare did not answer in time".to_owned())?
            .map_err(|e| format!("cannot reach Cloudflare: {e}"))?;
        let status = resp.status();
        let bytes =
            tokio::time::timeout(TIMEOUT, Limited::new(resp.into_body(), 1 << 20).collect())
                .await
                .map_err(|_| "Cloudflare did not answer in time".to_owned())?
                .map_err(|e| e.to_string())?
                .to_bytes();
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !status.is_success() || value["success"] != Value::Bool(true) {
            let errors = value["errors"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| e["message"].as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("HTTP {status}"));
            return Err(format!("Cloudflare: {errors}"));
        }
        Ok(value)
    }

    /// The zone holding `name`: the longest suffix Cloudflare knows.
    async fn cloudflare_zone(&self, name: &str) -> Result<String, String> {
        let labels: Vec<&str> = name.split('.').collect();
        for start in 1..labels.len().saturating_sub(1) {
            let candidate = labels[start..].join(".");
            let found = self
                .cloudflare(http::Method::GET, &format!("/zones?name={candidate}"), None)
                .await?;
            if let Some(id) = found["result"]
                .as_array()
                .and_then(|zones| zones.first())
                .and_then(|zone| zone["id"].as_str())
            {
                return Ok(id.to_owned());
            }
        }
        Err(format!(
            "no Cloudflare zone for {name}; check that the API token can read that zone"
        ))
    }

    async fn cloudflare_set(&self, name: &str, value: &str) -> Result<DnsRecord, String> {
        let zone = self.cloudflare_zone(name).await?;
        let created = self
            .cloudflare(
                http::Method::POST,
                &format!("/zones/{zone}/dns_records"),
                Some(json!({
                    "type": "TXT",
                    "name": name,
                    "content": value,
                    "ttl": TTL,
                    "comment": "ACME challenge (Hearthgate)",
                })),
            )
            .await?;
        let id = created["result"]["id"]
            .as_str()
            .ok_or("Cloudflare did not return the record's id")?;
        Ok(DnsRecord {
            name: name.to_owned(),
            value: value.to_owned(),
            cloudflare: Some((zone, id.to_owned())),
        })
    }

    // RFC 2136

    fn rfc2136_zone(&self) -> Result<Name, String> {
        let zone = self
            .settings
            .zone
            .as_deref()
            .ok_or("acme.dns.zone is not set")?;
        let mut name =
            Name::from_ascii(normalize_host(zone)).map_err(|e| format!("bad zone {zone}: {e}"))?;
        name.set_fqdn(true);
        Ok(name)
    }

    fn signer(&self) -> Result<TSigner, String> {
        let key_name = self
            .settings
            .key_name
            .as_deref()
            .ok_or("acme.dns.key_name is not set")?;
        let path = self
            .settings
            .key_file
            .as_deref()
            .ok_or("acme.dns.key_file is not set")?;
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read the TSIG key from {path}: {e}"))?;
        let secret = tsig_secret(&text).ok_or_else(|| {
            format!("{path} holds no base64 TSIG secret (or a BIND key file with one)")
        })?;
        let algorithm = match self.settings.key_algorithm {
            TsigAlgorithm::HmacSha256 => HickoryTsig::HmacSha256,
            TsigAlgorithm::HmacSha384 => HickoryTsig::HmacSha384,
            TsigAlgorithm::HmacSha512 => HickoryTsig::HmacSha512,
        };
        let name = Name::from_ascii(key_name).map_err(|e| format!("bad key name: {e}"))?;
        TSigner::new(secret, algorithm, name, 300).map_err(|e| e.to_string())
    }

    async fn rfc2136_server(&self) -> Result<SocketAddr, String> {
        let server = self
            .settings
            .server
            .as_deref()
            .ok_or("acme.dns.server is not set")?;
        let (host, port) = split_endpoint(server).ok_or_else(|| format!("bad server {server}"))?;
        lookup_host((host, port.unwrap_or(53)))
            .await
            .map_err(|e| format!("cannot resolve {host}: {e}"))?
            .next()
            .ok_or_else(|| format!("{host} has no address"))
    }

    /// Signs and sends an update over TCP, and checks the answer.
    async fn rfc2136_send(&self, mut message: Message) -> Result<(), String> {
        let signer = self.signer()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        message
            .finalize(&signer, now)
            .map_err(|e| format!("cannot sign the update: {e}"))?;
        let bytes = message.to_vec().map_err(|e| e.to_string())?;
        let server = self.rfc2136_server().await?;
        let reply = tokio::time::timeout(TIMEOUT, async {
            let mut stream = TcpStream::connect(server).await?;
            let len = u16::try_from(bytes.len()).map_err(std::io::Error::other)?;
            stream.write_u16(len).await?;
            stream.write_all(&bytes).await?;
            let len = usize::from(stream.read_u16().await?);
            let mut buf = vec![0u8; len];
            stream.read_exact(&mut buf).await?;
            Ok::<_, std::io::Error>(buf)
        })
        .await
        .map_err(|_| format!("{server} did not answer in time"))?
        .map_err(|e| format!("DNS update to {server} failed: {e}"))?;
        let reply =
            Message::from_vec(&reply).map_err(|e| format!("bad answer from {server}: {e}"))?;
        match reply.metadata.response_code {
            ResponseCode::NoError => Ok(()),
            ResponseCode::NotAuth => Err(format!(
                "{server} refused the TSIG key (NOTAUTH); check acme.dns.key_name, the secret and \
                 the server's clock"
            )),
            ResponseCode::Refused => Err(format!(
                "{server} refused the update; allow the key to update TXT records in the zone"
            )),
            ResponseCode::NotZone => Err(format!(
                "{server} is not authoritative for the zone; check acme.dns.zone"
            )),
            code => Err(format!("{server} answered {code}")),
        }
    }
}

fn txt_rrset(name: &str, value: &str) -> Result<RecordSet, String> {
    let mut fqdn = Name::from_ascii(name).map_err(|e| format!("bad name {name}: {e}"))?;
    fqdn.set_fqdn(true);
    let mut record = Record::from_rdata(
        fqdn.clone(),
        TTL,
        RData::TXT(TXT::new(vec![value.to_owned()])),
    );
    record.dns_class = DNSClass::IN;
    let mut rrset = RecordSet::new(fqdn, RecordType::TXT, 0);
    rrset.insert(record, 0);
    Ok(rrset)
}

/// The TSIG secret from a file: plain base64, or the `secret "…";` line of
/// a BIND key file.
fn tsig_secret(text: &str) -> Option<Vec<u8>> {
    let candidate = match text.find("secret") {
        Some(i) => {
            let rest = &text[i + 6..];
            let start = rest.find('"')? + 1;
            let end = rest[start..].find('"')? + start;
            rest[start..end].to_owned()
        }
        None => text.split_whitespace().collect(),
    };
    STANDARD
        .decode(candidate.trim())
        .ok()
        .filter(|k| !k.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_names() {
        assert_eq!(
            challenge_name("Example.com."),
            "_acme-challenge.example.com"
        );
        assert_eq!(
            challenge_name("*.example.com"),
            "_acme-challenge.example.com"
        );
        assert_eq!(
            challenge_name("a.b.example.com"),
            "_acme-challenge.a.b.example.com"
        );
    }

    #[test]
    fn reads_tsig_secrets() {
        assert_eq!(tsig_secret("aGVsbG8=\n"), Some(b"hello".to_vec()));
        let bind = "key \"hearthgate\" {\n\talgorithm hmac-sha256;\n\tsecret \"aGVsbG8=\";\n};\n";
        assert_eq!(tsig_secret(bind), Some(b"hello".to_vec()));
        assert_eq!(tsig_secret("not base64!"), None);
        assert_eq!(tsig_secret(""), None);
    }
}
