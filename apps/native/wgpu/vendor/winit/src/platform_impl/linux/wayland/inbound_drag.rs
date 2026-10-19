//! COPY-only external URI-list drops. Transfers never block the Wayland queue.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read},
    os::{fd::OwnedFd, unix::ffi::OsStringExt},
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use calloop::{
    generic::Generic,
    timer::{TimeoutAction, Timer},
    Interest, Mode, PostAction, RegistrationToken,
};
use sctk::{
    data_device_manager::{data_device::DataDeviceData, data_offer::DragOffer},
    reexports::client::{
        backend::ObjectId,
        protocol::{
            wl_data_device::WlDataDevice, wl_data_device_manager::DndAction, wl_surface::WlSurface,
        },
        Proxy,
    },
};

use super::{make_wid, state::WinitState, window::WindowState, DeviceId, WindowId};
use crate::{dpi::LogicalPosition, event::WindowEvent, platform::wayland::FileDropBatch};

const MAX_BYTES: usize = 1024 * 1024;
const MAX_TRANSFERS: usize = 8;

#[derive(Default)]
pub struct InboundDrags {
    active: HashMap<ObjectId, ObjectId>,
    transfers: HashMap<ObjectId, Transfer>,
}

struct Transfer {
    offer: DragOffer,
    device: ObjectId,
    window: Weak<Mutex<WindowState>>,
    destination: WindowId,
    position: LogicalPosition<f64>,
    action: DndAction,
    dropped: bool,
    bytes: Vec<u8>,
    paths: Option<Vec<PathBuf>>,
    reader: Option<RegistrationToken>,
    timeout: Option<RegistrationToken>,
    expires: Instant,
}

