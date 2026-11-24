// Copyright 2022-2022 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use crossbeam_channel::{unbounded, Receiver, Sender};
use keyboard_types::{Code, Modifiers};
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt, CreateWindowAux, EventMask, GrabMode, KeyButMask,
    Keycode, ModMask, Window, WindowClass,
};
use x11rb::protocol::{xkb, ErrorKind, Event};
use x11rb::rust_connection::RustConnection;
use xkeysym::RawKeysym;

use crate::{hotkey::HotKey, Error, GlobalHotKeyEvent};

enum ThreadMessage {
    RegisterHotKey(HotKey, Sender<crate::Result<()>>),
    RegisterHotKeys(Vec<HotKey>, Sender<crate::Result<()>>),
    UnRegisterHotKey(HotKey, Sender<crate::Result<()>>),
    UnRegisterHotKeys(Vec<HotKey>, Sender<crate::Result<()>>),
    WatchKeyboard(bool),
    DropThread,
}

/// The event thread's connection and its unmapped wake window. The thread
/// blocks until the X server has an event for it; a `ClientMessage` sent to
/// this window is that event when a command is waiting.
type Wake = Arc<OnceLock<(Arc<RustConnection>, Window)>>;

pub struct GlobalHotKeyManager {
    thread_tx: Sender<ThreadMessage>,
    wake: Wake,
}

impl GlobalHotKeyManager {
    pub fn new() -> crate::Result<Self> {
        let (thread_tx, thread_rx) = unbounded();
        let wake = Wake::default();
        let published = wake.clone();
        std::thread::spawn(move || {
            if let Err(_err) = events_processor(thread_rx, published) {
                #[cfg(feature = "tracing")]
                tracing::error!("{}", _err);
            }
        });
        Ok(Self { thread_tx, wake })
    }

    /// Queue `message` and wake the event thread. Before the thread has
    /// published its connection it has not blocked yet, and it reads queued
    /// commands before it first does.
    fn send(&self, message: ThreadMessage) {
        let _ = self.thread_tx.send(message);
        if let Some((conn, window)) = self.wake.get() {
            let event = ClientMessageEvent::new(32, *window, AtomEnum::NOTICE, [0u32; 5]);
            if let Ok(cookie) = conn.send_event(false, *window, EventMask::NO_EVENT, event) {
                cookie.ignore_error();
            }
            let _ = conn.flush();
        }
    }

    pub fn register(&self, hotkey: HotKey) -> crate::Result<()> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(ThreadMessage::RegisterHotKey(hotkey, tx));

        if let Ok(result) = rx.recv() {
            result?;
        }

        Ok(())
    }

    pub fn unregister(&self, hotkey: HotKey) -> crate::Result<()> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(ThreadMessage::UnRegisterHotKey(hotkey, tx));

        if let Ok(result) = rx.recv() {
            result?;
        }

        Ok(())
    }

    pub fn register_all(&self, hotkeys: &[HotKey]) -> crate::Result<()> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(ThreadMessage::RegisterHotKeys(hotkeys.to_vec(), tx));

        if let Ok(result) = rx.recv() {
            result?;
        }

        Ok(())
    }

    pub fn unregister_all(&self, hotkeys: &[HotKey]) -> crate::Result<()> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.send(ThreadMessage::UnRegisterHotKeys(hotkeys.to_vec(), tx));

        if let Ok(result) = rx.recv() {
            result?;
        }

        Ok(())
    }

    pub fn watch_keyboard(&self, watch: bool) {
        self.send(ThreadMessage::WatchKeyboard(watch));
    }
}

impl Drop for GlobalHotKeyManager {
    fn drop(&mut self) {
        self.send(ThreadMessage::DropThread);
    }
}

// XGrabKey works only with the exact state (modifiers)
// and since X11 considers NumLock, ScrollLock and CapsLock a modifier when it is ON,
// we also need to register our shortcut combined with these extra modifiers as well
fn ignored_mods() -> [ModMask; 4] {
    [
        ModMask::default(), // modifier only
        ModMask::M2,        // NumLock
        ModMask::LOCK,      // CapsLock
        ModMask::M2 | ModMask::LOCK,
    ]
}

