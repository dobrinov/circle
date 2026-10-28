//! The browser controller: owns the session, the live tab views and the
//! window layout, and reacts to commands from CEF callbacks and the UI.
//!
//! All state lives on the CEF UI thread. Callbacks never touch the controller
//! directly; they [`dispatch`] a [`Cmd`] into a queue that is drained whenever
//! the controller is not already busy. This keeps re-entrant CEF callbacks
//! (which can fire synchronously while the controller calls into CEF) safe.

use crate::{
    client::{self, Role},
    model::{Appearance, DevToolsDock, Location, Section, Session, TabId, now},
    views::{self, SIDEBAR_WIDTH},
};
use cef::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    os::raw::c_int,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

const SIDEBAR_URL: &str = "circle://ui/sidebar.html";
const PALETTE_URL: &str = "circle://ui/palette.html";
const PEEK_URL: &str = "circle://ui/sidebar.html?peek";
const RECORDER_URL: &str = "circle://ui/recorder.html";
const RECBAR_URL: &str = "circle://ui/recbar.html";
const SPLITTER_URL: &str = "circle://ui/splitter.html";
const SPLIT_DROP_URL: &str = "circle://ui/splitdrop.html";
/// How far the floating recording controls sit above the page's bottom edge.
const RECBAR_MARGIN: c_int = 20;
const UPDATE_EVERY_MS: i64 = 6 * 60 * 60 * 1000;
/// The splash shown when the space has no open tab.
const START_PAGE_URL: &str = "circle://ui/start.html";
/// How many downloads the sidebar's list keeps.
const MAX_DOWNLOADS: usize = 50;
/// Recordings share the downloads list; their ids start here, well above
/// Chromium's download ids.
const RECORDING_IDS: u32 = 1 << 30;
/// Width of the hover strip along the left edge that reveals the sidebar.
const PEEK_STRIP: c_int = 8;
/// Roughly the radius of the window's rounded corners. The folded peek strip
/// stops this far from the top and bottom so it never squares them off; 12
/// is also one stripe period, so its dev-server stripes stay aligned.
const WINDOW_RADIUS: c_int = 12;
const TICK_MS: i64 = 10 * 60 * 1000;
const SAVE_DELAY_MS: i64 = 1500;
const GAP: c_int = 8;
#[cfg(target_os = "macos")]
const PAGE_RADIUS: f64 = 8.0;
const SIDEBAR_ANIMATION_MS: f64 = 260.0;
const FRAME_MS: i64 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteMode {
    NewTab,
    Edit,
    Find,
    /// ⌘K: switch tabs, open a site in a new tab, or search.
    Spotlight,
    /// ⌘,: the settings panel over the blurred page.
    Settings,
    /// The release notes, shown once after an update (and from Help).
    WhatsNew,
}

/// Things the user can trigger from the keyboard, menus or UI buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    OpenPalette(PaletteMode),
    CloseTab,
    ReopenClosed,
    ToggleSidebar,
    TogglePin,
    Reload,
    ReloadHard,
    Stop,
    Back,
    Forward,
    NextTab,
    PrevTab,
    NextSpace,
    PrevSpace,
    Space(usize),
    TabIndex(usize),
    ToggleSplit,
    /// Back to one page from split view.
    Unsplit,
    DuplicateTab,
    /// Split view side by side ↔ one above the other.
    ToggleSplitDirection,
    Zoom(i32),
    DevTools,
    /// Opens or closes the Network tool.
    Network,
    ViewSource,
    CopyUrl,
    Fullscreen,
    NewSpace,
    ClearToday,
    ToggleAgentAccess,
    DockDevTools(DevToolsDock),
    ShowHistory,
    ClearBrowsingData,
    Appearance(Appearance),
    ShowSettings,
    /// Shows the recording controls (the person then presses Start), or
    /// stops the recording in progress.
    Record(crate::recorder::Format),
    /// From the recording controls' Start button.
    StartRecording(crate::recorder::Format),
    CancelRecording,
    StopRecording,
    ReportBug,
    ShowWhatsNew,
    CheckForUpdates,
    /// Quits and installs the downloaded update, then relaunches.
    RestartToUpdate,
    /// Quits and starts again (for extensions turned off or updated).
    Restart,
    CloseExtensionPopup,
    /// Opens (or goes to) the private space.
    PrivateSpace,
    /// Closes the private space at once, with everything in it.
    EndPrivate,
    Quit,
}

