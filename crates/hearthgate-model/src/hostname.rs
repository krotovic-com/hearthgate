/// Lowercases a host name and strips a trailing dot.
#[must_use]
pub fn normalize_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Checks a DNS host name. A single leading `*.` label is allowed for
/// wildcard domains when `allow_wildcard` is set.
#[must_use]
pub fn is_valid_hostname(host: &str, allow_wildcard: bool) -> bool {
    let host = host
        .strip_prefix("*.")
        .filter(|_| allow_wildcard)
        .unwrap_or(host);
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// Whether `host` is matched by `pattern`. A `*.example.com` pattern matches
/// exactly one extra label (`a.example.com`, not `example.com` or
/// `a.b.example.com`), like a wildcard certificate.
#[must_use]
pub fn host_matches(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(suffix)
            .and_then(|rest| rest.strip_suffix('.'))
            .is_some_and(|label| !label.is_empty() && !label.contains('.')),
        None => pattern == host,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames() {
        assert!(is_valid_hostname("app.example.com", false));
        assert!(is_valid_hostname("localhost", false));
        assert!(!is_valid_hostname("*.example.com", false));
        assert!(is_valid_hostname("*.example.com", true));
        assert!(!is_valid_hostname("App.example.com", false));
        assert!(!is_valid_hostname("-a.example.com", false));
        assert!(!is_valid_hostname("a..b", false));
        assert!(!is_valid_hostname("", false));
    }

    #[test]
    fn wildcard_matching() {
        assert!(host_matches("app.example.com", "app.example.com"));
        assert!(!host_matches("app.example.com", "x.example.com"));
        assert!(host_matches("*.example.com", "a.example.com"));
        assert!(!host_matches("*.example.com", "example.com"));
        assert!(!host_matches("*.example.com", "a.b.example.com"));
        assert!(!host_matches("*.example.com", "aexample.com"));
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize_host(" App.Example.COM. "), "app.example.com");
    }
}
