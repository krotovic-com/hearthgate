//! When a release is installed without asking.

use hearthgate_model::{AutoInstall, Updates, in_window};

use crate::Bump;

/// Whether `setting` lets a release of this kind install by itself. Major
/// releases never do: they can change the configuration.
#[must_use]
pub fn allows(setting: AutoInstall, bump: Bump) -> bool {
    match (setting, bump) {
        (AutoInstall::Patch, Bump::Patch) | (AutoInstall::Minor, Bump::Patch | Bump::Minor) => true,
        (_, Bump::Major | Bump::Minor | Bump::Patch) => false,
    }
}

/// Minutes after midnight, in this machine's time zone.
#[must_use]
pub fn local_minute() -> u16 {
    let now = jiff::Zoned::now();
    u16::try_from(i32::from(now.hour()) * 60 + i32::from(now.minute())).unwrap_or(0)
}

/// Whether now is inside the install window.
#[must_use]
pub fn in_install_window(settings: &Updates, minute: u16) -> bool {
    settings.window_minutes().is_some_and(|w| in_window(w, minute))
}

/// Seconds until the install window next opens (0 when it is open).
#[must_use]
pub fn secs_until_window(settings: &Updates, minute: u16) -> u64 {
    let Some((start, _)) = settings.window_minutes() else {
        return 86_400;
    };
    if in_install_window(settings, minute) {
        return 0;
    }
    let wait = (i32::from(start) - i32::from(minute)).rem_euclid(1440);
    u64::try_from(wait).unwrap_or(0) * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_table() {
        use AutoInstall::{Minor, Off, Patch};
        for (setting, bump, want) in [
            (Off, Bump::Patch, false),
            (Off, Bump::Minor, false),
            (Off, Bump::Major, false),
            (Patch, Bump::Patch, true),
            (Patch, Bump::Minor, false),
            (Patch, Bump::Major, false),
            (Minor, Bump::Patch, true),
            (Minor, Bump::Minor, true),
            (Minor, Bump::Major, false),
        ] {
            assert_eq!(allows(setting, bump), want, "{setting:?} {bump:?}");
        }
    }

    #[test]
    fn window_waits() {
        let s = Updates::default(); // 03:00-05:00
        assert_eq!(secs_until_window(&s, 180), 0);
        assert_eq!(secs_until_window(&s, 179), 60);
        assert_eq!(secs_until_window(&s, 300), (1440 - 120) * 60);
        assert!(local_minute() < 1440);
    }
}
