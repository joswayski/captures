//! Native key input for the shortcut recorder: Windows keyboard messages to
//! the DOM `KeyboardEvent.code` the shared recorder policy expects
//! (`captures_app::shortcuts::record_shortcut`; "hosts translate native
//! physical keys into DOM codes"). Codes name physical positions, so they
//! follow the scan code, as Chromium's do, not the layout's virtual key.
use captures_app::shortcuts::ShortcutKeyEvent;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// The DOM code for a key message's scan code (bit 24 is the extended flag).
fn code(vk: VIRTUAL_KEY, lp: LPARAM) -> &'static str {
    // Media keys often arrive without a scan code.
    match vk {
        VK_VOLUME_MUTE => return "AudioVolumeMute",
        VK_VOLUME_DOWN => return "AudioVolumeDown",
        VK_VOLUME_UP => return "AudioVolumeUp",
        VK_MEDIA_NEXT_TRACK => return "MediaTrackNext",
        VK_MEDIA_PREV_TRACK => return "MediaTrackPrevious",
        VK_MEDIA_STOP => return "MediaStop",
        VK_MEDIA_PLAY_PAUSE => return "MediaPlayPause",
        _ => {}
    }
    let mut scan = ((lp.0 >> 16) & 0xff) as u32;
    let mut extended = (lp.0 >> 24) & 1 == 1;
    if scan == 0 {
        let mapped = unsafe { MapVirtualKeyW(u32::from(vk.0), MAPVK_VK_TO_VSC_EX) };
        scan = mapped & 0xff;
        extended = mapped & 0xff00 != 0;
    }
    match (scan, extended) {
        (0x01, _) => "Escape",
        (0x02..=0x0a, _) => [
            "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
            "Digit9",
        ][(scan - 0x02) as usize],
        (0x0b, _) => "Digit0",
        (0x0c, _) => "Minus",
        (0x0d, _) => "Equal",
        (0x0e, _) => "Backspace",
        (0x0f, _) => "Tab",
        (0x10, true) => "MediaTrackPrevious",
        (0x19, true) => "MediaTrackNext",
        (0x10..=0x19, false) => [
            "KeyQ", "KeyW", "KeyE", "KeyR", "KeyT", "KeyY", "KeyU", "KeyI", "KeyO", "KeyP",
        ][(scan - 0x10) as usize],
        (0x1a, _) => "BracketLeft",
        (0x1b, _) => "BracketRight",
        (0x1c, false) => "Enter",
        (0x1c, true) => "NumpadEnter",
        (0x1d, false) => "ControlLeft",
        (0x1d, true) => "ControlRight",
        (0x20, true) => "AudioVolumeMute",
        (0x22, true) => "MediaPlayPause",
        (0x24, true) => "MediaStop",
        (0x2e, true) => "AudioVolumeDown",
        (0x30, true) => "AudioVolumeUp",
        (0x1e..=0x26, false) => [
            "KeyA", "KeyS", "KeyD", "KeyF", "KeyG", "KeyH", "KeyJ", "KeyK", "KeyL",
        ][(scan - 0x1e) as usize],
        (0x27, _) => "Semicolon",
        (0x28, _) => "Quote",
        (0x29, _) => "Backquote",
        (0x2a, _) => "ShiftLeft",
        (0x2b, _) => "Backslash",
        (0x2c..=0x32, false) => {
            ["KeyZ", "KeyX", "KeyC", "KeyV", "KeyB", "KeyN", "KeyM"][(scan - 0x2c) as usize]
        }
        (0x33, _) => "Comma",
        (0x34, _) => "Period",
        (0x35, false) => "Slash",
        (0x35, true) => "NumpadDivide",
        (0x36, _) => "ShiftRight",
        (0x37, false) => "NumpadMultiply",
        // Print Screen, and SysRq when Alt is held.
        (0x37, true) | (0x54, _) => "PrintScreen",
        (0x38, false) => "AltLeft",
        (0x38, true) => "AltRight",
        (0x39, _) => "Space",
        (0x3a, _) => "CapsLock",
        (0x3b..=0x44, _) => {
            ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10"][(scan - 0x3b) as usize]
        }
        (0x45, false) => "Pause",
        (0x45, true) => "NumLock",
        (0x46, _) => "ScrollLock",
        (0x47, false) => "Numpad7",
        (0x47, true) => "Home",
        (0x48, false) => "Numpad8",
        (0x48, true) => "ArrowUp",
        (0x49, false) => "Numpad9",
        (0x49, true) => "PageUp",
        (0x4a, _) => "NumpadSubtract",
        (0x4b, false) => "Numpad4",
        (0x4b, true) => "ArrowLeft",
        (0x4c, _) => "Numpad5",
        (0x4d, false) => "Numpad6",
        (0x4d, true) => "ArrowRight",
        (0x4e, _) => "NumpadAdd",
        (0x4f, false) => "Numpad1",
        (0x4f, true) => "End",
        (0x50, false) => "Numpad2",
        (0x50, true) => "ArrowDown",
        (0x51, false) => "Numpad3",
        (0x51, true) => "PageDown",
        (0x52, false) => "Numpad0",
        (0x52, true) => "Insert",
        (0x53, false) => "NumpadDecimal",
        (0x53, true) => "Delete",
        (0x56, _) => "IntlBackslash",
        (0x57, _) => "F11",
        (0x58, _) => "F12",
        (0x59, _) => "NumpadEqual",
        (0x5b, true) => "MetaLeft",
        (0x5c, true) => "MetaRight",
        (0x5d, true) => "ContextMenu",
        (0x64..=0x6e, _) => [
            "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23",
        ][(scan - 0x64) as usize],
        (0x76, _) => "F24",
        _ => "Unidentified",
    }
}

fn down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

/// The recorder's view of a key message, with the modifiers held now.
pub fn event(wp: WPARAM, lp: LPARAM) -> ShortcutKeyEvent {
    let vk = VIRTUAL_KEY(wp.0 as u16);
    ShortcutKeyEvent {
        code: code(vk, lp).into(),
        ctrl_key: down(VK_CONTROL),
        shift_key: down(VK_SHIFT),
        alt_key: down(VK_MENU),
        meta_key: down(VK_LWIN) || down(VK_RWIN),
    }
}

/// A modifier's key-up only refreshes the held keys (shipping `onKeyUp`).
pub fn is_modifier(wp: WPARAM) -> bool {
    matches!(
        VIRTUAL_KEY(wp.0 as u16),
        VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN
    )
}
