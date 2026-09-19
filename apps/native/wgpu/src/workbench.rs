use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};

use eframe::egui::{self, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use serde_json::json;

use captures_app::shortcuts::{CaptureShortcut, CaptureShortcuts};

use crate::{
    emit,
    live::{CaptureRequest, Live},
    options::{Options, Scene},
    preferences::Preferences,
    shortcut_input,
    tokens::{self, Tokens},
    tray::{self, Action as TrayAction, Tray},
};

// All state below is disposable fixture/UI state, not a second implementation
// of capture, settings persistence, history retention, or editor documents.
pub struct Workbench {
    options: Options,
    variants: BTreeMap<String, Tokens>,
    applied_variant: String,
    started: Instant,
    cycle: usize,
    frames: u64,
    settled_frames: u64,
    ui_ms: f64,
    max_ui_ms: f64,
    history_filter: usize,
    history_end: bool,
    selected_row: Option<usize>,
    paused: bool,
    muted: bool,
    texture: Option<egui::TextureHandle>,
    animation: Option<Instant>,
    deleted: bool,
    zoom: f32,
    rotation: f32,
    pan: Vec2,
    annotation: String,
    screenshot_requested: bool,
    screenshot_saved: bool,
    screenshot_tx: Sender<Arc<egui::ColorImage>>,
    screenshot_rx: Receiver<Arc<egui::ColorImage>>,
    preferences_state: Preferences,
    capture_controls: crate::capture_controls::CaptureControls,
    region_selector: crate::selector::Selector,
    window_selector: crate::window_selector::WindowSelector,
    window_display: captures_capture::DisplayDescriptor,
    capture_control_displays: Vec<captures_capture::DisplayDescriptor>,
    window_targets: Vec<captures_capture::WindowDescriptor>,
    window_shell: Vec<captures_capture::WindowDescriptor>,
    _temporary_settings: Option<tempfile::TempDir>,
    live: Option<Live>,
    live_preferences: bool,
    root_hidden: bool,
    tray: Option<Tray>,
    tray_error: Option<String>,
    shortcuts: Option<CaptureShortcuts>,
    shortcuts_generation: u64,
    shortcut_error: Option<String>,
    shortcut_suspension_error: Option<String>,
    action_tx: Sender<Result<(), String>>,
    action_rx: Receiver<Result<(), String>>,
    action_error: Option<String>,
    quitting: bool,
}

impl Workbench {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        options: Options,
        shortcut_input: shortcut_input::Bridge,
    ) -> Self {
        if options.scene == Scene::Idle && cc.winit_window().and_then(|w| w.is_visible()).is_none()
        {
            // winit's Wayland root cannot be hidden with set_visible. Do not
            // report a visible, resident window as a successful hidden workload.
            emit(
                "unsupported",
                json!({"capability": "hidden-idle",
                "reason": "This window backend cannot hide/query root visibility; hidden idle is not measured"}),
            );
            std::process::exit(3);
        }
        let adapter = cc
            .wgpu_render_state
            .as_ref()
            .expect("wgpu renderer")
            .adapter
            .get_info();
        emit(
            "ready",
            json!({
                "scene": if options.live { "live" } else { options.scene.name() }, "renderer": "eframe-wgpu", "adapter": adapter.name,
                "backend": format!("{:?}", adapter.backend), "deviceType": format!("{:?}", adapter.device_type),
                "os": std::env::consts::OS, "appearance": options.appearance, "theme": options.theme,
                "historyCount": options.history_count, "floating": options.floating,
                "readiness": "renderer initialized; not first presentation",
                "displayEnvironment": {
                    "wayland": std::env::var("WAYLAND_DISPLAY").ok(),
                    "x11": std::env::var("DISPLAY").ok()
                }
            }),
        );
        let temporary_settings = (options.settings_file.is_none()
            && (options.exercise
                || options.screenshot.is_some()
                || options.scene != Scene::Preferences))
            .then(|| tempfile::tempdir().expect("temporary native settings directory"));
        let settings_path = options.settings_file.clone().unwrap_or_else(|| {
            temporary_settings
                .as_ref()
                .map(|dir| dir.path().join("settings.json"))
                .unwrap_or_else(captures_settings::default_native_settings_path)
        });
        let preferences_state = Preferences::new_with_shortcut_input(
            cc.egui_ctx.clone(),
            settings_path,
            options
                .appearance_override
                .then(|| options.appearance.clone()),
            options.theme_override.then(|| options.theme.clone()),
            shortcut_input,
        );
        let live = options
            .live
            .then(|| Live::new(cc.egui_ctx.clone(), options.history_root.clone()));
        let (tray, tray_error) = if options.live {
            match Tray::new(cc.egui_ctx.clone()) {
                Ok(tray) => (Some(tray), None),
                Err(error) => (
                    None,
                    Some(format!(
                        "Tray unavailable; closing this window will quit Captures. {error}"
                    )),
                ),
            }
        } else {
            (None, None)
        };
        let (action_tx, action_rx) = mpsc::channel();
        let (screenshot_tx, screenshot_rx) = mpsc::channel();
        let (window_display, window_targets, window_shell) = window_fixture();
        let mut capture_control_displays = vec![window_display.clone()];
        capture_control_displays.push(captures_capture::DisplayDescriptor {
            id: "fixture-display-2".into(),
            name: "Secondary fixture display".into(),
            x: 900,
            is_primary: false,
            ..window_display.clone()
        });
        let this = Self {
            options,
            variants: tokens::load(),
            applied_variant: String::new(),
            started: Instant::now(),
            cycle: 0,
            frames: 0,
            settled_frames: 0,
            ui_ms: 0.,
            max_ui_ms: 0.,
            history_filter: 0,
            history_end: false,
            selected_row: None,
            paused: false,
            muted: false,
            texture: None,
            animation: None,
            deleted: false,
            zoom: 1.,
            rotation: 0.,
            pan: Vec2::ZERO,
            annotation: "A capture worth keeping".into(),
            screenshot_requested: false,
            screenshot_saved: false,
            screenshot_tx,
            screenshot_rx,
            preferences_state,
            capture_controls: crate::capture_controls::CaptureControls::fixture(),
            region_selector: crate::selector::Selector::default(),
            window_selector: crate::window_selector::WindowSelector::fixture(),
            window_display,
            capture_control_displays,
            window_targets,
            window_shell,
            _temporary_settings: temporary_settings,
            live,
            live_preferences: false,
            root_hidden: false,
            tray,
            tray_error,
            shortcuts: None,
            shortcuts_generation: 0,
            shortcut_error: None,
            shortcut_suspension_error: None,
            action_tx,
            action_rx,
            action_error: None,
            quitting: false,
        };
        this.schedule(&cc.egui_ctx);
        this
    }

    // Only real deadlines cause wakes; the ordinary static/hidden UI does not
    // own a polling timer, display link, or recurring repaint request.
    fn schedule(&self, ctx: &egui::Context) {
        let elapsed = self.started.elapsed();
        if let Some(quit) = self.options.quit_after {
            ctx.request_repaint_after(quit.saturating_sub(elapsed));
        }
        if self.options.exercise && self.cycle < 6 {
            ctx.request_repaint_after(exercise_at(self.cycle).saturating_sub(elapsed));
        }
        if self.options.screenshot.is_some() && !self.screenshot_requested {
            ctx.request_repaint_after(self.options.screenshot_after.saturating_sub(elapsed));
        }
    }

    fn show_root(&mut self, ctx: &egui::Context) {
        self.root_hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn handle_tray_action(&mut self, action: TrayAction, ctx: &egui::Context) {
        match action {
            TrayAction::NewCapture => {
                if let Some(live) = &mut self.live {
                    live.request_capture(CaptureRequest::NewCapture);
                }
            }
            TrayAction::CaptureDisplay => {
                if let Some(live) = &mut self.live {
                    live.request_capture(CaptureRequest::Display);
                }
            }
            TrayAction::CaptureRegion => {
                if let Some(live) = &mut self.live {
                    live.request_capture(CaptureRequest::Region);
                }
            }
            TrayAction::CaptureWindow => {
                if let Some(live) = &mut self.live {
                    live.request_capture(CaptureRequest::Window);
                }
            }
            TrayAction::History => {
                self.live_preferences = false;
                self.show_root(ctx);
            }
            TrayAction::Preferences => {
                self.live_preferences = true;
                self.show_root(ctx);
            }
            TrayAction::OpenOutputFolder => match self.preferences_state.snapshot() {
                Ok(settings) => {
                    let output = self.action_tx.clone();
                    let wake = ctx.clone();
                    thread::spawn(move || {
                        let path = Path::new(&settings.output_directory);
                        let result = fs::create_dir_all(path)
                            .map_err(|error| error.to_string())
                            .and_then(|()| tray::open_directory(path));
                        let _ = output.send(result);
                        wake.request_repaint();
                    });
                }
                Err(error) => {
                    self.action_error = Some(format!("Could not read output folder: {error}"));
                    self.show_root(ctx);
                }
            },
            #[cfg(target_os = "linux")]
            TrayAction::Unavailable => {
                self.tray_error = Some(
                    "The system tray host stopped. Closing this window will quit Captures.".into(),
                );
                self.tray.take();
                self.show_root(ctx);
            }
            TrayAction::Quit => self.quit(ctx),
        }
    }

    fn quit(&mut self, ctx: &egui::Context) {
        if self.quitting {
            return;
        }
        self.quitting = true;
        self.preferences_state.flush();
        if let Some(live) = &mut self.live {
            live.flush();
        }
        self.shortcuts.take();
        self.tray.take();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn sync_shortcuts(&mut self, ctx: &egui::Context) {
        let generation = self.preferences_state.persisted_generation();
        if !self.options.live || generation == 0 || generation == self.shortcuts_generation {
            return;
        }
        self.shortcuts_generation = generation;
        let settings = match self.preferences_state.snapshot() {
            Ok(settings) => settings,
            Err(error) => {
                self.shortcut_error = Some(error);
                return;
            }
        };
        let result = if let Some(shortcuts) = &mut self.shortcuts {
            shortcuts.update(&settings)
        } else {
            let wake = ctx.clone();
            CaptureShortcuts::new(&settings, move || wake.request_repaint()).map(|shortcuts| {
                self.shortcuts = Some(shortcuts);
            })
        };
        match result {
            Ok(()) => self.shortcut_error = None,
            Err(error) => {
                self.shortcut_error =
                    Some(format!("Global screenshot shortcuts unavailable: {error}"));
            }
        }
    }

    fn sync_shortcut_suspension(&mut self, suspended: bool) {
        let Some(shortcuts) = &mut self.shortcuts else {
            return;
        };
        self.shortcut_suspension_error = shortcuts.set_suspended(suspended).err().map(|error| {
            format!(
                "Could not {} global screenshot shortcuts: {error}",
                if suspended { "suspend" } else { "restore" }
            )
        });
    }

    fn request_screenshot(&mut self, ctx: &egui::Context) {
        let output = self.screenshot_tx.clone();
        let wake = ctx.clone();
        ctx.request_screenshot(move |image| {
            let _ = output.send(image);
            wake.request_repaint();
        });
        self.screenshot_requested = true;
    }

    fn tokens(&mut self, ctx: &egui::Context) -> Tokens {
        if self.options.scene == Scene::Preferences
            && let Some((appearance, theme)) = self.preferences_state.appearance_theme()
        {
            self.options.appearance = appearance;
            self.options.theme = theme;
        }
        let light = match self.options.appearance.as_str() {
            "light" => true,
            "system" => ctx.input(|i| i.raw.system_theme) == Some(egui::Theme::Light),
            _ => false,
        };
        let base = format!(
            "{}-{}",
            if light { "light" } else { "dark" },
            if self.options.theme == "custom" {
                "mustard"
            } else {
                &self.options.theme
            }
        );
        let mut name = base.clone();
        let mut tokens = self.variants[&base].clone();
        if let Some((accent, signal)) = self.preferences_state.custom_colors() {
            tokens = tokens.with_custom_colors(&accent, &signal, light);
            name = format!("{base}-{accent}-{signal}");
        }
        if name != self.applied_variant {
            tokens.apply(ctx, light);
            self.applied_variant = name;
        }
        tokens
    }

    fn change_scene(&mut self, scene: Scene) {
        if self.options.scene != scene {
            self.options.scene = scene;
            self.texture = None; // Release image residency when its scene closes.
            self.animation = None;
            self.deleted = false;
            emit("scene-changed", json!({"scene": scene.name()}));
        }
    }

    fn texture(&mut self, ctx: &egui::Context, cold: bool) -> egui::TextureId {
        if cold {
            self.texture = None;
        }
        if self.texture.is_none() {
            let start = Instant::now();
            let size = if matches!(
                self.options.scene,
                Scene::Editor | Scene::CaptureControls | Scene::Region | Scene::Window
            ) {
                [2048, 1152]
            } else {
                [568, 320]
            };
            let image = fixture_image(size);
            let raster_ms = start.elapsed().as_secs_f64() * 1000.;
            let upload = Instant::now();
            self.texture =
                Some(ctx.load_texture("synthetic capture", image, egui::TextureOptions::LINEAR));
            emit(
                "texture-preparation",
                json!({"rasterMs": raster_ms,
                "enqueueMs": upload.elapsed().as_secs_f64() * 1000., "pixels": size,
                "note": "CPU generation/enqueue; GPU completion is not measured"}),
            );
        }
        self.texture.as_ref().unwrap().id()
    }

    fn preferences(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        if self.preferences_state.ui(ui, t) {
            self.change_scene(Scene::History);
        }
    }

    fn history(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        ui.horizontal(|ui| {
            ui.label(format!("{} synthetic captures", self.options.history_count));
            for count in [0, 100, 1000] {
                if ui
                    .button(if count == 0 {
                        "Empty".into()
                    } else {
                        count.to_string()
                    })
                    .clicked()
                {
                    self.options.history_count = count;
                    self.selected_row = None;
                }
            }
        });
        ui.horizontal(|ui| {
            for (index, title) in ["All", "Screenshots", "Video", "GIF"].iter().enumerate() {
                ui.selectable_value(&mut self.history_filter, index, *title);
            }
        });
        let rows = history_rows(self.options.history_count, self.history_filter);
        if rows.is_empty() {
            self.texture = None;
            ui.add_space(t.number("s-10"));
            ui.heading("No captures yet");
            ui.label("This fixture does not read your capture history.");
            return;
        }
        let texture = self.texture(ui.ctx(), false);
        let row_height = 68.; // Match the AppKit fixture's logical row geometry.
        let mut scroll = egui::ScrollArea::vertical().id_salt("history");
        if self.options.exercise {
            scroll = scroll.vertical_scroll_offset(if self.history_end {
                rows.len() as f32 * (row_height + ui.spacing().item_spacing.y)
            } else {
                0.
            });
        }
        scroll.show_rows(ui, row_height, rows.len(), |ui, range| {
            for index in range {
                let row = rows[index];
                let kind = history_kind(row);
                egui::Frame::new()
                    .fill(t.color("surface-raised"))
                    .corner_radius(t.number("r-lg") as u8)
                    .inner_margin(t.number("s-4") as i8)
                    .show(ui, |ui| {
                        ui.set_min_size(egui::vec2(ui.available_width(), row_height - 16.));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Image::new((texture, egui::vec2(88., 50.)))
                                    .corner_radius(t.number("r-sm") as u8),
                            );
                            ui.vertical(|ui| {
                                if ui
                                    .selectable_label(
                                        self.selected_row == Some(row),
                                        format!("{kind} {}", row + 1),
                                    )
                                    .clicked()
                                {
                                    self.selected_row = Some(row);
                                    emit("history-selection", json!({"row": row}));
                                }
                                ui.label(
                                    RichText::new("Synthetic image · no file on disk")
                                        .small()
                                        .color(t.color("text-muted")),
                                );
                            });
                        });
                    });
            }
        });
    }

    fn hud(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        glass(t).show(ui, |ui| {
            ui.set_width(520.);
            t.glass_controls(ui);
            ui.horizontal(|ui| {
                ui.colored_label(t.color("theme-signal"), if self.paused { "Ⅱ" } else { "●" });
                ui.heading(if self.paused {
                    "Paused · 00:24"
                } else {
                    "Recording · 00:24"
                });
                if ui
                    .button(if self.paused { "Resume" } else { "Pause" })
                    .clicked()
                {
                    self.paused = !self.paused;
                }
                ui.checkbox(&mut self.muted, "Muted");
            });
            ui.label(
                RichText::new("Static timer fixture — no recording engine")
                    .small()
                    .color(t.color("glass-text-muted")),
            );
        });
    }

    fn dissolve(&mut self, ctx: &egui::Context, cold: bool) {
        let start = Instant::now();
        self.texture(ctx, cold);
        self.deleted = false;
        self.animation = if self.options.reduced_motion {
            self.deleted = true;
            None
        } else {
            Some(Instant::now())
        };
        emit(
            "first-action-total",
            json!({"milliseconds": start.elapsed().as_secs_f64() * 1000.,
            "effect": "fade/settle probe, NOT dust parity", "cold": cold}),
        );
        ctx.request_repaint();
    }

    fn preview(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        glass(t).show(ui, |ui| {
            t.glass_controls(ui);
            ui.label("Fade / settle probe · dust filtering not implemented");
            ui.horizontal(|ui| {
                if ui.button("Cold fade").clicked() {
                    self.dissolve(ui.ctx(), true);
                }
                if ui.button("Warm fade").clicked() {
                    self.dissolve(ui.ctx(), false);
                }
                if ui.button("Reset").clicked() {
                    self.animation = None;
                    self.deleted = false;
                }
                ui.checkbox(&mut self.options.reduced_motion, "Reduce motion");
            });
        });
        // Cold preparation replaces the handle. Read its ID after the controls
        // so this frame never paints the old, freed texture.
        let texture = self.texture(ui.ctx(), false);
        let progress = self
            .animation
            .map_or(if self.deleted { 1. } else { 0. }, |time| {
                (time.elapsed().as_secs_f32() / 0.7).min(1.)
            });
        if self.animation.is_some() {
            if progress >= 1. || self.options.reduced_motion {
                self.animation = None;
                self.deleted = true;
            } else {
                ui.ctx().request_repaint();
            }
        }
        let progress = if self.deleted { 1. } else { progress };
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 380.), Sense::hover());
        let left = rect.center().x - 142.;
        let top = rect.top() + 12.;
        let eased = progress * progress * (3. - 2. * progress);
        let survivor =
            Rect::from_min_size(Pos2::new(left, top + eased * 184.), egui::vec2(284., 160.));
        let front = Rect::from_min_size(Pos2::new(left, top + 184.), survivor.size());
        egui::Image::new((texture, survivor.size()))
            .corner_radius(t.number("r-lg") as u8)
            .paint_at(ui, survivor);
        if progress < 1. {
            egui::Image::new((texture, front.size()))
                .corner_radius(t.number("r-lg") as u8)
                .tint(Color32::WHITE.gamma_multiply(1. - progress))
                .paint_at(ui, front);
        }
    }

    fn editor(&mut self, ui: &mut egui::Ui, t: &Tokens) {
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.zoom, 0.25..=3.).text("Zoom"));
            ui.add(egui::Slider::new(&mut self.rotation, -180.0..=180.0).text("Rotate"));
            if ui.button("Reset view").clicked() {
                self.zoom = 1.;
                self.rotation = 0.;
                self.pan = Vec2::ZERO;
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.annotation)
                .hint_text("Editable text / IME probe")
                .desired_width(f32::INFINITY),
        );
        ui.label(RichText::new("2048 × 1152 synthetic image · drag to pan · scroll to zoom · not an editor document").small().color(t.color("text-muted")));
        let texture = self.texture(ui.ctx(), false);
        let (viewport, response) =
            ui.allocate_exact_size(ui.available_size().max(egui::vec2(1., 1.)), Sense::drag());
        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.zoom = (self.zoom * (scroll * 0.002).exp()).clamp(0.25, 3.);
        }
        let painter = ui.painter_at(viewport);
        painter.rect_filled(viewport, t.number("r-lg"), t.color("surface-sunken"));
        let rect = Rect::from_center_size(
            viewport.center() + self.pan,
            egui::vec2(568., 320.) * self.zoom,
        );
        let mut mesh = egui::Mesh::with_texture(texture);
        mesh.add_rect_with_uv(rect, uv(), Color32::WHITE);
        mesh.rotate(
            egui::emath::Rot2::from_angle(self.rotation.to_radians()),
            rect.center(),
        );
        painter.add(mesh);
        // Separate synthetic layers exercise clipping and text over a GPU image.
        for (i, key) in ["theme-accent", "positive", "info"].iter().enumerate() {
            let offset = egui::vec2(30. + i as f32 * 70., 40. + i as f32 * 50.) * self.zoom;
            painter.rect_stroke(
                Rect::from_min_size(rect.min + offset, egui::vec2(170., 70.) * self.zoom),
                t.number("r-md"),
                Stroke::new(2., t.color(key)),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            &self.annotation,
            FontId::proportional(t.number("text-2xl") * self.zoom),
            t.color("glass-text"),
        );
    }

    fn exercise(&mut self, ctx: &egui::Context) {
        let start = Instant::now();
        match self.options.scene {
            Scene::Preferences => {
                self.preferences_state.exercise(self.cycle);
                self.options.appearance = if self.cycle.is_multiple_of(2) {
                    "light"
                } else {
                    "dark"
                }
                .into();
            }
            Scene::History => self.history_end = !self.history_end,
            Scene::Hud => self.paused = !self.paused,
            Scene::Preview => self.dissolve(ctx, self.cycle.is_multiple_of(2)),
            Scene::Editor => {
                self.zoom = if self.cycle.is_multiple_of(2) {
                    1.5
                } else {
                    0.75
                };
                self.rotation = self.cycle as f32 * 15.;
            }
            Scene::CaptureControls => {
                let (width, height) = self.window_display.overlay_size();
                self.capture_controls.exercise(
                    self.cycle,
                    captures_app::selection::Bounds { width, height },
                    |point| {
                        fixture_window_hit_test(
                            &self.window_targets,
                            &self.window_shell,
                            &self.window_display,
                            point,
                        )
                    },
                );
            }
            Scene::Region => self.region_selector.exercise(
                self.cycle,
                captures_app::selection::Bounds {
                    width: 1000.,
                    height: 720.,
                },
            ),
            Scene::Window => {
                let windows = &self.window_targets;
                let shell = &self.window_shell;
                let display = &self.window_display;
                self.window_selector.exercise(self.cycle, |point| {
                    fixture_window_hit_test(windows, shell, display, point)
                });
            }
            Scene::Idle | Scene::Countdown => unreachable!("exercises rejected by options"),
        }
        emit(
            "scripted-action",
            json!({"scene": self.options.scene.name(), "cycle": self.cycle,
            "milliseconds": start.elapsed().as_secs_f64() * 1000., "paused": self.paused,
            "appearance": self.options.appearance, "historyEnd": self.history_end,
            "zoom": self.zoom, "rotation": self.rotation,
            "regionSelection": self.region_selector.rect(),
            "windowSelection": window_selection_name(self.window_selector.hovered(), &self.window_targets),
            "note": "CPU mutation, not presentation or hardware input latency"}),
        );
        self.cycle += 1;
    }
}