/// Messages from the sidebar and command bar pages.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum UiCmd {
    Ready {
        from: String,
    },
    Navigate {
        input: String,
        #[serde(default)]
        new_tab: bool,
    },
    Activate {
        id: TabId,
    },
    Close {
        id: TabId,
    },
    Reset {
        id: TabId,
    },
    Unload {
        id: TabId,
    },
    Duplicate {
        id: TabId,
    },
    CopyUrl {
        id: TabId,
    },
    Split {
        id: TabId,
        /// One above the other instead of side by side.
        #[serde(default)]
        stacked: bool,
        /// This tab goes on the left (or top), the other on the right.
        #[serde(default)]
        first: bool,
    },
    /// A tab is being dragged from the sidebar (or the drag ended): offer to
    /// drop it into split view.
    TabDrag {
        #[serde(default)]
        id: Option<TabId>,
    },
    Move {
        id: TabId,
        section: Section,
        #[serde(default)]
        space: Option<u64>,
        index: usize,
        /// Place the tab just before this one (used by drag and drop).
        #[serde(default)]
        before: Option<TabId>,
    },
    MoveToSpace {
        id: TabId,
        space: u64,
    },
    Action {
        action: Action,
    },
    ClosePalette,
    PaletteHeight {
        height: c_int,
    },
    /// The drag handle between the pages and docked DevTools is being dragged.
    DevtoolsGrip {
        phase: String,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// The Network tool connects to a tab, sends it a CDP message, or
    /// stops listening to it.
    NetConnect {
        tab: TabId,
    },
    NetSend {
        tab: TabId,
        message: String,
    },
    NetDisconnect {
        tab: TabId,
    },
    /// The Network tool's edge is being dragged to resize it.
    NetworkGrip {
        phase: String,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// The password suggestions' or save prompt's size.
    AutofillSize {
        width: c_int,
        height: c_int,
    },
    /// A saved login picked from the suggestions under a field.
    AutofillPick {
        id: u64,
    },
    /// The save prompt: save (or update) the login.
    AutofillSave,
    /// The save prompt: never offer for this site.
    AutofillNever,
    AutofillClose,
    /// Open the Passwords settings.
    AutofillManage,
    /// Settings: show or copy a saved password (after Touch ID or Windows
    /// Hello), delete one, or allow saving on a site again.
    PasswordReveal {
        id: u64,
    },
    PasswordCopy {
        id: u64,
    },
    PasswordDelete {
        id: u64,
    },
    PasswordForgetNever {
        origin: String,
    },
    /// The Chrome Web Store card's size, and its buttons.
    ExtCardSize {
        width: c_int,
        height: c_int,
    },
    ExtAdd,
    ExtConfirm,
    ExtCancel,
    ExtManage,
    ExtCardClose,
    /// An extension's button next to the address bar: open its popup under
    /// `rect` (CSS px in the sidebar, or the peek sidebar).
    ExtOpen {
        id: String,
        x: c_int,
        y: c_int,
        width: c_int,
        height: c_int,
        #[serde(default)]
        peek: bool,
    },
    /// Settings: turn an extension on or off, pin its button, open its
    /// options, remove it, or load one unpacked.
    ExtEnable {
        id: String,
        on: bool,
    },
    ExtPin {
        id: String,
        pinned: bool,
    },
    ExtOptions {
        id: String,
    },
    ExtRemove {
        id: String,
    },
    ExtLoadUnpacked,
    /// Open the Chrome Web Store.
    ExtStore,
    /// The floating recording controls' size, to fit their overlay to.
    RecbarSize {
        width: c_int,
        height: c_int,
    },
    NewSpace {
        #[serde(default)]
        name: Option<String>,
    },
    RenameSpace {
        id: u64,
        name: String,
        #[serde(default)]
        icon: Option<String>,
    },
    RecolorSpace {
        id: u64,
    },
    SetSpaceHue {
        id: u64,
        hue: u16,
    },
    DeleteHistory {
        url: String,
        at: u64,
    },
    ClearBrowsingData {
        /// Clear history from this many hours ago (none = all time).
        #[serde(default)]
        hours: Option<u64>,
        #[serde(default)]
        history: bool,
        #[serde(default)]
        cookies: bool,
        #[serde(default)]
        cache: bool,
    },
    /// Changes from the settings page; fields left out stay as they are.
    Settings {
        #[serde(default)]
        appearance: Option<Appearance>,
        #[serde(default)]
        search_url: Option<String>,
        #[serde(default)]
        archive_after_hours: Option<u64>,
        #[serde(default)]
        devtools_dock: Option<DevToolsDock>,
        #[serde(default)]
        agent_access: Option<bool>,
        #[serde(default)]
        auto_update: Option<bool>,
        #[serde(default)]
        spotlight_blur: Option<bool>,
        #[serde(default)]
        recording_clicks: Option<bool>,
        #[serde(default)]
        recording_keys: Option<bool>,
        #[serde(default)]
        recording_float: Option<bool>,
        #[serde(default)]
        passwords_enabled: Option<bool>,
        #[serde(default)]
        passwords_offer: Option<bool>,
        /// A remembered site permission to forget.
        #[serde(default)]
        forget_grant: Option<String>,
    },
    /// The settings page loaded and wants the current values.
    SettingsReady,
    /// From the bug report page: start logging (and maybe recording video)
    /// while the person reproduces the problem.
    BugReportStart {
        #[serde(default)]
        video: bool,
    },
    /// Stop logging and show the report.
    BugReportStop,
    /// The bug report page loaded and wants the current state.
    BugReportReady,
    /// The peek sidebar's own traffic lights (macOS).
    WindowControl {
        op: String,
    },
    /// The pointer entered (or left) the peek sidebar.
    Peek {
        open: bool,
    },
    DeleteSpace {
        id: u64,
    },
    SwitchSpace {
        id: u64,
    },
    Reopen {
        url: String,
    },
    Find {
        text: String,
        #[serde(default)]
        backward: bool,
    },
    Permission {
        id: u64,
        allow: bool,
    },
    ClearDownloads,
    RevealDownload {
        id: u32,
    },
    RenameTab {
        id: TabId,
        name: String,
    },
    NewFolder {
        #[serde(default)]
        name: Option<String>,
        /// A tab to put in the new folder.
        #[serde(default)]
        tab: Option<TabId>,
    },
    RenameFolder {
        id: u64,
        name: String,
    },
    DeleteFolder {
        id: u64,
    },
    ToggleFolder {
        id: u64,
    },
    MoveToFolder {
        id: TabId,
        folder: Option<u64>,
        #[serde(default)]
        before: Option<TabId>,
    },
    Theme {
        dark: bool,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct DownloadInfo {
    pub id: u32,
    /// When it started (Unix seconds); set by the controller.
    pub at: u64,
    pub name: String,
    pub path: String,
    pub percent: c_int,
    pub complete: bool,
    pub failed: bool,
}

pub enum PermissionCallback {
    Media(MediaAccessCallback),
    Prompt(PermissionPromptCallback),
}

pub struct PermissionRequest {
    pub tab: TabId,
    pub cef_prompt_id: Option<u64>,
    pub origin: String,
    pub mask: u32,
    pub media: bool,
    pub callback: PermissionCallback,
}

impl PermissionRequest {
    fn grant_key(&self) -> String {
        let kind = if self.media { "media" } else { "prompt" };
        format!("{kind}:{}", self.origin)
    }

    fn describe(&self) -> Vec<&'static str> {
        use sys::cef_media_access_permission_types_t::*;
        use sys::cef_permission_request_types_t::*;
        let table: &[(u32, &str)] = if self.media {
            &[
                (
                    CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32,
                    "microphone",
                ),
                (CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32, "camera"),
                (
                    CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32,
                    "your screen",
                ),
            ]
        } else {
            &[
                (CEF_PERMISSION_TYPE_CAMERA_STREAM as u32, "camera"),
                (CEF_PERMISSION_TYPE_MIC_STREAM as u32, "microphone"),
                (CEF_PERMISSION_TYPE_GEOLOCATION as u32, "your location"),
                (CEF_PERMISSION_TYPE_NOTIFICATIONS as u32, "notifications"),
                (CEF_PERMISSION_TYPE_CLIPBOARD as u32, "the clipboard"),
                (CEF_PERMISSION_TYPE_MIDI_SYSEX as u32, "MIDI devices"),
                (
                    CEF_PERMISSION_TYPE_MULTIPLE_DOWNLOADS as u32,
                    "multiple downloads",
                ),
                (CEF_PERMISSION_TYPE_STORAGE_ACCESS as u32, "storage access"),
                (
                    CEF_PERMISSION_TYPE_WINDOW_MANAGEMENT as u32,
                    "window management",
                ),
            ]
        };
        let mut names: Vec<_> = table
            .iter()
            .filter(|(bit, _)| self.mask & bit != 0)
            .map(|(_, name)| *name)
            .collect();
        if names.is_empty() {
            names.push("additional permissions");
        }
        names
    }

    fn finish(self, allow: bool) {
        match self.callback {
            PermissionCallback::Media(callback) => {
                if allow {
                    callback.cont(self.mask);
                } else {
                    callback.cancel();
                }
            }
            PermissionCallback::Prompt(callback) => callback.cont(if allow {
                PermissionRequestResult::ACCEPT
            } else {
                PermissionRequestResult::DENY
            }),
        }
    }
}

/// Everything that can happen to the controller.
pub enum Cmd {
    Ui(UiCmd),
    /// Script for the recorder page (a video frame, mostly).
    RecorderJs(String),
    /// The screencast ended; the recorder page should finish the video.
    RecorderStop,
    /// The recording stopped and its file is being written.
    RecordingSaving,
    UpdateStatus(crate::updater::Status),
    CheckUpdates,
    /// Shortly after starting a newer version: show what changed.
    WhatsNew,
    /// Screenshots of the visible pages (rect within the content area, JPEG
    /// as base64) for the command bar's blurred backdrop.
    PaletteBackdrop(u64, Vec<(Rect, String)>),
    ShowPalette(u64),
    /// Something for the bug report log, from a browser callback.
    BugLog {
        tab: Option<TabId>,
        kind: &'static str,
        text: String,
        url: Option<String>,
    },
    RecordingFinished(Result<PathBuf, String>),
    Action(Action),
    WindowCreated(Window, Option<OverlayController>, Option<OverlayController>),
    WindowResized,
    Title(TabId, String),
    Url(TabId, String),
    Favicon(TabId, String),
    Loading {
        id: TabId,
        loading: bool,
        back: bool,
        forward: bool,
    },
    OpenUrl {
        url: String,
        background: bool,
        opener: Option<TabId>,
    },
    OpenUrls(Vec<String>),
    Fullscreen(TabId, bool),
    DragRegions(Vec<DraggableRegion>),
    FindResult {
        count: c_int,
        active: c_int,
    },
    Download(DownloadInfo),
    Permission(PermissionRequest),
    PermissionDismissed(u64),
    Save,
    Tick,
    AnimateSidebar,
    /// A page view moved or resized.
    PageLayout,
    /// A short notice shown at the bottom of the sidebar.
    Toast(String),
    /// Re-check whether an agent is still active.
    AgentIdle,
    /// Inspect the element at a point in a tab (from the context menu).
    Inspect(TabId, Point),
    /// CDP messages from a tab for the Network tool.
    NetworkCdp(TabId, Vec<String>),
    /// A docked DevTools front-end finished loading.
    DevToolsLoaded(TabId),
    /// A tab's page finished loading (main frame).
    PageLoaded(TabId),
    /// A page's password helper has news to collect.
    PasswordsPing(TabId),
    /// What a page's password helper collected, from its world `context`.
    PasswordsNews(TabId, i64, Vec<Value>),
    /// The person did (or didn't) confirm it's them, to show or copy a
    /// saved password.
    PasswordConfirmed {
        id: u64,
        copy: bool,
        ok: bool,
    },
    /// The docked DevTools' resize grip is being dragged.
    DevToolsGrip(TabId, crate::client::GripDrag),
    /// Chromium loaded (or failed to load) an extension.
    ExtLoaded {
        path: PathBuf,
        expected: Option<String>,
        purpose: extensions_ui::LoadPurpose,
        result: Result<String, String>,
    },
    ExtUninstalled {
        id: String,
        result: Result<(), String>,
    },
    /// A download from the Chrome Web Store, verified and unpacked.
    ExtFetched {
        id: String,
        result: Result<(PathBuf, crate::extensions::Manifest), String>,
    },
    ExtUpdated {
        id: String,
        path: PathBuf,
        version: String,
    },
    ExtUnpackedChosen(PathBuf),
    /// Circle's bridge extension told a page's Chrome tab id.
    ExtTabId(TabId, i64),
    /// An extension popup's page wants this size.
    ExtPopupSize(c_int, c_int, c_int),
    ExtPopupClosed(c_int),
    /// Stop waiting for extensions and show the window.
    ExtStartupTimeout,
    ExtCheckUpdates,
    /// Keyboard focus moved to a browser of this role.
    Focused(Role),
    BrowserCreated(Role, Browser),
}

thread_local! {
    static CONTROLLER: RefCell<Option<Controller>> = const { RefCell::new(None) };
    static QUEUE: RefCell<VecDeque<Cmd>> = const { RefCell::new(VecDeque::new()) };
    static CLOSING: Cell<bool> = const { Cell::new(false) };
    static WINDOW_GONE: Cell<bool> = const { Cell::new(false) };
    /// Inside `can_close`, which closing a browser can re-enter.
    static IN_CAN_CLOSE: Cell<bool> = const { Cell::new(false) };
    /// `can_close` was re-entered, so the window should try closing again.
    static CLOSE_AGAIN: Cell<bool> = const { Cell::new(false) };
}

/// Queues a command and runs the queue if the controller is idle. UI thread only.
pub fn dispatch(cmd: Cmd) {
    debug_assert_ne!(currently_on(ThreadId::UI), 0);
    QUEUE.with(|q| q.borrow_mut().push_back(cmd));
    pump();
}

fn pump() {
    CONTROLLER.with(|cell| {
        let Ok(mut guard) = cell.try_borrow_mut() else {
            // Already running further up the stack; it will drain the queue.
            return;
        };
        let Some(controller) = guard.as_mut() else {
            return;
        };
        loop {
            while let Some(cmd) = QUEUE.with(|q| q.borrow_mut().pop_front()) {
                controller.handle(cmd);
            }
            controller.flush();
            if QUEUE.with(|q| q.borrow().is_empty()) {
                break;
            }
        }
    });
}

/// Entry point for UI commands, which arrive on the IO thread.
pub fn dispatch_from_any_thread(cmd: UiCmd) {
    if currently_on(ThreadId::UI) != 0 {
        dispatch(Cmd::Ui(cmd));
    } else {
        let mut task = UiTask::new(Arc::new(Mutex::new(Some(cmd))));
        post_task(ThreadId::UI, Some(&mut task));
    }
}

#[derive(Clone, Copy)]
enum Timer {
    Save,
    Tick,
    Animate,
    AgentIdle,
    /// Shows the command bar even if its backdrop isn't ready yet.
    ShowPalette(u64),
    UpdateCheck,
    WhatsNew,
    ExtensionsStartup,
    ExtensionsUpdate,
}

fn schedule(timer: Timer, delay_ms: i64) {
    let mut task = TimerTask::new(timer);
    post_delayed_task(ThreadId::UI, Some(&mut task), delay_ms);
}

wrap_task! {
    struct UiTask {
        cmd: Arc<Mutex<Option<UiCmd>>>,
    }

    impl Task {
        fn execute(&self) {
            if let Some(cmd) = self.cmd.lock().unwrap().take() {
                dispatch(Cmd::Ui(cmd));
            }
        }
    }
}

wrap_task! {
    struct TimerTask {
        timer: Timer,
    }

    impl Task {
        fn execute(&self) {
            dispatch(match self.timer {
                Timer::Save => Cmd::Save,
                Timer::Tick => Cmd::Tick,
                Timer::Animate => Cmd::AnimateSidebar,
                Timer::AgentIdle => Cmd::AgentIdle,
                Timer::ShowPalette(generation) => Cmd::ShowPalette(generation),
                Timer::UpdateCheck => Cmd::CheckUpdates,
                Timer::WhatsNew => Cmd::WhatsNew,
                Timer::ExtensionsStartup => Cmd::ExtStartupTimeout,
                Timer::ExtensionsUpdate => Cmd::ExtCheckUpdates,
            });
        }
    }
}

/// Creates the controller and the main window. Called once CEF is ready.
pub fn start(profile_dir: &Path, initial_urls: Vec<String>) {
    crate::history::load(profile_dir);
    let mut controller = Controller::new(profile_dir);
    controller.apply_appearance();
    // An update downloaded last time, not installed yet.
    if let Some(update) = crate::updater::pending(&profile_dir.join("updates")) {
        controller.update_status = Some(crate::updater::Status::Ready {
            version: update.version,
        });
    }
    crate::remote::start(profile_dir);
    crate::remote::set_agent_access(controller.session.agent_access);
    // Pages restored at startup should get their extensions' scripts, so
    // the window waits (briefly) for extensions to load.
    if controller.begin_loading_extensions() {
        schedule(Timer::ExtensionsStartup, extensions_ui::STARTUP_WAIT_MS);
    } else {
        controller.show_window_once();
    }
    CONTROLLER.with(|cell| *cell.borrow_mut() = Some(controller));
    if !initial_urls.is_empty() {
        QUEUE.with(|q| q.borrow_mut().push_back(Cmd::OpenUrls(initial_urls)));
    }
    schedule(Timer::ExtensionsUpdate, 60_000);
    schedule(Timer::Tick, TICK_MS);
    // The first update check waits until startup has settled.
    schedule(Timer::UpdateCheck, 15_000);
    pump();
}

/// Whether the main window is shutting down.
pub fn is_window_closing() -> bool {
    CLOSING.get()
}

/// Asks every browser to close; the window may close once all of them have.
pub fn can_close() -> c_int {
    // Closing a browser can ask the window to close again from inside
    // `try_close_browser`; asking the same browsers twice trips a CHECK in
    // CEF. Say "not yet" and try again once this call is done.
    if IN_CAN_CLOSE.replace(true) {
        CLOSE_AGAIN.set(true);
        return 0;
    }
    if !CLOSING.replace(true) {
        CONTROLLER.with(|cell| {
            if let Ok(guard) = cell.try_borrow()
                && let Some(controller) = guard.as_ref()
            {
                controller.save();
            }
        });
    }
    let mut ready = true;
    for browser in client::live_browsers() {
        if let Some(host) = browser.host()
            && host.try_close_browser() == 0
        {
            ready = false;
        }
    }
    IN_CAN_CLOSE.set(false);
    if CLOSE_AGAIN.replace(false) && !ready {
        crate::remote::run_on_ui(|| dispatch(Cmd::Action(Action::Quit)));
    }
    ready as c_int
}

pub fn on_window_destroyed() {
    WINDOW_GONE.set(true);
    // Agents can't reach a browser that's shutting down.
    crate::remote::set_agent_access(false);
    let controller = CONTROLLER.with(|cell| cell.try_borrow_mut().ok().and_then(|mut c| c.take()));
    if let Some(controller) = controller {
        controller.save();
    }
    on_browser_closed();
}

pub fn on_browser_closed() {
    if WINDOW_GONE.get() && client::live_browsers().is_empty() {
        quit_message_loop();
    }
}

struct Live {
    view: BrowserView,
    loading: bool,
    back: bool,
    forward: bool,
}

struct Prompt {
    id: u64,
    request: PermissionRequest,
}

pub struct Controller {
    session: Session,
    session_path: PathBuf,
    window: Option<Window>,
    sidebar: BrowserView,
    palette: BrowserView,
    content: Panel,
    /// Holds the visible tabs (side by side in split view); sits in `content`
    /// next to docked developer tools.
    pages: Panel,
    /// Docked DevTools, one per inspected tab; only the active tab's shows.
    docked_devtools: HashMap<TabId, BrowserView>,
    /// The docked DevTools' size when a grip drag started.
    devtools_drag_start: Option<c_int>,
    overlay: Option<OverlayController>,
    /// The sidebar that slides out over the page while the real one is
    /// collapsed: an 8pt strip along the left edge until hovered.
    peek: BrowserView,
    /// The splash in the page area while no tab is shown.
    start: BrowserView,
    start_ready: bool,
    peek_overlay: Option<OverlayController>,
    peek_open: bool,
    peek_ready: bool,
    /// The hidden page that encodes videos, created on first use.
    recorder: Option<(BrowserView, Option<OverlayController>)>,
    recorder_ready: bool,
    /// Recording controls over the page, created on first use.
    recbar: Option<(BrowserView, Option<OverlayController>)>,
    /// The card for dropping a dragged tab into split view, made on first use.
    split_drop: Option<(BrowserView, Option<OverlayController>)>,
    split_drop_ready: bool,
    /// The Network tool, while open.
    network: Option<BrowserView>,
    network_ready: bool,
    /// What the Network tool was last told, to skip repeats.
    network_pushed: std::cell::RefCell<String>,
    network_drag_start: Option<c_int>,
    /// The Network tool's connections to tabs.
    network_cdp: HashMap<TabId, crate::remote::LocalClient>,
    /// The tab being dragged from the sidebar.
    tab_drag: Option<TabId>,
    /// The drag handle over the gap next to docked DevTools, made on first use.
    splitter: Option<(BrowserView, Option<OverlayController>)>,
    recbar_ready: bool,
    recbar_size: (c_int, c_int),
    /// Saved passwords, opened on first use.
    vault: Option<crate::passwords::Vault>,
    vault_error: Option<String>,
    /// The password suggestions or save prompt over the page.
    autofill: Option<(BrowserView, Option<OverlayController>)>,
    autofill_ready: bool,
    autofill_size: (c_int, c_int),
    autofill_state: Option<passwords_ui::Autofill>,
    /// Installed extensions, and those running in Chromium (by ID, with the
    /// folder each was loaded from).
    ext: crate::extensions::Registry,
    ext_loaded: HashMap<String, PathBuf>,
    /// Why extensions didn't load, by ID.
    ext_failures: HashMap<String, String>,
    /// Extension loads the window waits for at startup.
    ext_waiting: usize,
    window_requested: bool,
    /// Each tab's id in Chromium's extension system.
    ext_tab_ids: HashMap<TabId, i64>,
    ext_popup: Option<extensions_ui::Popup>,
    /// The popup last closed, and when (see `open_extension_popup`).
    ext_popup_closed: Option<(String, std::time::Instant)>,
    /// The card on Chrome Web Store pages.
    ext_card: Option<(BrowserView, Option<OverlayController>)>,
    ext_card_ready: bool,
    ext_card_size: (c_int, c_int),
    ext_offer: Option<extensions_ui::Offer>,
    /// The settings section to show once the settings page is ready.
    settings_section: Option<&'static str>,
    /// A video recording waiting for the recorder page to load.
    pending_recording: Option<TabId>,
    /// What is being recorded and since when (Unix ms).
    recording: Option<(crate::recorder::Format, u64)>,
    recording_saving: bool,
    update_status: Option<crate::updater::Status>,
    update_checking: bool,
    /// Release notes to show once the window is up (after an update).
    pending_whats_new: Option<String>,
    /// The notes the What's new panel shows.
    whats_new: String,
    /// The memory-only browsing session of the private space, while open.
    private_context: Option<RequestContext>,
    /// The normal space to return to when the private one ends.
    last_normal_space: u64,
    /// The recording controls are showing, set to this format.
    record_prompt: Option<crate::recorder::Format>,
    /// A bug report being recorded, and the last finished one.
    bug: Option<crate::bugreport::Session>,
    bug_result: Option<Value>,
    live: HashMap<TabId, Live>,
    mru: Vec<TabId>,
    split: Option<(TabId, TabId)>,
    /// Split view stacks the pages vertically instead of side by side.
    split_stacked: bool,
    palette_mode: Option<PaletteMode>,
    palette_height: c_int,
    /// Counts command bar openings, so a late backdrop for an earlier one
    /// is ignored.
    palette_generation: u64,
    palette_shown: bool,
    sidebar_ready: bool,
    palette_ready: bool,
    find_text: String,
    find_result: (c_int, c_int),
    downloads: Vec<DownloadInfo>,
    next_recording_id: u32,
    prompts: Vec<Prompt>,
    next_prompt: u64,
    drag_regions: Vec<DraggableRegion>,
    toast: Option<(u64, String)>,
    dark: bool,
    fullscreen: bool,
    ui_dirty: bool,
    save_scheduled: bool,
    /// Until when the sidebar shows that an agent is in control.
    agent_active_until: Option<std::time::Instant>,
    /// The sidebar's current (possibly animating) width.
    sidebar_width: f64,
    sidebar_animation: Option<SidebarAnimation>,
}

struct SidebarAnimation {
    start: std::time::Instant,
    from: f64,
    to: f64,
}

/// Fast start, gentle landing: close to CSS `cubic-bezier(0.32, 0.72, 0, 1)`.
fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(4)
}

/// CSS-compatible HSL to opaque ARGB.
pub fn hsl(hue: f64, saturation: f64, lightness: f64) -> u32 {
    let c = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = (hue % 360.0) / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = lightness - c / 2.0;
    let channel = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    0xFF00_0000 | channel(r) << 16 | channel(g) << 8 | channel(b)
}

/// The stripes around pages from a local dev server: a deeper shade of the
/// frame's own color.
#[cfg(target_os = "macos")]
fn stripe_color(hue: u16, dark: bool) -> u32 {
    if dark {
        hsl(hue as f64, 0.24, 0.30)
    } else {
        hsl(hue as f64, 0.47, 0.66)
    }
}

/// Whether a page comes from this machine (a dev server), which Circle marks
/// with a striped frame. Keep in sync with `circle.isLocal` in common.js.
pub fn is_local_url(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain(host)) => host == "localhost" || host.ends_with(".localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// The pages panel: one page, or two in split view, side by side or stacked.
fn pages_layout(stacked: bool) -> BoxLayoutSettings {
    BoxLayoutSettings {
        horizontal: (!stacked) as c_int,
        between_child_spacing: GAP,
        cross_axis_alignment: AxisAlignment::STRETCH,
        default_flex: 1,
        ..Default::default()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn theme_color(hue: u16, dark: bool) -> u32 {
    if dark {
        hsl(hue as f64, 0.20, 0.16)
    } else {
        hsl(hue as f64, 0.50, 0.82)
    }
}

impl Controller {
    fn new(profile_dir: &Path) -> Self {
        let session_path = Session::path(profile_dir);
        let returning = session_path.exists();
        let mut session = Session::load(&session_path);
        // After an update, show what changed since the version last run.
        // Sessions from before this was tracked only get this version's notes;
        // new installs get none.
        let version = env!("CARGO_PKG_VERSION");
        let pending_whats_new = match session.seen_version.as_deref() {
            Some(seen) if crate::updater::is_newer(version, seen) => {
                Some(crate::whatsnew::notes(Some(seen), version))
            }
            None if returning => Some(crate::whatsnew::notes(None, version)),
            _ => None,
        }
        .filter(|notes| !notes.is_empty());
        session.seen_version = Some(version.to_owned());
        let background = theme_color(session.space().hue, false);
        let sidebar = views::create_browser_view(Role::Sidebar, SIDEBAR_URL, background);
        let palette = views::create_browser_view(Role::Palette, PALETTE_URL, 0xFFFF_FFFF);
        let peek = views::create_browser_view(Role::Peek, PEEK_URL, background);
        let start = views::create_browser_view(Role::Start, START_PAGE_URL, background);
        let content = panel_create(None).expect("failed to create content panel");
        let pages = panel_create(None).expect("failed to create pages panel");
        pages.set_to_box_layout(Some(&pages_layout(false)));
        // Hidden views take no room in the pages panel's box layout.
        start.set_visible(0);
        pages.add_child_view(Some(&mut View::from(&start)));
        content.add_child_view(Some(&mut View::from(&pages)));
        let session_sidebar_visible = session.sidebar_visible;
        Self {
            session,
            session_path,
            window: None,
            sidebar,
            palette,
            content,
            pages,
            docked_devtools: HashMap::new(),
            devtools_drag_start: None,
            overlay: None,
            live: HashMap::new(),
            mru: Vec::new(),
            split: None,
            split_stacked: false,
            palette_mode: None,
            palette_height: 320,
            palette_generation: 0,
            palette_shown: false,
            sidebar_ready: false,
            palette_ready: false,
            peek,
            start,
            start_ready: false,
            peek_overlay: None,
            peek_open: false,
            peek_ready: false,
            recorder: None,
            recorder_ready: false,
            recbar: None,
            splitter: None,
            split_drop: None,
            split_drop_ready: false,
            network: None,
            network_ready: false,
            network_pushed: Default::default(),
            network_drag_start: None,
            network_cdp: HashMap::new(),
            tab_drag: None,
            recbar_ready: false,
            recbar_size: (360, 48),
            vault: None,
            vault_error: None,
            autofill: None,
            autofill_ready: false,
            autofill_size: (280, 60),
            autofill_state: None,
            ext: crate::extensions::Registry::load(profile_dir),
            ext_loaded: HashMap::new(),
            ext_failures: HashMap::new(),
            ext_waiting: 0,
            window_requested: false,
            ext_tab_ids: HashMap::new(),
            ext_popup: None,
            ext_popup_closed: None,
            ext_card: None,
            ext_card_ready: false,
            ext_card_size: (330, 120),
            ext_offer: None,
            settings_section: None,
            pending_recording: None,
            recording: None,
            recording_saving: false,
            update_status: None,
            update_checking: false,
            pending_whats_new,
            whats_new: String::new(),
            private_context: None,
            last_normal_space: 0,
            record_prompt: None,
            bug: None,
            bug_result: None,
            find_text: String::new(),
            find_result: (0, 0),
            downloads: Vec::new(),
            next_recording_id: 0,
            prompts: Vec::new(),
            next_prompt: 1,
            drag_regions: Vec::new(),
            toast: None,
            dark: false,
            fullscreen: false,
            ui_dirty: true,
            save_scheduled: false,
            sidebar_width: if session_sidebar_visible {
                SIDEBAR_WIDTH as f64
            } else {
                0.0
            },
            sidebar_animation: None,
            agent_active_until: None,
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Ui(cmd) => self.handle_ui(cmd),
            Cmd::Action(action) => self.handle_action(action),
            Cmd::WindowCreated(window, overlay, peek) => {
                self.window = Some(window);
                self.overlay = overlay;
                self.peek_overlay = peek;
                self.archive_stale_tabs();
                self.apply_layout();
                self.apply_theme();
                match self.session.active_tab() {
                    Some(id) => self.activate(id),
                    None => self.layout_tabs(),
                }
            }
            Cmd::WindowResized => {
                self.place_extension_popup();
                self.place_palette();
                self.place_peek();
                self.place_recbar();
                self.apply_drag_regions();
            }
            Cmd::Title(id, title) => {
                if let Some(tab) = self.session.tab(id)
                    && !self.session.is_private_tab(id)
                {
                    crate::history::update_title(&tab.url, &title);
                }
                if let Some(tab) = self.session.tab_mut(id) {
                    tab.title = title;
                }
                if self.session.active_tab() == Some(id) {
                    self.update_window_title();
                    self.ext_follow_tab();
                }
                self.changed();
            }
            Cmd::Url(id, url) => {
                // Not for tabs already gone (fap mode may just have ended).
                let private = self.session.tab(id).is_none() || self.session.is_private_tab(id);
                if !private {
                    crate::history::record(&url, "", now());
                }
                let mut local_changed = false;
                if self.bug.is_some() && !private {
                    self.bug_log("navigate", "", Some(&url));
                }
                if let Some(tab) = self.session.tab_mut(id) {
                    local_changed = is_local_url(&tab.url) != is_local_url(&url);
                    tab.url = url;
                }
                if local_changed {
                    // The striped frame comes or goes.
                    self.update_page_corners();
                }
                if self.session.active_tab() == Some(id) {
                    self.ext_follow_tab();
                }
                self.changed();
            }
            Cmd::Favicon(id, url) => {
                if let Some(tab) = self.session.tab_mut(id) {
                    tab.favicon = Some(url);
                }
                self.changed();
            }
            Cmd::Loading {
                id,
                loading,
                back,
                forward,
            } => {
                if let Some(live) = self.live.get_mut(&id) {
                    live.loading = loading;
                    live.back = back;
                    live.forward = forward;
                }
                self.ui_dirty = true;
            }
            Cmd::OpenUrl {
                url,
                background,
                opener,
            } => {
                self.open_tab(&url, !background, opener);
            }
            Cmd::OpenUrls(urls) => {
                for url in urls {
                    let url = self.session.resolve_input(&url);
                    self.open_tab(&url, true, None);
                }
                if let Some(window) = &self.window {
                    window.show();
                    window.activate();
                }
            }
            Cmd::Fullscreen(_, fullscreen) => {
                self.fullscreen = fullscreen;
                if let Some(window) = &self.window {
                    window.set_fullscreen(fullscreen as c_int);
                }
                self.apply_layout();
            }
            Cmd::DragRegions(regions) => {
                self.drag_regions = regions;
                self.apply_drag_regions();
            }
            Cmd::FindResult { count, active } => {
                self.find_result = (count, active);
                self.ui_dirty = true;
            }
            Cmd::Download(info) => self.add_download(info),
            Cmd::Permission(request) => self.request_permission(request),
            Cmd::PermissionDismissed(cef_id) => {
                self.prompts
                    .retain(|p| p.request.cef_prompt_id != Some(cef_id));
                self.ui_dirty = true;
            }
            Cmd::Save => {
                self.save_scheduled = false;
                self.save();
            }
            Cmd::AnimateSidebar => self.step_sidebar_animation(),
            Cmd::PageLayout => self.update_page_corners(),
            Cmd::Toast(text) => self.toast(text),
            Cmd::RecorderJs(code) => {
                if let Some((view, _)) = &self.recorder {
                    run_js(view, &code);
                }
            }
            Cmd::RecorderStop => {
                if let Some((view, _)) = &self.recorder {
                    run_js(view, "window.circleRecorder&&circleRecorder.stop()");
                }
            }
            Cmd::PaletteBackdrop(generation, shots) => {
                if generation != self.palette_generation || self.palette_mode.is_none() {
                    return;
                }
                let images: Vec<Value> = shots
                    .iter()
                    .map(|(r, data)| {
                        json!({ "x": r.x, "y": r.y, "w": r.width, "h": r.height, "src": format!("data:image/jpeg;base64,{data}") })
                    })
                    .collect();
                run_js(
                    &self.palette,
                    &format!("window.circle&&circle.backdrop({})", json!(images)),
                );
                self.show_palette();
            }
            Cmd::ShowPalette(generation) => {
                if generation == self.palette_generation {
                    self.show_palette();
                }
            }
            Cmd::BugLog {
                tab,
                kind,
                text,
                url,
            } => {
                // Fap mode stays out of bug reports.
                if !tab.is_some_and(|t| self.session.is_private_tab(t)) {
                    self.bug_log(kind, &text, url.as_deref());
                }
            }
            Cmd::UpdateStatus(status) => {
                self.update_checking = false;
                self.update_status = Some(status);
                self.ui_dirty = true;
                self.push_settings();
            }
            Cmd::WhatsNew => {
                if let Some(notes) = self.pending_whats_new.take() {
                    self.whats_new = notes;
                    self.changed();
                    if self.palette_mode.is_none() {
                        self.open_palette(PaletteMode::WhatsNew);
                    }
                }
            }
            Cmd::CheckUpdates => {
                if self.session.auto_update {
                    crate::updater::check(self.updates_dir(), true);
                }
                schedule(Timer::UpdateCheck, UPDATE_EVERY_MS);
            }
            Cmd::RecordingSaving => {
                self.recording_saving = true;
                self.ui_dirty = true;
            }
            Cmd::RecordingFinished(result) => {
                self.recording = None;
                self.recording_saving = false;
                // A video made for a bug report is mentioned in the report.
                if let (Ok(path), Some(report)) = (&result, &mut self.bug_result)
                    && report["video"] == true
                    && report.get("video_file").is_none()
                {
                    report["video_file"] =
                        json!(path.file_name().unwrap_or_default().to_string_lossy());
                    self.push_bug_report();
                }
                match result {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        self.toast(format!("Saved “{name}” to Downloads"));
                        // Recordings are listed with the downloads.
                        self.next_recording_id += 1;
                        self.add_download(DownloadInfo {
                            id: RECORDING_IDS + self.next_recording_id,
                            at: 0,
                            name,
                            path: path.to_string_lossy().into_owned(),
                            percent: 100,
                            complete: true,
                            failed: false,
                        });
                    }
                    Err(err) => self.toast(err),
                }
                self.ui_dirty = true;
            }
            Cmd::AgentIdle => self.ui_dirty = true,
            Cmd::Inspect(tab, point) => self.open_dev_tools(tab, Some(point)),
            Cmd::DevToolsLoaded(tab) => {
                let side = match self.session.devtools_dock {
                    DevToolsDock::Bottom => "bottom",
                    _ => "right",
                };
                if let Some(view) = self.docked_devtools.get(&tab) {
                    run_devtools_js(
                        view,
                        &format!("({})({})", client::DEVTOOLS_GRIP_JS, json!(side)),
                    );
                }
            }
            Cmd::DevToolsGrip(_, drag) => self.drag_devtools(drag),
            Cmd::NetworkCdp(tab, batch) => self.network_messages(tab, batch),
            Cmd::ExtLoaded {
                path,
                expected,
                purpose,
                result,
            } => self.extension_loaded(path, expected, purpose, result),
            Cmd::ExtUninstalled { id, result } => self.extension_uninstalled(id, result),
            Cmd::ExtFetched { id, result } => self.extension_fetched(id, result),
            Cmd::ExtUpdated { id, path, version } => self.extension_updated(id, path, version),
            Cmd::ExtUnpackedChosen(path) => self.load_unpacked_extension(path),
            Cmd::ExtTabId(tab, chrome_id) => self.extension_tab_id(tab, chrome_id),
            Cmd::ExtPopupSize(browser, width, height) => {
                self.extension_popup_resized(browser, width, height)
            }
            Cmd::ExtPopupClosed(browser) => self.extension_popup_closed(browser),
            Cmd::ExtStartupTimeout => {
                self.ext_waiting = 0;
                self.show_window_once();
            }
            Cmd::ExtCheckUpdates => {
                self.check_extension_updates();
                schedule(Timer::ExtensionsUpdate, extensions_ui::UPDATE_EVERY_MS);
            }
            Cmd::Focused(role) => {
                // Clicking anywhere else closes an extension's popup.
                if !matches!(role, Role::ExtPopup) {
                    self.close_extension_popup();
                }
            }
            Cmd::PageLoaded(tab) => self.inject_password_helper(tab),
            Cmd::PasswordsPing(tab) => self.collect_password_news(tab),
            Cmd::PasswordsNews(tab, context, events) => self.password_news(tab, context, events),
            Cmd::PasswordConfirmed { id, copy, ok } => self.password_confirmed(id, copy, ok),
            Cmd::BrowserCreated(Role::ExtPopup, browser) => self.extension_popup_created(browser),
            Cmd::BrowserCreated(role, browser) => {
                // Round the page and command bar to sit inside the window
                // frame (macOS draws the corners; elsewhere they stay square).
                #[cfg(target_os = "macos")]
                match role {
                    Role::Tab(_) => self.update_page_corners(),
                    Role::Palette => {
                        crate::mac::round_browser_window(&browser, 12.0, crate::mac::ALL_CORNERS)
                    }
                    _ => {}
                }
                #[cfg(not(target_os = "macos"))]
                let _ = (role, browser);
            }
            Cmd::Tick => {
                self.archive_stale_tabs();
                schedule(Timer::Tick, TICK_MS);
            }
        }
    }

    fn handle_ui(&mut self, cmd: UiCmd) {
        match cmd {
            UiCmd::Ready { from } => {
                match from.as_str() {
                    "sidebar" => self.sidebar_ready = true,
                    "peek" => self.peek_ready = true,
                    "autofill" => {
                        self.autofill_ready = true;
                        self.push_autofill();
                    }
                    "splitdrop" => {
                        self.split_drop_ready = true;
                        self.place_split_drop();
                    }
                    "network" => {
                        self.network_ready = true;
                        self.network_pushed.borrow_mut().clear();
                        self.push_network();
                    }
                    "extcard" => {
                        self.ext_card_ready = true;
                        self.push_ext_card();
                    }
                    "recbar" => {
                        self.recbar_ready = true;
                        self.ui_dirty = true;
                    }
                    "start" => self.start_ready = true,
                    "recorder" => {
                        self.recorder_ready = true;
                        if let Some(tab) = self.pending_recording.take() {
                            self.begin_video(tab);
                        }
                    }
                    "palette" => {
                        self.palette_ready = true;
                        if self.pending_whats_new.is_some() {
                            // Once the first page has had a moment to show.
                            schedule(Timer::WhatsNew, 1500);
                        }
                        if let Some(mode) = self.palette_mode {
                            self.open_palette(mode);
                        }
                    }
                    _ => {}
                }
                self.ui_dirty = true;
            }
            UiCmd::Navigate { input, new_tab } => {
                let url = self.session.resolve_input(&input);
                self.close_palette();
                match self.session.active_tab() {
                    Some(id) if !new_tab => self.navigate(id, &url),
                    _ => {
                        self.open_tab(&url, true, None);
                    }
                }
            }
            UiCmd::Activate { id } => {
                self.close_palette();
                self.activate(id);
            }
            UiCmd::Close { id } => self.close_tab(id),
            UiCmd::Reset { id } => {
                if let Some(home) = self.session.tab(id).and_then(|t| t.home_url.clone()) {
                    self.navigate(id, &home);
                }
            }
            UiCmd::Unload { id } => {
                if !self.shown_tabs().contains(&id) {
                    self.destroy_live(id);
                    self.ui_dirty = true;
                }
            }
            UiCmd::Duplicate { id } => {
                if let Some(url) = self.session.tab(id).map(|t| t.url.clone()) {
                    self.open_tab(&url, true, Some(id));
                }
            }
            UiCmd::CopyUrl { id } => self.copy_url(id),
            UiCmd::Split { id, stacked, first } => {
                self.tab_drag = None;
                self.place_split_drop();
                self.split_with(id, stacked, first);
            }
            UiCmd::TabDrag { id } => {
                self.tab_drag = id;
                self.place_split_drop();
            }
            UiCmd::Move {
                id,
                section,
                space,
                index,
                before,
            } => {
                let space = space
                    .and_then(|s| self.session.space_index(s))
                    .unwrap_or(self.session.active_space);
                // Tabs never cross between private and normal spaces (or
                // into the favorites, which every space shares).
                let to_private =
                    section != Section::Favorites && self.session.spaces[space].private;
                if self.session.is_private_tab(id) != to_private {
                    return;
                }
                self.session.move_tab(id, section, space, index);
                if before.is_some() {
                    self.session.place_before(id, before);
                }
                self.changed();
            }
            UiCmd::MoveToSpace { id, space } => {
                if let Some(target) = self
                    .session
                    .space_index(space)
                    .filter(|&t| self.session.spaces[t].private == self.session.is_private_tab(id))
                {
                    let was_active = self.session.active_tab() == Some(id);
                    self.session.move_tab(id, Section::Today, target, 0);
                    if was_active {
                        self.session.space_mut().active = None;
                        self.switch_space(target);
                        self.activate(id);
                    }
                    self.changed();
                }
            }
            UiCmd::Action { action } => self.handle_action(action),
            UiCmd::ClosePalette => self.close_palette(),
            UiCmd::DevtoolsGrip { phase, dx, dy } => {
                use crate::client::GripDrag;
                match phase.as_str() {
                    "start" => self.drag_devtools(GripDrag::Start),
                    "move" => self.drag_devtools(GripDrag::Move(dx, dy)),
                    "end" => {
                        self.drag_devtools(GripDrag::End);
                        // Give the keyboard back to the page.
                        if let Some(live) =
                            self.session.active_tab().and_then(|id| self.live.get(&id))
                        {
                            live.view.request_focus();
                        }
                    }
                    _ => {}
                }
            }
            UiCmd::NetworkGrip { phase, dx, dy } => self.drag_network(&phase, dx, dy),
            UiCmd::NetConnect { tab } => self.network_connect(tab),
            UiCmd::NetSend { tab, message } => self.network_send(tab, &message),
            UiCmd::NetDisconnect { tab } => self.network_detach(tab),
            UiCmd::AutofillSize { width, height } => {
                self.autofill_size = (width.clamp(160, 480), height.clamp(32, 400));
                self.place_autofill();
            }
            UiCmd::AutofillPick { id } => self.fill_login(id),
            UiCmd::AutofillSave => self.save_offered_login(false),
            UiCmd::AutofillNever => self.save_offered_login(true),
            UiCmd::AutofillClose => self.hide_autofill(),
            UiCmd::AutofillManage => {
                self.hide_autofill();
                self.handle_action(Action::ShowSettings);
            }
            UiCmd::PasswordReveal { id } => confirm_owner(id, false),
            UiCmd::PasswordCopy { id } => confirm_owner(id, true),
            UiCmd::PasswordDelete { id } => {
                if let Some(vault) = self.vault() {
                    let _ = vault.remove(id);
                }
                self.push_settings();
            }
            UiCmd::PasswordForgetNever { origin } => {
                if let Some(vault) = self.vault() {
                    let _ = vault.forget_never(&origin);
                }
                self.push_settings();
            }
            UiCmd::ExtCardSize { width, height } => {
                self.ext_card_size = (width.clamp(200, 480), height.clamp(40, 500));
                self.place_ext_card();
            }
            UiCmd::ExtAdd => self.ext_card_command("add"),
            UiCmd::ExtConfirm => self.ext_card_command("confirm"),
            UiCmd::ExtCancel => self.ext_card_command("cancel"),
            UiCmd::ExtManage => self.ext_card_command("manage"),
            UiCmd::ExtCardClose => self.ext_card_command("close"),
            UiCmd::ExtOpen {
                id,
                x,
                y,
                width,
                height,
                peek,
            } => self.open_extension_popup(
                id,
                Rect {
                    x,
                    y,
                    width,
                    height,
                },
                peek,
            ),
            UiCmd::ExtEnable { id, on } => self.set_extension_enabled(&id, on),
            UiCmd::ExtPin { id, pinned } => self.set_extension_pinned(&id, pinned),
            UiCmd::ExtOptions { id } => {
                self.close_palette();
                self.open_extension_options(&id);
            }
            UiCmd::ExtRemove { id } => self.remove_extension(id),
            UiCmd::ExtLoadUnpacked => self.choose_unpacked_extension(),
            UiCmd::ExtStore => {
                self.close_palette();
                self.open_tab(
                    "https://chromewebstore.google.com/category/extensions",
                    true,
                    None,
                );
            }
            UiCmd::RecbarSize { width, height } => {
                self.recbar_size = (width.clamp(80, 900), height.clamp(24, 300));
                self.place_recbar();
            }
            UiCmd::PaletteHeight { height } => {
                self.palette_height = height.clamp(44, 560);
                self.place_palette();
            }
            UiCmd::NewSpace { name } => {
                let name = name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("Space {}", self.session.spaces.len() + 1));
                let index = self.session.add_space(name.trim());
                self.switch_space(index);
            }
            UiCmd::RenameSpace { id, name, icon } => {
                if let Some(i) = self.session.space_index(id) {
                    let space = &mut self.session.spaces[i];
                    if !name.trim().is_empty() {
                        space.name = name.trim().to_owned();
                    }
                    if let Some(icon) = icon {
                        space.icon = icon.chars().take(2).collect();
                    }
                    self.changed();
                }
            }
            UiCmd::RecolorSpace { id } => {
                if let Some(i) = self.session.space_index(id) {
                    let space = &mut self.session.spaces[i];
                    space.hue = (space.hue + 37) % 360;
                    self.apply_theme();
                    self.changed();
                }
            }
            UiCmd::SetSpaceHue { id, hue } => {
                if let Some(i) = self.session.space_index(id) {
                    self.session.spaces[i].hue = hue % 360;
                    self.apply_theme();
                    self.changed();
                }
            }
            UiCmd::DeleteHistory { url, at } => {
                crate::history::delete(&url, at);
                self.changed();
            }
            UiCmd::ClearBrowsingData {
                hours,
                history,
                cookies,
                cache,
            } => self.clear_browsing_data(hours, history, cookies, cache),
            UiCmd::Settings {
                appearance,
                search_url,
                archive_after_hours,
                devtools_dock,
                agent_access,
                auto_update,
                spotlight_blur,
                recording_clicks,
                recording_keys,
                recording_float,
                passwords_enabled,
                passwords_offer,
                forget_grant,
            } => {
                if let Some(on) = passwords_enabled {
                    self.session.passwords_enabled = on;
                    if !on {
                        self.hide_autofill();
                    }
                }
                if let Some(on) = passwords_offer {
                    self.session.passwords_offer = on;
                }
                if let Some(on) = recording_clicks {
                    self.session.recording_clicks = on;
                }
                if let Some(on) = recording_keys {
                    self.session.recording_keys = on;
                }
                if let Some(on) = recording_float {
                    self.session.recording_float = on;
                    self.place_recbar();
                }
                if let Some(on) = auto_update {
                    self.session.auto_update = on;
                }
                if let Some(on) = spotlight_blur {
                    self.session.spotlight_blur = on;
                }
                if let Some(appearance) = appearance {
                    self.handle_action(Action::Appearance(appearance));
                }
                if let Some(url) = search_url.filter(|u| {
                    u.contains("%s") && (u.starts_with("https://") || u.starts_with("http://"))
                }) {
                    self.session.search_url = url;
                }
                if let Some(hours) = archive_after_hours {
                    self.session.archive_after_hours = hours;
                }
                if let Some(dock) = devtools_dock.filter(|d| *d != self.session.devtools_dock) {
                    self.handle_action(Action::DockDevTools(dock));
                }
                if agent_access.is_some_and(|on| on != self.session.agent_access) {
                    self.handle_action(Action::ToggleAgentAccess);
                }
                if let Some(key) = forget_grant {
                    self.session.grants.remove(&key);
                }
                self.changed();
                self.push_settings();
            }
            UiCmd::SettingsReady => {
                self.vault();
                match self.settings_section.take() {
                    Some(section) => self.push_settings_with(json!({ "section": section })),
                    None => self.push_settings(),
                }
            }
            UiCmd::BugReportStart { video } => self.start_bug_report(video),
            UiCmd::BugReportStop => self.stop_bug_report(),
            UiCmd::BugReportReady => self.push_bug_report(),
            UiCmd::WindowControl { op } => {
                if let Some(window) = &self.window {
                    match op.as_str() {
                        "close" => window.close(),
                        "minimize" => window.minimize(),
                        "zoom" if window.is_maximized() != 0 => window.restore(),
                        "zoom" => window.maximize(),
                        _ => {}
                    }
                }
            }
            UiCmd::Peek { open } => {
                self.peek_open = open && !self.session.sidebar_visible;
                self.place_peek();
            }
            UiCmd::DeleteSpace { id } => self.delete_space(id),
            UiCmd::SwitchSpace { id } => {
                if let Some(i) = self.session.space_index(id) {
                    self.switch_space(i);
                }
            }
            UiCmd::Reopen { url } => {
                self.session.archive.retain(|a| a.url != url);
                self.close_palette();
                self.open_tab(&url, true, None);
            }
            UiCmd::Find { text, backward } => self.find(text, backward),
            UiCmd::Permission { id, allow } => {
                if let Some(i) = self.prompts.iter().position(|p| p.id == id) {
                    let prompt = self.prompts.remove(i);
                    // Private tabs get the permission once, not remembered,
                    // and so do pages without a real origin (local files
                    // all share "null").
                    if allow
                        && !self.session.is_private_tab(prompt.request.tab)
                        && is_web_url(&prompt.request.origin)
                    {
                        *self
                            .session
                            .grants
                            .entry(prompt.request.grant_key())
                            .or_default() |= prompt.request.mask;
                        self.changed();
                    }
                    prompt.request.finish(allow);
                    self.ui_dirty = true;
                }
            }
            UiCmd::ClearDownloads => {
                // Finished and failed ones; anything in progress stays.
                self.downloads.retain(|d| !d.complete && !d.failed);
                self.ui_dirty = true;
            }
            UiCmd::RevealDownload { id } => {
                if let Some(download) = self.downloads.iter().find(|d| d.id == id) {
                    reveal(&download.path);
                }
            }
            UiCmd::RenameTab { id, name } => {
                if let Some(tab) = self.session.tab_mut(id) {
                    let name = name.trim();
                    // An empty name (or the page title itself) goes back to the page title.
                    tab.custom_title =
                        (!name.is_empty() && name != tab.title).then(|| name.to_owned());
                }
                self.update_window_title();
                self.changed();
            }
            UiCmd::NewFolder { name, tab } => {
                let space = tab
                    .and_then(|t| match self.session.locate(t) {
                        Some(Location::Pinned(s, _) | Location::Today(s, _)) => Some(s),
                        _ => None,
                    })
                    .unwrap_or(self.session.active_space);
                let name = name
                    .filter(|n| !n.trim().is_empty())
                    .map(|n| n.trim().to_owned())
                    .unwrap_or_else(|| "New Folder".to_owned());
                let folder = self.session.add_folder(space, &name);
                if let Some(tab) = tab {
                    self.session.move_to_folder(tab, Some(folder));
                }
                self.changed();
            }
            UiCmd::RenameFolder { id, name } => {
                if let (Some((si, fi)), false) =
                    (self.session.locate_folder(id), name.trim().is_empty())
                {
                    self.session.spaces[si].folders[fi].name = name.trim().to_owned();
                    self.changed();
                }
            }
            UiCmd::DeleteFolder { id } => {
                self.session.delete_folder(id);
                self.changed();
            }
            UiCmd::ToggleFolder { id } => {
                if let Some((si, fi)) = self.session.locate_folder(id) {
                    let folder = &mut self.session.spaces[si].folders[fi];
                    folder.collapsed = !folder.collapsed;
                    self.changed();
                }
            }
            UiCmd::MoveToFolder { id, folder, before } => {
                self.session.move_to_folder(id, folder);
                if folder.is_some() {
                    self.session.place_before(id, before);
                }
                self.changed();
            }
            UiCmd::Theme { dark } => {
                self.dark = dark;
                self.apply_theme();
            }
        }
    }

    fn handle_action(&mut self, action: Action) {
        if self.bug.is_some() {
            self.bug_log("action", &format!("{action:?}"), None);
        }
        let active = self.session.active_tab();
        let host = active.and_then(|id| self.host(id));
        match action {
            Action::OpenPalette(mode) => self.open_palette(mode),
            Action::CloseTab => {
                if let Some(id) = active {
                    self.close_tab(id);
                }
            }
            Action::ReopenClosed => {
                if !self.session.archive.is_empty() {
                    let entry = self.session.archive.remove(0);
                    self.open_tab(&entry.url, true, None);
                }
            }
            Action::ToggleSidebar => self.set_sidebar_visible(!self.session.sidebar_visible),
            Action::TogglePin => {
                if let Some(id) = active {
                    match self.session.locate(id) {
                        Some(Location::Today(..)) => {
                            let end = self.session.space().pinned.len();
                            self.session.move_tab(
                                id,
                                Section::Pinned,
                                self.session.active_space,
                                end,
                            );
                        }
                        Some(_) => {
                            self.session
                                .move_tab(id, Section::Today, self.session.active_space, 0);
                        }
                        None => {}
                    }
                    self.changed();
                }
            }
            Action::Reload => self.with_browser(active, |b| b.reload()),
            Action::ReloadHard => self.with_browser(active, |b| b.reload_ignore_cache()),
            Action::Stop => self.with_browser(active, |b| b.stop_load()),
            Action::Back => self.with_browser(active, |b| b.go_back()),
            Action::Forward => self.with_browser(active, |b| b.go_forward()),
            Action::NextTab | Action::PrevTab => {
                let order = self.tab_order();
                if order.is_empty() {
                    return;
                }
                let current = active
                    .and_then(|id| order.iter().position(|t| *t == id))
                    .unwrap_or(0);
                let step = if action == Action::NextTab {
                    1
                } else {
                    order.len() - 1
                };
                self.activate(order[(current + step) % order.len()]);
            }
            Action::NextSpace | Action::PrevSpace => {
                let count = self.session.spaces.len();
                let step = if action == Action::NextSpace {
                    1
                } else {
                    count - 1
                };
                self.switch_space((self.session.active_space + step) % count);
            }
            Action::Space(index) => self.switch_space(index),
            Action::TabIndex(index) => {
                let order = self.tab_order();
                let target = if index == 8 {
                    order.last()
                } else {
                    order.get(index)
                };
                if let Some(id) = target.copied() {
                    self.activate(id);
                }
            }
            Action::Unsplit => {
                if self.split.take().is_some() {
                    self.layout_tabs();
                    self.changed();
                }
            }
            Action::DuplicateTab => {
                if let Some(url) = active
                    .and_then(|id| self.session.tab(id))
                    .map(|t| t.url.clone())
                {
                    self.open_tab(&url, true, active);
                }
            }
            Action::ToggleSplit => {
                if self.split.take().is_some() {
                    self.layout_tabs();
                } else if let Some(id) = active {
                    let order = self.tab_order();
                    let partner = self
                        .mru
                        .iter()
                        .copied()
                        .find(|t| *t != id && order.contains(t))
                        .or_else(|| order.iter().copied().find(|t| *t != id));
                    if let Some(partner) = partner {
                        self.split = Some((id, partner));
                        self.ensure_live(partner);
                        self.layout_tabs();
                    }
                }
                self.ui_dirty = true;
            }
            Action::ToggleSplitDirection => {
                self.split_stacked = !self.split_stacked;
                if self.split.is_some() {
                    self.layout_tabs();
                } else {
                    // Not split yet: split in the new direction.
                    self.handle_action(Action::ToggleSplit);
                }
            }
            Action::Zoom(direction) => {
                if let Some(host) = host {
                    let level = match direction {
                        0 => 0.0,
                        d => host.zoom_level() + 0.5 * d as f64,
                    };
                    host.set_zoom_level(level);
                }
            }
            Action::DevTools => {
                if let Some(id) = active {
                    if self.docked_devtools.contains_key(&id) {
                        self.close_docked_devtools(id);
                        if let Some(live) = self.live.get(&id) {
                            live.view.request_focus();
                        }
                    } else {
                        self.open_dev_tools(id, None);
                    }
                }
            }
            Action::Network => self.toggle_network(),
            Action::ShowHistory => self.show_history(false),
            Action::ShowSettings => {
                if self.palette_mode == Some(PaletteMode::Settings) {
                    self.close_palette();
                } else {
                    self.open_palette(PaletteMode::Settings);
                }
            }
            Action::Record(format) => {
                if self.recording.is_some() {
                    crate::recorder::stop();
                } else if self.record_prompt.is_some() {
                    self.record_prompt = None;
                } else {
                    self.record_prompt = Some(format);
                    // The controls live in the sidebar, or float over the
                    // page while it is hidden.
                    if !self.session.sidebar_visible && !self.session.recording_float {
                        self.set_sidebar_visible(true);
                    }
                }
                self.ui_dirty = true;
            }
            Action::StartRecording(format) => {
                self.record_prompt = None;
                if self.recording.is_none() {
                    self.start_recording(format);
                }
                self.ui_dirty = true;
            }
            Action::CancelRecording => {
                self.record_prompt = None;
                self.ui_dirty = true;
            }
            Action::StopRecording => crate::recorder::stop(),
            Action::ReportBug => self.show_page("circle://ui/bugreport.html"),
            Action::ShowWhatsNew => {
                self.whats_new = crate::whatsnew::notes(None, env!("CARGO_PKG_VERSION"));
                self.open_palette(PaletteMode::WhatsNew);
            }
            Action::CheckForUpdates => {
                // Asked for: download even when automatic updates are off.
                crate::updater::check(self.updates_dir(), true);
                self.update_status = None;
                self.update_checking = true;
                self.ui_dirty = true;
                self.push_settings();
            }
            Action::RestartToUpdate | Action::Restart => {
                crate::updater::relaunch_after_update();
                self.handle_action(Action::Quit);
            }
            Action::PrivateSpace => self.open_private_space(),
            Action::EndPrivate => self.end_private_space(),
            Action::Appearance(appearance) => {
                self.session.appearance = appearance;
                self.apply_appearance();
                self.changed();
            }
            Action::ClearBrowsingData => self.show_history(true),
            Action::DockDevTools(dock) => {
                let reopen = active.filter(|id| self.docked_devtools.contains_key(id));
                for tab in self.docked_devtools.keys().copied().collect::<Vec<_>>() {
                    self.close_docked_devtools(tab);
                }
                self.session.devtools_dock = dock;
                self.apply_layout();
                match reopen {
                    Some(id) => self.open_dev_tools(id, None),
                    None => {
                        let text = match dock {
                            DevToolsDock::Right => "Developer tools will open on the right",
                            DevToolsDock::Bottom => "Developer tools will open at the bottom",
                            DevToolsDock::Window => "Developer tools will open in a window",
                        };
                        self.toast(text.to_owned());
                    }
                }
                self.changed();
            }
            Action::ViewSource => {
                if let Some(url) = active
                    .and_then(|id| self.session.tab(id))
                    .map(|t| t.url.clone())
                    && (url.starts_with("http") || url.starts_with("file:"))
                {
                    self.open_tab(&format!("view-source:{url}"), true, active);
                }
            }
            Action::CopyUrl => {
                if let Some(id) = active {
                    self.copy_url(id);
                }
            }
            Action::Fullscreen => {
                if let Some(window) = &self.window {
                    let fullscreen = window.is_fullscreen() == 0;
                    window.set_fullscreen(fullscreen as c_int);
                }
            }
            Action::NewSpace => self.handle_ui(UiCmd::NewSpace { name: None }),
            Action::ToggleAgentAccess => {
                self.session.agent_access = !self.session.agent_access;
                crate::remote::set_agent_access(self.session.agent_access);
                let text = if self.session.agent_access {
                    "Agents can now control Circle"
                } else {
                    "Agent control turned off"
                };
                self.toast(text.to_owned());
                self.changed();
            }
            Action::ClearToday => {
                let space = self.session.active_space;
                let ids: Vec<TabId> = self.session.spaces[space]
                    .tabs
                    .iter()
                    .map(|t| t.id)
                    .collect();
                for id in ids {
                    self.close_tab(id);
                }
            }
            Action::CloseExtensionPopup => self.close_extension_popup(),
            Action::Quit => {
                if let Some(window) = &self.window {
                    window.close();
                }
            }
        }
    }

    // ----- tabs -------------------------------------------------------------

    fn host(&self, id: TabId) -> Option<BrowserHost> {
        self.live.get(&id)?.view.browser()?.host()
    }

    fn with_browser(&self, id: Option<TabId>, f: impl FnOnce(&Browser)) {
        if let Some(browser) = id.and_then(|id| self.live.get(&id)?.view.browser()) {
            f(&browser);
        }
    }

    /// Favorites, then the active space's pinned and today tabs.
    fn tab_order(&self) -> Vec<TabId> {
        let space = self.session.space();
        self.session
            .favorites
            .iter()
            .chain(space.pinned_in_display_order())
            .chain(&space.tabs)
            .map(|t| t.id)
            .collect()
    }

    /// Shows `id` in split view next to the tab you're on (or, if it is the
    /// tab you're on, next to the one it's split with or the last one used).
    fn split_with(&mut self, id: TabId, stacked: bool, first: bool) {
        if self.session.tab(id).is_none() {
            return;
        }
        let partner = match self.session.active_tab() {
            Some(active) if active != id => Some(active),
            _ => {
                let other = self.split.and_then(|(a, b)| match id {
                    _ if a == id => Some(b),
                    _ if b == id => Some(a),
                    _ => None,
                });
                let order = self.tab_order();
                other.or_else(|| {
                    self.mru
                        .iter()
                        .copied()
                        .find(|t| *t != id && order.contains(t))
                })
            }
        };
        let Some(partner) = partner else {
            return;
        };
        self.split = Some(if first { (id, partner) } else { (partner, id) });
        self.split_stacked = stacked;
        self.ensure_live(id);
        self.ensure_live(partner);
        self.layout_tabs();
        self.changed();
    }

    /// The drop card over the page while a tab is dragged from the sidebar.
    fn place_split_drop(&mut self) {
        let wanted = self.tab_drag.filter(|id| {
            let active = self.session.active_tab();
            active.is_some() && (active != Some(*id) || self.split.is_some() || self.mru.len() > 1)
        });
        let Some(id) = wanted else {
            if let Some((_, Some(overlay))) = &self.split_drop {
                overlay.set_visible(0);
            }
            return;
        };
        if self.split_drop.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let view =
                views::create_browser_view(Role::SplitDrop, SPLIT_DROP_URL, self.background());
            let overlay =
                window.add_overlay_view(Some(&mut View::from(&view)), DockingMode::CUSTOM, 1);
            self.split_drop = Some((view, overlay));
        }
        let Some((view, Some(overlay))) = &self.split_drop else {
            return;
        };
        if self.split_drop_ready {
            let state = json!({ "id": id, "hue": self.session.space().hue });
            run_js(view, &format!("window.circle&&circle.render({state})"));
        }
        let area = self.content.bounds();
        let (width, height) = (340.min(area.width), 190.min(area.height));
        overlay.set_bounds(Some(&Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        }));
        overlay.set_visible(1);
        #[cfg(target_os = "macos")]
        if let Some(browser) = view.browser() {
            crate::mac::round_browser_window(&browser, 14.0, crate::mac::ALL_CORNERS);
        }
    }

    fn shown_tabs(&self) -> Vec<TabId> {
        match (self.split, self.session.active_tab()) {
            (Some((a, b)), Some(active)) if active == a || active == b => vec![a, b],
            (_, Some(active)) => vec![active],
            _ => Vec::new(),
        }
    }

    fn background(&self) -> u32 {
        // The private space is always dark, so it's obvious where you are.
        if self.session.space().private {
            return hsl(268.0, 0.28, 0.11);
        }
        theme_color(self.session.space().hue, self.dark)
    }

    fn private_space_index(&self) -> Option<usize> {
        self.session.spaces.iter().position(|s| s.private)
    }

    fn open_private_space(&mut self) {
        let index = match self.private_space_index() {
            Some(index) => index,
            None => {
                let index = self.session.add_space("Fap mode");
                let space = &mut self.session.spaces[index];
                space.private = true;
                space.hue = 268;
                space.icon = String::new();
                index
            }
        };
        if !self.session.space().private {
            self.last_normal_space = self.session.space().id;
        }
        self.switch_space(index);
        if self.session.active_tab().is_none() {
            self.open_palette(PaletteMode::NewTab);
        }
    }

    /// Closes every private tab, drops the private session (its cookies and
    /// cache go with it) and goes back to the last normal space.
    fn end_private_space(&mut self) {
        let Some(index) = self.private_space_index() else {
            return;
        };
        self.close_palette();
        let ids: Vec<TabId> = {
            let space = &self.session.spaces[index];
            space
                .pinned
                .iter()
                .chain(&space.tabs)
                .map(|t| t.id)
                .collect()
        };
        for id in &ids {
            self.destroy_live(*id);
            self.mru.retain(|t| t != id);
        }
        if self
            .split
            .is_some_and(|(a, b)| ids.contains(&a) || ids.contains(&b))
        {
            self.split = None;
        }
        self.session.spaces.remove(index);
        self.private_context = None;
        let back = self
            .session
            .space_index(self.last_normal_space)
            .unwrap_or(0)
            .min(self.session.spaces.len().saturating_sub(1));
        self.session.active_space = back;
        self.switch_space(back);
        self.toast("Fap mode ended".to_owned());
        self.changed();
    }

    fn ensure_live(&mut self, id: TabId) {
        if self.live.contains_key(&id) {
            return;
        }
        let Some(url) = self.session.tab(id).map(|t| t.url.clone()) else {
            return;
        };
        let view = if self.session.is_private_tab(id) {
            if self.private_context.is_none() {
                self.private_context = views::create_private_context();
            }
            views::create_browser_view_in(
                Role::Tab(id),
                &url,
                0xFFFF_FFFF,
                self.private_context.as_mut(),
            )
        } else {
            views::create_browser_view(Role::Tab(id), &url, 0xFFFF_FFFF)
        };
        view.set_visible(0);
        self.pages.add_child_view(Some(&mut View::from(&view)));
        self.live.insert(
            id,
            Live {
                view,
                loading: true,
                back: false,
                forward: false,
            },
        );
    }

    fn destroy_live(&mut self, id: TabId) {
        self.ext_tab_ids.remove(&id);
        self.network_detach(id);
        let Some(live) = self.live.remove(&id) else {
            return;
        };
        if let Some(host) = live.view.browser().and_then(|b| b.host()) {
            host.close_browser(1);
        }
        self.pages
            .remove_child_view(Some(&mut View::from(&live.view)));
        self.close_docked_devtools(id);
        self.content.layout();
    }

    fn layout_tabs(&mut self) {
        let shown = self.shown_tabs();
        if shown.len() < 2 {
            self.split = None;
        }
        self.pages
            .set_to_box_layout(Some(&pages_layout(self.split_stacked)));
        // A new layout only applies once the panel lays out again.
        self.pages.invalidate_layout();
        for (id, live) in &self.live {
            live.view.set_visible(shown.contains(id) as c_int);
        }
        // No tab to show: the splash, focused so ⌘K works right away.
        let splash = shown.is_empty();
        if splash != (self.start.is_visible() != 0) {
            self.start.set_visible(splash as c_int);
            if splash {
                self.ui_dirty = true; // it needs the current space
            }
        }
        if splash && self.palette_mode.is_none() {
            self.start.request_focus();
        }
        let active = self.session.active_tab();
        for (tab, view) in &self.docked_devtools {
            view.set_visible((Some(*tab) == active) as c_int);
        }
        self.autofill_follow_tab();
        self.ext_follow_tab();
        self.content.layout();
        self.update_page_corners();
        self.update_window_title();
        self.ui_dirty = true;
    }

    /// Rounds the corners of the visible pages (macOS).
    fn update_page_corners(&self) {
        self.place_splitter();
        #[cfg(target_os = "macos")]
        if let Some(window) = &self.window {
            let content = self.content.bounds();
            let area = self.pages.bounds();
            let offset = |b: Rect, x: c_int, y: c_int| Rect {
                x: x + b.x,
                y: y + b.y,
                width: b.width,
                height: b.height,
            };
            let mut pages: Vec<(Rect, bool)> = self
                .shown_tabs()
                .iter()
                .filter_map(|id| Some((self.live.get(id)?, self.session.tab(*id)?)))
                .map(|(live, tab)| {
                    let rect = offset(live.view.bounds(), content.x + area.x, content.y + area.y);
                    (rect, is_local_url(&tab.url))
                })
                .collect();
            if self.start.is_visible() != 0 {
                pages.push((
                    offset(self.start.bounds(), content.x + area.x, content.y + area.y),
                    false,
                ));
            }
            pages.extend(
                self.docked_devtools
                    .values()
                    .chain(&self.network)
                    .filter(|view| view.is_visible() != 0)
                    .map(|view| (offset(view.bounds(), content.x, content.y), false)),
            );
            pages.retain(|(r, _)| r.width > 0 && r.height > 0);
            let gap = if self.fullscreen { 0.0 } else { GAP as f64 };
            let stripes = stripe_color(self.session.space().hue, self.dark);
            crate::mac::set_page_corners(
                window,
                &pages,
                gap,
                PAGE_RADIUS,
                self.background(),
                stripes,
            );
        }
    }

    fn activate(&mut self, id: TabId) {
        let Some(location) = self.session.locate(id) else {
            return;
        };
        if let Location::Pinned(space, _) | Location::Today(space, _) = location
            && space != self.session.active_space
        {
            self.session.active_space = space;
            self.split = None;
            self.apply_theme();
        }
        if self.split.is_some_and(|(a, b)| a != id && b != id) {
            self.split = None;
        }
        if self.bug.is_some()
            && self.session.active_tab() != Some(id)
            && !self.session.is_private_tab(id)
            && let Some(tab) = self.session.tab(id)
        {
            let (title, url) = (tab.display_title().to_owned(), tab.url.clone());
            self.bug_log("switch tab", &title, Some(&url));
        }
        self.session.space_mut().active = Some(id);
        if let Some(tab) = self.session.tab_mut(id) {
            tab.last_active = now();
        }
        self.mru.retain(|t| *t != id);
        self.mru.insert(0, id);
        self.ensure_live(id);
        self.layout_tabs();
        if self.palette_mode.is_none()
            && let Some(live) = self.live.get(&id)
        {
            live.view.request_focus();
        }
        self.changed();
    }

    fn open_tab(&mut self, url: &str, foreground: bool, opener: Option<TabId>) -> TabId {
        let tab = self.session.new_tab(url);
        let id = tab.id;
        // A tab opened from a page stays on the same side of fap mode as its
        // opener, whichever space is showing by then.
        let active = self.session.active_space;
        let space_index = match opener.map(|o| self.session.is_private_tab(o)) {
            Some(true) => self
                .session
                .spaces
                .iter()
                .position(|s| s.private)
                .unwrap_or(active),
            Some(false) if self.session.space().private => self
                .session
                .space_index(self.last_normal_space)
                .unwrap_or(0),
            _ => active,
        };
        // New tabs go to the top of "today", or right below their opener.
        let space = &mut self.session.spaces[space_index];
        let index = opener
            .and_then(|o| space.tabs.iter().position(|t| t.id == o))
            .map_or(0, |i| i + 1);
        space.tabs.insert(index, tab);
        if foreground {
            self.activate(id);
        } else {
            self.ensure_live(id);
            self.layout_tabs();
            self.changed();
        }
        id
    }

    fn navigate(&mut self, id: TabId, url: &str) {
        if let Some(tab) = self.session.tab_mut(id) {
            tab.url = url.to_owned();
        }
        match self
            .live
            .get(&id)
            .and_then(|l| l.view.browser()?.main_frame())
        {
            Some(frame) => frame.load_url(Some(&CefString::from(url))),
            None => self.activate(id),
        }
        if let Some(live) = self.live.get(&id) {
            live.view.request_focus();
        }
        self.changed();
    }

    fn close_tab(&mut self, id: TabId) {
        let Some(location) = self.session.locate(id) else {
            return;
        };
        if self.bug.is_some()
            && !self.session.is_private_tab(id)
            && let Some(tab) = self.session.tab(id)
        {
            let url = tab.url.clone();
            self.bug_log("close tab", "", Some(&url));
        }
        let was_active = self.session.active_tab() == Some(id);
        self.destroy_live(id);
        for prompt in std::mem::take(&mut self.prompts) {
            if prompt.request.tab == id {
                prompt.request.finish(false);
            } else {
                self.prompts.push(prompt);
            }
        }
        match location {
            Location::Today(..) => {
                let private = self.session.is_private_tab(id);
                if let Some(tab) = self.session.take(id)
                    && !private
                {
                    self.session.archive(&tab);
                }
            }
            // Pinned tabs and favorites are unloaded and reset, not removed.
            Location::Favorite(_) | Location::Pinned(..) => {
                if let Some(tab) = self.session.tab_mut(id)
                    && let Some(home) = tab.home_url.clone()
                {
                    tab.url = home;
                }
            }
        }
        for space in &mut self.session.spaces {
            if space.active == Some(id) {
                space.active = None;
            }
        }
        self.mru.retain(|t| *t != id);
        if was_active {
            let order = self.tab_order();
            let next = match location {
                Location::Today(space, index) if space == self.session.active_space => {
                    let tabs = &self.session.spaces[space].tabs;
                    tabs.get(index).or_else(|| tabs.last()).map(|t| t.id)
                }
                _ => None,
            }
            .or_else(|| self.mru.iter().copied().find(|t| order.contains(t)));
            match next {
                Some(next) => self.activate(next),
                None => self.layout_tabs(),
            }
        } else {
            self.layout_tabs();
        }
        self.changed();
    }

    fn switch_space(&mut self, index: usize) {
        if index >= self.session.spaces.len() {
            return;
        }
        self.session.active_space = index;
        self.split = None;
        self.apply_theme();
        match self.session.active_tab() {
            Some(id) => self.activate(id),
            None => self.layout_tabs(),
        }
        self.changed();
    }

    fn delete_space(&mut self, id: u64) {
        if self.session.spaces.len() < 2 {
            return;
        }
        let Some(index) = self.session.space_index(id) else {
            return;
        };
        let space = self.session.spaces.remove(index);
        for tab in space.pinned.iter().chain(&space.tabs) {
            self.destroy_live(tab.id);
            self.session.archive(tab);
            self.mru.retain(|t| *t != tab.id);
        }
        let active = self.session.active_space;
        self.session.active_space =
            if active > index { active - 1 } else { active }.min(self.session.spaces.len() - 1);
        self.switch_space(self.session.active_space);
    }

    fn archive_stale_tabs(&mut self) {
        let mut keep: Vec<TabId> = self
            .session
            .spaces
            .iter()
            .filter_map(|s| s.active)
            .collect();
        keep.extend(self.split.iter().flat_map(|(a, b)| [*a, *b]));
        let removed = self.session.auto_archive(&keep);
        for id in &removed {
            self.destroy_live(*id);
            self.mru.retain(|t| t != id);
        }
        if !removed.is_empty() {
            self.changed();
        }
    }

    fn copy_url(&mut self, id: TabId) {
        let Some(url) = self.session.tab(id).map(|t| t.url.clone()) else {
            return;
        };
        let copied = arboard::Clipboard::new().and_then(|mut c| c.set_text(url));
        let message = if copied.is_ok() {
            "Copied link"
        } else {
            "Couldn't copy link"
        };
        self.toast(message.to_owned());
    }

    /// Opens Chrome's DevTools for a tab in its own window, connected through
    /// Circle's local CDP bridge (see `remote.rs`).
    fn open_dev_tools(&mut self, tab: TabId, inspect_at: Option<Point>) {
        self.ensure_live(tab);
        let Some(url) = crate::remote::devtools_url(tab) else {
            self.toast("Developer tools are unavailable".to_owned());
            return;
        };
        if self.session.devtools_dock == DevToolsDock::Window {
            let title = self
                .session
                .tab(tab)
                .map(|t| t.display_title().to_owned())
                .unwrap_or_default();
            views::open_dev_tools_window(&url, &title);
        } else if let Some(view) = self.docked_devtools.get(&tab) {
            // Already open: just focus it. (Inspecting needs a fresh front-end.)
            if inspect_at.is_none() {
                view.request_focus();
                return;
            }
            self.close_docked_devtools(tab);
            return self.open_dev_tools(tab, inspect_at);
        } else {
            let view = views::create_browser_view(Role::DevTools(tab), &url, 0xFF20_2124);
            self.content.add_child_view(Some(&mut View::from(&view)));
            if self.splitter.is_none()
                && let Some(window) = &self.window
            {
                let handle =
                    views::create_browser_view(Role::Splitter, SPLITTER_URL, self.background());
                // Activatable: macOS only shows the resize cursor over it then.
                let overlay =
                    window.add_overlay_view(Some(&mut View::from(&handle)), DockingMode::CUSTOM, 1);
                self.splitter = Some((handle, overlay));
            }
            self.docked_devtools.insert(tab, view);
            self.keep_network_last();
            self.apply_layout();
            self.layout_tabs();
        }
        if let Some(point) = inspect_at {
            crate::remote::inspect_when_ready(tab, point);
        }
    }

    fn close_docked_devtools(&mut self, tab: TabId) {
        let Some(view) = self.docked_devtools.remove(&tab) else {
            return;
        };
        // Not a forced close: force-closing a DevTools front-end crashes CEF.
        if let Some(host) = view.browser().and_then(|b| b.host()) {
            host.close_browser(0);
        }
        self.content.remove_child_view(Some(&mut View::from(&view)));
        self.content.layout();
        self.update_page_corners();
    }

    /// Resizes docked DevTools as its grip is dragged.
    fn drag_devtools(&mut self, drag: crate::client::GripDrag) {
        use crate::client::GripDrag;
        let bottom = self.session.devtools_dock == DevToolsDock::Bottom;
        match drag {
            GripDrag::Start => {
                self.devtools_drag_start = Some(if bottom {
                    self.session.devtools_height
                } else {
                    self.session.devtools_width
                });
            }
            GripDrag::Move(dx, dy) => {
                let Some(start) = self.devtools_drag_start else {
                    return;
                };
                let area = self.content.bounds();
                // Leave the page at least 200pt; DevTools gets at least 200pt.
                let max = if bottom { area.height } else { area.width } - 200;
                // Dragging the grip towards the page makes DevTools bigger.
                let size = (start - if bottom { dy } else { dx }).clamp(200, max.max(200));
                if bottom {
                    self.session.devtools_height = size;
                } else {
                    self.session.devtools_width = size;
                }
                self.apply_layout();
            }
            GripDrag::End => {
                self.devtools_drag_start = None;
                self.changed();
            }
        }
    }

    /// Adds or updates a download, keeping its place and start time.
    fn add_download(&mut self, mut info: DownloadInfo) {
        match self.downloads.iter_mut().find(|d| d.id == info.id) {
            Some(existing) => {
                info.at = existing.at;
                *existing = info;
            }
            None => {
                info.at = now();
                self.downloads.insert(0, info);
                self.downloads.truncate(MAX_DOWNLOADS);
            }
        }
        self.ui_dirty = true;
    }

    fn updates_dir(&self) -> PathBuf {
        // The session always lives in the profile folder.
        self.session_path
            .parent()
            .unwrap_or_else(|| Path::new("/nonexistent"))
            .join("updates")
    }

    fn update_json(&self) -> Value {
        json!({
            "current": env!("CARGO_PKG_VERSION"),
            "checking": self.update_checking,
            "status": self.update_status,
        })
    }

    fn start_recording(&mut self, format: crate::recorder::Format) {
        use crate::recorder::Format;
        let Some(tab) = self
            .session
            .active_tab()
            .filter(|id| self.live.contains_key(id))
        else {
            self.toast("Open a page to record".to_owned());
            return;
        };
        self.recording = Some((format, now_ms()));
        self.ui_dirty = true;
        match format {
            Format::Gif => crate::recorder::start(tab, format, self.recording_marks()),
            Format::Video if self.recorder_ready => self.begin_video(tab),
            Format::Video => {
                self.pending_recording = Some(tab);
                if self.recorder.is_none() {
                    self.create_recorder();
                }
            }
        }
    }

    fn begin_video(&mut self, tab: TabId) {
        if let Some((view, _)) = &self.recorder {
            run_js(view, "circleRecorder.start()");
        }
        crate::recorder::start(tab, crate::recorder::Format::Video, self.recording_marks());
    }

    fn recording_marks(&self) -> crate::recorder::Marks {
        crate::recorder::Marks {
            clicks: self.session.recording_clicks,
            keys: self.session.recording_keys,
        }
    }

    /// The recorder page lives in a 2×2 overlay in the window's corner: a
    /// page has to be on screen for Chromium to encode video in real time.
    fn create_recorder(&mut self) {
        let Some(window) = &self.window else {
            return;
        };
        let view = views::create_browser_view(Role::Recorder, RECORDER_URL, self.background());
        let overlay = window.add_overlay_view(Some(&mut View::from(&view)), DockingMode::CUSTOM, 0);
        if let Some(overlay) = &overlay {
            overlay.set_bounds(Some(&Rect {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            }));
            overlay.set_visible(1);
        }
        self.recorder = Some((view, overlay));
    }

    /// Opens the history page, reusing a tab that already shows it.
    fn show_history(&mut self, clear: bool) {
        if clear {
            self.show_page("circle://ui/history.html?clear");
        } else {
            self.show_page("circle://ui/history.html");
        }
    }

    /// Opens one of Circle's own pages, reusing a tab that already shows it.
    fn show_page(&mut self, url: &str) {
        let page = url.split('?').next().unwrap_or(url);
        let existing = self
            .session
            .all_tabs()
            .find(|t| t.url.starts_with(page))
            .map(|t| (t.id, t.url != url));
        match existing {
            Some((id, other_query)) => {
                self.activate(id);
                if other_query {
                    self.navigate(id, url);
                }
            }
            None => {
                self.open_tab(url, true, None);
            }
        }
    }

    fn settings_json(&self) -> Value {
        json!({
            "appearance": self.session.appearance,
            "search_url": self.session.search_url,
            "archive_after_hours": self.session.archive_after_hours,
            "devtools_dock": self.session.devtools_dock,
            "agent_access": self.session.agent_access,
            "grants": self.session.grants.keys().collect::<Vec<_>>(),
            "profile_dir": self.session_path.parent().map(|p| p.display().to_string()),
            "version": env!("CARGO_PKG_VERSION"),
            "auto_update": self.session.auto_update,
            "spotlight_blur": self.session.spotlight_blur,
            "recording_clicks": self.session.recording_clicks,
            "recording_keys": self.session.recording_keys,
            "recording_float": self.session.recording_float,
            "hue": self.session.space().hue,
            "passwords_enabled": self.session.passwords_enabled,
            "passwords_offer": self.session.passwords_offer,
            "passwords": self.password_list(),
            "passwords_never": self.vault.as_ref().map(|v| v.never_list().to_vec()).unwrap_or_default(),
            "passwords_error": self.vault_error,
            "update": self.update_json(),
            "extensions": self.extensions_settings_json(),
        })
    }

    /// Opens the settings panel at a section.
    fn show_settings_section(&mut self, section: &'static str) {
        self.settings_section = Some(section);
        if self.palette_mode != Some(PaletteMode::Settings) {
            self.open_palette(PaletteMode::Settings);
        }
        self.push_settings_with(json!({ "section": section }));
    }

    fn start_bug_report(&mut self, video: bool) {
        if self.bug.is_some() {
            return;
        }
        self.bug_result = None;
        let mut bug = crate::bugreport::Session::start(video);
        bug.log("start", "Recording started", None);
        self.bug = Some(bug);
        // Back to the page the problem is on.
        let report_tab = self.session.active_tab();
        let previous = self
            .mru
            .iter()
            .copied()
            .find(|id| Some(*id) != report_tab && self.session.tab(*id).is_some());
        if let Some(id) = previous {
            self.activate(id);
        }
        if video && self.recording.is_none() {
            self.start_recording(crate::recorder::Format::Video);
        }
        self.ui_dirty = true;
        self.push_bug_report();
    }

    fn stop_bug_report(&mut self) {
        let Some(mut bug) = self.bug.take() else {
            return;
        };
        bug.log("stop", "Recording stopped", None);
        if bug.video && self.recording.is_some() {
            crate::recorder::stop();
        }
        self.bug_result = Some(bug.finish());
        self.ui_dirty = true;
        self.show_page("circle://ui/bugreport.html");
        self.push_bug_report();
    }

    fn bug_log(&mut self, kind: &'static str, text: &str, url: Option<&str>) {
        // The report page itself isn't part of the story.
        if url.is_some_and(|u| u.starts_with("circle://ui/bugreport")) {
            return;
        }
        if let Some(bug) = &mut self.bug {
            bug.log(kind, text, url);
        }
    }

    /// Sends the bug report state to any open bug report page.
    fn push_bug_report(&self) {
        let state = json!({
            "recording": self.bug.as_ref().map(|b| json!({ "elapsed": b.elapsed_ms(), "video": b.video })),
            "result": self.bug_result,
            "environment": crate::bugreport::environment(),
        });
        let code = format!("window.circle&&circle.render({state})");
        for (id, live) in &self.live {
            if self
                .session
                .tab(*id)
                .is_some_and(|t| t.url.starts_with("circle://ui/bugreport"))
            {
                run_js(&live.view, &code);
            }
        }
    }

    /// Sends the current settings to any open settings page.
    fn push_settings(&self) {
        self.push_settings_with(Value::Null);
    }

    /// Pushes the settings with one-off `extra` fields (a revealed password).
    fn push_settings_with(&self, extra: Value) {
        let json = || {
            let mut settings = self.settings_json();
            if let (Some(settings), Value::Object(extra)) =
                (settings.as_object_mut(), extra.clone())
            {
                settings.extend(extra);
            }
            settings
        };
        let mut code = None;
        for (id, live) in &self.live {
            if !self
                .session
                .tab(*id)
                .is_some_and(|t| t.url.starts_with("circle://ui/settings"))
            {
                continue;
            }
            let code =
                code.get_or_insert_with(|| format!("window.circle&&circle.render({})", json()));
            run_js(&live.view, code);
        }
        // The settings panel lives in the command bar's overlay.
        if self.palette_mode == Some(PaletteMode::Settings) && self.palette_ready {
            run_js(
                &self.palette,
                &format!("window.circle&&circle.settings({})", json()),
            );
        }
    }

    fn clear_browsing_data(
        &mut self,
        hours: Option<u64>,
        history: bool,
        cookies: bool,
        cache: bool,
    ) {
        if history {
            let since = hours.map_or(0, |h| now().saturating_sub(h * 3600));
            crate::history::clear_since(since);
            // "Recently closed" is history too.
            self.session
                .archive
                .retain(|a| since > 0 && a.closed_at < since);
        }
        if cookies && let Some(manager) = cookie_manager_get_global_manager(None) {
            manager.delete_cookies(None, None, None);
        }
        if cache && let Some(tab) = self.live.keys().next().copied() {
            // The cache is shared, so any loaded tab can clear it. CDP calls
            // wait on the UI thread, so this runs on a worker thread.
            std::thread::spawn(move || {
                let cleared = crate::remote::Session::open(tab).and_then(|mut session| {
                    session.call(
                        "Network.clearBrowserCache",
                        json!({}),
                        std::time::Duration::from_secs(20),
                    )
                });
                if let Err(err) = cleared {
                    eprintln!("circle: clearing the cache failed: {err}");
                }
            });
        }
        self.toast("Browsing data cleared".to_owned());
        self.changed();
    }

    fn toast(&mut self, text: String) {
        let nonce = self.toast.as_ref().map_or(1, |(n, _)| n + 1);
        self.toast = Some((nonce, text));
        self.ui_dirty = true;
    }

    fn find(&mut self, text: String, backward: bool) {
        let Some(host) = self.session.active_tab().and_then(|id| self.host(id)) else {
            return;
        };
        if text.is_empty() {
            host.stop_finding(1);
            self.find_result = (0, 0);
            self.ui_dirty = true;
        } else {
            let next = text == self.find_text;
            host.find(
                Some(&CefString::from(text.as_str())),
                !backward as c_int,
                0,
                next as c_int,
            );
        }
        self.find_text = text;
    }

    fn request_permission(&mut self, request: PermissionRequest) {
        // Fap mode never uses what was allowed outside it.
        let granted = if self.session.is_private_tab(request.tab) {
            0
        } else {
            self.session
                .grants
                .get(&request.grant_key())
                .copied()
                .unwrap_or(0)
        };
        if request.mask != 0 && request.mask & !granted == 0 {
            request.finish(true);
            return;
        }
        let id = self.next_prompt;
        self.next_prompt += 1;
        self.prompts.push(Prompt { id, request });
        if !self.session.sidebar_visible {
            self.set_sidebar_visible(true);
        }
        self.ui_dirty = true;
    }

    // ----- window & UI ------------------------------------------------------

    fn sidebar_shown(&self) -> bool {
        self.session.sidebar_visible && !self.fullscreen
    }

    /// Shows or hides the sidebar with an animation.
    fn set_sidebar_visible(&mut self, visible: bool) {
        self.session.sidebar_visible = visible;
        self.peek_open = false;
        run_js(
            &self.sidebar,
            &format!("window.circle&&circle.animate({visible})"),
        );
        self.sidebar_animation = Some(SidebarAnimation {
            start: std::time::Instant::now(),
            from: self.sidebar_width,
            to: if visible { SIDEBAR_WIDTH as f64 } else { 0.0 },
        });
        self.step_sidebar_animation();
        self.changed();
    }

    fn step_sidebar_animation(&mut self) {
        let Some(animation) = &self.sidebar_animation else {
            return;
        };
        let t = (animation.start.elapsed().as_secs_f64() * 1000.0 / SIDEBAR_ANIMATION_MS).min(1.0);
        self.sidebar_width = animation.from + (animation.to - animation.from) * ease_out(t);
        if t >= 1.0 {
            self.sidebar_animation = None;
            self.place_recbar();
        } else {
            schedule(Timer::Animate, FRAME_MS);
        }
        self.apply_layout();
    }

    fn apply_layout(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let width = if self.fullscreen {
            0.0
        } else {
            self.sidebar_width
        };
        views::set_sidebar_width(width.round() as c_int);
        let row = BoxLayoutSettings {
            horizontal: 1,
            cross_axis_alignment: AxisAlignment::STRETCH,
            ..Default::default()
        };
        if let Some(layout) = window.set_to_box_layout(Some(&row)) {
            layout.set_flex_for_view(Some(&mut View::from(&self.content)), 1);
        }
        self.sidebar.set_visible((width >= 1.0) as c_int);
        self.sidebar.invalidate_layout();
        // The frame is an even GAP on every side; while the sidebar is open it
        // takes the place of the left edge.
        let gap = if self.fullscreen { 0 } else { GAP };
        let left = (gap as f64 * (1.0 - width / SIDEBAR_WIDTH as f64)).round() as c_int;
        // Docked DevTools sit next to (right) or below (bottom) the pages.
        let bottom = self.session.devtools_dock == DevToolsDock::Bottom;
        views::set_devtools_size(if bottom {
            self.session.devtools_height
        } else {
            self.session.devtools_width
        });
        views::set_network_size(if bottom {
            self.session.network_height
        } else {
            self.session.network_width
        });
        let content = BoxLayoutSettings {
            horizontal: (!bottom) as c_int,
            inside_border_insets: Insets {
                top: gap,
                left,
                bottom: gap,
                right: gap,
            },
            between_child_spacing: GAP,
            cross_axis_alignment: AxisAlignment::STRETCH,
            ..Default::default()
        };
        if let Some(layout) = self.content.set_to_box_layout(Some(&content)) {
            layout.set_flex_for_view(Some(&mut View::from(&self.pages)), 1);
        }
        for view in self.docked_devtools.values().chain(&self.network) {
            view.invalidate_layout();
        }
        window.layout();
        self.update_page_corners();
        self.place_palette();
        self.place_peek();
        if self.sidebar_animation.is_none() {
            self.apply_drag_regions();
        }
        self.ui_dirty = true;
    }

    fn apply_drag_regions(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let mac = cfg!(target_os = "macos");
        // The frame's top edge always drags the window; with the sidebar open,
        // so do the sidebar's own draggable areas (listed after the strip so
        // its buttons, marked non-draggable, take precedence).
        let top_strip = DraggableRegion {
            bounds: Rect {
                x: 0,
                y: 0,
                width: window.bounds().width,
                height: GAP,
            },
            draggable: 1,
        };
        let regions = if !mac || self.fullscreen {
            Vec::new()
        } else if self.sidebar_shown() {
            std::iter::once(top_strip)
                .chain(self.drag_regions.iter().cloned())
                .collect()
        } else {
            vec![top_strip]
        };
        window.set_draggable_regions(Some(&regions));
        #[cfg(target_os = "macos")]
        {
            crate::mac::set_titlebar_regions(window, regions);
            crate::mac::set_traffic_lights_hidden(!self.sidebar_shown() && !self.fullscreen);
        }
    }

    /// Sets Chromium's color scheme, which every page (Circle's own UI
    /// included) sees as `prefers-color-scheme`. The sidebar then reports the
    /// result back with `UiCmd::Theme`.
    fn apply_appearance(&self) {
        let variant = match self.session.appearance {
            Appearance::System => ColorVariant::SYSTEM,
            Appearance::Light => ColorVariant::LIGHT,
            Appearance::Dark => ColorVariant::DARK,
        };
        if let Some(context) = request_context_get_global_context() {
            context.set_chrome_color_scheme(variant, 0);
        }
        #[cfg(target_os = "macos")]
        crate::mac::set_appearance(match self.session.appearance {
            Appearance::System => None,
            Appearance::Light => Some(false),
            Appearance::Dark => Some(true),
        });
    }

    fn apply_theme(&mut self) {
        // A private space stays out of screen sharing and screenshots.
        #[cfg(target_os = "macos")]
        if let Some(window) = &self.window {
            crate::mac::set_capture_hidden(window, self.session.space().private);
        }
        let color = self.background();
        if let Some(window) = &self.window {
            window.set_background_color(color);
        }
        self.content.set_background_color(color);
        self.sidebar.set_background_color(color);
        self.peek.set_background_color(color);
        self.start.set_background_color(color);
        if let Some((view, _)) = &self.splitter {
            view.set_background_color(color);
        }
        self.update_page_corners();
        self.ui_dirty = true;
    }

    fn update_window_title(&self) {
        let Some(window) = &self.window else {
            return;
        };
        let title = self
            .session
            .active_tab()
            .and_then(|id| self.session.tab(id))
            .map(|t| {
                if t.display_title().is_empty() {
                    t.url.clone()
                } else {
                    t.display_title().to_owned()
                }
            })
            .unwrap_or_default();
        // In fap mode nothing about the page shows in the window list or app
        // switcher, nor that it's fap mode.
        let title = if self.session.space().private || title.is_empty() {
            "Circle".to_owned()
        } else {
            format!("{title} – Circle")
        };
        window.set_title(Some(&CefString::from(title.as_str())));
    }

    fn open_palette(&mut self, mode: PaletteMode) {
        if mode == PaletteMode::Find && self.session.active_tab().is_none() {
            return;
        }
        let reopen = self.palette_mode.is_some();
        self.palette_mode = Some(mode);
        if self.overlay.is_none() {
            return;
        }
        self.palette_generation += 1;
        self.place_palette();
        // Everything but the find bar sits over a blurred snapshot of the
        // page. The overlay waits (briefly) for it, so the page never
        // flashes away.
        let snapshot = mode != PaletteMode::Find && !reopen && self.capture_backdrop();
        if mode == PaletteMode::Settings {
            self.push_settings();
        }
        if snapshot {
            self.palette_shown = false;
            schedule(Timer::ShowPalette(self.palette_generation), 250);
        } else {
            self.show_palette();
        }
        if !self.palette_ready {
            return;
        }
        let text = match mode {
            PaletteMode::Edit => self
                .session
                .active_tab()
                .and_then(|id| self.session.tab(id))
                .map(|t| t.url.clone())
                .unwrap_or_default(),
            PaletteMode::Find => self.find_text.clone(),
            PaletteMode::NewTab
            | PaletteMode::Spotlight
            | PaletteMode::Settings
            | PaletteMode::WhatsNew => String::new(),
        };
        self.push_state();
        let area = self.content.bounds();
        let open = json!({
            "mode": mode,
            "text": text,
            "backdrop": mode != PaletteMode::Find,
            "keep_backdrop": reopen,
            "blur": self.session.spotlight_blur,
            "notes": (mode == PaletteMode::WhatsNew).then_some(&self.whats_new),
            "version": env!("CARGO_PKG_VERSION"),
            // No page to blur: the splash shows, so the backdrop imitates it.
            "splash": self.shown_tabs().is_empty(),
            "frame": format!("#{:06x}", self.background() & 0xFF_FFFF),
            "top": (area.height / 7).max(40),
        });
        run_js(
            &self.palette,
            &format!("window.circle&&circle.open({open})"),
        );
    }

    fn show_palette(&mut self) {
        if self.palette_shown || self.palette_mode.is_none() {
            return;
        }
        if let Some(overlay) = &self.overlay {
            overlay.set_visible(1);
        }
        self.palette_shown = true;
        self.palette.request_focus();
        self.ui_dirty = true;
    }

    /// Where the command bar's backdrop goes: the page area plus the frame
    /// strip between it and the sidebar (which blurs itself).
    fn backdrop_area(&self) -> Rect {
        let area = self.content.bounds();
        let x = (area.x - GAP).max(0);
        Rect {
            x,
            y: area.y,
            width: area.width + (area.x - x),
            height: area.height,
        }
    }

    /// Screenshots the visible pages on a worker thread for the command
    /// bar's backdrop. False when there is nothing to capture.
    fn capture_backdrop(&self) -> bool {
        let content = self.content.bounds();
        let shift = content.x - self.backdrop_area().x;
        let pages = self.pages.bounds();
        let shots: Vec<(TabId, Rect)> = self
            .shown_tabs()
            .into_iter()
            .filter_map(|id| {
                let b = self.live.get(&id)?.view.bounds();
                let rect = Rect {
                    x: shift + pages.x + b.x,
                    y: pages.y + b.y,
                    width: b.width,
                    height: b.height,
                };
                (rect.width > 0 && rect.height > 0).then_some((id, rect))
            })
            .collect();
        if shots.is_empty() {
            return false;
        }
        let generation = self.palette_generation;
        std::thread::spawn(move || {
            let mut out = Vec::new();
            for (tab, rect) in shots {
                let shot = crate::remote::Session::open(tab).and_then(|mut session| {
                    session.call(
                        "Page.captureScreenshot",
                        json!({ "format": "jpeg", "quality": 50, "optimizeForSpeed": true }),
                        std::time::Duration::from_millis(800),
                    )
                });
                if let Some(data) = shot
                    .ok()
                    .and_then(|v| v["data"].as_str().map(str::to_owned))
                {
                    out.push((rect, data));
                }
            }
            crate::remote::run_on_ui(move || {
                dispatch(Cmd::PaletteBackdrop(generation, out));
            });
        });
        true
    }

    fn close_palette(&mut self) {
        let Some(mode) = self.palette_mode.take() else {
            return;
        };
        self.palette_shown = false;
        self.palette_generation += 1;
        if let Some(overlay) = &self.overlay {
            overlay.set_visible(0);
        }
        self.place_splitter();
        self.place_autofill();
        self.place_ext_card();
        if mode == PaletteMode::Find {
            if let Some(host) = self.session.active_tab().and_then(|id| self.host(id)) {
                host.stop_finding(1);
            }
            self.find_result = (0, 0);
        }
        if let Some(live) = self.session.active_tab().and_then(|id| self.live.get(&id)) {
            live.view.request_focus();
        }
        self.ui_dirty = true;
    }

    /// Shows the peek strip (or the open peek sidebar) while the sidebar is
    /// collapsed, and hides it otherwise.
    fn place_peek(&self) {
        let Some(overlay) = &self.peek_overlay else {
            return;
        };
        let collapsed =
            !self.session.sidebar_visible && !self.fullscreen && self.sidebar_animation.is_none();
        if !collapsed {
            overlay.set_visible(0);
            return;
        }
        let area = self.content.bounds();
        let height = area.y + area.height + area.y.min(GAP);
        let bounds = if self.peek_open {
            // It sits on the window's left edge, so it follows the window's
            // own rounded corners there (its view exists only once shown).
            #[cfg(target_os = "macos")]
            if let Some(browser) = self.peek.browser() {
                crate::mac::round_browser_window(
                    &browser,
                    f64::from(WINDOW_RADIUS),
                    crate::mac::LEFT_CORNERS,
                );
            }
            Rect {
                x: 0,
                y: 0,
                width: SIDEBAR_WIDTH,
                height,
            }
        } else {
            Rect {
                x: 0,
                y: WINDOW_RADIUS,
                width: PEEK_STRIP,
                height: height - 2 * WINDOW_RADIUS,
            }
        };
        overlay.set_bounds(Some(&bounds));
        overlay.set_visible(1);
    }

    /// Puts the drag handle over the gap between the pages and the active
    /// tab's docked DevTools, or hides it.
    fn place_splitter(&self) {
        let Some((_, Some(overlay))) = &self.splitter else {
            return;
        };
        let devtools = self
            .session
            .active_tab()
            .and_then(|tab| self.docked_devtools.get(&tab))
            .filter(|view| view.is_visible() != 0);
        // The command bar's backdrop covers the gap; the find bar doesn't.
        let covered = self
            .palette_mode
            .is_some_and(|mode| mode != PaletteMode::Find);
        let (Some(devtools), false, false) = (devtools, covered, self.fullscreen) else {
            overlay.set_visible(0);
            return;
        };
        let content = self.content.bounds();
        let pages = self.pages.bounds();
        let tools = devtools.bounds();
        let bounds = if self.session.devtools_dock == DevToolsDock::Bottom {
            Rect {
                x: content.x + pages.x,
                y: content.y + pages.y + pages.height,
                width: pages.width,
                height: tools.y - pages.y - pages.height,
            }
        } else {
            Rect {
                x: content.x + pages.x + pages.width,
                y: content.y + pages.y,
                width: tools.x - pages.x - pages.width,
                height: pages.height,
            }
        };
        if bounds.width <= 0 || bounds.height <= 0 {
            overlay.set_visible(0);
            return;
        }
        overlay.set_bounds(Some(&bounds));
        overlay.set_visible(1);
    }

    fn place_palette(&self) {
        self.place_splitter();
        self.place_autofill();
        self.place_ext_card();
        let (Some(overlay), Some(mode)) = (&self.overlay, self.palette_mode) else {
            return;
        };
        let area = self.content.bounds();
        let bounds = if mode == PaletteMode::Find {
            let width = 380.min(area.width - 2 * GAP);
            Rect {
                x: area.x + area.width - width - 2 * GAP,
                y: area.y + 2 * GAP,
                width,
                height: self.palette_height,
            }
        } else {
            self.backdrop_area()
        };
        overlay.set_bounds(Some(&bounds));
    }

    fn changed(&mut self) {
        self.ui_dirty = true;
        if !self.save_scheduled {
            self.save_scheduled = true;
            schedule(Timer::Save, SAVE_DELAY_MS);
        }
    }

    fn save(&self) {
        crate::history::save();
        if let Err(err) = self.session.save(&self.session_path) {
            eprintln!("circle: failed to save session: {err}");
        }
    }

    fn flush(&mut self) {
        if self.ui_dirty {
            self.ui_dirty = false;
            self.place_recbar();
            self.push_state();
        }
    }

    /// Recording controls float over the page while the sidebar (where they
    /// usually are) is hidden. They are their own overlay, so recordings of
    /// the page never include them.
    fn wants_recbar(&self) -> bool {
        self.session.recording_float
            && !self.session.sidebar_visible
            && self.sidebar_animation.is_none()
            && (self.record_prompt.is_some() || self.recording.is_some())
    }

    fn place_recbar(&mut self) {
        if !self.wants_recbar() {
            if let Some((_, Some(overlay))) = &self.recbar {
                overlay.set_visible(0);
            }
            return;
        }
        if self.recbar.is_none() {
            let Some(window) = &self.window else {
                return;
            };
            let view = views::create_browser_view(Role::RecBar, RECBAR_URL, self.background());
            let overlay =
                window.add_overlay_view(Some(&mut View::from(&view)), DockingMode::CUSTOM, 1);
            self.recbar = Some((view, overlay));
        }
        let Some((view, Some(overlay))) = &self.recbar else {
            return;
        };
        let area = self.content.bounds();
        let (width, height) = self.recbar_size;
        let width = width.min(area.width - 2 * RECBAR_MARGIN);
        overlay.set_bounds(Some(&Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + area.height - height - RECBAR_MARGIN,
            width,
            height,
        }));
        overlay.set_visible(1);
        #[cfg(target_os = "macos")]
        if let Some(browser) = view.browser() {
            crate::mac::round_browser_window(&browser, 14.0, crate::mac::ALL_CORNERS);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = view;
    }

    fn push_state(&self) {
        let state = self.state_json().to_string();
        let code = format!("window.circle&&circle.render({state})");
        if self.sidebar_ready {
            run_js(&self.sidebar, &code);
        }
        if self.peek_ready {
            run_js(&self.peek, &code);
        }
        if let Some((view, _)) = &self.recbar
            && self.recbar_ready
        {
            run_js(view, &code);
        }
        if self.start_ready && self.start.is_visible() != 0 {
            run_js(&self.start, &code);
        }
        if self.palette_ready && self.palette_mode.is_some() {
            run_js(&self.palette, &code);
        }
        self.push_settings();
        self.push_network();
    }

    fn tab_json(&self, tab: &crate::model::Tab) -> Value {
        let live = self.live.get(&tab.id);
        json!({
            "id": tab.id,
            "url": tab.url,
            "title": tab.display_title(),
            "page_title": tab.title,
            "renamed": tab.custom_title.is_some(),
            "favicon": tab.favicon,
            "live": live.is_some(),
            "loading": live.is_some_and(|l| l.loading),
            "moved": tab.home_url.as_ref().is_some_and(|h| *h != tab.url),
        })
    }

    fn state_json(&self) -> Value {
        let active = self.session.active_tab();
        let active_tab = active.and_then(|id| self.session.tab(id));
        let live = active.and_then(|id| self.live.get(&id));
        let spaces: Vec<Value> = self
            .session
            .spaces
            .iter()
            .map(|s| {
                json!({
                    "private": s.private,
                    "id": s.id,
                    "name": s.name,
                    "icon": s.icon,
                    "hue": s.hue,
                    "pinned": s
                        .pinned
                        .iter()
                        .filter(|t| t.folder.is_none_or(|f| !s.folders.iter().any(|folder| folder.id == f)))
                        .map(|t| self.tab_json(t))
                        .collect::<Vec<_>>(),
                    "folders": s
                        .folders
                        .iter()
                        .map(|f| {
                            let tabs: Vec<Value> = s
                                .pinned
                                .iter()
                                .filter(|t| t.folder == Some(f.id))
                                .map(|t| self.tab_json(t))
                                .collect();
                            json!({ "id": f.id, "name": f.name, "collapsed": f.collapsed, "tabs": tabs })
                        })
                        .collect::<Vec<_>>(),
                    "tabs": s.tabs.iter().map(|t| self.tab_json(t)).collect::<Vec<_>>(),
                })
            })
            .collect();
        let prompts: Vec<Value> = self
            .prompts
            .iter()
            .map(|p| {
                json!({
                    "id": p.id,
                    "origin": p.request.origin,
                    "kinds": p.request.describe(),
                })
            })
            .collect();
        json!({
            "platform": std::env::consts::OS,
            "dark": self.dark,
            "appearance": self.session.appearance,
            "fullscreen": self.fullscreen,
            "sidebar_width": SIDEBAR_WIDTH,
            "sidebar_visible": self.session.sidebar_visible,
            "active_space": self.session.active_space,
            "spaces": spaces,
            "favorites": self.session.favorites.iter().map(|t| self.tab_json(t)).collect::<Vec<_>>(),
            "active": active,
            "split": self.split.map(|(a, b)| [a, b]),
            "nav": {
                "url": active_tab.map(|t| t.url.as_str()),
                "title": active_tab.map(|t| t.display_title()),
                "loading": live.is_some_and(|l| l.loading),
                "back": live.is_some_and(|l| l.back),
                "forward": live.is_some_and(|l| l.forward),
            },
            "archive": self.session.archive.iter().take(100).collect::<Vec<_>>(),
            "downloads": self.downloads,
            "prompts": prompts,
            "toast": self.toast.as_ref().map(|(nonce, text)| json!({ "nonce": nonce, "text": text })),
            "devtools_dock": self.session.devtools_dock,
            "update": self.update_json(),
            "extensions": self.extensions_json(),
            "record_prompt": self.record_prompt.map(|format| json!({
                "format": format,
                "clicks": self.session.recording_clicks,
                "keys": self.session.recording_keys,
                "tab": self.session.active_tab().and_then(|id| self.session.tab(id)).map(|t| t.display_title().to_owned()),
            })),
            "bug_report": self.bug.as_ref().map(|b| json!({ "since": now_ms().saturating_sub(b.elapsed_ms()) })),
            "recording": self.recording.map(|(format, since)| json!({
                "format": format,
                "since": since,
                "saving": self.recording_saving,
            })),
            "agent": {
                "enabled": self.session.agent_access,
                "active": self.agent_active_until.is_some_and(|t| t > std::time::Instant::now()),
            },
            "palette": {
                "mode": self.palette_mode,
                "backdrop": self.palette_shown && self.palette_mode.is_some_and(|m| m != PaletteMode::Find),
                "find": { "count": self.find_result.0, "active": self.find_result.1 },
            },
        })
    }
}

