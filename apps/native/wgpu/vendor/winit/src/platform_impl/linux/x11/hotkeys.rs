//! Global capture keys share the window event queue, including their releases.
use std::{
    collections::BTreeMap,
    ffi::CString,
    sync::{Arc, Mutex},
};

use x11_dl::xlib::{self, XEvent, XKeyEvent};
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{ConnectionExt as _, GrabMode, ModMask},
};

use super::XConnection;
use crate::event::ElementState;

pub type Handler = Arc<dyn Fn(u32, ElementState) + Send + Sync>;

struct Binding {
    keycode: u8,
    modifiers: u16,
    pressed: bool,
}

pub(crate) struct State {
    connection: Arc<XConnection>,
    bindings: Mutex<BTreeMap<u32, Binding>>,
    handler: Handler,
}

/// Event-loop-owned registrations on winit's existing X connection.
pub struct HotKeyManager(pub(crate) Arc<State>);

const LOCK_VARIANTS: [u16; 4] = [0, 2, 16, 18];
const MODIFIERS: u16 = 1 | 4 | 8 | 64;

impl HotKeyManager {
    pub(crate) fn new(connection: Arc<XConnection>, handler: Handler) -> Self {
        Self(Arc::new(State {
            connection,
            bindings: Mutex::new(BTreeMap::new()),
            handler,
        }))
    }

    /// Register a DOM physical-code name with X11 Control/Shift/Mod1/Mod4 masks.
    pub fn register(&self, id: u32, code: &str, modifiers: u16) -> Result<(), String> {
        let name = keysym_name(code)?;
        let name = CString::new(name).map_err(|_| "invalid hotkey name".to_owned())?;
        let connection = &self.0.connection;
        let symbol = unsafe { (connection.xlib.XStringToKeysym)(name.as_ptr()) } as u32;
        if symbol == 0 {
            return Err(format!("unsupported hotkey {code}"));
        }
        let conn = connection.xcb_connection();
        let setup = conn.setup();
        let mapping = conn
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        let keycode = mapping
            .keysyms
            .chunks(usize::from(mapping.keysyms_per_keycode))
            .position(|keys| keys.contains(&symbol))
            .map(|offset| setup.min_keycode + offset as u8)
            .ok_or_else(|| format!("no X11 keycode for {code}"))?;
        let mut bindings = self.0.bindings.lock().unwrap();
        if bindings.contains_key(&id)
            || bindings
                .values()
                .any(|key| key.keycode == keycode && key.modifiers == modifiers)
        {
            return Err("hotkey is already registered".into());
        }
        let root = connection.default_root().root;
        let mut grabbed = Vec::new();
        for locks in LOCK_VARIANTS {
            let mask = ModMask::from(modifiers | locks);
            let result = conn
                .grab_key(false, root, mask, keycode, GrabMode::ASYNC, GrabMode::ASYNC)
                .map_err(|e| e.to_string())
                .and_then(|cookie| cookie.check().map_err(|e| e.to_string()));
            if let Err(error) = result {
                for mask in grabbed {
                    if let Ok(cookie) = conn.ungrab_key(keycode, root, mask) {
                        let _ = cookie.check();
                    }
                }
                return Err(error);
            }
            grabbed.push(mask);
        }
        bindings.insert(
            id,
            Binding {
                keycode,
                modifiers,
                pressed: false,
            },
        );
        Ok(())
    }

    /// Remove only this manager's passive grabs; never discard queued input.
    pub fn unregister(&self, id: u32) -> Result<(), String> {
        let mut bindings = self.0.bindings.lock().unwrap();
        let Some(binding) = bindings.get(&id) else {
            return Ok(());
        };
        let conn = self.0.connection.xcb_connection();
        let root = self.0.connection.default_root().root;
        for locks in LOCK_VARIANTS {
            conn.ungrab_key(
                binding.keycode,
                root,
                ModMask::from(binding.modifiers | locks),
            )
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|e| e.to_string())?;
        }
        bindings.remove(&id);
        Ok(())
    }
}

