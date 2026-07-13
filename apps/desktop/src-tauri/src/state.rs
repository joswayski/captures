use std::collections::HashMap;
use std::sync::Arc;

use ces_capture::{DisplayDescriptor, WindowDescriptor, XcapBackend};
use parking_lot::{Mutex, RwLock};
use uuid::Uuid;

use crate::{
    models::{AppSettings, CaptureArtifact, CaptureSession},
    storage,
};

pub struct AppState {
    pub settings: RwLock<AppSettings>,
    pub sessions: Mutex<HashMap<Uuid, CaptureSession>>,
    pub last_artifact: Mutex<Option<CaptureArtifact>>,
    pub backend: XcapBackend,
}

impl AppState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            settings: RwLock::new(storage::load_settings()),
            sessions: Mutex::new(HashMap::new()),
            last_artifact: Mutex::new(None),
            backend: XcapBackend,
        })
    }

    pub fn settings(&self) -> AppSettings {
        self.settings.read().clone()
    }

    pub fn monitors(&self) -> Result<Vec<DisplayDescriptor>, crate::AppError> {
        self.backend.displays().map_err(Into::into)
    }

    pub fn windows(&self) -> Result<Vec<WindowDescriptor>, crate::AppError> {
        self.backend.windows().map_err(Into::into)
    }
}
