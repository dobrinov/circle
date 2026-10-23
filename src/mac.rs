//! macOS integration: the `NSApplication` subclass CEF requires, the menu bar
//! (which is what makes Cmd+C/V/X/Z work in web content), the frameless
//! window's title-bar behavior (double-clicks, traffic lights that hide with the
//! sidebar), rounded page corners, and handling of links opened from other apps
//! when Circle is the default browser.

use crate::{
    controller::{self, Action, Cmd, PaletteMode},
    model::{Appearance, DevToolsDock},
    recorder::Format,
};
use cef::application_mac::{CefAppProtocol, CrAppControlProtocol, CrAppProtocol};
use objc2::{
    ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, extern_methods,
    msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel,
};
use objc2_app_kit::{
    NSApp, NSApplication, NSApplicationDelegate, NSApplicationTerminateReply, NSCursor,
    NSCursorFrameResizeDirections, NSCursorFrameResizePosition, NSEvent, NSEventMask,
    NSEventModifierFlags, NSEventTrackingRunLoopMode, NSEventType, NSMenu, NSMenuItem, NSView,
    NSWindow, NSWindowButton, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{NSArray, NSDate, NSString, NSURL, NSUserDefaults, ns_string};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction, kCAFillRuleEvenOdd};
use std::cell::{Cell, RefCell};

/// The frameless main window and the areas of it that act as a title bar
/// (the draggable regions, in top-left-origin window points).
struct Titlebar {
    window: Option<Retained<NSWindow>>,
    regions: Vec<cef::DraggableRegion>,
    /// The traffic lights live in the sidebar, so they hide with it.
    lights_hidden: bool,
}

thread_local! {
    static TITLEBAR: RefCell<Titlebar> = const {
        RefCell::new(Titlebar {
            window: None,
            regions: Vec::new(),
            lights_hidden: false,
        })
    };
}

const WINDOW_BUTTONS: [NSWindowButton; 3] = [
    NSWindowButton::CloseButton,
    NSWindowButton::MiniaturizeButton,
    NSWindowButton::ZoomButton,
];

fn ns_view(handle: *mut std::ffi::c_void) -> Option<&'static NSView> {
    // CEF window handles on macOS are NSView pointers owned by Chromium.
    unsafe { handle.cast::<NSView>().as_ref() }
}

pub fn set_titlebar_regions(window: &cef::Window, regions: Vec<cef::DraggableRegion>) {
    use cef::ImplWindow;
    let ns_window = ns_view(window.window_handle().cast()).and_then(|v| v.window());
    TITLEBAR.with(|t| {
        let mut t = t.borrow_mut();
        t.window = ns_window;
        t.regions = regions;
    });
}

fn set_lights_hidden(t: &mut Titlebar, hidden: bool) {
    if t.lights_hidden == hidden {
        return;
    }
    t.lights_hidden = hidden;
    if let Some(window) = &t.window {
        for kind in WINDOW_BUTTONS {
            if let Some(button) = window.standardWindowButton(kind) {
                button.setHidden(hidden);
            }
        }
    }
}

/// Forces the app's native appearance (menus, traffic lights) light or dark,
/// or lets it follow the system with `None`.
pub fn set_appearance(dark: Option<bool>) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(class) = AnyClass::get(c"NSAppearance") else {
        return;
    };
    let app = NSApp(mtm);
    unsafe {
        let appearance: Option<Retained<AnyObject>> = match dark {
            Some(true) => msg_send![class, appearanceNamed: ns_string!("NSAppearanceNameDarkAqua")],
            Some(false) => msg_send![class, appearanceNamed: ns_string!("NSAppearanceNameAqua")],
            None => None,
        };
        let _: () = msg_send![&*app, setAppearance: appearance.as_deref()];
    }
}

thread_local! {
    /// The last key event a browser already turned into an action, the
    /// action, and when.
    static HANDLED_KEY: RefCell<Option<(usize, Action, std::time::Instant)>> = const { RefCell::new(None) };
}

/// Records that a browser handled this key event (an `NSEvent*`), so the
/// menu item with the same shortcut doesn't run the action again.
pub fn note_handled_key_event(event: *mut u8, action: Action) {
    HANDLED_KEY
        .with(|h| *h.borrow_mut() = Some((event as usize, action, std::time::Instant::now())));
}

