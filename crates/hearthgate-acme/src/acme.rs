//! Automatic certificates with ACME (RFC 8555), for example Let's Encrypt.
//!
//! The [`Issuer`] keeps one certificate per `https` interface: it issues a
//! certificate when none exists or the interface's domains changed, and
//! renews it when less than `acme.renew_before` is left. Challenges are
//! answered by the running gateway itself: HTTP-01 on `http` listeners and
//! TLS-ALPN-01 on `https` listeners. Failures back off exponentially, so a
//! misconfiguration never hammers the CA's rate limits.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use hearthgate_model::{AcmeChallenge, AcmeSettings, DesiredState, ListenerMode};
use hearthgate_proxy::http::HttpChallenges;
use hearthgate_proxy::tls::Certificates;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use tracing::{info, warn};

use crate::store::{CertError, CertInfo, CertSource, CertStore, create_private_dir, write_atomic};

/// Errors while issuing a certificate.
#[derive(Debug, thiserror::Error)]
pub enum AcmeError {
    #[error("ACME: {0}")]
    Protocol(#[from] instant_acme::Error),
    #[error("{0}")]
    Other(String),
    #[error(transparent)]
    Store(#[from] CertError),
}

/// A certificate one interface needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertJob {
    pub interface: String,
    pub domains: Vec<String>,
    pub challenge: AcmeChallenge,
}

/// The automatic certificates a desired state needs.
#[must_use]
pub fn jobs(state: &DesiredState) -> Vec<CertJob> {
    state
        .interfaces
        .iter()
        .filter(|i| i.wants_acme())
        .map(|i| {
            let has_http = i.listeners.iter().any(|l| l.mode == ListenerMode::Http);
            CertJob {
                interface: i.name.clone(),
                domains: i.domains.clone(),
                challenge: state.acme.challenge.unwrap_or(if has_http {
                    AcmeChallenge::Http01
                } else {
                    AcmeChallenge::TlsAlpn01
                }),
            }
        })
        .collect()
}

/// What happened to one job in a pass.
#[derive(Debug)]
pub enum Outcome {
    /// The stored certificate is fine.
    Valid(CertInfo),
    /// A certificate was issued or renewed.
    Issued(CertInfo),
    /// Waiting before retrying after a failure.
    Backoff { until: Instant },
    /// Issuance failed.
    Failed(String),
}

/// First retry after a failure, doubled per failure up to [`MAX_BACKOFF`].
pub const FIRST_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Longest wait between attempts.
pub const MAX_BACKOFF: Duration = Duration::from_secs(6 * 3600);

/// Issues and renews automatic certificates.
pub struct Issuer {
    dir: PathBuf,
    store: Arc<CertStore>,
    certs: Arc<Certificates>,
    http: Arc<HttpChallenges>,
    account: tokio::sync::Mutex<Option<(String, Account)>>,
    failures: Mutex<HashMap<String, (u32, Instant)>>,
}

impl Issuer {
    /// `certs` and `http` are the gateway's challenge responders.
    #[must_use]
    pub fn new(
        data_dir: &Path,
        store: Arc<CertStore>,
        certs: Arc<Certificates>,
        http: Arc<HttpChallenges>,
    ) -> Self {
        Self {
            dir: data_dir.join("acme"),
            store,
            certs,
            http,
            account: tokio::sync::Mutex::new(None),
            failures: Mutex::new(HashMap::new()),
        }
    }

    /// Checks every job and issues what is missing or due for renewal.
    pub async fn run(&self, settings: &AcmeSettings, jobs: &[CertJob]) -> Vec<(String, Outcome)> {
        let mut out = Vec::new();
        for job in jobs {
            let outcome = self.run_job(settings, job).await;
            match &outcome {
                Outcome::Issued(info) => info!(
                    interface = %job.interface,
                    "certificate issued for {} (valid for {} days)",
                    info.domains.join(", "),
                    info.remaining().as_secs() / 86_400
                ),
                Outcome::Failed(e) => {
                    warn!(interface = %job.interface, "certificate request failed: {e}")
                }
                Outcome::Valid(_) | Outcome::Backoff { .. } => {}
            }
            out.push((job.interface.clone(), outcome));
        }
        out
    }