/// Runs `code` in one of Circle's own pages, never in anything else the
/// view might show.
fn run_js(view: &BrowserView, code: &str) {
    if let Some(frame) = view.browser().and_then(|b| b.main_frame())
        && CefString::from(&frame.url())
            .to_string()
            .starts_with("circle://ui/")
    {
        frame.execute_java_script(
            Some(&CefString::from(code)),
            Some(&CefString::from("circle://ui/")),
            0,
        );
    }
}

/// Runs Circle's own script (the resize grip and dock buttons) in a docked
/// DevTools front-end, and nowhere else.
fn run_devtools_js(view: &BrowserView, code: &str) {
    if let Some(frame) = view.browser().and_then(|b| b.main_frame())
        && CefString::from(&frame.url())
            .to_string()
            .starts_with("devtools://devtools/")
    {
        frame.execute_java_script(Some(&CefString::from(code)), None, 0);
    }
}

fn reveal(path: &str) {
    use std::process::Command;
    let result = if cfg!(target_os = "macos") {
        Command::new("open").args(["-R", path]).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("explorer")
            .arg(format!("/select,{path}"))
            .spawn()
    } else {
        let dir = Path::new(path).parent().unwrap_or(Path::new("/"));
        Command::new("xdg-open").arg(dir).spawn()
    };
    if let Err(err) = result {
        eprintln!("circle: failed to reveal {path}: {err}");
    }
}

