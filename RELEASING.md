# Building and releasing Circle

## Platforms

Every build produces these packages (`cargo xtask dist`, also run by CI):

| Platform | Runner | Packages |
| --- | --- | --- |
| macOS, Apple Silicon | `macos-latest` | `Circle-<v>-macos-arm64.dmg` |
| macOS, Intel | `macos-latest` (cross) | `Circle-<v>-macos-x86_64.dmg` |
| Windows, x86_64 | `windows-latest` | `Circle-<v>-windows-x86_64-setup.exe`, `Circle-<v>-windows-x86_64.zip` |
| Linux, x86_64 | `ubuntu-24.04` | `circle_<v>_amd64.deb`, `circle-<v>-linux-x86_64.tar.gz` |
| Linux, arm64 | `ubuntu-24.04-arm` | `circle_<v>_arm64.deb`, `circle-<v>-linux-aarch64.tar.gz` |

Each package bundles the matching CEF (Chromium) runtime and the
`circle-mcp` agent server. Unit tests run on every platform except Intel
macOS, which is cross-compiled.

## Channels

| Channel | When | Where | Workflow |
| --- | --- | --- | --- |
| **CI** | every push to `main` and every pull request | workflow artifacts, kept 14 days | `ci.yml` |
| **Nightly** | daily at 03:00 UTC if `main` changed, or on demand | the rolling [`nightly`](https://github.com/dobrinov/circle/releases/tag/nightly) pre-release | `nightly.yml` |
| **Pre-release** | a tag like `v0.2.0-beta.1` | a GitHub pre-release | `release.yml` |
| **Release** | a tag like `v0.2.0` | a GitHub release, offered on the website | `release.yml` |

All of them build through the reusable `build.yml`. The website's download
buttons use the latest (non-pre-)release, so nightlies and betas never
reach people who didn't ask for them.

## Versions

Circle follows [Semantic Versioning](https://semver.org/). While it is 0.x,
bump the minor version for new features and the patch version for fixes.
All three crates (`circle`, `circle-mcp`, `xtask`) share one version.

## Making a release

1. As changes land, describe them under **Unreleased** in `CHANGELOG.md`
   (Added, Changed, Fixed, Removed).
2. Make sure `main` is green in CI.
3. On a clean `main`:

   ```sh
   cargo xtask release 0.2.0          # or 0.2.0-beta.1 for a pre-release
   git push origin main v0.2.0
   ```

   `xtask release` sets the version everywhere, moves the Unreleased notes
   into a dated `0.2.0` section, refreshes `Cargo.lock`, commits
   "Release v0.2.0" and tags `v0.2.0`. It pushes nothing.
4. The **Release** workflow checks that the tag matches `Cargo.toml` and
   `CHANGELOG.md`, builds every platform, and publishes the release with
   the changelog section, a download table and `SHA256SUMS.txt`.

To redo a failed release, delete the GitHub release and the tag
(`git push --delete origin v0.2.0 && git tag -d v0.2.0`), fix, and tag again.

## Signing

Unsigned builds work, but macOS and Windows warn about them.

**macOS.** Without secrets, apps are signed ad hoc: people have to
right-click › Open the first time. To sign and notarize, add these
repository secrets (Settings › Secrets and variables › Actions):

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE` | base64 of your Developer ID Application `.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | its password |
| `MACOS_CODESIGN_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID`, `APPLE_APP_PASSWORD`, `APPLE_TEAM_ID` | for notarization (an app-specific password) |

**Windows.** Builds are unsigned, so SmartScreen warns on first run until
the installer builds reputation. Code signing would be added to
`xtask/src/windows.rs` once there is a certificate.

**Linux.** Packages are unsigned; checksums are in `SHA256SUMS.txt`.

## Website

The landing page is `docs/index.html`, served by GitHub Pages: Settings ›
Pages › Deploy from a branch › `main`, folder `/docs`. Its download links
find the latest release's files through the GitHub API, so they need no
changes per release.

The demo GIFs in `docs/assets/` show Circle's real window on GitHub, recorded
at 16:10 (the shape the page shows them in): launch Circle with
`--window-size=1280x800`, hide the sidebar unless the demo is about it, capture
frames of the window (PNG files plus `frames.json`, see `xtask/src/gif.rs`) and
encode them with:

```sh
cargo xtask gif <frames-dir> docs/assets/<name>.gif --width 1000 --bg F6F3FB
```
