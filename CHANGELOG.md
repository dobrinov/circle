# Changelog

All notable changes to Circle. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and versions follow
[Semantic Versioning](https://semver.org/). Add entries under **Unreleased**
as you go; `cargo xtask release <version>` turns them into a release.

## [Unreleased]

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

### Fixed

- Collapsing the sidebar no longer squares off the window's left corners.
- Linux packages are stripped of debug symbols, so they are much smaller.

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
