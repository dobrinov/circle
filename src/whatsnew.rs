//! "What's new": the release notes shown once after Circle updates, taken
//! from the CHANGELOG.md that was current when this version was built.

use crate::updater::is_newer;

const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// The CHANGELOG sections (Markdown, each starting with its `## [x.y.z]`
/// heading) for versions newer than `since` up to and including `current`,
/// newest first. With no `since`, only `current`'s section.
pub fn notes(since: Option<&str>, current: &str) -> String {
    notes_from(CHANGELOG, since, current)
}

fn notes_from(changelog: &str, since: Option<&str>, current: &str) -> String {
    let mut out = Vec::new();
    for section in changelog.split("\n## [").skip(1) {
        let Some((version, _)) = section.split_once(']') else {
            continue;
        };
        let wanted = match since {
            Some(since) => is_newer(version, since) && !is_newer(version, current),
            None => version == current,
        };
        // Released versions only ("Unreleased" never parses as newer).
        if wanted && version != "Unreleased" {
            out.push(format!("## [{}", section.trim_end()));
        }
    }
    out.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n- wip\n\n\
        ## [0.2.0] - 2026-11-01\n\n### Added\n\n- Two\n\n\
        ## [0.1.1] - 2026-10-23\n\n### Fixed\n\n- One\n\n\
        ## [0.1.0] - 2026-10-01\n\n- First\n";

    #[test]
    fn picks_versions_between_since_and_current() {
        let notes = notes_from(LOG, Some("0.1.0"), "0.2.0");
        assert!(notes.starts_with("## [0.2.0] - 2026-11-01"));
        assert!(notes.contains("- Two") && notes.contains("- One"));
        assert!(!notes.contains("First") && !notes.contains("wip"));
    }

    #[test]
    fn without_since_only_the_current_version() {
        assert_eq!(
            notes_from(LOG, None, "0.1.1"),
            "## [0.1.1] - 2026-10-23\n\n### Fixed\n\n- One"
        );
    }

    #[test]
    fn nothing_for_an_unknown_version() {
        assert_eq!(notes_from(LOG, Some("0.2.0"), "0.2.0"), "");
        assert_eq!(notes_from(LOG, None, "9.9.9"), "");
    }

    #[test]
    fn the_real_changelog_has_this_version() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(
            notes(None, version).starts_with(&format!("## [{version}]")),
            "CHANGELOG.md has no section for {version}"
        );
    }
}