/// Runs a menu item's action. Menu shortcuts are what make keys work when no
/// web view has keyboard focus; when one does, it has already acted on them.
fn menu_action(action: Action) {
    let handled = HANDLED_KEY.with(|h| *h.borrow());
    if let (Some((handled_event, handled_action, at)), Some(mtm)) =
        (handled, MainThreadMarker::new())
    {
        let same_event = NSApp(mtm).currentEvent().is_some_and(|event| {
            event.r#type() == NSEventType::KeyDown
                && std::ptr::eq(
                    &*event as *const NSEvent as *const u8,
                    handled_event as *const u8,
                )
        });
        // Belt and braces: the same action from a key a moment ago.
        let just_ran = handled_action == action && at.elapsed().as_millis() < 150;
        if same_event || just_ran {
            return;
        }
    }
    controller::dispatch(Cmd::Action(action));
}

/// Keeps the window out of screen recordings, screen sharing and
/// screenshots (while a private space shows).
pub fn set_capture_hidden(window: &cef::Window, hidden: bool) {
    use cef::ImplWindow;
    if let Some(ns_window) = ns_view(window.window_handle().cast()).and_then(|v| v.window()) {
        ns_window.setSharingType(if hidden {
            NSWindowSharingType::None
        } else {
            NSWindowSharingType::ReadOnly
        });
    }
}

pub fn set_traffic_lights_hidden(hidden: bool) {
    TITLEBAR.with(|t| set_lights_hidden(&mut t.borrow_mut(), hidden));
}

/// CACornerMask bits for the left-hand corners, and for all four.
pub const LEFT_CORNERS: usize = 1 | 4;
pub const ALL_CORNERS: usize = 15;

fn round_view(view: &NSView, radius: f64, corners: usize) {
    view.setWantsLayer(true);
    unsafe {
        let layer: Option<Retained<AnyObject>> = msg_send![view, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&*layer, setCornerRadius: radius];
            let _: () = msg_send![&*layer, setMaskedCorners: corners];
            let _: () = msg_send![&*layer, setMasksToBounds: Bool::YES];
            let _: () = msg_send![&*layer, setCornerCurve: ns_string!("continuous")];
        }
    }
}

/// Rounds the given corners of the window hosting `browser`. Used for the
/// command bar and the peek sidebar, which live in their own overlay windows.
pub fn round_browser_window(browser: &cef::Browser, radius: f64, corners: usize) {
    use cef::{ImplBrowser, ImplBrowserHost};
    // For browsers hosted in CEF Views this is the hosting window's content view.
    if let Some(view) = browser
        .host()
        .and_then(|h| ns_view(h.window_handle().cast()))
    {
        round_view(view, radius, corners);
    }
}

/// The frame-colored ring around one page, which rounds its corners, and the
/// stripes drawn over the ring when the page comes from a dev server.
struct PageRing {
    ring: Retained<CALayer>,
    stripes: Retained<CAShapeLayer>,
    stripes_mask: Retained<CAShapeLayer>,
}

thread_local! {
    static CORNER_RINGS: RefCell<Vec<PageRing>> = const { RefCell::new(Vec::new()) };
}

fn cg_color(argb: u32) -> CFRetained<CGColor> {
    let channel = |shift: u32| ((argb >> shift) & 0xFF) as f64 / 255.0;
    CGColor::new_srgb(channel(16), channel(8), channel(0), channel(24))
}

/// Diagonal stripes covering a `width` × `height` box.
fn stripes_path(width: f64, height: f64) -> CFRetained<CGMutablePath> {
    const STRIPE: f64 = 6.0;
    let path = CGMutablePath::new();
    let mut x = -height;
    while x < width {
        unsafe {
            CGMutablePath::move_to_point(Some(&path), std::ptr::null(), x, 0.0);
            CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), x + STRIPE, 0.0);
            CGMutablePath::add_line_to_point(
                Some(&path),
                std::ptr::null(),
                x + STRIPE + height,
                height,
            );
            CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), x + height, height);
        }
        CGMutablePath::close_subpath(Some(&path));
        x += 2.0 * STRIPE;
    }
    path
}

/// The band between a page and the ring around it: the ring's outer rounded
/// rectangle minus the page's (filled even-odd).
fn band_path(width: f64, height: f64, gap: f64, radius: f64) -> CFRetained<CGMutablePath> {
    let path = CGMutablePath::new();
    let outer = radius + gap;
    let inner = CGRect::new(
        CGPoint::new(gap, gap),
        CGSize::new(width - 2.0 * gap, height - 2.0 * gap),
    );
    unsafe {
        CGMutablePath::add_rounded_rect(
            Some(&path),
            std::ptr::null(),
            CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, height)),
            outer,
            outer,
        );
        CGMutablePath::add_rounded_rect(Some(&path), std::ptr::null(), inner, radius, radius);
    }
    path
}

