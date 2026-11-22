//! Redaction: what leaves the machine has no IP addresses, domains, email
//! addresses, URLs or secrets in it.
//!
//! Each distinct value becomes a stable placeholder such as `<PUBLIC_IP_1>`
//! or `app.<DOMAIN_2>`, so the model can still tell that two mentions are
//! the same thing, and its answer can be mapped back with
//! [`Redactor::unredact`]. Placeholders keep what matters for a review
//! (whether an address is public, private or loopback; a CIDR's prefix
//! length; subdomain labels) and drop what identifies the owner.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

static PEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN [A-Z0-9 ]+-----[\s\S]*?-----END [A-Z0-9 ]+-----").unwrap()
});
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s"'<>`]+"#).unwrap());
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}").unwrap()
});
/// `password=...`, `Authorization: Bearer ...` and the like.
static ASSIGNED_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)\b(bearer|basic|token|password|passwd|pwd|secret|api[_-]?key|apikey|access[_-]?key|private[_-]?key|preshared[_-]?key|authorization)(\s*[:=]\s*(?:(?:bearer|basic|token)\s+)?|\s+)("?)([^\s"',;<>]{6,})"#,
    )
    .unwrap()
});
/// Long random-looking strings: keys and tokens.
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_=-]{24,}").unwrap());
static IPV4: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[^0-9A-Za-z.])((?:\d{1,3}\.){3}\d{1,3})(?:$|[^0-9A-Za-z])").unwrap()
});
static IPV6: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[^0-9A-Za-z:_])((?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}(?:%[0-9A-Za-z]+)?)",
    )
    .unwrap()
});
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[A-Z_]+_\d+>").unwrap());
static DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^A-Za-z0-9.<_-])((?:\*\.)?(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z][a-z0-9-]{1,62})\b").unwrap()
});

/// File extensions and similar suffixes that look like top-level domains.
const NOT_TLDS: &[&str] = &[
    "toml", "json", "yaml", "yml", "rs", "conf", "cfg", "ini", "service", "socket", "target",
    "timer", "mount", "sock", "log", "txt", "md", "sh", "py", "js", "ts", "html", "css", "pem",
    "crt", "key", "csr", "der", "gz", "tgz", "tar", "zip", "xz", "db", "sqlite", "lock", "pid",
    "env", "tmp", "bak", "old", "new", "d", "so", "exe", "bin", "img", "iso", "deb", "rpm", "svg",
    "png", "jpg", "svelte", "go", "rb", "java", "c", "h", "cpp", "nft", "wg", "conf",
];

/// Object keys whose string values are always secret.
const SECRET_KEYS: &[&str] = &[
    "password",
    "secret",
    "token",
    "api_key",
    "apikey",
    "private_key",
    "preshared_key",
    "public_key",
];

/// Replaces sensitive text with stable placeholders, and maps them back.
#[derive(Debug, Default)]
pub struct Redactor {
    to_placeholder: HashMap<String, String>,
    to_original: HashMap<String, String>,
    counts: HashMap<&'static str, usize>,
    custom: Vec<Regex>,
}

impl Redactor {
    /// A redactor that also hides whatever `patterns` (regular expressions)
    /// match.
    ///
    /// # Errors
    /// When a pattern is not a valid regular expression.
    pub fn new(patterns: &[String]) -> Result<Self, regex::Error> {
        Ok(Self {
            custom: patterns
                .iter()
                .map(|p| Regex::new(p))
                .collect::<Result<_, _>>()?,
            ..Self::default()
        })
    }

    /// How many distinct values were replaced.
    #[must_use]
    pub fn len(&self) -> usize {
        self.to_placeholder.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.to_placeholder.is_empty()
    }

    fn placeholder(&mut self, kind: &'static str, original: &str) -> String {
        if let Some(p) = self.to_placeholder.get(original) {
            return p.clone();
        }
        let n = self.counts.entry(kind).or_default();
        *n += 1;
        let p = format!("<{kind}_{n}>");
        self.to_placeholder.insert(original.to_owned(), p.clone());
        self.to_original.insert(p.clone(), original.to_owned());
        p
    }

