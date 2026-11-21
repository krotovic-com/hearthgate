//! Durations written for people, such as `30s`, `1h 30m` or `7days`, and
//! timestamps in RFC 3339.
//!
//! Parsing accepts everything earlier releases wrote (they used the
//! `humantime` crate), including `months` (30.44 days) and `years`
//! (365.25 days).

use std::fmt::Write as _;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Why a duration didn't parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DurationError {
    #[error("value was empty")]
    Empty,
    #[error("expected a number at {0}")]
    NumberExpected(usize),
    #[error("a unit is needed after {0}, for example {0}s or {0}ms")]
    UnitNeeded(String),
    #[error(
        "unknown time unit {0:?}; use ms, s, m, h, days, weeks, months or years (and a few variations)"
    )]
    UnknownUnit(String),
    #[error("the number is too large")]
    Overflow,
}

fn unit_nanos(unit: &str) -> Option<u128> {
    const S: u128 = 1_000_000_000;
    Some(match unit {
        "nanos" | "nsec" | "ns" => 1,
        "usec" | "us" | "µs" => 1_000,
        "millis" | "msec" | "ms" => 1_000_000,
        "seconds" | "second" | "secs" | "sec" | "s" => S,
        "minutes" | "minute" | "min" | "mins" | "m" => 60 * S,
        "hours" | "hour" | "hr" | "hrs" | "h" => 3_600 * S,
        "days" | "day" | "d" => 86_400 * S,
        "weeks" | "week" | "wk" | "wks" | "w" => 604_800 * S,
        "months" | "month" | "M" => 2_630_016 * S,
        "years" | "year" | "yr" | "yrs" | "y" => 31_557_600 * S,
        _ => return None,
    })
}

/// Parses a duration such as `90s`, `1h 30m`, `1.5h` or `7days`.
pub fn parse_duration(text: &str) -> Result<Duration, DurationError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(DurationError::Empty);
    }
    if text == "0" {
        return Ok(Duration::ZERO);
    }
    let mut total: u128 = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let offset = text.len() - rest.len();
        let digits = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number = &rest[..digits];
        if number.is_empty() || number.starts_with('.') || number.matches('.').count() > 1 {
            return Err(DurationError::NumberExpected(offset));
        }
        rest = &rest[digits..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c.is_whitespace())
            .unwrap_or(rest.len());
        let unit = &rest[..unit_len];
        rest = rest[unit_len..].trim_start();
        if unit.is_empty() {
            return Err(DurationError::UnitNeeded(number.to_owned()));
        }
        let per = unit_nanos(unit).ok_or_else(|| DurationError::UnknownUnit(unit.to_owned()))?;
        let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
        let whole: u128 = whole.parse().map_err(|_| DurationError::Overflow)?;
        let mut nanos = whole.checked_mul(per).ok_or(DurationError::Overflow)?;
        if !fraction.is_empty() {
            // Up to nine fraction digits are exact enough for any unit.
            let digits = &fraction[..fraction.len().min(9)];
            let scale = 10u128.pow(u32::try_from(digits.len()).unwrap_or(9));
            let value: u128 = digits.parse().map_err(|_| DurationError::Overflow)?;
            nanos = nanos
                .checked_add(value * per / scale)
                .ok_or(DurationError::Overflow)?;
        }
        total = total.checked_add(nanos).ok_or(DurationError::Overflow)?;
    }
    let secs = u64::try_from(total / 1_000_000_000).map_err(|_| DurationError::Overflow)?;
    let nanos = u32::try_from(total % 1_000_000_000).unwrap_or(0);
    Ok(Duration::new(secs, nanos))
}

/// Formats a duration so that [`parse_duration`] reads it back exactly,
/// such as `1h 30m` or `7days`.
#[must_use]
pub fn format_duration(value: Duration) -> String {
    if value.is_zero() {
        return "0s".to_owned();
    }
    let secs = value.as_secs();
    let nanos = value.subsec_nanos();
    let parts = [
        (secs / 86_400, "days"),
        (secs % 86_400 / 3_600, "h"),
        (secs % 3_600 / 60, "m"),
        (secs % 60, "s"),
        (u64::from(nanos / 1_000_000), "ms"),
        (u64::from(nanos / 1_000 % 1_000), "us"),
        (u64::from(nanos % 1_000), "ns"),
    ];
    let mut out = String::new();
    for (n, unit) in parts {
        if n == 0 {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        let unit = if unit == "days" && n == 1 {
            "day"
        } else {
            unit
        };
        let _ = write!(out, "{n}{unit}");
    }
    out
}

/// `with = "hearthgate_model::duration::serde"` for `Duration` fields.
pub mod serde {
    use std::time::Duration;

    use ::serde::{Deserialize, Deserializer, Serializer};

    /// Writes the duration as text, such as `1h 30m`.
    pub fn serialize<S: Serializer>(value: &Duration, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&super::format_duration(*value))
    }

    /// Reads a duration written as text.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
        let text = String::deserialize(deserializer)?;
        super::parse_duration(&text).map_err(::serde::de::Error::custom)
    }
}

/// A time as RFC 3339 in UTC with whole seconds, such as
/// `2026-11-21T08:30:00Z`.
#[must_use]
pub fn format_rfc3339(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_earlier_releases_wrote() {
        let cases = [
            ("0", 0, 0),
            ("0s", 0, 0),
            ("30s", 30, 0),
            ("1h 30m", 5_400, 0),
            ("2h 37min", 9_420, 0),
            ("1day", 86_400, 0),
            ("7days", 604_800, 0),
            ("1month", 2_630_016, 0),
            ("1year 2months", 31_557_600 + 2 * 2_630_016, 0),
            ("1.5h", 5_400, 0),
            ("32ms", 0, 32_000_000),
            ("4.2s", 4, 200_000_000),
            ("1m30s", 90, 0),
        ];
        for (text, secs, nanos) in cases {
            assert_eq!(
                parse_duration(text),
                Ok(Duration::new(secs, nanos)),
                "{text}"
            );
        }
        for bad in [
            "",
            "5",
            "s",
            "1x",
            ".5s",
            "1..5s",
            "99999999999999999999999years",
        ] {
            assert!(parse_duration(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn formats_and_reads_back() {
        for d in [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(5_400),
            Duration::from_secs(86_400),
            Duration::from_secs(90 * 86_400 + 61),
            Duration::new(3, 1_002_003),
        ] {
            assert_eq!(parse_duration(&format_duration(d)), Ok(d), "{d:?}");
        }
        assert_eq!(format_duration(Duration::from_secs(5_400)), "1h 30m");
        assert_eq!(format_duration(Duration::from_secs(86_400)), "1day");
    }

    #[test]
    fn formats_rfc3339() {
        let at = |s| UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(format_rfc3339(at(0)), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(at(951_782_400)), "2000-02-29T00:00:00Z");
        assert_eq!(format_rfc3339(at(1_795_250_000)), "2026-11-21T08:33:20Z");
    }
}
