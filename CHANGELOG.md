# Changelog

All notable changes to Circle. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Add entries under **Unreleased**
as you go; `cargo xtask release <version>` turns them into a release.

## [Unreleased]

### Security

- Updates install on macOS only if they're signed by Circle's developer and
  notarized; the updater no longer bypasses Gatekeeper. Downloads are HTTPS
  only, and a downloaded update is rechecked before it installs.
- Agents can only open and drive web pages (not local files or Circle's own
  pages), can never reach fap-mode tabs, and can't use browser-wide CDP
  commands. Turning agent control off disconnects them, and each time it's
  turned on there's a new token.
- Circle's own views can't be navigated to a website (e.g. by dropping a
  link on the sidebar), and its pages run only their own scripts.
- Cookies on macOS are encrypted with Circle's own keychain key instead of
  the one all Chromium-based apps share. *You'll be signed out of websites
  once after updating.*
- Files that run when opened (`.app`, `.pkg`, `.command`, `.exe`, scripts…)
  are saved only after you confirm them, pages can't flood Downloads, and
  pop-ups need a click.
- Fap mode: tabs opened from it stay in it, what you allowed outside it
  doesn't apply inside, nothing from it reaches bug reports or history, and
  windows it opens are hidden from screen capture too.
- `javascript:` addresses are searched for instead of run, and the search
  engine must be an http(s) address.
- Bug reports go to GitHub through the clipboard, not the address bar.
- Tighter macOS entitlements, matching Chrome's.

### Added

- *What's new*: after an update, Circle shows the release notes of the new
  version once (and again from Help › What's New in Circle).
- Recordings can show the pointer, clicks and the keys you press (passwords
  as dots), switchable per recording and in Settings › Recording.
- While the sidebar is hidden, recording controls float over the page instead
  of opening the sidebar. They never appear in the recording.

### Fixed

- Quitting Circle (⌘Q) could crash on the way out and leave a crash report.
- The Video/GIF switch in the sidebar's recording controls was squashed.

## [0.1.1] - 2026-10-23

### Added

- Automatic updates: new versions download in the background, are verified
  against the release checksums and install on quit or with *Restart*.
- A splash screen when a space has no open tab: a varied greeting and a
  motivational line, a ⌘K hint and artwork in the space's colours.

- Settings open as a panel over the blurred page (`⌘,`), with section
  navigation, search, theme previews and a *Blur behind Spotlight* switch.

### Changed

- A design pass over the sidebar: shorter tab titles (`owner/repo` on GitHub),
  the address shows the host with a muted path, *Pinned* and *Today* labels
  (with *Clear*), and one status tray above the space switcher for
  recording, downloads, updates, bug reports and agent control.
- The splash screen hints at everyday shortcuts.
- Private browsing is now called fap mode 🙈.
- Linux packages are stripped of debug symbols, so they are much smaller.

### Fixed

- Collapsing the sidebar no longer squares off the window's left corners.

## [0.1.0] - 2026-10-23

The first release.

### Added

- Sidebar-first window: back/forward/reload and the address in the sidebar,
  which animates away (`⌘S`) and peeks back from the left edge.
- Spaces with their own colour, favorites, pinned tabs, folders and
  renameable tabs; unpinned tabs archive themselves after a while.
- Spotlight (`⌘K`) for open tabs, history, sites and web search, and a
  command bar for everything else.
- Split view, side by side or stacked; find in page; a downloads panel; site
  permission prompts.
- `--window-size=WIDTHxHEIGHT` to open the window at an exact size.
- Developer tools docked right, bottom or in a window.
- History with search, and Clear Browsing Data.
- Settings (`⌘,`): theme, search engine, tab archiving, site permissions,
  DevTools position and agent access.
- Light, dark and system appearance.
- Local dev servers marked with a DEV badge (and a striped frame on macOS).
- Recording the page as an MP4 video or a GIF into Downloads.
- A private space (⇧⌘N): a memory-only session that leaves no trace, hidden
  from screen capture on macOS, ended at once with ⇧⌘.
- Bug reports: a short interview, a recorded log of what happens while you
  reproduce the problem, and ready-to-post GitHub issue text.
- AI agent control through a bundled MCP server (`circle-mcp`), off by
  default.
- Installers for macOS (`.dmg`), Windows (installer and `.zip`) and Linux
  (`.deb` and `.tar.gz`); x86_64 and arm64 except Windows (x86_64).
