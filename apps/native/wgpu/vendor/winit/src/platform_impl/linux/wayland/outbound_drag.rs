// Captures local backport: source-side URI-list drag on winit 0.30's own queue.
// Adapted from winit's 0.31 winit-wayland/src/dnd.rs; SCTK 0.19 owns the
// wl_data_source, wl_data_offer and wl_data_device dispatch on this branch.
use std::{io::Write, os::unix::ffi::OsStrExt, path::PathBuf};

use sctk::data_device_manager::{
    data_device::DataDeviceHandler,
    data_offer::{DataOfferHandler, DragOffer},
    data_source::{DataSourceHandler, DragSource},
    WritePipe,
};
use sctk::reexports::client::{
    protocol::{
        wl_data_device::WlDataDevice, wl_data_device_manager::DndAction,
        wl_data_source::WlDataSource, wl_surface::WlSurface,
    },
    Connection, Proxy, QueueHandle,
};

use super::{make_wid, state::WinitState, WindowId};

pub struct OutboundDrag {
    pub source: DragSource,
    pub source_window: WindowId,
    pub uri: Vec<u8>,
    pub action: DndAction,
    pub dropped: bool,
    pub over_own_window: bool,
    pub over_source_window: bool,
    pub own_offer_copy: bool,
    pub own_drop: bool,
    pub same_source_drop: bool,
    pub finished: Option<Box<dyn FnOnce(bool, bool, bool) + Send>>,
}

// URI-list uses percent-encoded *bytes*, not lossy UTF-8 path conversion.
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

impl WinitState {
    fn finish_outbound_drag(&mut self, source: &WlDataSource, accepted: bool) {
        if self
            .outbound_drag
            .as_ref()
            .is_none_or(|drag| drag.source.inner() != source)
        {
            return;
        }
        if let Some(mut drag) = self.outbound_drag.take() {
            let own = drag.own_drop;
            let same_source = drag.same_source_drop;
            if let Some(finished) = drag.finished.take() {
                finished(
                    accepted && drag.dropped && drag.action.contains(DndAction::Copy),
                    own,
                    same_source,
                );
            }
        }
    }
}

impl DataSourceHandler for WinitState {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &WlDataSource,
        mime: String,
        mut fd: WritePipe,
    ) {
        if mime != "text/uri-list" {
            return;
        }
        if let Some(drag) = self
            .outbound_drag
            .as_ref()
            .filter(|drag| drag.source.inner() == source)
        {
            let uri = drag.uri.clone();
            // The receiver may read late; never block the compositor event loop.
            std::thread::spawn(move || {
                let _ = fd.write_all(&uri);
            });
        }
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        self.finish_outbound_drag(source, false);
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if let Some(drag) = self
            .outbound_drag
            .as_mut()
            .filter(|drag| drag.source.inner() == source)
        {
            drag.dropped = true;
            drag.own_drop = drag.over_own_window;
            drag.same_source_drop = drag.over_source_window;
        }
    }

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        self.finish_outbound_drag(source, true);
    }

    fn action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &WlDataSource,
        action: DndAction,
    ) {
        if let Some(drag) = self
            .outbound_drag
            .as_mut()
            .filter(|drag| drag.source.inner() == source)
        {
            drag.action = action;
        }
    }
}

impl DataDeviceHandler for WinitState {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        device: &WlDataDevice,
        _: f64,
        _: f64,
        surface: &WlSurface,
    ) {
        let destination = make_wid(surface);
        let own = self.windows.borrow().contains_key(&destination);
        if let Some(drag) = self.outbound_drag.as_mut() {
            drag.over_own_window = own;
            drag.over_source_window = destination == drag.source_window;
            drag.own_offer_copy = false;
            if own {
                if let Some(offer) = device.data().and_then(
                    |data: &sctk::data_device_manager::data_device::DataDeviceData| {
                        data.drag_offer()
                    },
                ) {
                    offer.accept_mime_type(offer.serial, Some("text/uri-list".into()));
                    offer.set_actions(DndAction::Copy, DndAction::Copy);
                }
            }
        }
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        if let Some(drag) = self.outbound_drag.as_mut() {
            drag.over_own_window = false;
            drag.over_source_window = false;
            drag.own_offer_copy = false;
        }
    }
    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, _: f64, _: f64) {}
    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}
    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, device: &WlDataDevice) {
        if self
            .outbound_drag
            .as_ref()
            .is_some_and(|drag| drag.over_own_window && drag.own_offer_copy)
        {
            if let Some(offer) = device.data().and_then(
                |data: &sctk::data_device_manager::data_device::DataDeviceData| data.drag_offer(),
            ) {
                offer.finish();
            }
        }
    }
}

impl DataOfferHandler for WinitState {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }
    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        action: DndAction,
    ) {
        if let Some(drag) = self
            .outbound_drag
            .as_mut()
            .filter(|drag| drag.over_own_window)
        {
            drag.own_offer_copy = action.contains(DndAction::Copy);
        }
    }
}

sctk::delegate_data_device!(WinitState);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uri_list_preserves_slashes_and_escapes_unicode_and_spaces() {
        assert_eq!(
            uri_list(&PathBuf::from("/tmp/Café a.png")),
            b"file:///tmp/Caf%C3%A9%20a.png\r\n"
        );
    }
}
