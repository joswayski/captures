// Captures local XDND source, sharing winit's X connection/event processor.
// Protocol: https://freedesktop.org/wiki/Specifications/XDND/
use std::{os::unix::ffi::OsStrExt, path::PathBuf};

use x11_dl::xlib::{self, XButtonEvent, XClientMessageEvent, XEvent, XMotionEvent,
                   XSelectionRequestEvent};
use x11rb::{connection::Connection as _, protocol::xproto::{self, ConnectionExt as _}};
use x11rb::wrapper::ConnectionExt as _;

use super::{atoms::*, ActiveEventLoop, CookieResultExt, WindowId};

pub struct OutboundDrag {
    source: xproto::Window,
    target: Option<xproto::Window>,
    accepted: bool,
    dropped: bool,
    uri: Vec<u8>,
    finished: Option<Box<dyn FnOnce(bool, bool) + Send>>,
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
    pub fn start_file_drag(&self, source: WindowId, path: PathBuf,
                           finished: Box<dyn FnOnce(bool, bool) + Send>) -> Result<(), &'static str> {
        if self.outbound_drag.borrow().is_some() { return Err("file drag is already active"); }
        let source = u64::from(source) as u32;
        if !self.windows.borrow().contains_key(&WindowId::from(u64::from(source))) {
            return Err("source preview window disappeared");
        }
        let conn = self.xconn.xcb_connection();
        let grab = conn.grab_pointer(false, source,
            xproto::EventMask::POINTER_MOTION | xproto::EventMask::BUTTON_RELEASE,
            xproto::GrabMode::ASYNC, xproto::GrabMode::ASYNC,
            x11rb::NONE, x11rb::NONE, x11rb::CURRENT_TIME)
            .map_err(|_| "could not grab X11 pointer")?.reply()
            .map_err(|_| "could not grab X11 pointer")?;
        if grab.status != xproto::GrabStatus::SUCCESS { return Err("X11 pointer is already grabbed"); }
        let atoms = self.xconn.atoms();
        if conn.set_selection_owner(source, atoms[XdndSelection], x11rb::CURRENT_TIME).is_err() {
            let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
            return Err("could not own X11 drag selection");
        }
        *self.outbound_drag.borrow_mut() = Some(OutboundDrag {
            source, target: None, accepted: false, dropped: false,
            uri: uri_list(&path), finished: Some(finished),
        });
        let _ = conn.flush();
        Ok(())
    }

    fn finish_file_drag(&self, accepted: bool) {
        let Some(mut drag) = self.outbound_drag.borrow_mut().take() else { return };
        let conn = self.xconn.xcb_connection();
        let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
        let _ = conn.set_selection_owner(x11rb::NONE, self.xconn.atoms()[XdndSelection], x11rb::CURRENT_TIME);
        let _ = conn.flush();
        let own = drag.target.is_some_and(|target| self.windows.borrow().contains_key(&WindowId::from(u64::from(target))));
        if let Some(finished) = drag.finished.take() { finished(accepted && drag.dropped, own); }
    }

    fn aware_target(&self) -> Option<xproto::Window> {
        let conn = self.xconn.xcb_connection();
        let mut window = self.root;
        let mut aware = None;
        for _ in 0..32 {
            let child = conn.query_pointer(window).ok()?.reply().ok()?.child;
            if child == x11rb::NONE || child == window { break; }
            window = child;
            let property = conn.get_property(false, window, self.xconn.atoms()[XdndAware],
                xproto::AtomEnum::ANY, 0, 1).ok()?.reply().ok()?;
            if property.format == 32 && property.value32().and_then(|mut value| value.next()).is_some() {
                aware = Some(window);
            }
        }
        aware
    }

    pub fn handle_outbound_drag_event(&self, event: &XEvent) -> bool {
        if self.outbound_drag.borrow().is_none() { return false; }
        let atoms = self.xconn.atoms();
        let conn = self.xconn.xcb_connection();
        match event.get_type() {
            xlib::MotionNotify => {
                let motion: &XMotionEvent = event.as_ref();
                let target = self.aware_target();
                let mut state = self.outbound_drag.borrow_mut();
                let drag = state.as_mut().unwrap();
                if target != drag.target {
                    if let Some(old) = drag.target {
                        self.xconn.send_client_msg(old, old, atoms[XdndLeave], None,
                            [drag.source, 0, 0, 0, 0]).expect_then_ignore_error("XdndLeave");
                    }
                    drag.target = target; drag.accepted = false;
                    if let Some(target) = target {
                        self.xconn.send_client_msg(target, target, atoms[XdndEnter], None,
                            [drag.source, 5 << 24, atoms[TextUriList], 0, 0]).expect_then_ignore_error("XdndEnter");
                    }
                }
                if let Some(target) = target {
                    let packed = ((motion.x_root as u16 as u32) << 16) | (motion.y_root as u16 as u32);
                    self.xconn.send_client_msg(target, target, atoms[XdndPosition], None,
                        [drag.source, 0, packed, motion.time as u32, atoms[XdndActionCopy]])
                        .expect_then_ignore_error("XdndPosition");
                }
                let _ = conn.flush();
                true
            }
            xlib::ButtonRelease => {
                let release: &XButtonEvent = event.as_ref();
                if release.button != 1 { return false; }
                let mut state = self.outbound_drag.borrow_mut();
                let drag = state.as_mut().unwrap();
                if let Some(target) = drag.target.filter(|_| drag.accepted) {
                    drag.dropped = true;
                    self.xconn.send_client_msg(target, target, atoms[XdndDrop], None,
                        [drag.source, 0, release.time as u32, 0, 0]).expect_then_ignore_error("XdndDrop");
                    let _ = conn.ungrab_pointer(x11rb::CURRENT_TIME);
                    let _ = conn.flush();
                } else {
                    drop(state);
                    self.finish_file_drag(false);
                }
                true
            }
            xlib::ClientMessage => {
                let client: &XClientMessageEvent = event.as_ref();
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
                    let accepted = self.outbound_drag.borrow().as_ref().is_some_and(|drag| {
                        drag.target == Some(client.data.get_long(0) as u32)
                            && client.data.get_long(1) & 1 != 0
                            && (client.data.get_long(2) == 0
                                || client.data.get_long(2) as u32 == atoms[XdndActionCopy])
                    });
                    self.finish_file_drag(accepted);
                    return true;
                }
                false
            }
            xlib::SelectionRequest => {
                let request: &XSelectionRequestEvent = event.as_ref();
                let drag = self.outbound_drag.borrow();
                let drag = drag.as_ref().unwrap();
                if request.selection as u32 != atoms[XdndSelection] { return false; }
                let property = if request.target as u32 == atoms[TextUriList] {
                    let property = if request.property == 0 { request.target } else { request.property } as u32;
                    let _ = conn.change_property8(xproto::PropMode::REPLACE,
                        request.requestor as u32, property, atoms[TextUriList], &drag.uri);
                    property
                } else { x11rb::NONE };
                let response = xproto::SelectionNotifyEvent {
                    response_type: xproto::SELECTION_NOTIFY_EVENT, sequence: 0,
                    time: request.time as u32, requestor: request.requestor as u32,
                    selection: request.selection as u32, target: request.target as u32,
                    property,
                };
                let _ = conn.send_event(false, request.requestor as u32,
                    xproto::EventMask::NO_EVENT, response);
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
        assert_eq!(uri_list(&PathBuf::from("/tmp/Café a.png")),
                   b"file:///tmp/Caf%C3%A9%20a.png\r\n");
    }
}