impl eframe::App for Workbench {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.; 4]
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.preferences_state.set_presented(if self.options.live {
            self.live_preferences && !self.root_hidden
        } else {
            self.options.scene == Scene::Preferences
        });
        self.preferences_state.receive(ctx);
        while let Ok(result) = self.action_rx.try_recv() {
            self.action_error = result
                .err()
                .map(|error| format!("Output folder action failed: {error}"));
            if self.action_error.is_some() {
                self.show_root(ctx);
            }
        }
        if let Some(live) = &mut self.live {
            live.logic(ctx, frame);
        }
        self.sync_shortcuts(ctx);
        if self.options.live
            && self.tray.is_some()
            && !self.quitting
            && ctx.input(|input| input.viewport().close_requested())
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.root_hidden = true;
        }
        let root_focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
        let shortcuts_suspended =
            shortcuts_should_be_suspended(self.live_preferences, !self.root_hidden, root_focused);
        self.sync_shortcut_suspension(shortcuts_suspended);
        let shortcuts_enabled = self.live.as_ref().is_some_and(Live::can_launch_capture);
        let shortcut_action = self.shortcuts.as_ref().and_then(|shortcuts| {
            shortcuts.set_enabled(shortcuts_enabled);
            shortcuts.next_action()
        });
        if let (Some(action), Some(live)) = (shortcut_action, &mut self.live) {
            live.request_capture(match action {
                CaptureShortcut::NewCapture => CaptureRequest::NewCapture,
                CaptureShortcut::Region => CaptureRequest::Region,
                CaptureShortcut::Window => CaptureRequest::Window,
                CaptureShortcut::Display => CaptureRequest::Display,
            });
        }
        let mut tray_actions = Vec::new();
        if let Some(tray) = &self.tray {
            while let Some(action) = tray.try_recv() {
                tray_actions.push(action);
            }
        }
        for action in tray_actions {
            self.handle_tray_action(action, ctx);
        }
        if let Some(live) = &mut self.live {
            live.launch_requested_capture(ctx, frame, self.preferences_state.snapshot());
        }
        let quit_key = !self.preferences_state.is_recording_shortcut()
            && ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::Q)
                    || input.consume_key(egui::Modifiers::CTRL, egui::Key::Q)
            });
        if self.options.live && quit_key {
            self.quit(ctx);
        }
        let screenshot = self.screenshot_rx.try_recv().ok();
        if let (Some(image), Some(path)) = (screenshot, self.options.screenshot.as_ref()) {
            if let Err(error) = image::save_buffer(
                path,
                image.as_raw(),
                image.width() as u32,
                image.height() as u32,
                image::ColorType::Rgba8,
            ) {
                eprintln!("Screenshot failed: {error}");
                std::process::exit(1);
            }
            emit("screenshot-saved", json!({"path": self.options.screenshot}));
            self.screenshot_saved = true;
            if self.options.live {
                self.quit(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self
            .options
            .quit_after
            .is_some_and(|quit| self.started.elapsed() >= quit)
        {
            if self.options.screenshot.is_some() && !self.screenshot_saved {
                eprintln!("Screenshot did not finish before quit deadline");
                std::process::exit(1);
            }
            emit(
                "lifecycle-check",
                json!({
                    "scene": if self.options.live { "live" } else { self.options.scene.name() },
                    "nativeVisible": frame.winit_window().and_then(|window| window.is_visible())
                }),
            );
            if self.options.live {
                self.quit(ctx);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.options.exercise
            && self.cycle < 6
            && self.started.elapsed() >= exercise_at(self.cycle)
        {
            self.exercise(ctx);
        }
        self.schedule(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let start = Instant::now();
        let ctx = ui.ctx().clone();
        // Font/layout initialization and the settings load can require several
        // initial passes. Count settled work separately, without a sampling timer.
        if self.started.elapsed() >= Duration::from_secs(2) {
            self.settled_frames += 1;
        }
        if self.options.scene == Scene::Idle {
            // eframe 0.36.2 auto-shows the root after its first paint, even if
            // the builder requested hidden. Viewport commands run after that
            // show, so hide once here; deadlines continue through logic().
            if self.frames == 0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            self.frames += 1;
            return;
        }
        let t = self.tokens(&ctx);
        ui.set_style(ctx.style_of(ctx.theme()));
        if let Some(live) = &mut self.live {
            live.viewports(&ctx, &t, self.preferences_state.snapshot());
            if live.take_open_history_requested() {
                self.live_preferences = false;
            }
            if self.tray_error.is_some()
                || self.shortcut_error.is_some()
                || self.shortcut_suspension_error.is_some()
                || self.action_error.is_some()
            {
                egui::Panel::bottom("live-lifecycle-errors").show(ui, |ui| {
                    if let Some(error) = &self.tray_error {
                        ui.colored_label(t.color("theme-signal"), error);
                    }
                    if let Some(error) = &self.shortcut_error {
                        ui.colored_label(t.color("theme-signal"), error);
                    }
                    if let Some(error) = &self.shortcut_suspension_error {
                        ui.colored_label(t.color("theme-signal"), error);
                    }
                    if let Some(error) = &self.action_error {
                        ui.colored_label(t.color("theme-signal"), error);
                    }
                });
            }
            egui::Panel::top("live-navigation").show(ui, |ui| {
                if live.is_capturing() {
                    ui.disable();
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.live_preferences, false, "Capture workspace");
                    ui.selectable_value(&mut self.live_preferences, true, "Preferences");
                });
            });
            if self.live_preferences {
                egui::Panel::left("live-preferences-sidebar")
                    .exact_size(196.)
                    .show(ui, |ui| {
                        self.preferences_state.sidebar(ui, &t);
                    });
                egui::CentralPanel::default().show(ui, |ui| {
                    if self.preferences_state.ui(ui, &t) {
                        self.live_preferences = false;
                    }
                });
            } else {
                live.ui(ui, &t, frame, || self.preferences_state.snapshot());
            }
            // Navigation can change presentation after logic() has run. Apply
            // that event's focus boundary before returning to the native loop
            // so the next physical key sees the correct OS registration state.
            let root_focused = ctx.input(|input| input.viewport().focused.unwrap_or(false));
            let shortcuts_suspended = shortcuts_should_be_suspended(
                self.live_preferences,
                !self.root_hidden,
                root_focused,
            );
            self.sync_shortcut_suspension(shortcuts_suspended);
            if self.options.screenshot.is_some()
                && !self.screenshot_requested
                && self.started.elapsed() >= self.options.screenshot_after
            {
                self.request_screenshot(&ctx);
            }
            let elapsed = start.elapsed().as_secs_f64() * 1000.;
            self.ui_ms += elapsed;
            self.max_ui_ms = self.max_ui_ms.max(elapsed);
            self.frames += 1;
            return;
        }
        if !self.options.floating
            && !matches!(
                self.options.scene,
                Scene::CaptureControls | Scene::Region | Scene::Window
            )
        {
            egui::Panel::left("navigation")
                .exact_size(196.)
                .resizable(false)
                .frame(
                    egui::Frame::new()
                        .fill(t.color("surface-sunken"))
                        .inner_margin(t.number("s-5") as i8),
                )
                .show(ui, |ui| {
                    ui.add_space(t.number("s-5"));
                    ui.heading("Captures");
                    ui.add_space(t.number("s-8"));
                    if self.options.scene == Scene::Preferences {
                        self.preferences_state.sidebar(ui, &t);
                    } else {
                        for scene in Scene::VISIBLE {
                            if ui
                                .add_sized(
                                    [ui.available_width(), t.number("h-lg")],
                                    egui::Button::new(scene.title())
                                        .selected(scene == self.options.scene),
                                )
                                .clicked()
                            {
                                self.change_scene(scene);
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                        ui.label(
                            RichText::new("Capture engine not connected")
                                .small()
                                .color(t.color("text-muted")),
                        );
                        ui.label(
                            RichText::new("Native development build")
                                .small()
                                .color(t.color("text-muted")),
                        );
                    });
                });
        }
        let frame = egui::Frame::new()
            .fill(if self.options.floating {
                Color32::TRANSPARENT
            } else {
                t.color("surface-canvas")
            })
            .inner_margin(
                if matches!(
                    self.options.scene,
                    Scene::CaptureControls | Scene::Region | Scene::Window
                ) {
                    0
                } else {
                    t.number("s-8") as i8
                },
            );
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if !self.options.floating
                && !matches!(
                    self.options.scene,
                    Scene::Preferences | Scene::CaptureControls | Scene::Region | Scene::Window
                )
            {
                ui.heading(self.options.scene.title());
                ui.label(
                    RichText::new(
                        "Native rendering workbench · synthetic data, not functional parity",
                    )
                    .small()
                    .color(t.color("text-muted")),
                );
                ui.add_space(t.number("s-6"));
            } else if self.options.floating {
                glass(&t).show(ui, |ui| {
                    t.glass_controls(ui);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new("Move window").sense(Sense::drag()))
                            .drag_started()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                        }
                        if ui.button("Close").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            }
            match self.options.scene {
                Scene::Preferences => self.preferences(ui, &t),
                Scene::History => self.history(ui, &t),
                Scene::Hud => self.hud(ui, &t),
                Scene::Preview => self.preview(ui, &t),
                Scene::Editor => self.editor(ui, &t),
                Scene::CaptureControls => {
                    let texture = self.texture(ui.ctx(), false);
                    let texture = self.texture.as_ref().filter(|image| image.id() == texture);
                    let windows = &self.window_targets;
                    let shell = &self.window_shell;
                    let display = &self.window_display;
                    if let Some(action) = self.capture_controls.show(
                        ui,
                        &t,
                        crate::capture_controls::View {
                            panel_id: egui::Id::unique("capture-controls-fixture-toolbar"),
                            frozen: texture,
                            display,
                            displays: &self.capture_control_displays,
                            windows,
                            auto_start: false,
                        },
                        |point| fixture_window_hit_test(windows, shell, display, point),
                    ) {
                        emit(
                            "capture-controls-action",
                            json!({"action": format!("{action:?}")}),
                        );
                    }
                }
                Scene::Region => {
                    let texture = self.texture(ui.ctx(), false);
                    let texture = self.texture.as_ref().filter(|image| image.id() == texture);
                    if let Some(action) = self.region_selector.show(ui, &t, texture, false, None)
                        && let Some(event) =
                            apply_region_fixture_action(&mut self.region_selector, action)
                    {
                        match event {
                            RegionFixtureEvent::Confirm(rect) => {
                                emit("region-confirm", json!({"rect": rect, "capture": false}));
                            }
                            RegionFixtureEvent::Cancel => {
                                emit("region-cancel", json!({"selectionReset": true}));
                            }
                        }
                    }
                }
                Scene::Window => {
                    let texture = self.texture(ui.ctx(), false);
                    let texture = self.texture.as_ref().filter(|image| image.id() == texture);
                    let windows = &self.window_targets;
                    let shell = &self.window_shell;
                    let display = &self.window_display;
                    if let Some(action) = self.window_selector.show(
                        ui,
                        &t,
                        crate::window_selector::View {
                            frozen: texture,
                            display,
                            windows,
                            auto_start: false,
                        },
                        |point| fixture_window_hit_test(windows, shell, display, point),
                    ) {
                        match action {
                            crate::window_selector::Action::Confirm(target) => emit(
                                "window-confirm",
                                json!({
                                    "target": window_selection_name(Some(target), windows),
                                    "capture": false
                                }),
                            ),
                            crate::window_selector::Action::Cancel => {
                                self.window_selector.reset();
                                emit("window-cancel", json!({"selectionReset": true}));
                            }
                        }
                    }
                }
                Scene::Countdown => crate::countdown::show(ui, &t, 3),
                Scene::Idle => {}
            }
        });
        // Take only this native viewport's framebuffer; never capture the desktop.
        if self.options.screenshot.is_some()
            && !self.screenshot_requested
            && self.started.elapsed() >= self.options.screenshot_after
        {
            self.request_screenshot(&ctx);
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        self.frames += 1;
        self.ui_ms += elapsed;
        self.max_ui_ms = self.max_ui_ms.max(elapsed);
    }

    fn on_exit(&mut self) {
        self.preferences_state.flush();
        if let Some(live) = &mut self.live {
            live.flush();
        }
        self.shortcuts.take();
        self.tray.take();
        emit(
            "exit",
            json!({"uiPasses": self.frames, "totalUiConstructionWallMs": self.ui_ms,
            "uiPassesAfterTwoSeconds": self.settled_frames,
            "maxUiConstructionWallMs": self.max_ui_ms, "scriptedActions": self.cycle,
            "elapsedSeconds": self.started.elapsed().as_secs_f64(),
            "note": "UI construction only; not GPU presentation FPS"}),
        );
    }
}

fn glass(t: &Tokens) -> egui::Frame {
    egui::Frame::new()
        .fill(t.color("glass-strong"))
        .stroke(Stroke::new(1., t.color("glass-border")))
        .corner_radius(t.number("r-xl") as u8)
        .inner_margin(t.number("s-6") as i8)
}

fn uv() -> Rect {
    Rect::from_min_max(Pos2::ZERO, Pos2::new(1., 1.))
}
fn exercise_at(cycle: usize) -> Duration {
    Duration::from_secs(2 + cycle as u64 * 4)
}
fn history_kind(row: usize) -> &'static str {
    match row % 3 {
        0 => "Video",
        1 => "Screenshot",
        _ => "GIF",
    }
}
fn history_rows(count: usize, filter: usize) -> Vec<usize> {
    (0..count)
        .filter(|row| match filter {
            1 => row % 3 == 1,
            2 => row % 3 == 0,
            3 => row % 3 == 2,
            _ => true,
        })
        .collect()
}

#[derive(Debug, PartialEq)]
enum RegionFixtureEvent {
    Confirm(captures_app::selection::Rect),
    Cancel,
}

fn apply_region_fixture_action(
    selector: &mut crate::selector::Selector,
    action: crate::selector::Action,
) -> Option<RegionFixtureEvent> {
    match action {
        crate::selector::Action::Confirm => selector.rect().map(RegionFixtureEvent::Confirm),
        crate::selector::Action::Cancel => {
            selector.reset();
            Some(RegionFixtureEvent::Cancel)
        }
    }
}

fn window_fixture() -> (
    captures_capture::DisplayDescriptor,
    Vec<captures_capture::WindowDescriptor>,
    Vec<captures_capture::WindowDescriptor>,
) {
    let display = captures_capture::DisplayDescriptor {
        id: "fixture-display".into(),
        name: "Fixture display".into(),
        x: -100,
        y: 50,
        width: 1000,
        height: 720,
        scale_factor: 1.,
        is_primary: true,
    };
    let target = |id: &str, title: &str, app: &str, z_order, x, y, width, height| {
        captures_capture::WindowDescriptor {
            id: id.into(),
            title: title.into(),
            app_name: Some(app.into()),
            z_order,
            x,
            y,
            width,
            height,
            display_id: display.id.clone(),
            corner_radius: Some(10.),
        }
    };
    let windows = vec![
        target(
            "project",
            "Project board — Captures",
            "Browser",
            10,
            20,
            140,
            620,
            420,
        ),
        target(
            "export",
            "Export settings",
            "Captures",
            20,
            380,
            230,
            330,
            240,
        ),
        target(
            "terminal",
            "Release checklist",
            "Terminal",
            5,
            560,
            420,
            280,
            220,
        ),
    ];
    let shell = vec![target(
        "shell",
        "Desktop shell",
        "System",
        30,
        -100,
        50,
        1000,
        42,
    )];
    (display, windows, shell)
}

fn fixture_window_hit_test(
    windows: &[captures_capture::WindowDescriptor],
    shell: &[captures_capture::WindowDescriptor],
    display: &captures_capture::DisplayDescriptor,
    point: captures_app::selection::Point,
) -> Option<usize> {
    captures_app::window::target_index_at_point(
        windows,
        shell,
        point,
        captures_app::selection::Point {
            x: f64::from(display.x),
            y: f64::from(display.y),
        },
        1.,
    )
}

fn window_selection_name(
    target: Option<crate::window_selector::SelectionTarget>,
    windows: &[captures_capture::WindowDescriptor],
) -> Option<&str> {
    match target {
        Some(crate::window_selector::SelectionTarget::Display) => Some("display"),
        Some(crate::window_selector::SelectionTarget::Window(index)) => {
            windows.get(index).map(|window| window.id.as_str())
        }
        None => None,
    }
}

fn shortcuts_should_be_suspended(
    preferences_selected: bool,
    root_visible: bool,
    root_focused: bool,
) -> bool {
    preferences_selected && root_visible && root_focused
}

fn fixture_image([width, height]: [usize; 2]) -> egui::ColorImage {
    // Asymmetric synthetic content, same layout as the AppKit fixture. No file
    // reads, screen capture, personal images, or per-frame texture allocation.
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let x = x as f32 / width as f32 * 284.;
            let y = (1. - y as f32 / height as f32) * 160.;
            let rgb = if (x - 233.).powi(2) + (y - 123.).powi(2) < 225. {
                [245, 189, 74]
            } else if (38.0..113.).contains(&x) && (50.0..130.).contains(&y) {
                [217, 84, 105]
            } else if y < 45. {
                [51, 122, 102]
            } else {
                [31, 69, 107]
            };
            pixels.push(Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
        }
    }
    egui::ColorImage::new([width, height], pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filtered_history_preserves_original_ids_and_boundaries() {
        assert_eq!(history_rows(8, 1), vec![1, 4, 7]);
        assert_eq!(history_rows(8, 2), vec![0, 3, 6]);
        assert_eq!(history_rows(8, 3), vec![2, 5]);
        assert!(history_rows(0, 0).is_empty());
        assert_eq!(history_rows(1, 2), vec![0]);
        assert!(history_rows(1, 1).is_empty());
    }

    #[test]
    fn shortcuts_suspend_focused_preferences_but_not_hidden_or_unfocused_preferences() {
        assert!(shortcuts_should_be_suspended(true, true, true));
        assert!(!shortcuts_should_be_suspended(true, false, true));
        assert!(!shortcuts_should_be_suspended(true, true, false));
        assert!(!shortcuts_should_be_suspended(false, true, true));
    }
    #[test]
    fn image_has_top_right_sun_and_bottom_green_strip() {
        let image = fixture_image([284, 160]);
        assert_eq!(image[(233, 37)], Color32::from_rgb(245, 189, 74));
        assert_eq!(image[(40, 60)], Color32::from_rgb(217, 84, 105));
        assert_eq!(image[(10, 150)], Color32::from_rgb(51, 122, 102));
        assert_eq!(image[(10, 10)], Color32::from_rgb(31, 69, 107));
    }

    #[test]
    fn region_fixture_reports_confirmation_and_resets_on_cancel() {
        let bounds = captures_app::selection::Bounds {
            width: 1000.,
            height: 720.,
        };
        let mut selector = crate::selector::Selector::default();
        selector.exercise(0, bounds);
        let rect = selector.rect().unwrap();
        assert_eq!(
            apply_region_fixture_action(&mut selector, crate::selector::Action::Confirm),
            Some(RegionFixtureEvent::Confirm(rect))
        );
        assert_eq!(selector.rect(), Some(rect));
        assert_eq!(
            apply_region_fixture_action(&mut selector, crate::selector::Action::Cancel),
            Some(RegionFixtureEvent::Cancel)
        );
        assert_eq!(selector.rect(), None);
    }

    #[test]
    fn window_fixture_uses_shared_frontmost_and_shell_hit_testing() {
        let (display, windows, shell) = window_fixture();
        assert_eq!(
            fixture_window_hit_test(
                &windows,
                &shell,
                &display,
                captures_app::selection::Point { x: 210., y: 130. }
            ),
            Some(0)
        );
        assert_eq!(
            fixture_window_hit_test(
                &windows,
                &shell,
                &display,
                captures_app::selection::Point { x: 540., y: 250. }
            ),
            Some(1),
            "overlap must select the frontmost window"
        );
        assert_eq!(
            fixture_window_hit_test(
                &windows,
                &shell,
                &display,
                captures_app::selection::Point { x: 500., y: 20. }
            ),
            None,
            "shell is an entire-display target"
        );
        assert_eq!(
            fixture_window_hit_test(
                &windows,
                &shell,
                &display,
                captures_app::selection::Point { x: 920., y: 680. }
            ),
            None,
            "empty desktop is an entire-display target"
        );
        assert_eq!(
            fixture_window_hit_test(
                &windows,
                &shell,
                &display,
                captures_app::selection::Point { x: 800., y: 550. }
            ),
            Some(2),
            "terminal-only area must avoid the higher-z project window"
        );
    }
}