    /// `text` with every sensitive value replaced.
    pub fn redact(&mut self, text: &str) -> String {
        let mut out = text.to_owned();
        out = self.replace(&PEM, &out, |r, c| r.placeholder("SECRET", &c[0]));
        out = self.replace(&URL, &out, |r, c| r.placeholder("URL", &c[0]));
        out = self.replace(&EMAIL, &out, |r, c| r.placeholder("EMAIL", &c[0]));
        out = self.replace(&ASSIGNED_SECRET, &out, |r, c| {
            if c[4].starts_with('<') {
                return c[0].to_owned();
            }
            format!(
                "{}{}{}{}",
                &c[1],
                &c[2],
                &c[3],
                r.placeholder("SECRET", &c[4])
            )
        });
        out = self.replace(&TOKEN, &out, |r, c| {
            let t = &c[0];
            if looks_random(t) {
                r.placeholder("SECRET", t)
            } else {
                t.to_owned()
            }
        });
        out = self.replace_group(&IPV6, &out, |r, ip| {
            let addr: IpAddr = ip.split('%').next()?.parse().ok()?;
            Some(r.ip_placeholder(addr, ip))
        });
        out = self.replace_group(&IPV4, &out, |r, ip| {
            let addr: IpAddr = ip.parse().ok()?;
            Some(r.ip_placeholder(addr, ip))
        });
        out = self.replace_group(&DOMAIN, &out, Redactor::domain_placeholder);
        // Last, so they don't hide what the built-in kinds would find (a
        // pattern for `photos` keeps `photos.example.com` a domain), and
        // never inside a placeholder.
        for i in 0..self.custom.len() {
            let re = self.custom[i].clone();
            let taken: Vec<(usize, usize)> = PLACEHOLDER
                .find_iter(&out)
                .map(|m| (m.start(), m.end()))
                .collect();
            out = self.replace(&re, &out, |r, c| {
                let m = c.get(0).expect("group 0");
                if taken.iter().any(|&(a, b)| m.start() < b && a < m.end()) {
                    c[0].to_owned()
                } else {
                    r.placeholder("REDACTED", &c[0])
                }
            });
        }
        out
    }

    fn ip_placeholder(&mut self, addr: IpAddr, original: &str) -> String {
        let kind = if addr.is_loopback() {
            "LOOPBACK_IP"
        } else if addr.is_unspecified() {
            "ANY_IP"
        } else if is_private(addr) {
            "PRIVATE_IP"
        } else {
            "PUBLIC_IP"
        };
        self.placeholder(kind, original)
    }

    /// `app.example.com` becomes `app.<DOMAIN_1>`: subdomain labels stay,
    /// the registered domain goes.
    fn domain_placeholder(&mut self, name: &str) -> Option<String> {
        let lower = name.to_ascii_lowercase();
        let labels: Vec<&str> = lower.split('.').collect();
        let tld = *labels.last()?;
        if NOT_TLDS.contains(&tld) || labels.len() < 2 {
            return None;
        }
        // example.co.uk keeps three labels together.
        let keep = if labels.len() >= 3 && tld.len() == 2 && labels[labels.len() - 2].len() <= 3 {
            3
        } else {
            2
        };
        let keep = keep.min(labels.len());
        let split = labels.len() - keep;
        let registered = labels[split..].join(".");
        let prefix = &labels[..split];
        let p = self.placeholder("DOMAIN", &registered);
        Some(if prefix.is_empty() {
            p
        } else {
            format!("{}.{p}", prefix.join("."))
        })
    }