#[inline]
fn register_hotkey(
    conn: &RustConnection,
    root: Window,
    hotkeys: &mut BTreeMap<Keycode, Vec<HotKeyState>>,
    hotkey: HotKey,
) -> crate::Result<()> {
    let (mods, key) = (
        modifiers_to_x11_mods(hotkey.mods),
        keycode_to_x11_keysym(hotkey.key),
    );

    let Some(key) = key else {
        return Err(Error::FailedToRegister(format!(
            "Unknown scancode for key: {}",
            hotkey.key
        )));
    };

    let keycode = keysym_to_keycode(conn, key).map_err(Error::FailedToRegister)?;

    let Some(keycode) = keycode else {
        return Err(Error::FailedToRegister(format!(
            "Unable to find keycode for key: {}",
            hotkey.key
        )));
    };

    for m in ignored_mods() {
        let result = conn
            .grab_key(
                false,
                root,
                mods | m,
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(|err| Error::FailedToRegister(err.to_string()))?;

        if let Err(err) = result.check() {
            return match err {
                ReplyError::ConnectionError(err) => Err(Error::FailedToRegister(err.to_string())),
                ReplyError::X11Error(err) => {
                    if let ErrorKind::Access = err.error_kind {
                        for m in ignored_mods() {
                            if let Ok(result) = conn.ungrab_key(keycode, root, mods | m) {
                                result.ignore_error();
                            }
                        }
                        conn.flush()
                            .map_err(|err| Error::FailedToRegister(err.to_string()))?;

                        Err(Error::AlreadyRegistered(hotkey))
                    } else {
                        Err(Error::FailedToRegister(format!("{err:?}")))
                    }
                }
            };
        }
    }

    let entry = hotkeys.entry(keycode).or_default();
    match entry.iter().find(|e| e.mods == mods) {
        None => {
            let state = HotKeyState {
                id: hotkey.id(),
                mods,
                pressed: false,
                polled: false,
            };
            entry.push(state);
            Ok(())
        }
        Some(_) => Err(Error::AlreadyRegistered(hotkey)),
    }
}

#[inline]
fn unregister_hotkey(
    conn: &RustConnection,
    root: Window,
    hotkeys: &mut BTreeMap<Keycode, Vec<HotKeyState>>,
    hotkey: HotKey,
) -> crate::Result<()> {
    let (modifiers, key) = (
        modifiers_to_x11_mods(hotkey.mods),
        keycode_to_x11_keysym(hotkey.key),
    );

    let Some(key) = key else {
        return Err(Error::FailedToUnRegister(hotkey));
    };

    let keycode = keysym_to_keycode(conn, key).map_err(|_err| Error::FailedToUnRegister(hotkey))?;

    let Some(keycode) = keycode else {
        return Err(Error::FailedToUnRegister(hotkey));
    };

    for m in ignored_mods() {
        if let Ok(result) = conn.ungrab_key(keycode, root, modifiers | m) {
            result.ignore_error();
        }
    }
    // poll_for_event only reads. Without a flush, Escape remains grabbed until
    // another registration happens to send the buffered release requests.
    conn.flush()
        .map_err(|_| Error::FailedToUnRegister(hotkey))?;

    let entry = hotkeys.entry(keycode).or_default();
    entry.retain(|k| k.mods != modifiers);
    Ok(())
}

struct HotKeyState {
    id: u32,
    pressed: bool,
    /// Pressed as read from the key map, so its release may never arrive as
    /// an event: keep reading the key map until it is released.
    polled: bool,
    mods: ModMask,
}

/// While another client actively grabs the keyboard (GTK 4.6 and 4.8 do for
/// a drag), our passive grabs never fire. The key map and the pointer's
/// modifier state still report the keys, whoever owns them.
fn poll_keyboard(
    conn: &RustConnection,
    root: Window,
    full_mask: KeyButMask,
    hotkeys: &mut BTreeMap<Keycode, Vec<HotKeyState>>,
) -> Result<(), String> {
    let keys = conn
        .query_keymap()
        .map_err(|err| err.to_string())?
        .reply()
        .map_err(|err| err.to_string())?
        .keys;
    let mods = conn
        .query_pointer(root)
        .map_err(|err| err.to_string())?
        .reply()
        .map_err(|err| err.to_string())?
        .mask;
    let mods = ModMask::from((mods & full_mask).bits());
    for (keycode, entry) in hotkeys.iter_mut() {
        let down = keys[usize::from(*keycode) / 8] & (1 << (keycode % 8)) != 0;
        for state in entry {
            if down && mods == state.mods && !state.pressed {
                GlobalHotKeyEvent::send(GlobalHotKeyEvent {
                    id: state.id,
                    state: crate::HotKeyState::Pressed,
                });
                state.pressed = true;
                state.polled = true;
            } else if !down && state.pressed {
                GlobalHotKeyEvent::send(GlobalHotKeyEvent {
                    id: state.id,
                    state: crate::HotKeyState::Released,
                });
                state.pressed = false;
                state.polled = false;
            }
        }
    }
    Ok(())
}

fn events_processor(thread_rx: Receiver<ThreadMessage>, wake: Wake) -> Result<(), String> {
    let mut hotkeys = BTreeMap::<Keycode, Vec<HotKeyState>>::new();

    let (conn, screen) = RustConnection::connect(None)
        .map_err(|err| format!("Unable to open x11 connection, maybe you are not running under X11? Other window systems on Linux are not supported by `global-hotkey` crate: {err}"))?;
    let conn = Arc::new(conn);

    xkb::ConnectionExt::xkb_use_extension(&*conn, 1, 0)
        .map_err(|err| format!("Unable to send xkb_use_extension request to x11 server: {err}"))?
        .reply()
        .map_err(|err| format!("xkb_use_extension request to x11 server has failed: {err}"))?;

    xkb::ConnectionExt::xkb_per_client_flags(
        &*conn,
        xkb::ID::USE_CORE_KBD.into(),
        xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
        xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT,
        Default::default(),
        Default::default(),
        Default::default(),
    )
    .map_err(|err| format!("Unable to send xkb_per_client_flags request to x11 server: {err}"))?
    .reply()
    .map_err(|err| format!("xkb_per_client_flags request to x11 server has failed: {err}"))?;

    let root = conn.setup().roots[screen].root;

    // An unmapped window of our own: a `ClientMessage` sent to it with no
    // event mask is delivered to this connection, which ends the wait below.
    let wake_window = conn
        .generate_id()
        .map_err(|err| format!("Unable to allocate the x11 wake window: {err}"))?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        wake_window,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        x11rb::COPY_FROM_PARENT,
        &CreateWindowAux::new(),
    )
    .map_err(|err| format!("Unable to send create_window request to x11 server: {err}"))?
    .check()
    .map_err(|err| format!("create_window request to x11 server has failed: {err}"))?;
    let _ = wake.set((conn.clone(), wake_window));

    // X11 sends masks for Lock keys as well, and we only care about the 4 below
    let full_mask = KeyButMask::CONTROL | KeyButMask::SHIFT | KeyButMask::MOD4 | KeyButMask::MOD1;

    let key_event = |event: Event, hotkeys: &mut BTreeMap<Keycode, Vec<HotKeyState>>| match event {
        Event::KeyPress(event) => {
            let keycode = event.detail;

            let event_mods = event.state & full_mask;
            let event_mods = ModMask::from(event_mods.bits());

            if let Some(entry) = hotkeys.get_mut(&keycode) {
                for state in entry {
                    if event_mods == state.mods && !state.pressed {
                        GlobalHotKeyEvent::send(GlobalHotKeyEvent {
                            id: state.id,
                            state: crate::HotKeyState::Pressed,
                        });
                        state.pressed = true;
                    }
                }
            }
        }
        Event::KeyRelease(event) => {
            let keycode = event.detail;

            if let Some(entry) = hotkeys.get_mut(&keycode) {
                for state in entry {
                    if state.pressed {
                        GlobalHotKeyEvent::send(GlobalHotKeyEvent {
                            id: state.id,
                            state: crate::HotKeyState::Released,
                        });
                        state.pressed = false;
                        state.polled = false;
                    }
                }
            }
        }
        _ => {}
    };

    let mut watching = false;
    loop {
        // Commands first. One queued before `wake` was published above is
        // read here; later ones arrive with a wake event.
        while let Ok(msg) = thread_rx.try_recv() {
            match msg {
                ThreadMessage::RegisterHotKey(hotkey, tx) => {
                    let _ = tx.send(register_hotkey(&conn, root, &mut hotkeys, hotkey));
                }
                ThreadMessage::RegisterHotKeys(keys, tx) => {
                    for hotkey in keys {
                        if let Err(e) = register_hotkey(&conn, root, &mut hotkeys, hotkey) {
                            let _ = tx.send(Err(e));
                        }
                    }
                    let _ = tx.send(Ok(()));
                }
                ThreadMessage::UnRegisterHotKey(hotkey, tx) => {
                    let _ = tx.send(unregister_hotkey(&conn, root, &mut hotkeys, hotkey));
                }
                ThreadMessage::UnRegisterHotKeys(keys, tx) => {
                    for hotkey in keys {
                        if let Err(e) = unregister_hotkey(&conn, root, &mut hotkeys, hotkey) {
                            let _ = tx.send(Err(e));
                        }
                    }
                    let _ = tx.send(Ok(()));
                }
                ThreadMessage::WatchKeyboard(watch) => watching = watch,
                ThreadMessage::DropThread => {
                    return Ok(());
                }
            }
        }

        if watching || hotkeys.values().flatten().any(|state| state.polled) {
            // Events first, so a grab's own press and release win.
            while let Ok(Some(event)) = conn.poll_for_event() {
                key_event(event, &mut hotkeys);
            }
            poll_keyboard(&conn, root, full_mask, &mut hotkeys)?;
            std::thread::sleep(std::time::Duration::from_millis(20));
            continue;
        }

        // Sleep until a key or wake event arrives, instead of polling the
        // connection every 50 ms for as long as the application runs.
        let event = conn
            .wait_for_event()
            .map_err(|err| format!("The x11 connection was lost: {err}"))?;
        key_event(event, &mut hotkeys);
        while let Ok(Some(event)) = conn.poll_for_event() {
            key_event(event, &mut hotkeys);
        }
    }
}

