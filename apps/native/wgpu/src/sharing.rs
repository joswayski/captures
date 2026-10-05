//! Native sign-in and share settings. The shared Rust worker, not this window
//! or the mini-preview, owns accepted operations and exact original bytes.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

use captures_account::{
    native::{Auth, Command, Event, Selection, State, Worker, error_text},
    sharing::{AssetInfo, Error, Opened, Patch, SharePatch},
};
use eframe::egui::{self, RichText};

use crate::tokens::Tokens;

pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("native-sharing")
}

fn wake(ctx: &egui::Context) {
    crate::live::request_hidden_root_paint(ctx);
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.request_repaint_of(viewport_id());
}

#[derive(Default)]
pub struct Window {
    form: Arc<Mutex<Form>>,
    painted: Option<(egui::Color32, egui::Color32)>,
}

#[derive(Default)]
struct Form {
    open: bool,
    selection: Option<Selection>,
    texture: Option<egui::TextureHandle>,
    worker: Option<Worker>,
    state: Option<State>,
    busy: bool,
    uploading: bool,
    applying_settings: bool,
    status: String,
    email: String,
    code: String,
    password: String,
    remove_password: bool,
    expiry: String,
    remove_expiry: bool,
    progress: Option<(u64, u64)>,
    confirm_trash: bool,
    footer_height: Option<f32>,
    /// Render-only workbench fixture: no vault, network, or worker.
    fixture: bool,
}