/// Gives the visible pages rounded corners, and pages flagged `true` (from a
/// local dev server) a striped frame.
pub fn set_page_corners(
    window: &cef::Window,
    pages: &[(cef::Rect, bool)],
    gap: f64,
    radius: f64,
    argb: u32,
    stripes_argb: u32,
) {
    use cef::ImplWindow;
    let Some(content) = ns_view(window.window_handle().cast()) else {
        return;
    };
    let Some(root) = content.layer() else {
        return;
    };
    let height = content.frame().size.height;
    let flipped = root.isGeometryFlipped();
    let color = cg_color(argb | 0xFF00_0000);
    let stripes_color = cg_color(stripes_argb | 0xFF00_0000);
    let show = gap > 0.0 && radius > 0.0;

    CATransaction::begin();
    CATransaction::setDisableActions(true);
    CORNER_RINGS.with(|rings| {
        let mut rings = rings.borrow_mut();
        let wanted = if show { pages.len() } else { 0 };
        while rings.len() > wanted {
            let gone = rings.pop().unwrap();
            gone.ring.removeFromSuperlayer();
            gone.stripes.removeFromSuperlayer();
        }
        while rings.len() < wanted {
            let ring = CALayer::new();
            // Above the compositor's layer, which holds all web content.
            ring.setZPosition(1000.0);
            let stripes = CAShapeLayer::new();
            let stripes_mask = CAShapeLayer::new();
            stripes_mask.setFillRule(unsafe { kCAFillRuleEvenOdd });
            unsafe { stripes.setMask(Some(&stripes_mask)) };
            // A sibling above the ring: a layer's border covers its sublayers.
            stripes.setZPosition(1001.0);
            root.addSublayer(&ring);
            root.addSublayer(&stripes);
            rings.push(PageRing {
                ring,
                stripes,
                stripes_mask,
            });
        }
        for (page_ring, (page, local)) in rings.iter().zip(pages) {
            let PageRing {
                ring,
                stripes,
                stripes_mask,
            } = page_ring;
            let (w, h) = (
                page.width as f64 + 2.0 * gap,
                page.height as f64 + 2.0 * gap,
            );
            let top = page.y as f64 - gap;
            let y = if flipped { top } else { height - top - h };
            let frame = CGRect::new(CGPoint::new(page.x as f64 - gap, y), CGSize::new(w, h));
            ring.setFrame(frame);
            ring.setBorderWidth(gap);
            ring.setCornerRadius(radius + gap);
            ring.setBorderColor(Some(&color));

            stripes.setHidden(!local);
            if *local {
                let bounds = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w, h));
                stripes.setFrame(frame);
                stripes.setPath(Some(&stripes_path(w, h)));
                stripes.setFillColor(Some(&stripes_color));
                stripes.setOpacity(0.85);
                stripes_mask.setFrame(bounds);
                stripes_mask.setPath(Some(&band_path(w, h, gap, radius)));
            }
        }
    });
    CATransaction::commit();
}

/// How close to an edge (points) the pointer must be to resize the window,
/// and how far along an edge a corner reaches.
const RESIZE_EDGE: f64 = 5.0;
const RESIZE_CORNER: f64 = 14.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

/// What a point in the main window's chrome does.
enum ChromeHit {
    Resize(Edges),
    Drag,
}

thread_local! {
    /// Whether we last set a resize cursor, so it can be reset when leaving the edge.
    static RESIZE_CURSOR_SHOWN: Cell<bool> = const { Cell::new(false) };
}

/// Which window edges a point (window coordinates, bottom-left origin) is on.
fn resize_edges(window: &NSWindow, x: f64, y: f64) -> Option<Edges> {
    let size = window.frame().size;
    let near = |d: f64| (0.0..=RESIZE_EDGE).contains(&d);
    let corner = |d: f64| (0.0..=RESIZE_CORNER).contains(&d);
    let (from_left, from_right) = (x, size.width - x);
    let (from_bottom, from_top) = (y, size.height - y);
    let horizontal = near(from_left) || near(from_right);
    let vertical = near(from_bottom) || near(from_top);
    if !horizontal && !vertical {
        return None;
    }
    // Near a corner, either edge grabs both directions.
    Some(Edges {
        left: near(from_left) || (vertical && corner(from_left)),
        right: near(from_right) || (vertical && corner(from_right)),
        top: near(from_top) || (horizontal && corner(from_top)),
        bottom: near(from_bottom) || (horizontal && corner(from_bottom)),
    })
}