fn keycode_to_x11_keysym(key: Code) -> Option<RawKeysym> {
    Some(match key {
        Code::KeyA => xkeysym::key::A,
        Code::KeyB => xkeysym::key::B,
        Code::KeyC => xkeysym::key::C,
        Code::KeyD => xkeysym::key::D,
        Code::KeyE => xkeysym::key::E,
        Code::KeyF => xkeysym::key::F,
        Code::KeyG => xkeysym::key::G,
        Code::KeyH => xkeysym::key::H,
        Code::KeyI => xkeysym::key::I,
        Code::KeyJ => xkeysym::key::J,
        Code::KeyK => xkeysym::key::K,
        Code::KeyL => xkeysym::key::L,
        Code::KeyM => xkeysym::key::M,
        Code::KeyN => xkeysym::key::N,
        Code::KeyO => xkeysym::key::O,
        Code::KeyP => xkeysym::key::P,
        Code::KeyQ => xkeysym::key::Q,
        Code::KeyR => xkeysym::key::R,
        Code::KeyS => xkeysym::key::S,
        Code::KeyT => xkeysym::key::T,
        Code::KeyU => xkeysym::key::U,
        Code::KeyV => xkeysym::key::V,
        Code::KeyW => xkeysym::key::W,
        Code::KeyX => xkeysym::key::X,
        Code::KeyY => xkeysym::key::Y,
        Code::KeyZ => xkeysym::key::Z,
        Code::Backslash => xkeysym::key::backslash,
        Code::BracketLeft => xkeysym::key::bracketleft,
        Code::BracketRight => xkeysym::key::bracketright,
        Code::Backquote => xkeysym::key::quoteleft,
        Code::Comma => xkeysym::key::comma,
        Code::Digit0 => xkeysym::key::_0,
        Code::Digit1 => xkeysym::key::_1,
        Code::Digit2 => xkeysym::key::_2,
        Code::Digit3 => xkeysym::key::_3,
        Code::Digit4 => xkeysym::key::_4,
        Code::Digit5 => xkeysym::key::_5,
        Code::Digit6 => xkeysym::key::_6,
        Code::Digit7 => xkeysym::key::_7,
        Code::Digit8 => xkeysym::key::_8,
        Code::Digit9 => xkeysym::key::_9,
        Code::Equal => xkeysym::key::equal,
        Code::Minus => xkeysym::key::minus,
        Code::Period => xkeysym::key::period,
        Code::Quote => xkeysym::key::leftsinglequotemark,
        Code::Semicolon => xkeysym::key::semicolon,
        Code::Slash => xkeysym::key::slash,
        Code::Backspace => xkeysym::key::BackSpace,
        Code::CapsLock => xkeysym::key::Caps_Lock,
        Code::Enter => xkeysym::key::Return,
        Code::Space => xkeysym::key::space,
        Code::Tab => xkeysym::key::Tab,
        Code::Delete => xkeysym::key::Delete,
        Code::End => xkeysym::key::End,
        Code::Home => xkeysym::key::Home,
        Code::Insert => xkeysym::key::Insert,
        Code::PageDown => xkeysym::key::Page_Down,
        Code::PageUp => xkeysym::key::Page_Up,
        Code::ArrowDown => xkeysym::key::Down,
        Code::ArrowLeft => xkeysym::key::Left,
        Code::ArrowRight => xkeysym::key::Right,
        Code::ArrowUp => xkeysym::key::Up,
        Code::Numpad0 => xkeysym::key::KP_0,
        Code::Numpad1 => xkeysym::key::KP_1,
        Code::Numpad2 => xkeysym::key::KP_2,
        Code::Numpad3 => xkeysym::key::KP_3,
        Code::Numpad4 => xkeysym::key::KP_4,
        Code::Numpad5 => xkeysym::key::KP_5,
        Code::Numpad6 => xkeysym::key::KP_6,
        Code::Numpad7 => xkeysym::key::KP_7,
        Code::Numpad8 => xkeysym::key::KP_8,
        Code::Numpad9 => xkeysym::key::KP_9,
        Code::NumpadAdd => xkeysym::key::KP_Add,
        Code::NumpadDecimal => xkeysym::key::KP_Decimal,
        Code::NumpadDivide => xkeysym::key::KP_Divide,
        Code::NumpadMultiply => xkeysym::key::KP_Multiply,
        Code::NumpadSubtract => xkeysym::key::KP_Subtract,
        Code::Escape => xkeysym::key::Escape,
        Code::PrintScreen => xkeysym::key::Print,
        Code::ScrollLock => xkeysym::key::Scroll_Lock,
        Code::NumLock => xkeysym::key::F1,
        Code::F1 => xkeysym::key::F1,
        Code::F2 => xkeysym::key::F2,
        Code::F3 => xkeysym::key::F3,
        Code::F4 => xkeysym::key::F4,
        Code::F5 => xkeysym::key::F5,
        Code::F6 => xkeysym::key::F6,
        Code::F7 => xkeysym::key::F7,
        Code::F8 => xkeysym::key::F8,
        Code::F9 => xkeysym::key::F9,
        Code::F10 => xkeysym::key::F10,
        Code::F11 => xkeysym::key::F11,
        Code::F12 => xkeysym::key::F12,
        Code::F13 => xkeysym::key::F13,
        Code::F14 => xkeysym::key::F14,
        Code::F15 => xkeysym::key::F15,
        Code::F16 => xkeysym::key::F16,
        Code::F17 => xkeysym::key::F17,
        Code::F18 => xkeysym::key::F18,
        Code::F19 => xkeysym::key::F19,
        Code::F20 => xkeysym::key::F20,
        Code::F21 => xkeysym::key::F21,
        Code::F22 => xkeysym::key::F22,
        Code::F23 => xkeysym::key::F23,
        Code::F24 => xkeysym::key::F24,
        Code::AudioVolumeDown => xkeysym::key::XF86_AudioLowerVolume,
        Code::AudioVolumeMute => xkeysym::key::XF86_AudioMute,
        Code::AudioVolumeUp => xkeysym::key::XF86_AudioRaiseVolume,
        Code::MediaPlay => xkeysym::key::XF86_AudioPlay,
        Code::MediaPause => xkeysym::key::XF86_AudioPause,
        Code::MediaStop => xkeysym::key::XF86_AudioStop,
        Code::MediaTrackNext => xkeysym::key::XF86_AudioNext,
        Code::MediaTrackPrevious => xkeysym::key::XF86_AudioPrev,
        Code::Pause => xkeysym::key::Pause,
        _ => return None,
    })
}

