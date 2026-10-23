//! `cargo xtask release <version>`: prepares a release commit and tag.
//!
//! Sets the version of every crate, turns CHANGELOG.md's "Unreleased"
//! section into one for the version, refreshes Cargo.lock, then commits and
//! tags `v<version>`. Pushing the tag starts the Release workflow, which
//! builds and publishes it (see RELEASING.md). Nothing is pushed from here.

use anyhow::{Context, Result, bail};
use std::{fs, path::Path, process::Command};

const MANIFESTS: [&str; 3] = ["Cargo.toml", "mcp/Cargo.toml", "xtask/Cargo.toml"];

pub fn run(args: &[String]) -> Result<()> {
    let [version] = args else {
        bail!("usage: cargo xtask release <version>   (e.g. 0.2.0 or 0.2.0-beta.1)");
    };
    let version = version.trim_start_matches('v');
    check_version(version)?;

    let status = git(&["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        bail!("commit or stash your changes first:\n{status}");
    }
    let tag = format!("v{version}");
    if !git(&["tag", "--list", &tag])?.trim().is_empty() {
        bail!("tag {tag} already exists");
    }

    let changelog = fs::read_to_string("CHANGELOG.md").context("reading CHANGELOG.md")?;
    let changelog = stamp_changelog(&changelog, version, &today())?;
    for manifest in MANIFESTS {
        set_version(Path::new(manifest), version)?;
    }
    fs::write("CHANGELOG.md", changelog)?;
    run_command(Command::new("cargo").args(["update", "--workspace", "--quiet"]))?;

    let mut files = vec!["CHANGELOG.md", "Cargo.lock"];
    files.extend(MANIFESTS);
    git(&[&["add", "--"], files.as_slice()].concat())?;
    git(&["commit", "--quiet", "-m", &format!("Release {tag}")])?;
    git(&["tag", "-a", &tag, "-m", &format!("Circle {version}")])?;
    println!("Committed and tagged {tag}. Publish it with:\n\n    git push origin main {tag}\n");
    Ok(())
}

/// Accepts `MAJOR.MINOR.PATCH` with an optional `-prerelease` suffix.
fn check_version(version: &str) -> Result<()> {
    let (core, pre) = version.split_once('-').unwrap_or((version, ""));
    let parts: Vec<&str> = core.split('.').collect();
    let numeric = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    let pre_ok = pre.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
    if !numeric || !pre_ok || version.ends_with('-') {
        bail!("{version} isn't a version like 0.2.0 or 0.2.0-beta.1");
    }
    Ok(())
}

/// Moves the notes under "## [Unreleased]" to a new "## [version] - date"
/// section, leaving an empty Unreleased section above it.
fn stamp_changelog(text: &str, version: &str, date: &str) -> Result<String> {
    const UNRELEASED: &str = "## [Unreleased]";
    let start = text
        .find(UNRELEASED)
        .context("CHANGELOG.md has no \"## [Unreleased]\" section")?;
    let body_start = start + UNRELEASED.len();
    let body_end = text[body_start..]
        .find("\n## [")
        .map_or(text.len(), |i| body_start + i);
    if text[body_start..body_end].trim().is_empty() {
        bail!("CHANGELOG.md's Unreleased section is empty: describe what changed first");
    }
    Ok(format!(
        "{}{UNRELEASED}\n\n## [{version}] - {date}{}",
        &text[..start],
        &text[body_start..]
    ))
}

/// Replaces the `version = "…"` line of a manifest's `[package]` section.
fn set_version(manifest: &Path, version: &str) -> Result<()> {
    let text =
        fs::read_to_string(manifest).with_context(|| format!("reading {}", manifest.display()))?;
    let mut in_package = false;
    let mut done = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') {
                in_package = trimmed == "[package]";
            } else if in_package && !done && trimmed.starts_with("version") {
                done = true;
                return format!("version = \"{version}\"");
            }
            line.to_owned()
        })
        .collect();
    if !done {
        bail!("no [package] version in {}", manifest.display());
    }
    fs::write(manifest, lines.join("\n") + "\n")?;
    Ok(())
}

fn today() -> String {
    Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|d| d.len() == 10)
        .unwrap_or_else(|| "unreleased".to_owned())
}

fn git(args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn run_command(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("running {command:?}"))?;
    if !status.success() {
        bail!("{command:?} failed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        for ok in ["0.2.0", "1.10.3", "0.2.0-beta.1", "1.0.0-rc1"] {
            assert!(check_version(ok).is_ok(), "{ok}");
        }
        for bad in ["0.2", "v0.2.0", "0.2.x", "0.2.0-", "0.2.0-beta_1", ""] {
            assert!(check_version(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn stamps_the_changelog() {
        let text =
            "# Changelog\n\n## [Unreleased]\n\n- New thing\n\n## [0.1.0] - 2026-10-23\n\n- First\n";
        let out = stamp_changelog(text, "0.2.0", "2026-11-01").unwrap();
        assert_eq!(
            out,
            "# Changelog\n\n## [Unreleased]\n\n## [0.2.0] - 2026-11-01\n\n- New thing\n\n## [0.1.0] - 2026-10-23\n\n- First\n"
        );
        let empty = "## [Unreleased]\n\n## [0.1.0] - 2026-10-23\n";
        assert!(stamp_changelog(empty, "0.2.0", "x").is_err());
    }
}