/// The main window, if `event` belongs to it and it is not full screen.
fn main_window_for(event: &NSEvent) -> Option<Retained<NSWindow>> {
    let mtm = MainThreadMarker::new()?;
    let window = event.window(mtm)?;
    let is_main = TITLEBAR.with(|t| {
        t.borrow()
            .window
            .as_ref()
            .is_some_and(|w| std::ptr::eq(&**w, &*window))
    });
    let fullscreen = window.styleMask().contains(NSWindowStyleMask::FullScreen);
    (is_main && !fullscreen).then_some(window)
}

/// Classifies a point in the main window: an edge to resize, a draggable
/// area, or neither. The traffic-light buttons are left alone.
fn chrome_hit(window: &NSWindow, event: &NSEvent) -> Option<ChromeHit> {
    let point = event.locationInWindow();
    for kind in WINDOW_BUTTONS {
        if let Some(button) = window.standardWindowButton(kind) {
            if button.isHidden() {
                continue;
            }
            let frame = button.convertRect_toView(button.bounds(), None);
            let inside = point.x >= frame.origin.x
                && point.x <= frame.origin.x + frame.size.width
                && point.y >= frame.origin.y
                && point.y <= frame.origin.y + frame.size.height;
            if inside {
                return None;
            }
        }
    }
    if let Some(edges) = resize_edges(window, point.x, point.y) {
        return Some(ChromeHit::Resize(edges));
    }
    let height = window.contentView()?.frame().size.height;
    let draggable =
        TITLEBAR.with(|t| in_drag_region(&t.borrow().regions, point.x, height - point.y));
    draggable.then_some(ChromeHit::Drag)
}

/// Moves and resizes the frameless main window ourselves rather than relying
/// on Chromium's hit-testing: a press in a draggable area starts a native
/// window drag (double-click zooms or minimizes, per System Settings), and a
/// press on an edge resizes. Returns true if the event was consumed.
fn handle_window_chrome(event: &NSEvent) -> bool {
    if event.r#type() != NSEventType::LeftMouseDown {
        return false;
    }
    let Some(window) = main_window_for(event) else {
        return false;
    };
    match chrome_hit(&window, event) {
        Some(ChromeHit::Resize(edges)) => {
            track_resize(&window, edges);
            true
        }
        Some(ChromeHit::Drag) if event.clickCount() == 2 => {
            perform_titlebar_action(&window);
            true
        }
        Some(ChromeHit::Drag) => {
            window.performWindowDragWithEvent(event);
            true
        }
        None => false,
    }
}

/// Follows the pointer until the button is released, resizing from `edges`.
fn track_resize(window: &NSWindow, edges: Edges) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApp(mtm);
    let start = NSEvent::mouseLocation();
    let initial = window.frame();
    let min = window.minSize();
    let until = NSDate::distantFuture();
    let mask = NSEventMask::LeftMouseDragged | NSEventMask::LeftMouseUp;
    loop {
        let Some(next) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            mask,
            Some(&until),
            unsafe { NSEventTrackingRunLoopMode },
            true,
        ) else {
            break;
        };
        let now = NSEvent::mouseLocation();
        let (dx, dy) = (now.x - start.x, now.y - start.y);
        let mut frame = initial;
        // Screen coordinates have a bottom-left origin.
        if edges.right {
            frame.size.width = (initial.size.width + dx).max(min.width);
        }
        if edges.left {
            frame.size.width = (initial.size.width - dx).max(min.width);
            frame.origin.x = initial.origin.x + initial.size.width - frame.size.width;
        }
        if edges.top {
            frame.size.height = (initial.size.height + dy).max(min.height);
        }
        if edges.bottom {
            frame.size.height = (initial.size.height - dy).max(min.height);
            frame.origin.y = initial.origin.y + initial.size.height - frame.size.height;
        }
        window.setFrame_display(frame, true);
        if next.r#type() == NSEventType::LeftMouseUp {
            break;
        }
    }
}