impl Window {
    #[cfg(test)]
    pub fn with_worker(worker: Worker) -> Self {
        Self {
            form: Arc::new(Mutex::new(Form {
                worker: Some(worker),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub fn selection(&self) -> Option<Selection> {
        self.form.lock().unwrap().selection.clone()
    }

    pub fn open(
        &self,
        ctx: &egui::Context,
        root: PathBuf,
        selection: Selection,
        texture: Option<egui::TextureHandle>,
    ) {
        let mut form = self.form.lock().unwrap_or_else(PoisonError::into_inner);
        form.poll();
        form.open = true;
        crate::live::request_hidden_root_ui(ctx);
        ctx.send_viewport_cmd_to(viewport_id(), egui::ViewportCommand::Focus);
        // Do not redirect an accepted operation to a different capture.
        if form.busy {
            return;
        }
        let changed = form
            .selection
            .as_ref()
            .is_none_or(|s| s.artifact_id != selection.artifact_id);
        if changed {
            form.password.clear();
            form.expiry.clear();
            form.remove_password = false;
            form.remove_expiry = false;
            form.confirm_trash = false;
            form.state = None;
        }
        form.selection = Some(selection.clone());
        form.texture = texture;
        if form.worker.is_none() {
            let ctx = ctx.clone();
            form.worker = Some(Worker::production(root, Arc::new(move || wake(&ctx))));
        }
        form.send(Command::Open(selection), "Opening selected capture…", false);
    }

    pub fn shutdown(&self) {
        if let Some(mut worker) = self
            .form
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .worker
            .take()
        {
            worker.shutdown();
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, t: &Tokens, hidden: bool) {
        {
            let mut form = self.form.lock().unwrap_or_else(PoisonError::into_inner);
            form.poll();
            if !form.open {
                self.painted = None;
                return;
            }
        }
        let form = self.form.clone();
        let tokens = t.clone();
        ctx.show_viewport_deferred(
            viewport_id(),
            egui::ViewportBuilder::default()
                .with_title("Share capture")
                .with_inner_size([480., 720.])
                .with_min_inner_size([380., 520.])
                .with_active(true)
                .with_visible(!hidden),
            move |ui, _| {
                let mut form = form.lock().unwrap_or_else(PoisonError::into_inner);
                if ui.input(|i| i.viewport().close_requested()) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    form.open = false;
                    form.code.clear();
                    // Retain settings and accepted worker operations; no logout,
                    // upload cancellation or local-file mutation on closure.
                    wake(ui.ctx());
                    return;
                }
                form.ui(ui, &tokens);
            },
        );
        let painted = (t.color("surface-canvas"), t.color("theme-accent"));
        if self.painted != Some(painted) {
            self.painted = Some(painted);
            ctx.request_repaint_of(viewport_id());
        }
    }

    pub fn fixture(ui: &mut egui::Ui, t: &Tokens) {
        let key = egui::Id::unique("share-fixture");
        let form = ui.ctx().data_mut(|data| {
            data.get_temp_mut_or_default::<Arc<Mutex<Form>>>(key)
                .clone()
        });
        let mut form = form.lock().unwrap_or_else(PoisonError::into_inner);
        if form.selection.is_none() {
            let fixture = std::env::var("CAPTURES_NATIVE_SHARE_FIXTURE").unwrap_or_default();
            let state = fixture_state(&fixture);
            *form = Form {
                fixture: true,
                texture: Some(ui.ctx().load_texture(
                    "share-fixture-preview",
                    crate::workbench::fixture_image([320, 180]),
                    Default::default(),
                )),
                busy: fixture == "uploading",
                uploading: fixture == "uploading",
                progress: (fixture == "uploading").then_some((3_145_728, 8_388_608)),
                expiry: match &state.opened {
                    Opened::Asset(asset) => asset
                        .share
                        .as_ref()
                        .and_then(|s| s.expires_at.clone())
                        .unwrap_or_default(),
                    _ => String::new(),
                },
                status: state.error.as_ref().map_or_else(
                    || {
                        if fixture == "uploading" {
                            "Uploading original capture…".into()
                        } else {
                            "Ready".into()
                        }
                    },
                    |e| error_text(e).into(),
                ),
                selection: Some(Selection {
                    artifact_id: "fixture".into(),
                    path: PathBuf::new(),
                    name: "Capture.png".into(),
                    content_type: "image/png".into(),
                }),
                state: Some(state),
                ..Default::default()
            };
        }
        form.ui(ui, t);
    }
}

impl Form {
    fn send(&mut self, command: Command, status: &str, uploading: bool) {
        if self.busy || self.fixture {
            return;
        }
        self.applying_settings = matches!(
            &command,
            Command::Upload(_) | Command::Configure { enabled: true, .. }
        );
        if self.worker.as_ref().is_some_and(|w| w.send(command)) {
            self.busy = true;
            self.uploading = uploading;
            self.status = status.into();
            self.progress = None;
            if let Some(state) = &mut self.state {
                state.error = None;
            }
        } else {
            self.status = "Sharing worker unavailable. Close and reopen the popup.".into();
        }
    }

    fn poll(&mut self) {
        while let Some(event) = self.worker.as_ref().and_then(Worker::try_recv) {
            match event {
                Event::Progress { read, total } => self.progress = Some((read, total)),
                Event::Finished(state) => self.finish(state),
            }
        }
    }

    fn finish(&mut self, state: State) {
        self.busy = false;
        self.uploading = false;
        self.status = state
            .error
            .as_ref()
            .map_or_else(|| "Ready".into(), |e| error_text(e).into());
        if state.error.is_none()
            && (self.applying_settings
                || (self.password.is_empty()
                    && self.expiry.is_empty()
                    && !self.remove_password
                    && !self.remove_expiry))
            && let Opened::Asset(asset) = &state.opened
        {
            self.expiry = asset
                .share
                .as_ref()
                .and_then(|s| s.expires_at.clone())
                .unwrap_or_default();
            self.password.clear();
            self.remove_password = false;
            self.remove_expiry = false;
        }
        if matches!(state.auth, Auth::SignedIn(_)) {
            self.code.clear();
        }
        self.state = Some(state);
        self.applying_settings = false;
    }

    fn patch(&self) -> SharePatch {
        SharePatch {
            password: if self.remove_password {
                Patch::Clear
            } else if self.password.is_empty() {
                Patch::Keep
            } else {
                Patch::Set(self.password.clone())
            },
            expires_at: if self.remove_expiry {
                Patch::Clear
            } else if self.expiry.trim().is_empty() {
                Patch::Keep
            } else {
                Patch::Set(self.expiry.trim().into())
            },
        }
    }

    fn asset(&self) -> Option<&AssetInfo> {
        self.state.as_ref().and_then(|s| match &s.opened {
            Opened::Asset(a) => Some(a.as_ref()),
            _ => None,
        })
    }

    fn link(&self) -> Option<String> {
        if self.busy {
            return None;
        }
        let state = self.state.as_ref()?;
        if state.error.is_some() || !matches!(state.auth, Auth::SignedIn(_)) {
            return None;
        }
        self.asset()?
            .share
            .as_ref()
            .map(|s| format!("{}/s/{}", captures_account::DEFAULT_API, s.id))
    }

    fn ui(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        self.poll();
        let footer = egui::Panel::bottom("sharing-actions")
            .frame(
                egui::Frame::new()
                    .fill(t.color("surface-raised"))
                    .inner_margin(t.number("s-6")),
            )
            .show(ui, |ui| self.footer(ui, t));
        // Bottom panels begin with the previous pass's height. Hide the
        // sizing pass when status/actions/wrapping change, so the first
        // presented frame includes every action rather than clipping it.
        let height = footer.response.rect.height();
        if self.footer_height.replace(height) != Some(height) {
            ui.ctx().request_discard("Share footer height changed");
        }
        egui::CentralPanel::default().frame(egui::Frame::new().fill(t.color("surface-canvas"))).show(ui, |ui| {
            crate::primitives::scroll_area(ui, t, egui::ScrollArea::vertical(), |ui| {
                egui::Frame::new().inner_margin(t.number("s-7")).show(ui, |ui| {
                    ui.label(RichText::new("CAPTURES").small().color(t.color("text-subtle")));
                    ui.heading("Share capture");
                    ui.label("Local captures stay private. Upload only when you choose.");
                    ui.add_space(t.number("s-5"));
                    if let Some(texture) = &self.texture {
                        ui.add(egui::Image::new(texture).fit_to_exact_size(egui::vec2(ui.available_width(), 150.)).maintain_aspect_ratio(true));
                    }
                    if let Some(s) = &self.selection { ui.label(RichText::new(&s.name).strong()); }
                    ui.add_space(t.number("s-5"));
                    let auth = self.state.as_ref().map_or(Auth::Unavailable, |s| s.auth.clone());
                    ui.add_enabled_ui(!self.busy && !self.fixture, |ui| match &auth {
                        Auth::SignedOut | Auth::CodeSent => {
                            ui.label("Sign in with email to share this capture.");
                            ui.label("Email");
                            ui.add(egui::TextEdit::singleline(&mut self.email).hint_text("you@example.com").desired_width(f32::INFINITY))
                                .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), "Email"));
                            if ui.button(if auth == Auth::CodeSent { "Send another code" } else { "Send code" }).clicked() {
                                self.send(Command::RequestCode(self.email.trim().into()), "Sending code…", false);
                            }
                            if auth == Auth::CodeSent {
                                ui.label("Six-character email code");
                                ui.add(egui::TextEdit::singleline(&mut self.code).char_limit(6).password(true).desired_width(f32::INFINITY))
                                    .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), "Email code"));
                                if ui.button("Verify and sign in").clicked() {
                                    self.send(Command::Verify(self.code.trim().to_ascii_uppercase()), "Verifying code…", false);
                                }
                            }
                        }
                        Auth::SaveRequired => {
                            ui.label("Code accepted. Unlock your credential vault to save the session.");
                            if ui.button("Retry saving session").clicked() { self.send(Command::RetrySave, "Saving session…", false); }
                        }
                        Auth::Unavailable => {
                            ui.label("Account or credential vault unavailable. Retry to continue sharing.");
                            if ui.button("Retry account").clicked() { self.send(Command::Refresh, "Checking account…", false); }
                        }
                        Auth::SignedIn(user) => {
                            ui.label(format!("Signed in as {}", user.email));
                            if ui.button("Sign out").clicked() { self.send(Command::Logout, "Signing out…", false); }
                        }
                    });
                    ui.separator();
                    ui.label(RichText::new("Link access").strong());
                    ui.label("Anyone with the link can open it. Links are not publicly indexed.");
                    ui.add_enabled_ui(!self.busy && !self.fixture, |ui| {
                        ui.label(if self.asset().and_then(|a| a.share.as_ref()).is_some_and(|s| s.password_protected) { "New password (blank keeps current)" } else { "Password (optional)" });
                        ui.add_enabled(!self.remove_password, egui::TextEdit::singleline(&mut self.password).password(true).desired_width(f32::INFINITY))
                            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled() && !self.remove_password, "Share password"));
                        ui.checkbox(&mut self.remove_password, "Remove password");
                        ui.label("Expiry (optional, RFC3339 with timezone)");
                        ui.add_enabled(!self.remove_expiry, egui::TextEdit::singleline(&mut self.expiry).hint_text("2027-01-31T18:00:00Z").desired_width(f32::INFINITY))
                            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled() && !self.remove_expiry, "Share expiry"));
                        ui.checkbox(&mut self.remove_expiry, "Remove expiry");
                    });
                });
            });
        });
    }

    /// Keep explicit upload/cancel and completed links reachable while settings
    /// scroll, including the minimum-size window and long service failures.
    fn footer(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        if let Some((read, total)) = self.progress {
            ui.add(
                egui::ProgressBar::new(read as f32 / total.max(1) as f32)
                    .text(format!("{read} / {total} bytes read")),
            );
        }
        ui.label(RichText::new(&self.status).color(
            if self.state.as_ref().is_some_and(|s| s.error.is_some()) {
                t.color("theme-signal-text")
            } else {
                t.color("text-subtle")
            },
        ));
        if self.uploading
            && self.busy
            && ui
                .add_enabled(!self.fixture, egui::Button::new("Cancel upload"))
                .clicked()
        {
            if let Some(w) = &self.worker {
                w.cancel();
            }
            self.status = "Cancelling… Current HTTP request may take up to two minutes.".into();
        }
        if self.fixture {
            ui.label("Rendering fixture — sign-in and uploads disabled.");
        }
        let signed_in = self
            .state
            .as_ref()
            .is_some_and(|s| matches!(s.auth, Auth::SignedIn(_)));
        ui.add_enabled_ui(!self.busy && signed_in && !self.fixture, |ui| {
            let asset = self.asset().cloned();
            if asset.as_ref().is_some_and(|a| a.deleted_at.is_some()) {
                ui.label("Cloud file is in Trash. Restore keeps it private.");
                if ui.button("Restore cloud file").clicked() {
                    self.send(Command::Restore, "Restoring…", false);
                }
            } else {
                ui.horizontal_wrapped(|ui| {
                    let label = if asset.as_ref().is_some_and(|a| a.share.is_some()) {
                        "Save share settings"
                    } else if asset.is_some() {
                        "Share"
                    } else {
                        "Upload and share"
                    };
                    if crate::preferences_widgets::button(ui, t, label, true).clicked() {
                        self.send(
                            Command::Upload(self.patch()),
                            "Uploading and configuring share…",
                            true,
                        );
                    }
                    if asset.as_ref().is_some_and(|a| a.share.is_some())
                        && ui.button("Stop sharing").clicked()
                    {
                        self.send(
                            Command::Configure {
                                enabled: false,
                                patch: SharePatch::default(),
                            },
                            "Stopping sharing…",
                            false,
                        );
                    }
                });
                if asset.is_some() {
                    if ui
                        .button(if self.confirm_trash {
                            "Confirm move cloud file to Trash"
                        } else {
                            "Move cloud file to Trash"
                        })
                        .clicked()
                    {
                        if self.confirm_trash {
                            self.send(Command::Trash, "Moving to Trash…", false);
                        }
                        self.confirm_trash = !self.confirm_trash;
                    }
                    if self.confirm_trash && ui.button("Keep cloud file").clicked() {
                        self.confirm_trash = false;
                    }
                }
            }
            if ui.button("Refresh share status").clicked() {
                self.send(Command::Refresh, "Refreshing…", false);
            }
        });
        if let Some(link) = self.link() {
            if let Some(share) = self.asset().and_then(|a| a.share.as_ref()) {
                ui.label(format!("Shared {}", share.shared_at));
            }
            ui.label(&link);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.fixture, egui::Button::new("Copy link"))
                    .clicked()
                {
                    ui.ctx().copy_text(link.clone());
                }
                if ui
                    .add_enabled(!self.fixture, egui::Button::new("Open link"))
                    .clicked()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(link));
                }
            });
        }
    }
}

