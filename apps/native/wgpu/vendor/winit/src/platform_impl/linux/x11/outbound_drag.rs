// Captures local XDND source, sharing winit's X connection/event processor.
// Protocol: https://freedesktop.org/wiki/Specifications/XDND/
use std::{
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    time::{Duration, Instant},
};

use x11_dl::xlib::{
    self, XButtonEvent, XClientMessageEvent, XEvent, XMotionEvent, XSelectionRequestEvent,
};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{self, ConnectionExt as _},
};

use super::{atoms::*, ActiveEventLoop, CookieResultExt, WindowId};

pub struct OutboundDrag {
    source: xproto::Window,
    target: Option<xproto::Window>,
    target_version: u32,
    accepted: bool,
    dropped: bool,
    deadline: Option<Instant>,
    uri: Vec<u8>,
    finished: Option<Box<dyn FnOnce(bool, bool, bool) + Send>>,
}

pub fn uri_list(path: &PathBuf) -> Vec<u8> {
    let mut uri = b"file://".to_vec();
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~".contains(byte) {
            uri.push(*byte);
        } else {
            uri.extend_from_slice(format!("%{byte:02X}").as_bytes());
        }
    }
    uri.extend_from_slice(b"\r\n");
    uri
}

impl ActiveEventLoop {
    pub fn start_file_drag(
        &self,
        source: WindowId,
        path: PathBuf,
        finished: Box<dyn FnOnce(bool, bool, bool) + Send>,
    ) -> Result<(), &'static str> {
        if self.outbound_drag.borrow().is_some() {
            return Err("file drag is already active");
        }
        let source = u64::from(source) as u32;
        if !self
            .windows
            .borrow()
            .contains_key(&WindowId::from(u64::from(source)))
        {
            return Err("source preview window disappeared");
        }
        let conn = self.xconn.xcb_connection();
        // winit selected XI2 input, whose implicit press grab cannot be
        // replaced by a core pointer grab. Release this client's XI2 grabs
        // before taking over the gesture for XDND (other clients are unaffected).
        if let Some(devices) = conn
            .xinput_xi_query_device(xinput::Device::ALL_MASTER)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            for device in devices.infos {
                if device.type_ == xinput::DeviceType::MASTER_POINTER {
                    let _ = conn.xinput_xi_ungrab_device(x11rb::CURRENT_TIME, device.deviceid);
                }
            }
        }
        let grab = conn
            .grab_pointer(
                false,
                source,
                xproto::EventMask::POINTER_MOTION | xproto::EventMask::BUTTON_RELEASE,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
                x11rb::NONE,
                x11rb::NONE,
                x11rb::CURRENT_TIME,
            )
            .map_err(|_| "could not grab X11 pointer")?
            .reply()
            .map_err(|_| "could not grab X11 pointer")?;
        if grab.status != xproto::GrabStatus::SUCCESS {
            return Err(if grab.status == xproto::GrabStatus::NOT_VIEWABLE {
                "X11 drag source is not viewable"
            } else if grab.status == xproto::GrabStatus::ALREADY_GRABBED {
                "X11 pointer is already grabbed"
            } else {
                "X11 pointer grab was refused"
            });
        }
        let keyboard = conn
            .grab_keyboard(
                false,
                source,
                x11rb::CURRENT_TIME,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
            )
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        if keyboard.is_none_or(|reply| reply.status != xproto::GrabStatus::SUCCESS) {
            let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
            let _ = conn.flush();
            return Err("X11 keyboard is already grabbed");
        }
        let atoms = self.xconn.atoms();
        let owns_selection = conn
            .set_selection_owner(source, atoms[XdndSelection], x11rb::CURRENT_TIME)
            .is_ok()
            && conn
                .get_selection_owner(atoms[XdndSelection])
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .is_some_and(|reply| reply.owner == source);
        if !owns_selection {
            let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
            let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
            return Err("could not own X11 drag selection");
        }
        *self.outbound_drag.borrow_mut() = Some(OutboundDrag {
            source,
            target: None,
            target_version: 0,
            accepted: false,
            dropped: false,
            deadline: None,
            uri: uri_list(&path),
            finished: Some(finished),
        });
        let _ = conn.flush();
        Ok(())
    }

    fn finish_file_drag(&self, accepted: bool) {
        let Some(mut drag) = self.outbound_drag.borrow_mut().take() else {
            return;
        };
        let conn = self.xconn.xcb_connection();
        let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = conn.ungrab_keyboard(x11rb::CURRENT_TIME);
        if !drag.dropped {
            if let Some(target) = drag.target {
                self.xconn
                    .send_client_msg(
                        target,
                        target,
                        self.xconn.atoms()[XdndLeave],
                        None,
                        [drag.source, 0, 0, 0, 0],
                    )
                    .expect_then_ignore_error("XdndLeave");
            }
        }
        if conn
            .get_selection_owner(self.xconn.atoms()[XdndSelection])
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|reply| reply.owner == drag.source)
        {
            let _ = conn.set_selection_owner(
                x11rb::NONE,
                self.xconn.atoms()[XdndSelection],
                x11rb::CURRENT_TIME,
            );
        }
        let _ = conn.flush();
        let own = drag.dropped
            && drag.target.is_some_and(|target| {
                self.windows
                    .borrow()
                    .contains_key(&WindowId::from(u64::from(target)))
            });
        let same_source = drag.dropped && drag.target == Some(drag.source);
        if let Some(finished) = drag.finished.take() {
            finished(accepted && drag.dropped, own, same_source);
        }
    }

    pub fn outbound_drag_timeout(&self) -> Option<Duration> {
        self.outbound_drag
            .borrow()
            .as_ref()?
            .deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    pub fn expire_outbound_drag(&self) {
        let expired = self
            .outbound_drag
            .borrow()
            .as_ref()
            .and_then(|drag| drag.deadline)
            .is_some_and(|deadline| Instant::now() >= deadline);
        if expired {
            self.finish_file_drag(false);
        }
    }

    fn aware_target(&self) -> Option<(xproto::Window, u32)> {
        let conn = self.xconn.xcb_connection();
        let mut window = self.root;
        let mut aware = None;
        for _ in 0..32 {
            let child = conn.query_pointer(window).ok()?.reply().ok()?.child;
            if child == x11rb::NONE || child == window {
                break;
            }
            window = child;
            let property = conn
                .get_property(
                    false,
                    window,
                    self.xconn.atoms()[XdndAware],
                    xproto::AtomEnum::ANY,
                    0,
                    1,
                )
                .ok()?
                .reply()
                .ok()?;
            if property.format == 32 {
                if let Some(version) = property.value32().and_then(|mut value| value.next()) {
                    if version >= 3 {
                        aware = Some((window, version.min(5)));
                    }
                }
            }
        }
        aware
    }

    pub fn handle_outbound_drag_event(&self, event: &XEvent) -> bool {
        if self.outbound_drag.borrow().is_none() {
            return false;
        }
        let atoms = self.xconn.atoms();
        let conn = self.xconn.xcb_connection();
        match event.get_type() {
            xlib::MotionNotify => {
                let motion: &XMotionEvent = event.as_ref();
                let aware = self.aware_target();
                let target = aware.map(|(target, _)| target);
                let mut state = self.outbound_drag.borrow_mut();
                let drag = state.as_mut().unwrap();
                if target != drag.target {
                    if let Some(old) = drag.target {
                        self.xconn
                            .send_client_msg(
                                old,
                                old,
                                atoms[XdndLeave],
                                None,
                                [drag.source, 0, 0, 0, 0],
                            )
                            .expect_then_ignore_error("XdndLeave");
                    }
                    drag.target = target;
                    drag.target_version = aware.map_or(0, |(_, version)| version);
                    drag.accepted = false;
                    if let Some(target) = target {
                        self.xconn
                            .send_client_msg(
                                target,
                                target,
                                atoms[XdndEnter],
                                None,
                                [
                                    drag.source,
                                    drag.target_version << 24,
                                    atoms[TextUriList],
                                    0,
                                    0,
                                ],
                            )
                            .expect_then_ignore_error("XdndEnter");
                    }
                }
                if let Some(target) = target {
                    let packed =
                        ((motion.x_root as u16 as u32) << 16) | (motion.y_root as u16 as u32);
                    self.xconn
                        .send_client_msg(
                            target,
                            target,
                            atoms[XdndPosition],
                            None,
                            [
                                drag.source,
                                0,
                                packed,
                                motion.time as u32,
                                atoms[XdndActionCopy],
                            ],
                        )
                        .expect_then_ignore_error("XdndPosition");
                }
                let _ = conn.flush();
                true
            }
            xlib::ButtonRelease => {
                let release: &XButtonEvent = event.as_ref();
                if release.button != 1 {
                    return false;
                }
                let mut state = self.outbound_drag.borrow_mut();
                let drag = state.as_mut().unwrap();
                if let Some(target) = drag.target.filter(|_| drag.accepted) {
                    drag.dropped = true;
                    drag.deadline = Some(Instant::now() + Duration::from_secs(5));
                    self.xconn
                        .send_client_msg(
                            target,
                            target,
                            atoms[XdndDrop],
                            None,
                            [drag.source, 0, release.time as u32, 0, 0],
                        )
                        .expect_then_ignore_error("XdndDrop");
                    let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
                    let _ = conn.flush();
                } else {
                    drop(state);
                    self.finish_file_drag(false);
                }
                // The passive grab must not hide the release from winit's pointer state.
                false
            }
            xlib::ClientMessage => {
                let client: &XClientMessageEvent = event.as_ref();
                let own_destination = self.outbound_drag.borrow().as_ref().is_some_and(|drag| {
                    client.data.get_long(0) as u32 == drag.source
                        && self
                            .windows
                            .borrow()
                            .contains_key(&WindowId::from(client.window as u64))
                });
                if own_destination && client.message_type as u32 == atoms[XdndEnter] {
                    return true;
                }
                if own_destination && client.message_type as u32 == atoms[XdndPosition] {
                    self.xconn
                        .send_client_msg(
                            client.window as u32,
                            self.outbound_drag.borrow().as_ref().unwrap().source,
                            atoms[XdndStatus],
                            None,
                            [client.window as u32, 1, 0, 0, atoms[XdndActionCopy]],
                        )
                        .expect_then_ignore_error("XdndStatus");
                    let _ = conn.flush();
                    return true;
                }
                if own_destination && client.message_type as u32 == atoms[XdndDrop] {
                    self.xconn
                        .send_client_msg(
                            client.window as u32,
                            self.outbound_drag.borrow().as_ref().unwrap().source,
                            atoms[XdndFinished],
                            None,
                            [client.window as u32, 1, atoms[XdndActionCopy], 0, 0],
                        )
                        .expect_then_ignore_error("XdndFinished");
                    let _ = conn.flush();
                    return true;
                }
                if client.message_type as u32 == atoms[XdndStatus] {
                    let mut state = self.outbound_drag.borrow_mut();
                    let drag = state.as_mut().unwrap();
                    if drag.target == Some(client.data.get_long(0) as u32) {
                        drag.accepted = client.data.get_long(1) & 1 != 0
                            && client.data.get_long(4) as u32 == atoms[XdndActionCopy];
                    }
                    return true;
                }
                if client.message_type as u32 == atoms[XdndFinished] {
                    let matching = self.outbound_drag.borrow().as_ref().is_some_and(|drag| {
                        drag.dropped && drag.target == Some(client.data.get_long(0) as u32)
                    });
                    if !matching {
                        return true;
                    }
                    let accepted = self.outbound_drag.borrow().as_ref().is_some_and(|drag| {
                        drag.target_version < 5
                            || (client.data.get_long(1) & 1 != 0
                                && (client.data.get_long(2) == 0
                                    || client.data.get_long(2) as u32 == atoms[XdndActionCopy]))
                    });
                    self.finish_file_drag(accepted);
                    return true;
                }
                false
            }
            xlib::KeyPress => {
                let key: &x11_dl::xlib::XKeyEvent = event.as_ref();
                let escape = unsafe {
                    (self.xconn.xlib.XKeysymToKeycode)(
                        self.xconn.display,
                        x11_dl::keysym::XK_Escape as _,
                    )
                } as u32;
                if key.keycode == escape {
                    self.finish_file_drag(false);
                    return true;
                }
                false
            }
            xlib::SelectionClear => {
                let clear: &xlib::XSelectionClearEvent = event.as_ref();
                if clear.selection as u32 != atoms[XdndSelection] {
                    return false;
                }
                self.finish_file_drag(false);
                true
            }
            xlib::DestroyNotify => {
                let destroyed = unsafe { event.destroy_window.window as u32 };
                let cancel =
                    self.outbound_drag.borrow().as_ref().is_some_and(|drag| {
                        drag.source == destroyed || drag.target == Some(destroyed)
                    });
                if cancel {
                    self.finish_file_drag(false);
                }
                false
            }
            xlib::SelectionRequest => {
                let request: &XSelectionRequestEvent = event.as_ref();
                let drag = self.outbound_drag.borrow();
                let drag = drag.as_ref().unwrap();
                if request.selection as u32 != atoms[XdndSelection] {
                    return false;
                }
                let property = if request.target as u32 == atoms[TextUriList] {
                    let property = if request.property == 0 {
                        request.target
                    } else {
                        request.property
                    } as u32;
                    if conn
                        .change_property8(
                            xproto::PropMode::REPLACE,
                            request.requestor as u32,
                            property,
                            atoms[TextUriList],
                            &drag.uri,
                        )
                        .ok()
                        .is_some_and(|cookie| cookie.check().is_ok())
                    {
                        property
                    } else {
                        x11rb::NONE
                    }
                } else {
                    x11rb::NONE
                };
                let response = xproto::SelectionNotifyEvent {
                    response_type: xproto::SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: request.time as u32,
                    requestor: request.requestor as u32,
                    selection: request.selection as u32,
                    target: request.target as u32,
                    property,
                };
                let _ = conn.send_event(
                    false,
                    request.requestor as u32,
                    xproto::EventMask::NO_EVENT,
                    response,
                );
                let _ = conn.flush();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_uri_preserves_path_separators_and_encodes_spaces() {
        assert_eq!(
            uri_list(&PathBuf::from("/tmp/Café a.png")),
            b"file:///tmp/Caf%C3%A9%20a.png\r\n"
        );
    }
}