// The classic cursors are deprecated but are all macOS 12–14 offer.
#[allow(deprecated)]
fn resize_cursor(edges: Edges) -> Retained<NSCursor> {
    use NSCursorFrameResizePosition as P;
    let position = match (edges.top, edges.bottom, edges.left, edges.right) {
        (true, _, true, _) => P::TopLeft,
        (true, _, _, true) => P::TopRight,
        (_, true, true, _) => P::BottomLeft,
        (_, true, _, true) => P::BottomRight,
        (true, ..) => P::Top,
        (_, true, ..) => P::Bottom,
        (_, _, true, _) => P::Left,
        _ => P::Right,
    };
    if objc2::available!(macos = 15.0) {
        NSCursor::frameResizeCursorFromPosition_inDirections(
            position,
            NSCursorFrameResizeDirections::All,
        )
    } else if edges.left || edges.right {
        NSCursor::resizeLeftRightCursor()
    } else {
        NSCursor::resizeUpDownCursor()
    }
}

/// Shows a resize cursor over the window's edges. Runs after the event was
/// dispatched so it wins over the cursor the web page asked for.
fn update_resize_cursor(event: &NSEvent) {
    if event.r#type() != NSEventType::MouseMoved {
        return;
    }
    let edges = main_window_for(event).and_then(|window| {
        let point = event.locationInWindow();
        resize_edges(&window, point.x, point.y)
    });
    match edges {
        Some(edges) => {
            resize_cursor(edges).set();
            RESIZE_CURSOR_SHOWN.set(true);
        }
        None if RESIZE_CURSOR_SHOWN.replace(false) => NSCursor::arrowCursor().set(),
        None => {}
    }
}

/// Whether a point (top-left-origin window coordinates) is draggable. Later
/// regions override earlier ones, matching Chromium.
fn in_drag_region(regions: &[cef::DraggableRegion], x: f64, y: f64) -> bool {
    let mut draggable = false;
    for region in regions {
        let b = &region.bounds;
        if x >= b.x as f64
            && x < (b.x + b.width) as f64
            && y >= b.y as f64
            && y < (b.y + b.height) as f64
        {
            draggable = region.draggable != 0;
        }
    }
    draggable
}

fn perform_titlebar_action(window: &NSWindow) {
    let defaults = NSUserDefaults::standardUserDefaults();
    let action = defaults
        .stringForKey(ns_string!("AppleActionOnDoubleClick"))
        .map(|s| s.to_string());
    let minimize = match action.as_deref() {
        Some("None") => return,
        Some("Minimize") => true,
        Some(_) => false,
        // Older systems only stored this boolean.
        None => defaults.boolForKey(ns_string!("AppleMiniaturizeOnDoubleClick")),
    };
    if minimize {
        window.miniaturize(None);
    } else {
        window.zoom(None);
    }
}

#[derive(Default)]
pub struct CircleApplicationIvars {
    handling_send_event: Cell<Bool>,
}

define_class!(
    /// `NSApplication` subclass implementing the protocols CEF expects.
    #[unsafe(super(NSApplication))]
    #[ivars = CircleApplicationIvars]
    pub struct CircleApplication;

    impl CircleApplication {
        #[unsafe(method(sendEvent:))]
        unsafe fn send_event(&self, event: &NSEvent) {
            if handle_window_chrome(event) {
                return;
            }
            let was_sending = self.ivars().handling_send_event.replace(Bool::YES);
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
            self.ivars().handling_send_event.set(was_sending);
            update_resize_cursor(event);
        }

        /// Chromium must leave the run loop to shut down cleanly, so quitting
        /// closes the window instead of calling `exit()`.
        #[unsafe(method(terminate:))]
        unsafe fn terminate(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Quit);
        }
    }

    unsafe impl CrAppControlProtocol for CircleApplication {
        #[unsafe(method(setHandlingSendEvent:))]
        unsafe fn _set_handling_send_event(&self, handling_send_event: Bool) {
            self.ivars().handling_send_event.set(handling_send_event);
        }
    }

    unsafe impl CrAppProtocol for CircleApplication {
        #[unsafe(method(isHandlingSendEvent))]
        unsafe fn _is_handling_send_event(&self) -> Bool {
            self.ivars().handling_send_event.get()
        }
    }

    unsafe impl CefAppProtocol for CircleApplication {}
);

impl CircleApplication {
    extern_methods! {
        #[unsafe(method(sharedApplication))]
        fn shared_application() -> Retained<Self>;
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    pub struct CircleAppDelegate;

    unsafe impl NSObjectProtocol for CircleAppDelegate {}

    /// Menu bar actions; they reach the delegate through the responder chain.
    impl CircleAppDelegate {
        #[unsafe(method(circleToggleSidebar:))]
        fn toggle_sidebar(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ToggleSidebar);
        }

        #[unsafe(method(circleOpenLocation:))]
        fn open_location(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::OpenPalette(PaletteMode::Edit));
        }