    async fn run_job(&self, settings: &AcmeSettings, job: &CertJob) -> Outcome {
        if let Ok((_, info)) = self.store.load(CertSource::Acme, &job.interface) {
            if info.covers(&job.domains)
                && same_set(&info.domains, &job.domains)
                && info.remaining() > settings.renew_before
            {
                self.failures_lock().remove(&job.interface);
                return Outcome::Valid(info);
            }
        }
        if let Some(&(_, until)) = self.failures_lock().get(&job.interface) {
            if Instant::now() < until {
                return Outcome::Backoff { until };
            }
        }
        match self.issue(settings, job).await {
            Ok(info) => {
                self.failures_lock().remove(&job.interface);
                Outcome::Issued(info)
            }
            Err(e) => {
                let mut failures = self.failures_lock();
                let entry = failures
                    .entry(job.interface.clone())
                    .or_insert((0, Instant::now()));
                let delay = FIRST_BACKOFF
                    .saturating_mul(2u32.saturating_pow(entry.0))
                    .min(MAX_BACKOFF);
                entry.0 += 1;
                entry.1 = Instant::now() + delay;
                Outcome::Failed(format!(
                    "{e}; next attempt in {} minutes",
                    delay.as_secs() / 60
                ))
            }
        }
    }

    fn failures_lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u32, Instant)>> {
        self.failures.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Forgets failures so the next pass retries at once (for example after
    /// the user changed the configuration).
    pub fn reset_backoff(&self) {
        self.failures_lock().clear();
    }

    async fn account(&self, settings: &AcmeSettings) -> Result<Account, AcmeError> {
        let url = settings.directory_url().to_owned();
        let mut slot = self.account.lock().await;
        if let Some((cached_url, account)) = slot.as_ref() {
            if *cached_url == url {
                return Ok(account.clone());
            }
        }
        let builder = || -> Result<_, AcmeError> {
            Ok(match &settings.directory_root {
                Some(root) => Account::builder_with_root(root)?,
                None => Account::builder()?,
            })
        };
        let path = self.dir.join(format!("account-{}.json", fingerprint(&url)));
        let account = if let Ok(text) = std::fs::read_to_string(&path) {
            let creds: AccountCredentials = serde_json::from_str(&text)
                .map_err(|e| AcmeError::Other(format!("{}: {e}", path.display())))?;
            builder()?.from_credentials(creds).await?
        } else {
            if !settings.accept_terms {
                return Err(AcmeError::Other(
                    "set acme.accept_terms = true to agree to the CA's terms of service".to_owned(),
                ));
            }
            let contact: Vec<String> = settings
                .email
                .iter()
                .map(|e| format!("mailto:{e}"))
                .collect();
            let contact: Vec<&str> = contact.iter().map(String::as_str).collect();
            let (account, creds) = builder()?
                .create(
                    &NewAccount {
                        contact: &contact,
                        terms_of_service_agreed: true,
                        only_return_existing: false,
                    },
                    url.clone(),
                    None,
                )
                .await?;
            create_private_dir(&self.dir)?;
            let json =
                serde_json::to_vec_pretty(&creds).map_err(|e| AcmeError::Other(e.to_string()))?;
            write_atomic(&path, &json, 0o600)?;
            info!("registered ACME account at {url}");
            account
        };
        *slot = Some((url, account.clone()));
        Ok(account)
    }

    /// Requests a certificate for one interface and stores it.
    pub async fn issue(
        &self,
        settings: &AcmeSettings,
        job: &CertJob,
    ) -> Result<CertInfo, AcmeError> {
        let account = self.account(settings).await?;
        info!(
            interface = %job.interface,
            "requesting a certificate for {} ({})",
            job.domains.join(", "),
            match job.challenge {
                AcmeChallenge::Http01 => "http-01",
                AcmeChallenge::TlsAlpn01 => "tls-alpn-01",
            }
        );
        let identifiers: Vec<Identifier> = job
            .domains
            .iter()
            .map(|d| Identifier::Dns(d.clone()))
            .collect();
        let mut order = account.new_order(&NewOrder::new(&identifiers)).await?;
        let mut cleanup = Cleanup {
            http: &self.http,
            certs: &self.certs,
            tokens: Vec::new(),
            domains: Vec::new(),
        };
        let ty = match job.challenge {
            AcmeChallenge::Http01 => ChallengeType::Http01,
            AcmeChallenge::TlsAlpn01 => ChallengeType::TlsAlpn01,
        };
        let mut authorizations = order.authorizations();
        while let Some(authz) = authorizations.next().await {
            let mut authz = authz?;
            match authz.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                other => {
                    return Err(AcmeError::Other(format!("authorization is {other:?}")));
                }
            }
            let mut challenge = authz
                .challenge(ty.clone())
                .ok_or_else(|| AcmeError::Other(format!("the CA does not offer {ty:?}")))?;
            let key_auth = challenge.key_authorization();
            match job.challenge {
                AcmeChallenge::Http01 => {
                    self.http.set(&challenge.token, key_auth.as_str());
                    cleanup.tokens.push(challenge.token.clone());
                }
                AcmeChallenge::TlsAlpn01 => {
                    let domain = challenge.identifier().to_string();
                    let key = alpn_challenge_cert(&domain, key_auth.digest().as_ref())?;
                    self.certs.set_challenge(&domain, Arc::new(key));
                    cleanup.domains.push(domain);
                }
            }
            challenge.set_ready().await?;
        }
        let retry = RetryPolicy::new()
            .initial_delay(Duration::from_millis(500))
            .timeout(Duration::from_secs(120));
        let status = order.poll_ready(&retry).await?;
        if status != OrderStatus::Ready {
            let detail = order
                .state()
                .error
                .as_ref()
                .and_then(|p| p.detail.clone())
                .unwrap_or_default();
            return Err(AcmeError::Other(format!(
                "order is {status:?} {detail}; check that the domains point to this server and \
                 the listener port is reachable from the internet"
            )));
        }
        let key_pem = order.finalize().await?;
        let chain_pem = order.poll_certificate(&retry).await?;
        drop(cleanup);
        Ok(self.store.save(
            CertSource::Acme,
            &job.interface,
            chain_pem.as_bytes(),
            key_pem.as_bytes(),
        )?)
    }
}

fn same_set(a: &[String], b: &[String]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// Short stable id for a directory URL, so accounts for different CAs (or
/// staging and production) are kept apart.
fn fingerprint(url: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    format!("{:016x}", h.finish())
}

/// The self-signed certificate TLS-ALPN-01 validation expects (RFC 8737).
fn alpn_challenge_cert(
    domain: &str,
    digest: &[u8],
) -> Result<rustls::sign::CertifiedKey, AcmeError> {
    let mut params = rcgen::CertificateParams::new(vec![domain.to_owned()])
        .map_err(|e| AcmeError::Other(e.to_string()))?;
    params
        .custom_extensions
        .push(rcgen::CustomExtension::new_acme_identifier(digest));
    let key = rcgen::KeyPair::generate().map_err(|e| AcmeError::Other(e.to_string()))?;
    let cert = params
        .self_signed(&key)
        .map_err(|e| AcmeError::Other(e.to_string()))?;
    // Not `certified_key_from_pem`: its consistency check parses the
    // certificate, and rustls rightly refuses the critical acmeIdentifier
    // extension that this certificate must carry.
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into());
    let signing = rustls::crypto::ring::sign::any_supported_type(&key_der)
        .map_err(|e| AcmeError::Other(e.to_string()))?;
    Ok(rustls::sign::CertifiedKey::new(
        vec![cert.der().clone()],
        signing,
    ))
}

/// Removes challenge answers when issuance ends, however it ends.
struct Cleanup<'a> {
    http: &'a HttpChallenges,
    certs: &'a Certificates,
    tokens: Vec<String>,
    domains: Vec<String>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for t in &self.tokens {
            self.http.clear(t);
        }
        for d in &self.domains {
            self.certs.clear_challenge(d);
        }
    }
}
