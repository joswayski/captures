use crate::overlay::PointerSample;

pub struct PointerSource {
    #[cfg(target_os = "linux")]
    connection: Option<(x11rb::rust_connection::RustConnection, u32)>,
}

impl PointerSource {
    pub fn new() -> Self {
        #[cfg(target_os = "windows")]
        {
            Self {}
        }
        #[cfg(target_os = "linux")]
        {
            use x11rb::connection::Connection;

            let connection = if pointer_features_available() {
                x11rb::connect(None).ok().and_then(|(connection, screen)| {
                    let root = connection.setup().roots.get(screen)?.root;
                    Some((connection, root))
                })
            } else {
                None
            };
            Self { connection }
        }
    }

    pub fn sample(&self) -> Option<PointerSample> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::{
                Foundation::POINT,
                UI::{
                    Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON},
                    WindowsAndMessaging::GetCursorPos,
                },
            };

            let mut point = POINT { x: 0, y: 0 };
            // SAFETY: `point` is valid for writes for the duration of the call,
            // and the key-state calls do not retain pointers or references.
            let available = unsafe { GetCursorPos(&mut point) } != 0;
            if !available {
                return None;
            }
            let primary_down = unsafe { GetAsyncKeyState(i32::from(VK_LBUTTON)) } & i16::MIN != 0;
            let secondary_down = unsafe { GetAsyncKeyState(i32::from(VK_RBUTTON)) } & i16::MIN != 0;
            Some(PointerSample {
                x: point.x,
                y: point.y,
                primary_down,
                secondary_down,
            })
        }
        #[cfg(target_os = "linux")]
        {
            use x11rb::protocol::xproto::{ConnectionExt, KeyButMask};

            let (connection, root) = self.connection.as_ref()?;
            let reply = connection.query_pointer(*root).ok()?.reply().ok()?;
            let mask = u16::from(reply.mask);
            Some(PointerSample {
                x: i32::from(reply.root_x),
                y: i32::from(reply.root_y),
                primary_down: mask & u16::from(KeyButMask::BUTTON1) != 0,
                secondary_down: mask & u16::from(KeyButMask::BUTTON3) != 0,
            })
        }
    }
}

pub fn pointer_features_available() -> bool {
    #[cfg(target_os = "windows")]
    {
        true
    }
    #[cfg(target_os = "linux")]
    {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var_os("XDG_SESSION_TYPE")
                .is_some_and(|session| session.to_string_lossy().eq_ignore_ascii_case("wayland"));
        std::env::var_os("DISPLAY").is_some() && !wayland
    }
}
