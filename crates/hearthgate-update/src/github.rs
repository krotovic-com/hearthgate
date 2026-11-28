//! The GitHub Releases API, read-only and unauthenticated.

use serde::Deserialize;

use crate::{REPO, Version};

/// The release list, newest first.
#[must_use]
pub fn releases_url() -> String {
    format!("https://api.github.com/repos/{REPO}/releases?per_page=20")
}

/// Download URL of a release asset. Assets are always fetched from this
/// fixed place, never from a URL in the API answer.
#[must_use]
pub fn asset_url(version: Version, name: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{version}/{name}")
}

/// Web page of a release.
#[must_use]
pub fn release_page(version: Version) -> String {
    format!("https://github.com/{REPO}/releases/tag/v{version}")
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// RFC 3339, as GitHub gives it.
    pub published_at: String,
    /// The release notes, Markdown.
    pub body: String,
    /// Asset names.
    pub assets: Vec<String>,
}

#[derive(Deserialize)]
struct RawRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

#[derive(Deserialize)]
struct RawAsset {
    name: String,
}

/// Parses the release list and returns the stable releases, newest first.
pub fn parse_releases(json: &[u8]) -> Result<Vec<Release>, String> {
    let raw: Vec<RawRelease> =
        serde_json::from_slice(json).map_err(|e| format!("unexpected answer from GitHub: {e}"))?;
    let mut out: Vec<Release> = raw
        .into_iter()
        .filter(|r| !r.draft && !r.prerelease)
        .filter_map(|r| {
            Some(Release {
                version: Version::parse(&r.tag_name)?,
                published_at: r.published_at.unwrap_or_default(),
                body: r.body.unwrap_or_default(),
                assets: r.assets.into_iter().map(|a| a.name).collect(),
            })
        })
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.version));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_stable_releases_newest_first() {
        let json = br#"[
          {"tag_name":"v1.2.1","published_at":"2026-11-21T10:00:00Z","body":"b","assets":[{"name":"SHA256SUMS"}]},
          {"tag_name":"v1.4.0-rc.1","prerelease":true},
          {"tag_name":"v1.3.0","published_at":"2026-11-26T10:00:00Z","assets":[]},
          {"tag_name":"v9.9.9","draft":true},
          {"tag_name":"nightly"}
        ]"#;
        let r = parse_releases(json).unwrap();
        let versions: Vec<String> = r.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(versions, ["1.3.0", "1.2.1"]);
        assert_eq!(r[1].assets, ["SHA256SUMS"]);
        assert!(parse_releases(b"{\"message\":\"rate limited\"}").is_err());
    }

    #[test]
    fn urls_are_fixed() {
        let v = Version::parse("1.3.0").unwrap();
        assert_eq!(
            asset_url(v, "SHA256SUMS"),
            "https://github.com/krotovic-com/hearthgate/releases/download/v1.3.0/SHA256SUMS"
        );
    }
}
