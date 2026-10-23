# Circle

A calm, sidebar-first web browser in the spirit of Arc: the arc, completed.
Written in Rust on top of Chromium via the
[Chromium Embedded Framework](https://github.com/chromiumembedded/cef)
([`cef-rs`](https://github.com/tauri-apps/cef-rs)).

## What it does

- **Vertical sidebar** with back/forward/reload, the address and your tabs;
  it animates in and out (`⌘S`)
- **Quiet window chrome**: an even frame around rounded pages; the macOS traffic
  lights live in the sidebar. With the sidebar hidden, point at the left edge to
  peek at it (lights included)
- **Favorites** — a grid of icons shared by every space
- **Spaces** — separate sets of pinned and today tabs, each with its own color;
  switch with the dots, `Ctrl+1…9` (macOS) / `Alt+1…9`, `⌘⌥←/→`, or a two-finger swipe on the sidebar
- **Pinned tabs** that remember their home page; closing one unloads and resets it
- **Folders** for pinned tabs: collapsible, created from a space's ••• menu or a
  tab's right-click menu; drag tabs in and out
- **Rename tabs**: double-click a tab's title (or right-click › Rename…); the
  page title is used until you do
- **Today tabs** that archive themselves after 12 idle hours (configurable)
- **Command bar** (`⌘T`, `⌘L`): search, open URLs, jump to open tabs, reopen
  recently closed tabs and run commands
- **Split view** (`⌘⇧=` or right-click → *Open in Split View*), side by side
  or stacked (`⌘⇧-`, or *Open in Split View Below*)
- **Find in page** (`⌘F`), downloads list, site permission prompts in the sidebar
- Drag tabs between favorites, pinned, today and other spaces; right-click for more
- **Developer tools** (`⌥⌘I`, View › Developer, or right-click › Inspect Element),
  docked right or bottom (drag the edge to resize) or in a separate window
- **AI agent control**: agents like Claude Code can drive Circle through MCP
  (see below); off until you allow it
- **History** (`⌘Y` / `Ctrl+H`): searchable, grouped by day, entries removable;
  also offered in Spotlight. **Clear browsing data** (`⌘⇧⌫`) by time range:
  history, cookies, cache
- **Local dev servers stand out**: tabs on `localhost`, `*.localhost`, `127.x.x.x`,
  `[::1]` or `0.0.0.0` get a "DEV" badge in the sidebar and address bar (which
  also shows the port), and on macOS a striped frame in the space's color
- **Record the page** as a video or a GIF: `⌥⌘R` (View › Record Video… or the
  command bar) shows the recording controls in the sidebar; pick Video or GIF,
  press **Start recording**, then **Stop** to save it to Downloads.
  Videos are MP4 (AV1; WebM where unavailable), GIFs up to 60 s at ~20 fps
- **Bug reports** (Help › Report a Bug…, the command bar or Settings): answer
  a few questions, reproduce the problem while Circle logs navigations, actions,
  page errors, failed loads and crashes, then get editable GitHub-issue text
  (addresses trimmed to sites unless you opt in)
- **Fap mode** 🙈 (`⇧⌘N` / `Ctrl+Shift+N`): private browsing in a dark space whose tabs use a
  memory-only session (no cookies, cache or storage on disk) and leave nothing
  behind: no history, no recently closed, not saved, no remembered permissions,
  invisible to AI agents and to other spaces' Spotlight. On macOS the window is
  hidden from screen sharing and screenshots while it shows. `⇧⌘.` ends it at
  once, closing every private tab
- **Automatic updates**: Circle checks GitHub Releases every few hours,
  downloads the new version in the background, verifies it against the
  release's `SHA256SUMS.txt` and installs it when you quit (or press
  *Restart* in the sidebar). Settings › Updates turns it off. A `.deb`
  install in `/opt` is told about new versions instead
- **Light and dark themes**, or follow the system; websites get the same
  `prefers-color-scheme`
- **Settings** (`⌘,` / `Ctrl+,`, Circle › Settings…, or the command bar):
  theme, search engine, tab archiving, site permissions, DevTools position,
  agent access, and a shortcut list
- **Downloads panel**: the button at the bottom of the sidebar opens a list of
  downloads and recordings (with progress and Show in folder) and closed tabs
- Sessions persist across restarts; can be set as the system default browser

Keyboard shortcuts use `⌘` on macOS and `Ctrl` elsewhere.

| Shortcut | Action |
| --- | --- |
| `⌘K` | Spotlight: switch tabs, open a site, or search |
| `⌘T` / `⌘L` | Command bar (new tab / edit address) |
| `⌘W`, `⌘⇧T` | Close tab, reopen closed tab |
| `⌘S` | Toggle sidebar |
| `⌘D` | Pin / unpin tab |
| `⌘1…9`, `Ctrl+Tab`, `⌘⌥↑/↓` | Switch tabs |
| `⌘⇧=` | Toggle split view |
| `⌘F` | Find in page |
| `⌘⇧C` | Copy URL |
| `⌘[` `⌘]`, `⌘R` | Back, forward, reload |
| `⌘⌥I` / `F12` | Developer tools |
| `⌘⌥U` | View page source |

## AI agents

Circle can be controlled by AI agents through the
[Model Context Protocol](https://modelcontextprotocol.io). It is off by
default:

1. In Circle, open the command bar (`⌘T`) and run **Allow Agent Control**.
   While an agent is acting, the sidebar shows **Agent in control** with a
   **Stop** button.
2. Register the bundled MCP server with your agent, e.g. Claude Code:

   ```sh
   claude mcp add circle -- /Applications/Circle.app/Contents/MacOS/circle-mcp   # macOS
   claude mcp add circle -- /opt/circle/circle-mcp                               # Linux (.deb)
   ```

   On Windows it is `circle-mcp.exe` next to `circle.exe`.
3. Optionally install the skill that teaches agents how to use it well:
   `cp -r skills/circle-browser ~/.claude/skills/`.

Tools: `circle_tabs`, `circle_open`, `circle_navigate`, `circle_activate`,
`circle_close`, `circle_back`/`forward`/`reload`, `circle_read`,
`circle_snapshot` (interactive elements with refs), `circle_click`,
`circle_type`, `circle_press`, `circle_scroll`, `circle_wait_for`,
`circle_screenshot` and `circle_eval`.

Agents act in your real browser with your logins, so only allow agents you
trust. Access ends when you turn it off or quit Circle; the token is
regenerated on every launch.

## Building

Requirements: Rust (stable), CMake and Ninja; on macOS, Xcode command-line tools.
The first build downloads the CEF binary distribution (~300 MB) for the target.
Set `CEF_PATH` to share one download between builds:

```sh
export CEF_PATH="$HOME/.local/share/cef"
```

```sh
cargo xtask run                 # build, bundle and launch (debug)
cargo xtask bundle --release    # just assemble the app into dist/<os>-<arch>/
cargo xtask dist                # release build + installers in dist/
cargo test --lib                # unit tests
```

`cargo xtask dist` produces:

| Platform | Output |
| --- | --- |
| macOS | `Circle-<ver>-macos-<arch>.dmg` (ad-hoc signed; set `CIRCLE_CODESIGN_IDENTITY`, and `APPLE_ID`/`APPLE_APP_PASSWORD`/`APPLE_TEAM_ID` to sign and notarize) |
| Linux | `circle-<ver>-linux-<arch>.tar.gz` and `circle_<ver>_<arch>.deb` |
| Windows | `Circle-<ver>-windows-<arch>.zip` and, with Inno Setup installed, `…-setup.exe` |

Cross-architecture builds work with `--target`, e.g.
`cargo xtask dist --target x86_64-apple-darwin` on an Apple Silicon Mac.

GitHub Actions (`.github/workflows/build.yml`) builds all of the above for
macOS (arm64, x86_64), Linux (x86_64, arm64) and Windows (x86_64), and attaches
them to a GitHub release when a `v*` tag is pushed.

## Releases

Download Circle from the [website](https://dobrinov.github.io/circle/) or
[GitHub Releases](https://github.com/dobrinov/circle/releases); a rolling
[nightly](https://github.com/dobrinov/circle/releases/tag/nightly) build
tracks `main`. What changed is in [CHANGELOG.md](CHANGELOG.md), and how
builds and releases work (platforms, channels, signing, the website) is in
[RELEASING.md](RELEASING.md).

## How it is put together

```
src/
  main.rs, bin/circle-helper.rs   process entry points (macOS needs a helper)
  lib.rs     CEF initialization and the Windows sandbox entry point
  app.rs     CEF App: custom scheme registration, relaunch handling
  controller.rs  the browser: tabs, spaces, layout, command bar, persistence
  client.rs  per-browser CEF handlers, tagged with their tab
  views.rs   CEF Views window and browser view delegates
  scheme.rs  circle://ui/ — serves the UI and receives its commands
  remote.rs  local CDP bridge: DevTools windows and the agent API
  agent.rs   agent commands (tabs, snapshot, click, type, screenshot, …)
  model.rs   the persisted session (plain data, unit tested)
  keys.rs    keyboard shortcuts
  mac.rs     NSApplication subclass, menu bar, default-browser URL handling
ui/          the sidebar and command bar (HTML/CSS/JS, embedded in the binary)
mcp/         circle-mcp, the MCP server agents talk to
skills/      a Claude skill for driving Circle
xtask/       bundling and packaging for each OS
```

The window is built with CEF Views: the sidebar and each tab are separate
browser views, and the command bar is an overlay. All of them use CEF's
*Alloy* style because a Chrome-style window can only host one Chrome-style
browser. The UI pages talk to Rust with `fetch("circle://ui/api")`. Only the
UI's own browsers may use that endpoint, and the scheme is display-isolated,
so websites cannot reach it. Rust pushes state back with `circle.render(...)`.

DevTools and agents use a second channel: a server on `127.0.0.1` (random
port) that speaks the Chrome DevTools Protocol by relaying to CEF's
`SendDevToolsMessage`, rather than enabling Chromium's unauthenticated
remote-debugging port. DevTools windows use a per-launch secret that never
leaves the process; agents use a token that Circle writes to `agent.json` in
the profile (owner-only permissions) only while agent control is allowed. Requests
must target a loopback host name, which blocks DNS-rebinding attacks from web
pages. Several clients can attach to a tab at once; Circle rewrites request
ids so each gets its own responses.

The profile (cookies, cache, `session.json`) lives in
`~/Library/Application Support/Circle`, `%LOCALAPPDATA%\Circle` or
`~/.local/share/circle`. Set `CIRCLE_PROFILE_DIR` to use another one, e.g.
`CIRCLE_PROFILE_DIR=/tmp/circle-dev cargo xtask run`; `CIRCLE_DOWNLOADS_DIR`
likewise redirects downloads and recordings. `--window-size=1280x800` (or
`CIRCLE_WINDOW_SIZE`) opens the window at an exact size, e.g. for recordings
at the shape they'll be shown in.

## Known limitations

- Rounded page corners and the striped dev-server frame are drawn on macOS only.
- Recordings capture the page itself (not the sidebar), only while its tab is
  shown. Chromium here has no H.264 encoder, so videos are AV1 MP4: they play in
  browsers, VLC and Windows, and in QuickTime on Macs with M3 or newer.
- One window; no incognito, sync, extensions, password manager or autofill UI
  (Alloy style doesn't include Chrome's UI for these).
- Find shows a match count but does not yet step the "active" match on the first search.
- Windows and Linux builds are produced by CI; only macOS has been run by hand so far.
- Linux: the sandbox needs either unprivileged user namespaces or the setuid
  `chrome-sandbox` helper. The `.deb` sets this up; with the tarball, run
  `sudo chown root:root chrome-sandbox && sudo chmod 4755 chrome-sandbox` or
  start with `--no-sandbox`.
- Until releases are signed with a Developer ID and notarized, macOS says
  "Apple could not verify Circle.app" the first time. Choose *Done*, then
  *System Settings → Privacy & Security → Open Anyway* (or run
  `xattr -dr com.apple.quarantine /Applications/Circle.app`). macOS also asks once for keychain access to "Circle Safe Storage" (the key
  that encrypts cookies); choose *Always Allow*. Debug builds use a mock
  keychain so rebuilds don't re-prompt.