        #[unsafe(method(circlePrivateSpace:))]
        fn private_space(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::PrivateSpace);
        }

        #[unsafe(method(circleEndPrivate:))]
        fn end_private(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::EndPrivate);
        }

        #[unsafe(method(circleCloseTab:))]
        fn close_tab(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::CloseTab);
        }

        #[unsafe(method(circleReopenClosed:))]
        fn reopen_closed(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ReopenClosed);
        }

        #[unsafe(method(circleCommandBar:))]
        fn command_bar(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::OpenPalette(PaletteMode::NewTab));
        }

        #[unsafe(method(circleSpotlight:))]
        fn spotlight(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::OpenPalette(PaletteMode::Spotlight));
        }

        #[unsafe(method(circleFind:))]
        fn find(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::OpenPalette(PaletteMode::Find));
        }

        #[unsafe(method(circleReload:))]
        fn reload(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Reload);
        }

        #[unsafe(method(circleZoomIn:))]
        fn zoom_in(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Zoom(1));
        }

        #[unsafe(method(circleZoomOut:))]
        fn zoom_out(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Zoom(-1));
        }

        #[unsafe(method(circleActualSize:))]
        fn actual_size(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Zoom(0));
        }

        #[unsafe(method(circleDevTools:))]
        fn dev_tools(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::DevTools);
        }

        #[unsafe(method(circleDockRight:))]
        fn dock_right(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::DockDevTools(DevToolsDock::Right));
        }

        #[unsafe(method(circleDockBottom:))]
        fn dock_bottom(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::DockDevTools(DevToolsDock::Bottom));
        }

        #[unsafe(method(circleDockWindow:))]
        fn dock_window(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::DockDevTools(DevToolsDock::Window));
        }

        #[unsafe(method(circleAppearanceSystem:))]
        fn appearance_system(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Appearance(Appearance::System));
        }

        #[unsafe(method(circleAppearanceLight:))]
        fn appearance_light(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Appearance(Appearance::Light));
        }

        #[unsafe(method(circleAppearanceDark:))]
        fn appearance_dark(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Appearance(Appearance::Dark));
        }

        #[unsafe(method(circleShowSettings:))]
        fn show_settings(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ShowSettings);
        }

        #[unsafe(method(circleRecordVideo:))]
        fn record_video(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Record(Format::Video));
        }

        #[unsafe(method(circleRecordGif:))]
        fn record_gif(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::Record(Format::Gif));
        }

        #[unsafe(method(circleShowWhatsNew:))]
        fn show_whats_new(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ShowWhatsNew);
        }

        #[unsafe(method(circleReportBug:))]
        fn report_bug(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ReportBug);
        }

        #[unsafe(method(circleShowHistory:))]
        fn show_history(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ShowHistory);
        }

        #[unsafe(method(circleClearBrowsingData:))]
        fn clear_browsing_data(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ClearBrowsingData);
        }

        #[unsafe(method(circleViewSource:))]
        fn view_source(&self, _sender: Option<&AnyObject>) {
            menu_action(Action::ViewSource);
        }
    }

    unsafe impl NSApplicationDelegate for CircleAppDelegate {
        #[unsafe(method(applicationShouldTerminate:))]
        unsafe fn application_should_terminate(
            &self,
            _sender: &NSApplication,
        ) -> NSApplicationTerminateReply {
            NSApplicationTerminateReply::TerminateNow
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        unsafe fn application_should_handle_reopen(
            &self,
            _sender: &NSApplication,
            _visible: Bool,
        ) -> Bool {
            controller::dispatch(Cmd::OpenUrls(Vec::new()));
            Bool::NO
        }

        #[unsafe(method(application:openURLs:))]
        unsafe fn application_open_urls(
            &self,
            _application: &NSApplication,
            urls: &NSArray<NSURL>,
        ) {
            let urls = urls
                .iter()
                .filter_map(|url| url.absoluteString())
                .map(|s| s.to_string())
                .collect();
            controller::dispatch(Cmd::OpenUrls(urls));
        }

        #[unsafe(method(applicationSupportsSecureRestorableState:))]
        unsafe fn application_supports_secure_restorable_state(
            &self,
            _sender: &NSApplication,
        ) -> Bool {
            Bool::YES
        }
    }
);