impl WinitState {
    pub fn inbound_enter(&mut self, device: &WlDataDevice, surface: &WlSurface, x: f64, y: f64) {
        self.inbound_leave(device);
        let Some(offer) = device
            .data::<DataDeviceData>()
            .and_then(|data| data.take_drag_offer())
        else {
            return;
        };
        let destination = make_wid(surface);
        let window = self.windows.borrow().get(&destination).cloned();
        let supported = offer.inner().version() >= 3
            && offer.with_mime_types(|types| types.iter().any(|mime| mime == "text/uri-list"));
        let Some(window) =
            window.filter(|_| supported && self.inbound_drags.transfers.len() < MAX_TRANSFERS)
        else {
            offer.accept_mime_type(offer.serial, None);
            offer.destroy();
            return;
        };
        offer.accept_mime_type(offer.serial, Some("text/uri-list".into()));
        offer.set_actions(DndAction::Copy, DndAction::Copy);
        let pipe = offer.receive("text/uri-list".into()).and_then(|pipe| {
            let fd: OwnedFd = pipe.into();
            let flags = rustix::fs::fcntl_getfl(&fd)?;
            rustix::fs::fcntl_setfl(&fd, flags | rustix::fs::OFlags::NONBLOCK)?;
            Ok(File::from(fd))
        });
        let Ok(pipe) = pipe else {
            offer.destroy();
            return;
        };
        let id = offer.inner().id();
        let read_id = id.clone();
        let expires = Instant::now() + Duration::from_secs(2);
        let reader = self.loop_handle.insert_source(
            Generic::new(pipe, Interest::READ, Mode::Level),
            move |_, file, state| {
                let mut file: &File = file;
                // Bound both memory and time spent in one native dispatch.
                let mut buffer = [0u8; 8192];
                for _ in 0..8 {
                    let Some(transfer) = state.inbound_drags.transfers.get_mut(&read_id) else {
                        return Ok(PostAction::Remove);
                    };
                    if Instant::now() >= transfer.expires {
                        transfer.reader = None;
                        state.cancel_inbound(&read_id);
                        return Ok(PostAction::Remove);
                    }
                    match file.read(&mut buffer) {
                        Ok(0) => {
                            transfer.reader = None;
                            let paths = parse_uri_list(&transfer.bytes);
                            if let Ok(paths) = paths {
                                transfer.bytes.clear();
                                transfer.paths = Some(paths);
                                if let Some(token) = transfer.timeout.take() {
                                    state.loop_handle.remove(token);
                                }
                                state.inbound_ready(&read_id);
                            } else {
                                state.cancel_inbound(&read_id);
                            }
                            return Ok(PostAction::Remove);
                        }
                        Ok(count) if transfer.bytes.len() + count <= MAX_BYTES => {
                            transfer.bytes.extend_from_slice(&buffer[..count])
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            return Ok(PostAction::Continue)
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        _ => {
                            transfer.reader = None;
                            state.cancel_inbound(&read_id);
                            return Ok(PostAction::Remove);
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        );
        let Ok(reader) = reader else {
            offer.destroy();
            return;
        };
        let timeout_id = id.clone();
        let timeout =
            self.loop_handle
                .insert_source(Timer::from_deadline(expires), move |_, _, state| {
                    if let Some(transfer) = state.inbound_drags.transfers.get_mut(&timeout_id) {
                        transfer.timeout = None;
                    }
                    state.cancel_inbound(&timeout_id);
                    TimeoutAction::Drop
                });
        let Ok(timeout) = timeout else {
            self.loop_handle.remove(reader);
            offer.destroy();
            return;
        };
        self.inbound_drags.active.insert(device.id(), id.clone());
        self.inbound_drags.transfers.insert(
            id,
            Transfer {
                action: offer.selected_action,
                offer,
                device: device.id(),
                window: Arc::downgrade(&window),
                destination,
                position: LogicalPosition::new(x, y),
                dropped: false,
                bytes: Vec::new(),
                paths: None,
                reader: Some(reader),
                timeout: Some(timeout),
                expires,
            },
        );
        self.drag_cursor(destination, x, y);
    }

    pub fn drag_cursor(&mut self, destination: WindowId, x: f64, y: f64) {
        if let Some(window) = self.windows.borrow().get(&destination) {
            let position =
                LogicalPosition::new(x, y).to_physical(window.lock().unwrap().scale_factor());
            self.events_sink.push_window_event(
                WindowEvent::CursorMoved {
                    device_id: crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(
                        DeviceId,
                    )),
                    position,
                },
                destination,
            );
            self.dispatched_events = true;
        }
    }

    pub fn inbound_motion(&mut self, device: &WlDataDevice, x: f64, y: f64) {
        let Some(id) = self.inbound_drags.active.get(&device.id()) else {
            return;
        };
        let Some(transfer) = self.inbound_drags.transfers.get_mut(id) else {
            return;
        };
        transfer.position = LogicalPosition::new(x, y);
        let destination = transfer.destination;
        self.drag_cursor(destination, x, y);
    }

    pub fn inbound_leave(&mut self, device: &WlDataDevice) {
        if let Some(id) = self.inbound_drags.active.remove(&device.id()) {
            self.clear_inbound_hover(&id);
            self.cancel_inbound(&id);
        }
    }

    pub fn inbound_action(&mut self, offer: &DragOffer, action: DndAction) {
        if let Some(transfer) = self.inbound_drags.transfers.get_mut(&offer.inner().id()) {
            // Never inspect MIME data here: SCTK holds the offer's mutex.
            if !transfer.dropped {
                transfer.action = action;
            }
        }
    }

    pub fn inbound_drop(&mut self, device: &WlDataDevice) {
        let Some(id) = self.inbound_drags.active.remove(&device.id()) else {
            return;
        };
        self.clear_inbound_hover(&id);
        let Some(transfer) = self.inbound_drags.transfers.get_mut(&id) else {
            return;
        };
        if transfer.action != DndAction::Copy {
            self.cancel_inbound(&id);
            return;
        }
        // Detach the dropped transfer from the seat. Leave/new enter cannot
        // destroy its offer or overwrite its frozen target/position.
        transfer.dropped = true;
        self.inbound_ready(&id);
    }

    fn inbound_ready(&mut self, id: &ObjectId) {
        let Some(transfer) = self.inbound_drags.transfers.get(id) else {
            return;
        };
        let live = transfer.window.upgrade().is_some_and(|window| {
            self.windows
                .borrow()
                .get(&transfer.destination)
                .is_some_and(|current| Arc::ptr_eq(current, &window))
        });
        if !live {
            self.cancel_inbound(id);
            return;
        }
        let Some(paths) = transfer.paths.as_ref() else {
            return;
        };
        if !transfer.dropped {
            for path in paths {
                self.events_sink.push_window_event(
                    WindowEvent::HoveredFile(path.clone()),
                    transfer.destination,
                );
            }
            self.dispatched_events = true;
            return;
        }
        let destination = transfer.destination;
        let position = transfer.position;
        let paths = paths.clone();
        transfer.offer.finish();
        self.deliver_file_drop(destination, paths, position);
        self.cancel_inbound(id);
    }

    pub fn deliver_file_drop(
        &mut self,
        destination: WindowId,
        paths: Vec<PathBuf>,
        position: LogicalPosition<f64>,
    ) {
        if let Some(window) = self.windows.borrow().get(&destination) {
            window
                .lock()
                .unwrap()
                .file_drop_batches
                .push(FileDropBatch {
                    paths: paths.clone(),
                    position,
                });
            for path in paths {
                self.events_sink
                    .push_window_event(WindowEvent::DroppedFile(path), destination);
            }
            self.dispatched_events = true;
        }
    }

    fn clear_inbound_hover(&mut self, id: &ObjectId) {
        if let Some(transfer) = self.inbound_drags.transfers.get(id) {
            self.events_sink
                .push_window_event(WindowEvent::HoveredFileCancelled, transfer.destination);
            self.dispatched_events = true;
        }
    }

    pub fn cancel_window_drags(&mut self, window: WindowId) {
        let ids: Vec<_> = self
            .inbound_drags
            .transfers
            .iter()
            .filter(|(_, transfer)| transfer.destination == window)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.cancel_inbound(&id);
        }
    }

    pub fn cancel_device_drags(&mut self, device: &ObjectId) {
        let ids: Vec<_> = self
            .inbound_drags
            .transfers
            .iter()
            .filter(|(_, transfer)| &transfer.device == device)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.cancel_inbound(&id);
        }
    }

    fn cancel_inbound(&mut self, id: &ObjectId) {
        if let Some(transfer) = self.inbound_drags.transfers.remove(id) {
            if self.inbound_drags.active.get(&transfer.device) == Some(id) {
                self.inbound_drags.active.remove(&transfer.device);
                self.events_sink
                    .push_window_event(WindowEvent::HoveredFileCancelled, transfer.destination);
                self.dispatched_events = true;
            }
            if let Some(token) = transfer.reader {
                self.loop_handle.remove(token);
            }
            if let Some(token) = transfer.timeout {
                self.loop_handle.remove(token);
            }
            transfer.offer.destroy();
        }
    }
}

fn parse_uri_list(bytes: &[u8]) -> Result<Vec<PathBuf>, ()> {
    let text = std::str::from_utf8(bytes).map_err(|_| ())?;
    let mut paths = Vec::new();
    for line in text
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let path = line
            .strip_prefix("file://localhost/")
            .map(|path| format!("/{path}"))
            .or_else(|| {
                line.strip_prefix("file://")
                    .filter(|path| path.starts_with('/'))
                    .map(str::to_owned)
            })
            .ok_or(())?;
        if path.contains(['?', '#']) {
            return Err(());
        }
        let mut decoded = Vec::new();
        let mut bytes = path.bytes();
        while let Some(byte) = bytes.next() {
            let byte = if byte == b'%' {
                let high = (bytes.next().ok_or(())? as char).to_digit(16).ok_or(())?;
                let low = (bytes.next().ok_or(())? as char).to_digit(16).ok_or(())?;
                (high * 16 + low) as u8
            } else {
                byte
            };
            if byte == 0 {
                return Err(());
            }
            decoded.push(byte);
        }
        paths.push(PathBuf::from(std::ffi::OsString::from_vec(decoded)));
    }
    if paths.is_empty() {
        Err(())
    } else {
        Ok(paths)
    }
}
