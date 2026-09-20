//! Patch notes. `CHANGELOG.md` is the single source: it is compiled into the binary and
//! parsed into the structure the "Patch notes" tab renders.

use axum::Json;
use serde::Serialize;
use serde_json::{Value, json};

use crate::auth::AuthUser;

const SOURCE: &str = include_str!("../CHANGELOG.md");

#[derive(Debug, Serialize, PartialEq)]
pub struct Release {
    pub version: String,
    pub date: Option<String>,
    pub summary: Option<String>,
    pub groups: Vec<Group>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Group {
    /// "Added", "Changed", "Fixed", "Removed"…
    pub kind: String,
    pub items: Vec<String>,
}

/// Reads `## [version] - date` sections with `### Kind` groups of `- ` bullets.
/// Anything before the first release heading (the file's own introduction) is ignored.
pub fn parse(source: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = vec![];
    for line in source.lines() {
        let trimmed = line.trim_end();
        if let Some(heading) = trimmed.strip_prefix("## ") {
            let (version, date) = match heading.split_once(" - ") {
                Some((v, d)) => (v, Some(d.trim().to_string())),
                None => (heading, None),
            };
            let version = version.trim().trim_start_matches('[').trim_end_matches(']').to_string();
            releases.push(Release { version, date, summary: None, groups: vec![] });
            continue;
        }
        let Some(release) = releases.last_mut() else { continue };
        if let Some(kind) = trimmed.strip_prefix("### ") {
            release.groups.push(Group { kind: kind.trim().to_string(), items: vec![] });
        } else if let Some(item) = trimmed.trim_start().strip_prefix("- ") {
            match release.groups.last_mut() {
                Some(group) => group.items.push(item.trim().to_string()),
                None => release.groups.push(Group { kind: "Changed".into(), items: vec![item.trim().to_string()] }),
            }
        } else if !trimmed.trim().is_empty() {
            let text = trimmed.trim();
            match release.groups.last_mut().and_then(|g| g.items.last_mut()) {
                // A wrapped bullet continues on the next line.
                Some(item) => {
                    item.push(' ');
                    item.push_str(text);
                }
                None => match &mut release.summary {
                    Some(s) => {
                        s.push(' ');
                        s.push_str(text);
                    }
                    None => release.summary = Some(text.to_string()),
                },
            }
        }
    }
    releases
}

pub async fn changelog(_user: AuthUser) -> Json<Value> {
    Json(json!({ "current": env!("CARGO_PKG_VERSION"), "releases": parse(SOURCE) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_releases_groups_and_wrapped_bullets() {
        let r = parse("# Notes\nintro is ignored\n\n## [1.1.0] - 2026-01-02\n\nA summary\nover two lines.\n\n### Added\n- one\n- two that\n  wraps\n\n### Fixed\n- three\n\n## [1.0.0]\n- loose bullet\n");
        assert_eq!(r.len(), 2);
        assert_eq!((r[0].version.as_str(), r[0].date.as_deref()), ("1.1.0", Some("2026-01-02")));
        assert_eq!(r[0].summary.as_deref(), Some("A summary over two lines."));
        assert_eq!(r[0].groups[0], Group { kind: "Added".into(), items: vec!["one".into(), "two that wraps".into()] });
        assert_eq!(r[0].groups[1].items, vec!["three"]);
        assert_eq!((r[1].date.as_deref(), r[1].groups[0].kind.as_str()), (None, "Changed"));
    }

    /// The shipped file must be well-formed and must describe the version being built,
    /// so a release can't go out without its notes.
    #[test]
    fn the_shipped_changelog_covers_this_version() {
        let releases = parse(SOURCE);
        assert_eq!(releases.first().map(|r| r.version.as_str()), Some(env!("CARGO_PKG_VERSION")), "add a CHANGELOG.md entry for this version");
        for r in &releases {
            assert!(r.version.split('.').count() == 3 && r.version.split('.').all(|p| p.parse::<u32>().is_ok()), "bad version heading: {}", r.version);
            assert!(r.date.as_deref().is_some_and(|d| d.len() == 10), "release {} needs a YYYY-MM-DD date", r.version);
            assert!(r.groups.iter().any(|g| !g.items.is_empty()), "release {} has no notes", r.version);
            assert!(r.groups.iter().all(|g| ["Added", "Changed", "Fixed", "Removed"].contains(&g.kind.as_str())), "unknown group in {}", r.version);
            // The app folds releases by minor series and headlines each fold with its x.y.0 summary.
            if r.version.ends_with(".0") {
                assert!(r.summary.as_deref().is_some_and(|s| s.len() >= 8), "release {} opens a series and needs a one-sentence summary: it is that series' headline in Patch notes", r.version);
            }
        }
    }
}