    fn replace(
        &mut self,
        re: &Regex,
        text: &str,
        mut f: impl FnMut(&mut Self, &Captures<'_>) -> String,
    ) -> String {
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for caps in re.captures_iter(text) {
            let m = caps.get(0).expect("group 0");
            out.push_str(&text[last..m.start()]);
            out.push_str(&f(self, &caps));
            last = m.end();
        }
        out.push_str(&text[last..]);
        out
    }

    /// Like `replace`, for patterns whose group 1 is the value and whose
    /// surroundings must stay. Matches can share their boundary characters,
    /// so this scans repeatedly from the end of the last value.
    fn replace_group(
        &mut self,
        re: &Regex,
        text: &str,
        mut f: impl FnMut(&mut Self, &str) -> Option<String>,
    ) -> String {
        let mut out = String::with_capacity(text.len());
        let mut pos = 0;
        while pos <= text.len() {
            let Some(caps) = re.captures_at(text, pos) else {
                break;
            };
            let g = caps.get(1).expect("group 1");
            out.push_str(&text[pos..g.start()]);
            match f(self, g.as_str()) {
                Some(p) => out.push_str(&p),
                None => out.push_str(g.as_str()),
            }
            pos = g.end();
        }
        if pos < text.len() {
            out.push_str(&text[pos..]);
        }
        out
    }

    /// Redacts every string in a JSON document. Values under secret-sounding
    /// keys, and every container environment value, are hidden whole.
    pub fn redact_json(&mut self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.redact(s),
            Value::Array(items) => items.iter_mut().for_each(|v| self.redact_json(v)),
            Value::Object(map) => {
                for (key, v) in map.iter_mut() {
                    let key = key.to_ascii_lowercase();
                    if key == "env"
                        && let Value::Object(env) = v
                    {
                        for (_, e) in env.iter_mut() {
                            if let Value::String(s) = e
                                && !s.is_empty()
                            {
                                *s = self.placeholder("SECRET", s);
                            }
                        }
                        continue;
                    }
                    if SECRET_KEYS.iter().any(|k| key == *k)
                        && let Value::String(s) = v
                        && !s.is_empty()
                    {
                        *s = self.placeholder("SECRET", s);
                        continue;
                    }
                    self.redact_json(v);
                }
            }
            _ => {}
        }
    }

    /// `text` with every placeholder this redactor made put back.
    #[must_use]
    pub fn unredact(&self, text: &str) -> String {
        PLACEHOLDER
            .replace_all(text, |c: &Captures<'_>| {
                self.to_original
                    .get(&c[0])
                    .cloned()
                    .unwrap_or_else(|| c[0].to_owned())
            })
            .into_owned()
    }

    /// Puts the originals back into every string of a JSON document.
    pub fn unredact_json(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.unredact(s),
            Value::Array(items) => items.iter_mut().for_each(|v| self.unredact_json(v)),
            Value::Object(map) => map.values_mut().for_each(|v| self.unredact_json(v)),
            _ => {}
        }
    }
}

/// Mixed letters and digits, like a key or token, rather than a word or path.
fn looks_random(t: &str) -> bool {
    let digits = t.chars().filter(char::is_ascii_digit).count();
    let upper = t.chars().filter(char::is_ascii_uppercase).count();
    let lower = t.chars().filter(char::is_ascii_lowercase).count();
    let hex = t.len() >= 32 && t.chars().all(|c| c.is_ascii_hexdigit());
    let mixed = digits >= 2 && upper >= 2 && lower >= 2;
    let path_like = t.contains("//") || t.matches('/').count() > 3 || t.matches('-').count() > 3;
    hex || (mixed && !path_like)
}