impl CircleAppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Must run before anything touches `NSApp`, so that the shared application
/// is a [`CircleApplication`].
pub fn init_application() {
    let _ = CircleApplication::shared_application();
    let mtm = MainThreadMarker::new().expect("not on the main thread");
    assert!(NSApp(mtm).isKindOfClass(CircleApplication::class()));
}

pub fn install_app_delegate() -> Retained<CircleAppDelegate> {
    let mtm = MainThreadMarker::new().expect("not on the main thread");
    let delegate = CircleAppDelegate::new(mtm);
    let app = NSApp(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    install_menu(mtm, &app);
    delegate
}

fn item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
    modifiers: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if !key.is_empty() {
        item.setKeyEquivalentModifierMask(modifiers);
    }
    item
}

fn submenu(
    mtm: MainThreadMarker,
    title: &str,
    items: Vec<Retained<NSMenuItem>>,
) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for entry in &items {
        menu.addItem(entry);
    }
    let parent = item(mtm, title, None, "", NSEventModifierFlags::empty());
    parent.setSubmenu(Some(&menu));
    parent
}

fn install_menu(mtm: MainThreadMarker, app: &NSApplication) {
    let cmd = NSEventModifierFlags::Command;
    let cmd_alt = NSEventModifierFlags::Command | NSEventModifierFlags::Option;
    let cmd_shift = NSEventModifierFlags::Command | NSEventModifierFlags::Shift;
    let cmd_alt_shift = cmd_alt | NSEventModifierFlags::Shift;
    let none = NSEventModifierFlags::empty();
    let separator = || NSMenuItem::separatorItem(mtm);

    let app_menu = submenu(
        mtm,
        "Circle",
        vec![
            item(
                mtm,
                "About Circle",
                Some(sel!(orderFrontStandardAboutPanel:)),
                "",
                none,
            ),
            separator(),
            item(mtm, "Settings…", Some(sel!(circleShowSettings:)), ",", cmd),
            separator(),
            item(mtm, "Hide Circle", Some(sel!(hide:)), "h", cmd),
            item(
                mtm,
                "Hide Others",
                Some(sel!(hideOtherApplications:)),
                "h",
                cmd_alt,
            ),
            item(
                mtm,
                "Show All",
                Some(sel!(unhideAllApplications:)),
                "",
                none,
            ),
            separator(),
            item(mtm, "Quit Circle", Some(sel!(terminate:)), "q", cmd),
        ],
    );
    let edit_menu = submenu(
        mtm,
        "Edit",
        vec![
            item(mtm, "Undo", Some(sel!(undo:)), "z", cmd),
            item(mtm, "Redo", Some(sel!(redo:)), "z", cmd_shift),
            separator(),
            item(mtm, "Cut", Some(sel!(cut:)), "x", cmd),
            item(mtm, "Copy", Some(sel!(copy:)), "c", cmd),
            item(mtm, "Paste", Some(sel!(paste:)), "v", cmd),
            item(
                mtm,
                "Paste and Match Style",
                Some(sel!(pasteAndMatchStyle:)),
                "v",
                cmd_alt_shift,
            ),
            item(mtm, "Delete", Some(sel!(delete:)), "", none),
            item(mtm, "Select All", Some(sel!(selectAll:)), "a", cmd),
        ],
    );
    let developer_menu = submenu(
        mtm,
        "Developer",
        vec![
            item(
                mtm,
                "Developer Tools",
                Some(sel!(circleDevTools:)),
                "i",
                cmd_alt,
            ),
            item(
                mtm,
                "View Source",
                Some(sel!(circleViewSource:)),
                "u",
                cmd_alt,
            ),
            separator(),
            item(mtm, "Dock to Right", Some(sel!(circleDockRight:)), "", none),
            item(
                mtm,
                "Dock to Bottom",
                Some(sel!(circleDockBottom:)),
                "",
                none,
            ),
            item(
                mtm,
                "Open in Separate Window",
                Some(sel!(circleDockWindow:)),
                "",
                none,
            ),
        ],
    );
    let file_menu = submenu(
        mtm,
        "File",
        vec![
            item(mtm, "New Tab", Some(sel!(circleCommandBar:)), "t", cmd),
            item(
                mtm,
                "Open Location…",
                Some(sel!(circleOpenLocation:)),
                "l",
                cmd,
            ),
            item(
                mtm,
                "Fap Mode",
                Some(sel!(circlePrivateSpace:)),
                "n",
                cmd | NSEventModifierFlags::Shift,
            ),
            item(
                mtm,
                "End Fap Mode",
                Some(sel!(circleEndPrivate:)),
                ".",
                cmd | NSEventModifierFlags::Shift,
            ),
            separator(),
            item(mtm, "Close Tab", Some(sel!(circleCloseTab:)), "w", cmd),
            item(
                mtm,
                "Reopen Closed Tab",
                Some(sel!(circleReopenClosed:)),
                "t",
                cmd | NSEventModifierFlags::Shift,
            ),
        ],
    );
    let view_menu = submenu(
        mtm,
        "View",
        vec![
            item(mtm, "Spotlight", Some(sel!(circleSpotlight:)), "k", cmd),
            item(
                mtm,
                "Toggle Sidebar",
                Some(sel!(circleToggleSidebar:)),
                "s",
                cmd,
            ),
            item(mtm, "Find in Page", Some(sel!(circleFind:)), "f", cmd),
            separator(),
            submenu(
                mtm,
                "Appearance",
                vec![
                    item(
                        mtm,
                        "Match System",
                        Some(sel!(circleAppearanceSystem:)),
                        "",
                        none,
                    ),
                    item(mtm, "Light", Some(sel!(circleAppearanceLight:)), "", none),
                    item(mtm, "Dark", Some(sel!(circleAppearanceDark:)), "", none),
                ],
            ),
            separator(),
            item(
                mtm,
                "Record Video…",
                Some(sel!(circleRecordVideo:)),
                "r",
                cmd_alt,
            ),
            item(
                mtm,
                "Record GIF…",
                Some(sel!(circleRecordGif:)),
                "r",
                cmd_alt | NSEventModifierFlags::Shift,
            ),
            separator(),
            item(mtm, "History", Some(sel!(circleShowHistory:)), "y", cmd),
            item(
                mtm,
                "Clear Browsing Data…",
                Some(sel!(circleClearBrowsingData:)),
                "\u{8}",
                cmd | NSEventModifierFlags::Shift,
            ),
            separator(),
            item(mtm, "Reload Page", Some(sel!(circleReload:)), "r", cmd),
            separator(),
            item(mtm, "Actual Size", Some(sel!(circleActualSize:)), "0", cmd),
            item(mtm, "Zoom In", Some(sel!(circleZoomIn:)), "=", cmd),
            item(mtm, "Zoom Out", Some(sel!(circleZoomOut:)), "-", cmd),
            separator(),
            developer_menu,
        ],
    );
    let help_menu = submenu(
        mtm,
        "Help",
        vec![
            item(
                mtm,
                "What's New in Circle",
                Some(sel!(circleShowWhatsNew:)),
                "",
                none,
            ),
            item(mtm, "Report a Bug…", Some(sel!(circleReportBug:)), "", none),
        ],
    );
    let window_menu = submenu(
        mtm,
        "Window",
        vec![
            item(mtm, "Minimize", Some(sel!(performMiniaturize:)), "m", cmd),
            item(mtm, "Zoom", Some(sel!(performZoom:)), "", none),
            separator(),
            item(
                mtm,
                "Bring All to Front",
                Some(sel!(arrangeInFront:)),
                "",
                none,
            ),
        ],
    );

    let main_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));
    main_menu.addItem(&app_menu);
    main_menu.addItem(&file_menu);
    main_menu.addItem(&edit_menu);
    main_menu.addItem(&view_menu);
    main_menu.addItem(&window_menu);
    main_menu.addItem(&help_menu);
    app.setMainMenu(Some(&main_menu));
    app.setWindowsMenu(window_menu.submenu().as_deref());
}

#[cfg(test)]
mod tests {
    use super::in_drag_region;
    use cef::{DraggableRegion, Rect};

    fn region(x: i32, y: i32, width: i32, height: i32, draggable: bool) -> DraggableRegion {
        DraggableRegion {
            bounds: Rect {
                x,
                y,
                width,
                height,
            },
            draggable: draggable as _,
        }
    }

    #[test]
    fn no_drag_regions_carve_out_buttons() {
        // A 252x46 title bar with a button at x=180..208.
        let regions = [region(0, 0, 252, 46, true), region(180, 9, 28, 28, false)];
        assert!(in_drag_region(&regions, 130.0, 22.0));
        assert!(!in_drag_region(&regions, 190.0, 22.0));
        assert!(!in_drag_region(&regions, 130.0, 60.0));
        assert!(!in_drag_region(&[], 10.0, 10.0));
    }
}
