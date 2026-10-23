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

**macOS.** Releases must be signed with a Developer ID and notarized: the
Release workflow fails on macOS without these secrets, so nothing ships that
macOS refuses to open. (CI and nightly builds without them are signed ad hoc.)
Add them under Settings › Secrets and variables › Actions:

| Secret | Value |
| --- | --- |
| `MACOS_CERTIFICATE` | base64 of your Developer ID Application `.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | its password |
| `MACOS_CODESIGN_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | the Apple Account email of the developer account |
| `APPLE_APP_PASSWORD` | an app-specific password for it |
| `APPLE_TEAM_ID` | the 10-character team ID |

One-time setup (needs a paid Apple Developer Program membership; only the
account holder can create Developer ID certificates):

1. **Certificate.** In Xcode › Settings › Accounts, select the team, then
   *Manage Certificates…* › **+** › *Developer ID Application*. (Or on
   developer.apple.com › Certificates › **+** › *Developer ID Application*,
   with a CSR from Keychain Access › Certificate Assistant.)
2. **Export it.** In Keychain Access › login › My Certificates, right-click
   *Developer ID Application: …* › *Export…* as `circle.p12` with a password.
   Then `base64 -i circle.p12 | pbcopy` and paste that as `MACOS_CERTIFICATE`.
3. **Identity.** `security find-identity -v -p codesigning` prints the exact
   name for `MACOS_CODESIGN_IDENTITY`; the team ID is in the parentheses.
4. **App-specific password.** account.apple.com › Sign-In and Security ›
   App-Specific Passwords › **+** (name it "Circle notarization").
5. Delete `circle.p12` afterwards.

Each macOS build then signs the app with the hardened runtime
(`packaging/macos/*.entitlements` give Chromium its JIT), signs and
notarizes the disk image, staples the ticket, and checks the result with
`spctl` before anything is published.

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

The page shows each demo as a 1280×800 WebM (VP9) video, with that GIF as the
fallback for browsers without VP9. The videos are made from the same frames
by replaying them into a canvas in Circle and recording it with MediaRecorder,
so no other tools are needed.

## Updates

Installed copies check `releases/latest` every 6 hours (and from
Settings › Updates), download their platform's file, verify it against
`SHA256SUMS.txt` and install it on quit. That means every release must keep
the file names in the table above and include `SHA256SUMS.txt`, which the
Release workflow does. Pre-releases and the nightly are never offered.