impl Drop for HotKeyManager {
    fn drop(&mut self) {
        let ids = self
            .0
            .bindings
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self.unregister(id);
        }
    }
}

impl State {
    pub(crate) fn event(&self, event: &XEvent) -> bool {
        let state = match event.get_type() {
            xlib::KeyPress => ElementState::Pressed,
            xlib::KeyRelease => ElementState::Released,
            _ => return false,
        };
        let key: &XKeyEvent = event.as_ref();
        if key.window != u64::from(self.connection.default_root().root) {
            return false;
        }
        let (matched, callbacks) = {
            let mut bindings = self.bindings.lock().unwrap();
            let mut callbacks = Vec::new();
            let mut matched = false;
            for (&id, binding) in bindings.iter_mut() {
                if u32::from(binding.keycode) != key.keycode {
                    continue;
                }
                if state == ElementState::Pressed
                    && key.state as u16 & MODIFIERS == binding.modifiers
                {
                    matched = true;
                    if !binding.pressed {
                        binding.pressed = true;
                        callbacks.push(id);
                    }
                } else if state == ElementState::Released && binding.pressed {
                    matched = true;
                    binding.pressed = false;
                    callbacks.push(id);
                }
            }
            (matched, callbacks)
        };
        // A callback may reconfigure registrations. Do not retain their lock.
        for id in callbacks {
            (self.handler)(id, state);
        }
        matched
    }
}

fn keysym_name(code: &str) -> Result<String, String> {
    if let Some(key) = code
        .strip_prefix("Key")
        .filter(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_uppercase())
    {
        return Ok(key.into());
    }
    if let Some(key) = code
        .strip_prefix("Digit")
        .filter(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_digit())
    {
        return Ok(key.into());
    }
    if let Some(key) = code
        .strip_prefix("Numpad")
        .filter(|key| key.len() == 1 && key.as_bytes()[0].is_ascii_digit())
    {
        return Ok(format!("KP_{key}"));
    }
    if code
        .strip_prefix('F')
        .and_then(|key| key.parse::<u8>().ok())
        .is_some_and(|key| (1..=24).contains(&key))
    {
        return Ok(code.into());
    }
    let name = match code {
        "Backslash" => "backslash",
        "BracketLeft" => "bracketleft",
        "BracketRight" => "bracketright",
        "Backquote" => "quoteleft",
        "Comma" => "comma",
        "Equal" => "equal",
        "Minus" => "minus",
        "Period" => "period",
        "Quote" => "leftsinglequotemark",
        "Semicolon" => "semicolon",
        "Slash" => "slash",
        "Backspace" => "BackSpace",
        "CapsLock" => "Caps_Lock",
        "Enter" => "Return",
        "Space" => "space",
        "Tab" => "Tab",
        "Delete" => "Delete",
        "End" => "End",
        "Home" => "Home",
        "Insert" => "Insert",
        "PageDown" => "Page_Down",
        "PageUp" => "Page_Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "ArrowUp" => "Up",
        "NumpadAdd" => "KP_Add",
        "NumpadDecimal" => "KP_Decimal",
        "NumpadDivide" => "KP_Divide",
        "NumpadMultiply" => "KP_Multiply",
        "NumpadSubtract" => "KP_Subtract",
        "Escape" => "Escape",
        "PrintScreen" => "Print",
        "ScrollLock" => "Scroll_Lock",
        // Preserve global-hotkey 0.8's existing mapping, including NumLock.
        "NumLock" => "F1",
        "AudioVolumeDown" => "XF86AudioLowerVolume",
        "AudioVolumeMute" => "XF86AudioMute",
        "AudioVolumeUp" => "XF86AudioRaiseVolume",
        "MediaPlay" => "XF86AudioPlay",
        "MediaPause" => "XF86AudioPause",
        "MediaStop" => "XF86AudioStop",
        "MediaTrackNext" => "XF86AudioNext",
        "MediaTrackPrevious" => "XF86AudioPrev",
        "Pause" => "Pause",
        _ => return Err(format!("unsupported hotkey {code}")),
    };
    Ok(name.into())
}
