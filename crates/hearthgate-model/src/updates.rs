//! Updates of Hearthgate itself: whether the dashboard shows new releases,
//! and which ones are installed on their own.

use serde::{Deserialize, Serialize};

/// Update settings. Hearthgate checks for new releases and tells you, but
/// installs nothing by itself unless `auto_install` says so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Updates {
    /// Ask GitHub once a day whether a newer release exists.
    #[serde(default = "yes")]
    pub check: bool,
    /// Which releases are installed without asking. Major releases never
    /// are.
    #[serde(default)]
    pub auto_install: AutoInstall,
    /// When automatic installs may start, in this machine's local time, as
    /// `HH:MM-HH:MM`. It may run past midnight, such as `23:00-01:00`.
    #[serde(default = "default_window")]
    pub window: String,
}

fn yes() -> bool {
    true
}

fn default_window() -> String {
    "03:00-05:00".to_owned()
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            check: true,
            auto_install: AutoInstall::default(),
            window: default_window(),
        }
    }
}

impl Updates {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The install window as minutes after midnight, start and end.
    #[must_use]
    pub fn window_minutes(&self) -> Option<(u16, u16)> {
        parse_window(&self.window)
    }
}

/// Which releases are installed without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AutoInstall {
    /// Nothing; new releases are only shown.
    #[default]
    Off,
    /// Patch releases, such as 1.2.0 to 1.2.1.
    Patch,
    /// Minor and patch releases, such as 1.2.1 to 1.3.0.
    Minor,
}

/// Parses `HH:MM-HH:MM` into minutes after midnight.
#[must_use]
pub fn parse_window(text: &str) -> Option<(u16, u16)> {
    let (start, end) = text.trim().split_once('-')?;
    let (start, end) = (parse_clock(start)?, parse_clock(end)?);
    (start != end).then_some((start, end))
}

fn parse_clock(text: &str) -> Option<u16> {
    let (h, m) = text.trim().split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u16, u16) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Whether `minute` (after midnight) falls inside the window.
#[must_use]
pub fn in_window((start, end): (u16, u16), minute: u16) -> bool {
    if start < end {
        (start..end).contains(&minute)
    } else {
        minute >= start || minute < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows() {
        assert_eq!(parse_window("03:00-05:00"), Some((180, 300)));
        assert_eq!(parse_window("23:30-01:00"), Some((1410, 60)));
        assert_eq!(parse_window("3:00-05:00"), None);
        assert_eq!(parse_window("24:00-05:00"), None);
        assert_eq!(parse_window("05:00-05:00"), None);
        assert_eq!(parse_window("nonsense"), None);
        assert!(in_window((180, 300), 180));
        assert!(!in_window((180, 300), 300));
        assert!(in_window((1410, 60), 1435));
        assert!(in_window((1410, 60), 10));
        assert!(!in_window((1410, 60), 600));
    }

    #[test]
    fn defaults_install_nothing() {
        let u = Updates::default();
        assert!(u.check);
        assert_eq!(u.auto_install, AutoInstall::Off);
        assert_eq!(u.window_minutes(), Some((180, 300)));
    }
}