fn fixture_state(name: &str) -> State {
    let auth = match name {
        "otp" => Auth::CodeSent,
        "vault" => Auth::SaveRequired,
        "shared" | "uploading" | "trash" | "error" => Auth::SignedIn(captures_account::User {
            id: "fixture-owner".into(),
            email: "you@example.com".into(),
        }),
        _ => Auth::SignedOut,
    };
    let opened = if matches!(name, "shared" | "trash" | "error") {
        Opened::Asset(Box::new(AssetInfo {
            id: "fixture-asset".into(),
            name: "Capture.png".into(),
            content_type: "image/png".into(),
            byte_size: 8_388_608,
            created_at: "2026-10-05T12:00:00Z".into(),
            deleted_at: (name == "trash").then(|| "2026-10-05T13:00:00Z".into()),
            share: (name == "shared").then(|| captures_account::sharing::ShareInfo {
                id: "fixture-link".into(),
                password_protected: true,
                expires_at: Some("2027-01-31T18:00:00Z".into()),
                shared_at: "2026-10-05T12:01:00Z".into(),
            }),
        }))
    } else {
        Opened::Unassociated
    };
    State {
        auth,
        opened,
        error: (name == "error").then_some(Error::Unavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_require_signed_in_completed_configured_success_and_hide_during_mutation() {
        let mut form = Form {
            state: Some(fixture_state("shared")),
            ..Default::default()
        };
        assert_eq!(
            form.link().as_deref(),
            Some("https://captur.es/s/fixture-link")
        );
        form.busy = true;
        assert!(form.link().is_none());
        form.busy = false;
        form.state.as_mut().unwrap().error = Some(Error::Unavailable);
        assert!(form.link().is_none());
        form.state.as_mut().unwrap().error = None;
        form.state.as_mut().unwrap().auth = Auth::SignedOut;
        assert!(form.link().is_none());
        form.state = Some(fixture_state("uploading"));
        assert!(form.link().is_none());
        form.state.as_mut().unwrap().opened = Opened::Pending;
        assert!(form.link().is_none());
    }

    #[test]
    fn sign_in_and_errors_preserve_settings_but_successful_explicit_configuration_consumes_them() {
        let mut form = Form {
            password: "my-password".into(),
            expiry: "2028-03-01T09:00:00Z".into(),
            code: "ABC234".into(),
            ..Default::default()
        };
        form.finish(fixture_state("shared"));
        assert_eq!(form.password, "my-password");
        assert_eq!(form.expiry, "2028-03-01T09:00:00Z");
        assert!(form.code.is_empty());
        form.applying_settings = true;
        form.finish(fixture_state("error"));
        assert_eq!(form.password, "my-password");
        form.applying_settings = true;
        form.finish(fixture_state("shared"));
        assert!(form.password.is_empty());
        assert_eq!(form.expiry, "2027-01-31T18:00:00Z");
        let patch = form.patch();
        assert!(matches!(patch.password, Patch::Keep));
        assert!(matches!(patch.expires_at, Patch::Set(_)));
        form.remove_password = true;
        form.remove_expiry = true;
        let patch = form.patch();
        assert!(matches!(patch.password, Patch::Clear));
        assert!(matches!(patch.expires_at, Patch::Clear));
    }

    #[test]
    fn a_new_preview_cannot_redirect_an_accepted_operation_or_replace_its_settings() {
        let window = Window::default();
        let selection = |id: &str| Selection {
            artifact_id: id.into(),
            path: id.into(),
            name: id.into(),
            content_type: "image/png".into(),
        };
        {
            let mut form = window.form.lock().unwrap();
            form.selection = Some(selection("first"));
            form.busy = true;
            form.password = "keep-me".into();
        }
        window.open(
            &egui::Context::default(),
            PathBuf::new(),
            selection("second"),
            None,
        );
        let form = window.form.lock().unwrap();
        assert_eq!(form.selection.as_ref().unwrap().artifact_id, "first");
        assert_eq!(form.password, "keep-me");
        assert!(
            form.worker.is_none(),
            "busy reopen must not start another worker"
        );
    }

    #[test]
    fn native_form_exposes_named_inputs_and_actions_in_each_nondefault_state() {
        for (name, expected) in [
            ("", "Send code"),
            ("otp", "Verify and sign in"),
            ("vault", "Retry saving session"),
            ("shared", "Stop sharing"),
            ("trash", "Restore cloud file"),
            ("error", "Share"),
        ] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            let mut form = Form {
                state: Some(fixture_state(name)),
                ..Default::default()
            };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480., 1000.),
                    )),
                    ..Default::default()
                },
                |ui| form.ui(ui, &tokens),
            );
            let tree = output.platform_output.accesskit_update.take().unwrap();
            let labels: Vec<_> = tree.nodes.iter().filter_map(|(_, n)| n.label()).collect();
            assert!(
                labels.contains(&expected),
                "missing action {expected} in {name}"
            );
            for input in ["Share password", "Share expiry"] {
                assert!(labels.contains(&input));
            }
            assert!(
                form.worker.is_none(),
                "rendering must not create account or upload work"
            );
            output.textures_delta.clear();
        }
    }

    #[test]
    fn accessibility_reports_disabled_inputs_during_work_and_for_removed_fields() {
        for (busy, remove) in [(true, false), (false, true)] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let tokens = crate::tokens::load()["dark-mustard"].clone();
            let mut form = Form {
                state: Some(fixture_state("otp")),
                busy,
                remove_password: remove,
                remove_expiry: remove,
                ..Default::default()
            };
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(480., 1000.),
                    )),
                    ..Default::default()
                },
                |ui| form.ui(ui, &tokens),
            );
            let tree = output.platform_output.accesskit_update.take().unwrap();
            for name in ["Email", "Email code", "Share password", "Share expiry"] {
                let node = tree
                    .nodes
                    .iter()
                    .find(|(_, n)| n.label() == Some(name))
                    .unwrap()
                    .1
                    .clone();
                assert_eq!(
                    node.is_disabled(),
                    busy || (remove && name.starts_with("Share")),
                    "{name}"
                );
            }
            output.textures_delta.clear();
        }
    }

    #[test]
    fn footer_actions_fit_the_first_presented_frame_after_state_and_size_changes() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let tokens = crate::tokens::load()["dark-mustard"].clone();
        tokens.apply(&ctx, false);
        let mut form = Form::default();
        for (state, width, height) in [
            ("", 480., 720.),
            ("uploading", 480., 720.),
            ("error", 380., 520.),
            ("shared", 380., 520.),
            ("trash", 380., 520.),
        ] {
            form.finish(fixture_state(state));
            form.busy = state == "uploading";
            form.uploading = form.busy;
            form.progress = form.busy.then_some((3, 7));
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, height),
                    )),
                    ..Default::default()
                },
                |ui| form.ui(ui, &tokens),
            );
            let tree = output.platform_output.accesskit_update.take().unwrap();
            let actions: &[&str] = match state {
                "uploading" => &["Cancel upload", "Refresh share status"],
                "shared" => &[
                    "Save share settings",
                    "Stop sharing",
                    "Copy link",
                    "Open link",
                ],
                "trash" => &["Restore cloud file", "Refresh share status"],
                _ => &["Refresh share status"],
            };
            for label in actions {
                let bounds = tree
                    .nodes
                    .iter()
                    .find(|(_, n)| n.label() == Some(*label))
                    .unwrap()
                    .1
                    .bounds()
                    .unwrap();
                assert!(
                    bounds.x0 >= 0.
                        && bounds.y0 >= 0.
                        && bounds.x1 <= width as f64
                        && bounds.y1 <= height as f64,
                    "{state}: {label} clipped at {bounds:?}"
                );
            }
            output.textures_delta.clear();
        }
    }
}
