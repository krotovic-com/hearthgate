//! Small helpers for secrets: random tokens, hashing, constant-time
//! comparison, password hashing and TOTP codes.

use std::fmt::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// Shortest password accepted for a dashboard account.
pub const MIN_PASSWORD_LEN: usize = 12;
/// Longest password accepted (argon2 is slow on purpose; this bounds it).
pub const MAX_PASSWORD_LEN: usize = 1024;

/// Seconds since the Unix epoch.
#[must_use]
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// `n` random bytes.
pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; n];
    // The OS random source failing is unrecoverable for anything security
    // related, so treat it like an allocation failure.
    getrandom::fill(&mut bytes).expect("the operating system's random source failed");
    bytes
}

/// Lowercase hex of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// A random token of `n` bytes, as hex.
#[must_use]
pub fn random_token(n: usize) -> String {
    hex(&random_bytes(n))
}

/// SHA-256 of `text`, as hex. Tokens are stored only in this form.
#[must_use]
pub fn sha256_hex(text: &str) -> String {
    hex(&Sha256::digest(text.as_bytes()))
}

/// Compares in constant time, so a secret cannot be guessed byte by byte.
#[must_use]
pub fn equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Why a password was refused.
pub fn check_password_policy(password: &str) -> Result<(), String> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(format!(
            "use at least {MIN_PASSWORD_LEN} characters for the password"
        ));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(format!(
            "use at most {MAX_PASSWORD_LEN} bytes for the password"
        ));
    }
    Ok(())
}

/// Hashes a password with argon2id (OWASP's recommended parameters).
pub fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::encode_b64(&random_bytes(16)).map_err(|e| e.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Checks a password against a stored hash.
#[must_use]
pub fn verify_password(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// A hash of a random password, verified against when the username does
/// not exist so that the response takes as long as a real check.
pub fn dummy_hash() -> &'static str {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| hash_password(&random_token(16)).unwrap_or_default())
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 base32 without padding, as authenticator apps expect.
#[must_use]
pub fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Decodes base32, ignoring case, spaces and padding.
#[must_use]
pub fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0;
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let c = c.to_ascii_uppercase();
        let v = u32::try_from(BASE32.iter().position(|&b| char::from(b) == c)?).ok()?;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// TOTP step length (RFC 6238 default).
pub const TOTP_STEP: i64 = 30;

/// The 6-digit TOTP code for `secret` at time step `step`.
#[must_use]
pub fn totp_code(secret: &[u8], step: i64) -> u32 {
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(secret).expect("HMAC takes any key length");
    mac.update(&step.to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = usize::from(digest[19] & 0x0f);
    let value = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    value % 1_000_000
}

/// The time step a code matches, allowing one step of clock drift either
/// way. Returns `None` for a wrong code.
#[must_use]
pub fn totp_match(secret_b32: &str, code: &str, now: i64) -> Option<i64> {
    let secret = unbase32(secret_b32)?;
    let code = code.trim().replace(' ', "");
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let wanted: u32 = code.parse().ok()?;
    let current = now / TOTP_STEP;
    (current - 1..=current + 1).find(|&step| totp_code(&secret, step) == wanted)
}

/// An `otpauth://` URI for authenticator apps.
#[must_use]
pub fn totp_uri(secret_b32: &str, username: &str) -> String {
    let label: String = username
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("otpauth://totp/Hearthgate:{label}?secret={secret_b32}&issuer=Hearthgate")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords() {
        let hash = hash_password("correct horse battery").unwrap();
        assert!(hash.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery", &hash));
        assert!(!verify_password("wrong horse battery", &hash));
        assert!(!verify_password("anything", "not a hash"));
        assert!(check_password_policy("short").is_err());
        assert!(check_password_policy("long enough pass").is_ok());
    }

    #[test]
    fn base32_round_trip() {
        // RFC 4648 test vectors.
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(base32(b"f"), "MY");
        assert_eq!(unbase32("mzxw 6ytb oi").unwrap(), b"foobar");
        assert_eq!(unbase32("MZXW6YTBOI======").unwrap(), b"foobar");
        assert_eq!(unbase32("not*base32"), None);
    }

    #[test]
    fn totp_rfc6238_vectors() {
        // RFC 6238 appendix B, SHA-1, truncated to 6 digits.
        let secret = b"12345678901234567890";
        assert_eq!(totp_code(secret, 59 / 30), 287_082);
        assert_eq!(totp_code(secret, 1_111_111_109 / 30), 81_804);
        assert_eq!(totp_code(secret, 1_234_567_890 / 30), 5_924);
        let b32 = base32(secret);
        assert_eq!(totp_match(&b32, "287082", 59), Some(1));
        // One step of drift is accepted, two are not.
        assert_eq!(totp_match(&b32, "287082", 59 + 30), Some(1));
        assert_eq!(totp_match(&b32, "287082", 59 + 60), None);
        assert_eq!(totp_match(&b32, "28708", 59), None);
    }

    #[test]
    fn otpauth_uri_escapes_the_label() {
        assert_eq!(
            totp_uri("ABC", "lukas k"),
            "otpauth://totp/Hearthgate:lukas%20k?secret=ABC&issuer=Hearthgate"
        );
    }
}