fn is_private(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_link_local() || {
                // Carrier-grade NAT, 100.64.0.0/10.
                let o = v4.octets();
                o[0] == 100 && (o[1] & 0xc0) == 64
            }
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments()[0];
            (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str) -> (String, Redactor) {
        let mut red = Redactor::new(&[]).unwrap();
        (red.redact(text), red)
    }

    #[test]
    fn hides_addresses_and_keeps_their_kind() {
        let (out, red) = r(
            "from 203.0.113.7 and 192.168.1.20:8080, net 10.0.0.0/8, lo 127.0.0.1, v6 2001:db8::1 and fd00::5 and [::1]:22",
        );
        assert_eq!(
            out,
            "from <PUBLIC_IP_2> and <PRIVATE_IP_2>:8080, net <PRIVATE_IP_3>/8, lo <LOOPBACK_IP_2>, v6 <PUBLIC_IP_1> and <PRIVATE_IP_1> and [<LOOPBACK_IP_1>]:22"
        );
        assert_eq!(
            red.unredact(&out),
            "from 203.0.113.7 and 192.168.1.20:8080, net 10.0.0.0/8, lo 127.0.0.1, v6 2001:db8::1 and fd00::5 and [::1]:22"
        );
    }

    #[test]
    fn same_value_same_placeholder() {
        let (out, _) = r("203.0.113.7 then 203.0.113.7 then 203.0.113.8");
        assert_eq!(out, "<PUBLIC_IP_1> then <PUBLIC_IP_1> then <PUBLIC_IP_2>");
    }

    #[test]
    fn hides_registered_domains_but_keeps_subdomains() {
        let (out, red) = r("cloud.example.com, *.example.com, shop.example.co.uk and nas.lan");
        assert_eq!(
            out,
            "cloud.<DOMAIN_1>, *.<DOMAIN_1>, shop.<DOMAIN_2> and <DOMAIN_3>"
        );
        assert_eq!(
            red.unredact(&out),
            "cloud.example.com, *.example.com, shop.example.co.uk and nas.lan"
        );
    }

    #[test]
    fn leaves_files_versions_and_times_alone() {
        let (out, _) = r(
            "read /etc/hearthgate/hearthgate.toml at 12:30:45, unit hearthgate.service, version 1.0.0, ratio 0.5",
        );
        assert_eq!(
            out,
            "read /etc/hearthgate/hearthgate.toml at 12:30:45, unit hearthgate.service, version 1.0.0, ratio 0.5"
        );
    }

    #[test]
    fn hides_urls_emails_and_secrets() {
        let (out, _) = r(
            "hook https://hooks.slack.com/services/T000/B000/XXXX mail me@example.com \
             Authorization: Bearer abcDEF123456ghiJKL key=uK3vT9qLm2Xc8RbN0aYpW5sE7dF4hJ6gZ1oI= password=hunter2hunter",
        );
        assert!(!out.contains("slack"), "{out}");
        assert!(!out.contains("example.com"), "{out}");
        assert!(!out.contains("abcDEF123456ghiJKL"), "{out}");
        assert!(
            !out.contains("uK3vT9qLm2Xc8RbN0aYpW5sE7dF4hJ6gZ1oI"),
            "{out}"
        );
        assert!(!out.contains("hunter2"), "{out}");
        assert!(out.contains("Authorization: Bearer <SECRET_"), "{out}");
    }

    #[test]
    fn hides_pem_blocks() {
        let (out, _) = r(
            "x -----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n-----END PRIVATE KEY----- y",
        );
        assert_eq!(out, "x <SECRET_1> y");
    }

    #[test]
    fn custom_patterns() {
        let mut red = Redactor::new(&["(?i)project-[a-z]+".to_owned()]).unwrap();
        assert_eq!(
            red.redact("deploy Project-Falcon now"),
            "deploy <REDACTED_1> now"
        );
    }

    #[test]
    fn json_env_and_secret_keys() {
        let mut v = serde_json::json!({
            "containers": [{"name": "db", "env": {"POSTGRES_PASSWORD": "pw", "TZ": "Europe/Prague"}}],
            "vpn": {"peers": [{"name": "phone", "public_key": "abc", "address": "10.8.0.2"}]},
            "interfaces": [{"name": "web", "domains": ["cloud.example.com"]}]
        });
        let mut red = Redactor::new(&[]).unwrap();
        red.redact_json(&mut v);
        let text = v.to_string();
        assert!(
            !text.contains("Europe/Prague") && !text.contains("\"pw\""),
            "{text}"
        );
        assert!(
            !text.contains("abc") && !text.contains("10.8.0.2"),
            "{text}"
        );
        assert!(
            text.contains("\"db\"") && text.contains("cloud.<DOMAIN_1>"),
            "{text}"
        );
        red.unredact_json(&mut v);
        assert_eq!(v["interfaces"][0]["domains"][0], "cloud.example.com");
        assert_eq!(v["containers"][0]["env"]["TZ"], "Europe/Prague");
    }
}
