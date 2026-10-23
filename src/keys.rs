//! Keyboard shortcuts. Every browser (tabs, sidebar, command bar) routes its
//! key events through [`action_for`] before the page sees them.

use crate::{
    controller::{Action, PaletteMode},
    recorder::Format,
};
use cef::*;

const SHIFT: u32 = 1 << 1;
const CTRL: u32 = 1 << 2;
const ALT: u32 = 1 << 3;
const CMD: u32 = 1 << 7;

/// Cmd on macOS, Ctrl elsewhere.
const PRIMARY: u32 = if cfg!(target_os = "macos") { CMD } else { CTRL };

const VK_BACK: i32 = 0x08;
const VK_TAB: i32 = 0x09;
const VK_LEFT: i32 = 0x25;
const VK_UP: i32 = 0x26;
const VK_RIGHT: i32 = 0x27;
const VK_DOWN: i32 = 0x28;
const VK_F5: i32 = 0x74;
const VK_F11: i32 = 0x7A;
const VK_F12: i32 = 0x7B;
const VK_ADD: i32 = 0x6B;
const VK_SUBTRACT: i32 = 0x6D;
const VK_OEM_PLUS: i32 = 0xBB;
const VK_OEM_COMMA: i32 = 0xBC;
const VK_OEM_MINUS: i32 = 0xBD;
const VK_OEM_PERIOD: i32 = 0xBE;
const VK_OEM_4: i32 = 0xDB; // [
const VK_OEM_6: i32 = 0xDD; // ]

fn key(c: char) -> i32 {
    c as i32
}