fn modifiers_to_x11_mods(modifiers: Modifiers) -> ModMask {
    let mut x11mods = ModMask::default();
    if modifiers.contains(Modifiers::SHIFT) {
        x11mods |= ModMask::SHIFT;
    }
    if modifiers.intersects(Modifiers::SUPER | Modifiers::META) {
        x11mods |= ModMask::M4;
    }
    if modifiers.contains(Modifiers::ALT) {
        x11mods |= ModMask::M1;
    }
    if modifiers.contains(Modifiers::CONTROL) {
        x11mods |= ModMask::CONTROL;
    }
    x11mods
}

fn keysym_to_keycode(conn: &RustConnection, keysym: RawKeysym) -> Result<Option<Keycode>, String> {
    let setup = conn.setup();
    let min_keycode = setup.min_keycode;
    let max_keycode = setup.max_keycode;
    let count = max_keycode - min_keycode + 1;

    let mapping = conn
        .get_keyboard_mapping(min_keycode, count)
        .map_err(|err| err.to_string())?
        .reply()
        .map_err(|err| err.to_string())?;

    let keysyms_per_keycode = mapping.keysyms_per_keycode as usize;

    for (i, keysyms) in mapping.keysyms.chunks(keysyms_per_keycode).enumerate() {
        if keysyms.contains(&keysym) {
            return Ok(Some(min_keycode + i as u8));
        }
    }

    Ok(None)
}
