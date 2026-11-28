//! Release notes, from the Markdown git-cliff writes into each release.

use serde::{Deserialize, Serialize};

/// What a change is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    Feature,
    Fix,
    Security,
    Breaking,
}

/// One line of release notes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Note {
    pub kind: NoteKind,
    pub text: String,
}

/// Most lines kept; the rest are on GitHub.
const MAX_NOTES: usize = 100;

/// Parses a release body: `### Features` and `### Bug fixes` sections of
/// `- **scope:** [**breaking**] Message (abc1234)` lines. Documentation and
/// refactoring sections are left out; they don't change what runs.
#[must_use]
pub fn parse(body: &str) -> Vec<Note> {
    let mut notes = Vec::new();
    let mut section = None;
    for line in body.lines() {
        let line = line.trim();
        if let Some(heading) = line.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim().to_ascii_lowercase();
            section = if heading.starts_with("feature") {
                Some(NoteKind::Feature)
            } else if heading.starts_with("bug")
                || heading.starts_with("fix")
                || heading.starts_with("perf")
            {
                Some(NoteKind::Fix)
            } else if heading.starts_with("security") {
                Some(NoteKind::Security)
            } else {
                None
            };
            continue;
        }
        let (Some(kind), Some(item)) = (section, line.strip_prefix("- ")) else {
            continue;
        };
        let mut text = item.trim().to_owned();
        // Trailing commit hash, such as " (abc1234)".
        if let Some(open) = text.rfind(" (")
            && text.ends_with(')')
            && text[open + 2..text.len() - 1]
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            text.truncate(open);
        }
        let breaking = text.contains("[**breaking**]");
        text = text.replace("[**breaking**]", "");
        let mut scope = None;
        if let Some(rest) = text.trim_start().strip_prefix("**")
            && let Some((s, after)) = rest.split_once(":**")
        {
            scope = Some(s.to_ascii_lowercase());
            text = after.to_owned();
        }
        let text = text.replace("**", "").replace('`', "").trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let lower = text.to_ascii_lowercase();
        let kind = if breaking {
            NoteKind::Breaking
        } else if scope.as_deref() == Some("security")
            || lower.contains("rustsec-")
            || lower.contains("cve-")
            || lower.contains("security")
        {
            NoteKind::Security
        } else {
            kind
        };
        notes.push(Note { kind, text });
        if notes.len() == MAX_NOTES {
            break;
        }
    }
    // Breaking and security changes first, then features, then fixes.
    notes.sort_by_key(|n| match n.kind {
        NoteKind::Breaking => 0,
        NoteKind::Security => 1,
        NoteKind::Feature => 2,
        NoteKind::Fix => 3,
    });
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_cliff_notes() {
        let body = "\n### Features\n- **dashboard:** Ask AI side panel on every screen (1a2b3c4)\n\
            - Check for updates from the dashboard (abcdef0)\n- **vpn:** [**breaking**] Rename port to `listen_port` (1111111)\n\n\
            ### Bug fixes\n- UPnP lease is renewed before it expires (2222222)\n\
            - **security:** Update rustls (3333333)\n\n### Documentation\n- Typo (4444444)\n";
        let notes = parse(body);
        let got: Vec<(NoteKind, &str)> = notes.iter().map(|n| (n.kind, n.text.as_str())).collect();
        assert_eq!(
            got,
            [
                (NoteKind::Breaking, "Rename port to listen_port"),
                (NoteKind::Security, "Update rustls"),
                (NoteKind::Feature, "Ask AI side panel on every screen"),
                (NoteKind::Feature, "Check for updates from the dashboard"),
                (NoteKind::Fix, "UPnP lease is renewed before it expires"),
            ]
        );
    }

    #[test]
    fn ignores_anything_else() {
        assert_eq!(parse(""), Vec::<Note>::new());
        assert_eq!(
            parse("Just some text\n- a list outside a section"),
            Vec::<Note>::new()
        );
    }
}