pub fn action_for(event: &KeyEvent) -> Option<Action> {
    if event.type_ != KeyEventType::RAWKEYDOWN {
        return None;
    }
    let mods = event.modifiers & (SHIFT | CTRL | ALT | CMD);
    let code = event.windows_key_code;
    let editable = event.focus_on_editable_field != 0;
    let mac = cfg!(target_os = "macos");

    let digit = (key('1')..=key('9'))
        .contains(&code)
        .then(|| (code - key('1')) as usize);

    let action = match (mods, code) {
        (PRIMARY, c) if c == key('T') || c == key('N') => Action::OpenPalette(PaletteMode::NewTab),
        (PRIMARY, c) if c == key('L') => Action::OpenPalette(PaletteMode::Edit),
        (PRIMARY, c) if c == key('K') => Action::OpenPalette(PaletteMode::Spotlight),
        (CMD, c) if mac && c == key('Y') => Action::ShowHistory,
        (PRIMARY, VK_OEM_COMMA) => Action::ShowSettings,
        (m, c) if m == PRIMARY | SHIFT && c == key('N') => Action::PrivateSpace,
        (m, VK_OEM_PERIOD) if m == PRIMARY | SHIFT => Action::EndPrivate,
        (m, c) if m == PRIMARY | ALT && c == key('R') => Action::Record(Format::Video),
        (m, c) if m == PRIMARY | ALT | SHIFT && c == key('R') => Action::Record(Format::Gif),
        (CTRL, c) if !mac && c == key('H') => Action::ShowHistory,
        (m, VK_BACK) if m == PRIMARY | SHIFT => Action::ClearBrowsingData,
        (PRIMARY, c) if c == key('F') => Action::OpenPalette(PaletteMode::Find),
        (PRIMARY, c) if c == key('W') => Action::CloseTab,
        (m, c) if m == PRIMARY | SHIFT && c == key('T') => Action::ReopenClosed,
        (PRIMARY, c) if c == key('S') => Action::ToggleSidebar,
        (PRIMARY, c) if c == key('D') => Action::TogglePin,
        (PRIMARY, c) if c == key('R') => Action::Reload,
        (0, VK_F5) => Action::Reload,
        (m, c) if m == PRIMARY | SHIFT && c == key('R') => Action::ReloadHard,
        (PRIMARY, VK_OEM_4) => Action::Back,
        (PRIMARY, VK_OEM_6) => Action::Forward,
        (CMD, VK_LEFT) if mac && !editable => Action::Back,
        (CMD, VK_RIGHT) if mac && !editable => Action::Forward,
        (ALT, VK_LEFT) if !mac && !editable => Action::Back,
        (ALT, VK_RIGHT) if !mac && !editable => Action::Forward,
        (CTRL, VK_TAB) => Action::NextTab,
        (m, VK_TAB) if m == CTRL | SHIFT => Action::PrevTab,
        (m, VK_DOWN) if m == PRIMARY | ALT => Action::NextTab,
        (m, VK_UP) if m == PRIMARY | ALT => Action::PrevTab,
        (m, VK_RIGHT) if m == PRIMARY | ALT => Action::NextSpace,
        (m, VK_LEFT) if m == PRIMARY | ALT => Action::PrevSpace,
        (CTRL, _) if mac && digit.is_some() => Action::Space(digit?),
        (ALT, _) if !mac && digit.is_some() => Action::Space(digit?),
        (PRIMARY, _) if digit.is_some() => Action::TabIndex(digit?),
        (m, VK_OEM_PLUS) if m == PRIMARY | SHIFT => Action::ToggleSplit,
        (m, VK_OEM_MINUS) if m == PRIMARY | SHIFT => Action::ToggleSplitDirection,
        (PRIMARY, VK_OEM_PLUS | VK_ADD) => Action::Zoom(1),
        (PRIMARY, VK_OEM_MINUS | VK_SUBTRACT) => Action::Zoom(-1),
        (PRIMARY, c) if c == key('0') => Action::Zoom(0),
        (m, c) if m == PRIMARY | ALT && c == key('I') => Action::DevTools,
        (m, c) if !mac && m == CTRL | SHIFT && c == key('I') => Action::DevTools,
        (0, VK_F12) => Action::DevTools,
        (m, c) if m == PRIMARY | ALT && c == key('U') => Action::ViewSource,
        (m, c) if m == PRIMARY | SHIFT && c == key('C') => Action::CopyUrl,
        (m, c) if mac && m == CMD | CTRL && c == key('F') => Action::Fullscreen,
        (0, VK_F11) if !mac => Action::Fullscreen,
        (CTRL, c) if !mac && c == key('Q') => Action::Quit,
        _ => return None,
    };
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(mods: u32, code: i32) -> Option<Action> {
        action_for(&KeyEvent {
            type_: KeyEventType::RAWKEYDOWN,
            modifiers: mods,
            windows_key_code: code,
            ..Default::default()
        })
    }

    #[test]
    fn maps_core_shortcuts() {
        assert_eq!(
            press(PRIMARY, key('T')),
            Some(Action::OpenPalette(PaletteMode::NewTab))
        );
        assert_eq!(press(PRIMARY | SHIFT, key('T')), Some(Action::ReopenClosed));
        assert_eq!(
            press(PRIMARY, key('K')),
            Some(Action::OpenPalette(PaletteMode::Spotlight))
        );
        assert_eq!(press(PRIMARY, key('3')), Some(Action::TabIndex(2)));
        assert_eq!(
            press(PRIMARY | SHIFT, VK_OEM_PLUS),
            Some(Action::ToggleSplit)
        );
        assert_eq!(
            press(PRIMARY | SHIFT, VK_OEM_MINUS),
            Some(Action::ToggleSplitDirection)
        );
        assert_eq!(press(PRIMARY, VK_OEM_MINUS), Some(Action::Zoom(-1)));
        assert_eq!(press(PRIMARY | SHIFT, key('N')), Some(Action::PrivateSpace));
        assert_eq!(
            press(PRIMARY, key('N')),
            Some(Action::OpenPalette(PaletteMode::NewTab))
        );
        assert_eq!(
            press(PRIMARY | SHIFT, VK_OEM_PERIOD),
            Some(Action::EndPrivate)
        );
        assert_eq!(press(0, key('T')), None);
    }

    #[test]
    fn ignores_key_up() {
        let event = KeyEvent {
            type_: KeyEventType::KEYUP,
            modifiers: PRIMARY,
            windows_key_code: key('T'),
            ..Default::default()
        };
        assert_eq!(action_for(&event), None);
    }
}