// ----- entry points for the agent API and the CDP bridge (UI thread) ---------

/// Runs `f` with the controller, then processes anything it queued.
fn with_controller<R>(f: impl FnOnce(&mut Controller) -> R) -> Option<R> {
    let result = CONTROLLER.with(|cell| {
        let mut guard = cell.try_borrow_mut().ok()?;
        Some(f(guard.as_mut()?))
    });
    pump();
    result
}

pub fn tab_browser(tab: TabId) -> Option<Browser> {
    CONTROLLER.with(|cell| {
        cell.try_borrow()
            .ok()?
            .as_ref()?
            .live
            .get(&tab)?
            .view
            .browser()
    })
}

pub struct TabSummary {
    pub id: TabId,
    pub title: String,
    pub url: String,
}

/// Live, non-private tabs for CDP clients; for agents only web pages.
pub fn tab_summaries(agent: bool) -> Vec<TabSummary> {
    with_controller(|c| {
        c.session
            .all_tabs()
            .filter(|t| {
                c.live.contains_key(&t.id)
                    && !c.session.is_private_tab(t.id)
                    && (!agent || is_web_url(&t.url))
            })
            .map(|t| TabSummary {
                id: t.id,
                title: t.display_title().to_owned(),
                url: t.url.clone(),
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn agent_tabs() -> Value {
    with_controller(|c| {
        let tab = |t: &crate::model::Tab, section: &str| {
            let mut entry = json!({ "tab": t.id, "title": t.display_title(), "url": t.url, "section": section, "loaded": c.live.contains_key(&t.id) });
            if t.custom_title.is_some() {
                entry["page_title"] = json!(t.title);
            }
            if let Some(folder) = t.folder.and_then(|f| c.session.locate_folder(f)) {
                entry["folder"] = json!(c.session.spaces[folder.0].folders[folder.1].name);
            }
            entry
        };
        // The private space is invisible to agents.
        let spaces: Vec<Value> = c
            .session
            .spaces
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.private)
            .map(|(i, s)| {
                let tabs: Vec<Value> = s
                    .pinned
                    .iter()
                    .map(|t| tab(t, "pinned"))
                    .chain(s.tabs.iter().map(|t| tab(t, "today")))
                    .collect();
                json!({ "space": s.name, "active": i == c.session.active_space, "tabs": tabs })
            })
            .collect();
        let favorites: Vec<Value> = c.session.favorites.iter().map(|t| tab(t, "favorite")).collect();
        json!({ "active_tab": c.session.active_tab(), "favorites": favorites, "spaces": spaces })
    })
    .unwrap_or(Value::Null)
}

pub fn agent_active_tab() -> Option<TabId> {
    with_controller(|c| c.session.active_tab()).flatten()
}

/// Whether a tab is in fap mode (assumed so if the browser is busy).
pub fn tab_is_private(tab: TabId) -> bool {
    with_controller(|c| c.session.is_private_tab(tab)).unwrap_or(true)
}

/// Pages agents may open and drive: the web, not local files or Circle's
/// own pages.
pub fn is_web_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://") || lower == "about:blank"
}

pub fn agent_open(input: &str, background: bool) -> Result<TabId, String> {
    with_controller(|c| {
        // New tabs go to the active space; never let an agent into fap mode.
        if c.session.space().private {
            return Err("Circle is in fap mode, where agents can't open tabs".to_owned());
        }
        let url = c.session.resolve_input(input);
        if !is_web_url(&url) {
            return Err("agents can only open http and https pages".to_owned());
        }
        Ok(c.open_tab(&url, !background, None))
    })
    .unwrap_or_else(|| Err("the browser is busy".to_owned()))
}

/// Whether agents may use a tab: never one in the private space, nor one
/// showing Circle's own pages or local files.
pub fn agent_may_use(tab: TabId) -> bool {
    with_controller(|c| {
        !c.session.is_private_tab(tab) && c.session.tab(tab).is_some_and(|t| is_web_url(&t.url))
    })
    .unwrap_or(false)
}

pub fn agent_activate(tab: TabId) {
    with_controller(|c| {
        if !c.session.is_private_tab(tab) && c.session.active_tab() != Some(tab) {
            c.activate(tab);
        }
    });
}

pub fn agent_close(tab: TabId) {
    with_controller(|c| {
        if !c.session.is_private_tab(tab) {
            c.close_tab(tab);
        }
    });
}

pub fn agent_navigate(tab: TabId, input: &str) -> Result<(), String> {
    with_controller(|c| {
        if c.session.is_private_tab(tab) || c.session.tab(tab).is_none() {
            return Err(format!("no tab {tab}"));
        }
        let url = c.session.resolve_input(input);
        if !is_web_url(&url) {
            return Err("agents can only open http and https pages".to_owned());
        }
        c.navigate(tab, &url);
        Ok(())
    })
    .unwrap_or_else(|| Err("the browser is busy".to_owned()))
}

pub fn agent_ensure_loaded(tab: TabId) -> Result<(), String> {
    with_controller(|c| {
        if c.session.tab(tab).is_none() || c.session.is_private_tab(tab) {
            return Err(format!("no tab {tab}"));
        }
        if !c.live.contains_key(&tab) {
            c.ensure_live(tab);
            c.layout_tabs();
        }
        Ok(())
    })
    .unwrap_or_else(|| Err("the browser is busy".into()))
}

/// Shows "agent active" in the sidebar for a little while after each request.
pub fn note_agent_activity() {
    const SHOW_FOR: std::time::Duration = std::time::Duration::from_secs(20);
    with_controller(|c| {
        c.agent_active_until = Some(std::time::Instant::now() + SHOW_FOR);
        c.ui_dirty = true;
    });
    schedule(Timer::AgentIdle, SHOW_FOR.as_millis() as i64 + 500);
}

mod extensions_ui;
mod network_ui;
mod passwords_ui;
use passwords_ui::confirm_owner;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_get_only_web_pages() {
        for url in [
            "https://example.com",
            "http://localhost:3000/x",
            "about:blank",
            "HTTPS://A.B",
        ] {
            assert!(is_web_url(url), "{url}");
        }
        for url in [
            "file:///etc/passwd",
            "circle://ui/settings.html",
            "chrome://version",
            "data:text/html,x",
            "javascript:1",
            "view-source:https://a.b",
        ] {
            assert!(!is_web_url(url), "{url}");
        }
    }

    #[test]
    fn local_urls() {
        for url in [
            "http://localhost:3000/",
            "https://app.localhost/",
            "http://127.0.0.1:8080/x",
            "http://127.4.5.6/",
            "http://[::1]:5173/",
            "http://0.0.0.0:8000/",
        ] {
            assert!(is_local_url(url), "{url}");
        }
        for url in [
            "https://example.com/",
            "https://localhost.example.com/",
            "http://128.0.0.1/",
            "file:///tmp/a.html",
            "circle://ui/history.html",
            "not a url",
        ] {
            assert!(!is_local_url(url), "{url}");
        }
    }

    #[test]
    fn hsl_matches_css() {
        assert_eq!(hsl(0.0, 1.0, 0.5), 0xFFFF0000);
        assert_eq!(hsl(120.0, 1.0, 0.5), 0xFF00FF00);
        assert_eq!(hsl(240.0, 1.0, 0.5), 0xFF0000FF);
        assert_eq!(hsl(0.0, 0.0, 1.0), 0xFFFFFFFF);
        // hsl(262 50% 82%) in a browser is rgb(203, 186, 232).
        assert_eq!(hsl(262.0, 0.5, 0.82), 0xFFCBBAE8);
    }

    #[test]
    fn ui_commands_parse() {
        let cmd: UiCmd =
            serde_json::from_str(r#"{"cmd":"navigate","input":"rust","new_tab":true}"#).unwrap();
        assert!(matches!(cmd, UiCmd::Navigate { new_tab: true, .. }));
        let cmd: UiCmd =
            serde_json::from_str(r#"{"cmd":"action","action":{"open_palette":"new_tab"}}"#)
                .unwrap();
        assert!(matches!(
            cmd,
            UiCmd::Action {
                action: Action::OpenPalette(PaletteMode::NewTab)
            }
        ));
        let cmd: UiCmd =
            serde_json::from_str(r#"{"cmd":"action","action":"toggle_sidebar"}"#).unwrap();
        assert!(matches!(
            cmd,
            UiCmd::Action {
                action: Action::ToggleSidebar
            }
        ));
        let cmd: UiCmd =
            serde_json::from_str(r#"{"cmd":"move","id":3,"section":"pinned","index":0}"#).unwrap();
        assert!(matches!(
            cmd,
            UiCmd::Move {
                section: Section::Pinned,
                ..
            }
        ));
    }
}
