//! Output cards use shipping menu copy and the same accepted preview/save
//! transaction as the timeline. Draft values stay on the UI thread.
use captures_app::controls::{number, range};
use captures_app::recording_editor_ui::ResolutionChoice;
use captures_media::{AudioEdit, EditSpec, ExportFormat, ExportSpec, QualityPreset};

use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Values {
    pub resolution: ResolutionChoice,
    pub custom: (u32, u32),
    pub fps: u16,
    pub maximum_width: u32,
    pub quality: QualityPreset,
    pub audio: AudioEdit,
}

impl Values {
    pub fn new(width: u32, height: u32, settings: &captures_settings::RecordingSettings) -> Self {
        Self {
            resolution: ResolutionChoice::Original,
            custom: (width, height),
            fps: settings.gif_fps,
            maximum_width: settings.gif_max_width,
            quality: ui::DEFAULT_COMPRESS_PRESET,
            audio: AudioEdit::default(),
        }
    }

    /// Shipping editorOutputDimensions, dimensionsAtMaximumWidth and
    /// gifMaxColorsForQuality. Presets never upscale; custom is exact/even.
    pub fn specs(
        &self,
        width: u32,
        height: u32,
        mut edit: EditSpec,
        mut export: ExportSpec,
    ) -> (EditSpec, ExportSpec) {
        let even = |v: f64| {
            let v = v.round().max(2.) as u32;
            v - v % 2
        };
        let (width, height) = edit.crop.map_or((width, height), |c| (c.width, c.height));
        let limit = match self.resolution {
            ResolutionChoice::P1080 => 1080.,
            ResolutionChoice::P720 => 720.,
            _ => f64::from(height),
        };
        let scale = (limit / f64::from(height)).min(1.);
        let mut size = if self.resolution == ResolutionChoice::Custom {
            (even(self.custom.0.into()), even(self.custom.1.into()))
        } else {
            (
                even(f64::from(width) * scale),
                even(f64::from(height) * scale),
            )
        };
        let gif = export.format == ExportFormat::Gif;
        if gif && size.0 > self.maximum_width {
            let scale = f64::from(self.maximum_width) / f64::from(size.0);
            size = (
                even(self.maximum_width.into()),
                even(f64::from(size.1) * scale),
            );
        }
        let resized = gif || self.resolution != ResolutionChoice::Original;
        edit.output_width = resized.then_some(size.0);
        edit.output_height = resized.then_some(size.1);
        let (system, mic) = (
            edit.audio.source_has_system_audio,
            edit.audio.source_has_microphone_audio,
        );
        edit.audio = self.audio.clone();
        edit.audio.source_has_system_audio = system;
        edit.audio.source_has_microphone_audio = mic;
        if gif {
            edit.audio.mute_system_audio = true;
            edit.audio.mute_microphone = true;
            export.quality = self.quality;
            export.frames_per_second = Some(self.fps);
            export.gif_max_colors = Some(match self.quality {
                QualityPreset::Tiny => 64,
                QualityPreset::Small => 96,
                QualityPreset::Standard => 128,
                _ => 256,
            });
        }
        (edit, export)
    }
}

pub(super) struct Output {
    pub values: Values,
    pub fields: [TextField; 2],
    pub cards: Vec<(Rect, &'static str)>,
    pub slider: Option<Target>,
    pub due: Option<std::time::Instant>,
}

impl Output {
    pub fn new(settings: &captures_settings::RecordingSettings) -> Self {
        Self {
            values: Values::new(2, 2, settings),
            fields: std::array::from_fn(|_| TextField::new(Style::Option, "")),
            cards: Vec::new(),
            slider: None,
            due: None,
        }
    }
}

impl Target {
    pub fn is_output(self) -> bool {
        matches!(
            self,
            Self::Resolution
                | Self::Width
                | Self::Height
                | Self::Fps
                | Self::GifWidth
                | Self::Quality
                | Self::System
                | Self::SystemVolume
                | Self::Microphone
                | Self::MicrophoneVolume
                | Self::Mono
        )
    }
    pub fn is_select(self) -> bool {
        matches!(
            self,
            Self::Resolution | Self::Fps | Self::GifWidth | Self::Quality
        )
    }
    pub fn is_number(self) -> bool {
        matches!(self, Self::Width | Self::Height)
    }
    pub fn is_volume(self) -> bool {
        matches!(self, Self::SystemVolume | Self::MicrophoneVolume)
    }
    pub fn is_check(self) -> bool {
        matches!(self, Self::System | Self::Microphone | Self::Mono)
    }
    fn field_index(self) -> usize {
        usize::from(self == Self::Height)
    }
}

impl Recording {
    pub(super) fn output_shown(&self, target: Target) -> bool {
        let Some(p) = &self.presented else {
            return false;
        };
        let gif = p.export.format == ExportFormat::Gif;
        let audio = &p.edit.audio;
        match target {
            Target::Width | Target::Height => {
                self.output.values.resolution == ResolutionChoice::Custom
            }
            Target::Fps | Target::GifWidth | Target::Quality => gif,
            Target::System | Target::SystemVolume => !gif && audio.source_has_system_audio,
            Target::Microphone | Target::MicrophoneVolume => {
                !gif && audio.source_has_microphone_audio
            }
            Target::Mono => {
                !gif && (audio.source_has_system_audio || audio.source_has_microphone_audio)
            }
            _ => true,
        }
    }

    pub(super) fn reset_output(&mut self) {
        if let Some(p) = &self.presented {
            self.output.values = p.output.clone();
        }
        self.sync_output_fields();
        for target in TARGETS.into_iter().filter(|t| t.is_output()) {
            self.events.push(if target.is_volume() {
                Event::Range(target)
            } else if target.is_check() {
                Event::Toggle(target)
            } else {
                Event::OutputValue(target)
            });
        }
    }

    fn sync_output_fields(&mut self) {
        for (i, value) in [self.output.values.custom.0, self.output.values.custom.1]
            .into_iter()
            .enumerate()
        {
            self.output.fields[i].editor.set_text(&value.to_string());
        }
    }

    pub fn output_value(&self, target: Target) -> String {
        if target.is_number() {
            return String::from_utf16_lossy(
                self.output.fields[target.field_index()].editor.units(),
            );
        }
        if target == Target::Fps {
            return format!("{} FPS", self.output.values.fps);
        }
        if target == Target::GifWidth {
            return format!("{} px", self.output.values.maximum_width);
        }
        self.output_choices(target)
            .get(self.output_selected(target))
            .map_or(String::new(), |c| c.label.clone())
    }

    pub fn output_choices(&self, target: Target) -> Vec<ui::Choice> {
        let Some(p) = &self.presented else {
            return vec![];
        };
        let (w, h) = p
            .edit
            .crop
            .map_or((p.width, p.height), |c| (c.width, c.height));
        let menus = ui::menus(p.export.format == ExportFormat::Gif, w, h);
        match target {
            Target::Resolution => menus.resolutions,
            Target::Fps => menus.gif_frame_rates,
            Target::GifWidth => menus.gif_maximum_widths,
            Target::Quality => menus.quality_presets,
            _ => vec![],
        }
    }

    pub fn output_selected(&self, target: Target) -> usize {
        let v = &self.output.values;
        match target {
            Target::Resolution => ResolutionChoice::ALL
                .iter()
                .position(|&r| r == v.resolution),
            Target::Fps => ui::GIF_FRAME_RATES.iter().position(|&r| r == v.fps),
            Target::GifWidth => ui::GIF_MAXIMUM_WIDTHS
                .iter()
                .position(|&r| r == v.maximum_width),
            Target::Quality => ui::COMPRESS_PRESETS.iter().position(|&r| r == v.quality),
            _ => None,
        }
        .unwrap_or(0)
    }

    pub fn choose_output(&mut self, target: Target, index: usize) {
        if !self.enabled(target) || index >= self.output_choices(target).len() {
            return;
        }
        let v = &mut self.output.values;
        match target {
            Target::Resolution => v.resolution = ResolutionChoice::ALL[index],
            Target::Fps => v.fps = ui::GIF_FRAME_RATES[index],
            Target::GifWidth => v.maximum_width = ui::GIF_MAXIMUM_WIDTHS[index],
            Target::Quality => v.quality = ui::COMPRESS_PRESETS[index],
            _ => return,
        }
        self.close_output_menu();
        self.output_changed(target);
    }

    pub(super) fn output_changed(&mut self, target: Target) {
        self.saved = None;
        self.saved_fingerprint = None;
        self.timeline.resume |= self.preview.playing;
        self.stop_preview();
        // Invalidate any older decode immediately, even during the debounce.
        self.timeline.generation += 1;
        self.timeline.pending = true;
        self.output.due = Some(
            std::time::Instant::now() + std::time::Duration::from_millis(ui::LIVE_APPLY_DELAY_MS),
        );
        if !self.hwnd.is_invalid() {
            unsafe {
                SetTimer(
                    Some(self.hwnd),
                    OUTPUT_TIMER,
                    ui::LIVE_APPLY_DELAY_MS as u32,
                    None,
                );
            }
        }
        self.notify("", false);
        self.events.push(if target.is_volume() {
            Event::Range(target)
        } else if target.is_check() {
            Event::Toggle(target)
        } else {
            Event::OutputValue(target)
        });
        let _ = self.layout();
        self.refresh_footer();
    }

    pub(super) fn apply_output_due(&mut self) {
        if self
            .output
            .due
            .is_some_and(|due| due <= std::time::Instant::now())
        {
            self.output.due = None;
            unsafe {
                let _ = KillTimer(Some(self.hwnd), OUTPUT_TIMER);
            }
            if !self.closing {
                self.seek_timeline(self.preview.position_ms);
            }
        }
    }

    pub fn volume_range(&self, target: Target) -> Option<(f64, f64, f64)> {
        let a = &self.output.values.audio;
        let value = match target {
            Target::SystemVolume => a.system_volume,
            Target::MicrophoneVolume => a.microphone_volume,
            _ => return None,
        };
        Some(((f64::from(value) * 100.).round(), 0., 200.))
    }

    pub fn set_volume(&mut self, target: Target, value: f64) {
        if !self.enabled(target) || !value.is_finite() || !target.is_volume() {
            return;
        }
        let value = (value.round().clamp(0., 200.) / 100.) as f32;
        let a = &mut self.output.values.audio;
        let current = if target == Target::SystemVolume {
            &mut a.system_volume
        } else {
            &mut a.microphone_volume
        };
        if *current == value {
            return;
        }
        *current = value;
        self.output_changed(target);
    }

    pub(super) fn activate_output(&mut self, target: Target) {
        if target.is_select() {
            self.toggle_output_menu(target);
            return;
        }
        let a = &mut self.output.values.audio;
        match target {
            Target::System => a.mute_system_audio = !a.mute_system_audio,
            Target::Microphone => a.mute_microphone = !a.mute_microphone,
            Target::Mono => a.mono_output = !a.mono_output,
            _ => return,
        }
        self.output_changed(target);
    }

    pub fn set_output_number(&mut self, target: Target, text: &str) {
        if !target.is_number() || !self.enabled(target) {
            return;
        }
        let c = self.output.fields[target.field_index()]
            .editor
            .set_text(text);
        self.output_edited(target, c);
    }

    fn output_edited(&mut self, target: Target, change: Change) {
        let i = target.field_index();
        let _ = self.output.fields[i].apply(&self.g, &self.t, change);
        self.dirty[slot(target)] = true;
        self.place_caret();
        if change.text
            && let Some(value) = self
                .output_value(target)
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v >= 2. && *v <= u32::MAX as f64)
        {
            let value = value.clamp(2., u32::MAX as f64).round() as u32;
            if target == Target::Width {
                self.output.values.custom.0 = value;
            } else {
                self.output.values.custom.1 = value;
            }
            self.output_changed(target);
        }
    }

    pub(super) fn commit_output_number(&mut self) {
        let Some(t) = self.focus.filter(|t| t.is_number()) else {
            return;
        };
        let current = if t == Target::Width {
            self.output.values.custom.0
        } else {
            self.output.values.custom.1
        };
        let value = self
            .output_value(t)
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map_or(current, |v| v.clamp(2., u32::MAX as f64).round() as u32);
        if value != current {
            if t == Target::Width {
                self.output.values.custom.0 = value;
            } else {
                self.output.values.custom.1 = value;
            }
            self.output_changed(t);
        }
        self.output.fields[t.field_index()]
            .editor
            .set_text(&value.to_string());
        let r = self.target_rect(t);
        let _ = self.output.fields[t.field_index()].relayout(
            &self.g,
            &self.t,
            r.w - 26. - 2. * self.t.n("s-4"),
        );
        self.dirty[slot(t)] = true;
    }

    pub(super) fn output_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        if self.output_menu_key(key) {
            return true;
        }
        let Some(t) = self.focus.filter(|&t| self.enabled(t)) else {
            return false;
        };
        if t.is_volume() {
            let (v, _, _) = self.volume_range(t).expect("volume");
            let next = match key {
                VK_LEFT | VK_DOWN => v - 1.,
                VK_RIGHT | VK_UP => v + 1.,
                VK_HOME => 0.,
                VK_END => 200.,
                VK_PRIOR => v + 20.,
                VK_NEXT => v - 20.,
                _ => return false,
            };
            self.set_volume(t, next);
            return true;
        }
        if !t.is_number() || key == VK_TAB {
            return false;
        }
        if matches!(key, VK_UP | VK_DOWN) {
            self.step_output_number(t, key == VK_UP);
            return true;
        }
        if matches!(key, VK_RETURN | VK_ESCAPE) {
            self.commit_output_number();
            self.place_caret();
            return true;
        }
        let field = &mut self.output.fields[t.field_index()];
        let edit = match key {
            VK_LEFT => Some(Key::Left),
            VK_RIGHT => Some(Key::Right),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_BACK => Some(Key::Backspace),
            VK_DELETE => Some(Key::Delete),
            VK_A if ctrl => Some(Key::SelectAll),
            VK_Z if ctrl => Some(if shift { Key::Redo } else { Key::Undo }),
            VK_Y if ctrl => Some(Key::Redo),
            _ => None,
        };
        let c = if let Some(k) = edit {
            field.editor.key(k, shift, ctrl)
        } else if ctrl && key == VK_C {
            textfield::copy(self.hwnd, &field.editor.selected_text());
            Change::default()
        } else if ctrl && key == VK_X {
            if let Some(text) = field.editor.cut() {
                textfield::copy(self.hwnd, &text);
                Change {
                    text: true,
                    caret: true,
                }
            } else {
                Change::default()
            }
        } else if ctrl && key == VK_V {
            textfield::paste(self.hwnd).map_or(Change::default(), |s| field.editor.insert(&s))
        } else {
            return false;
        };
        self.output_edited(t, c);
        true
    }

    pub(super) fn output_char(&mut self, unit: u16) -> bool {
        let Some(t) = self.focus.filter(|&t| t.is_number() && self.enabled(t)) else {
            return false;
        };
        if unit >= 32 {
            let c = self.output.fields[t.field_index()]
                .editor
                .insert(&String::from_utf16_lossy(&[unit]));
            self.output_edited(t, c);
        }
        true
    }

    fn step_output_number(&mut self, t: Target, up: bool) {
        let mut text = self.output_value(t);
        if text.trim().is_empty() {
            text = if t == Target::Width {
                self.output.values.custom.0
            } else {
                self.output.values.custom.1
            }
            .to_string();
        }
        let v = number::step_from(&text, 1., up, Some(2.), Some(u32::MAX as f64));
        self.set_output_number(t, &v.to_string());
    }

    pub(super) fn output_pointer(&mut self, msg: u32, x: f32, y: f32) -> bool {
        if self.output_menu_pointer(msg, x, y) {
            return true;
        }
        if let Some(t) = self.output.slider {
            if msg == WM_MOUSEMOVE || msg == WM_LBUTTONDOWN {
                let r = self.volume_track(t);
                self.set_volume(t, range::value_at(((x - r.x) / r.w).into(), 0., 200., 1.));
            }
            if msg == WM_LBUTTONUP {
                self.output.slider = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            return true;
        }
        if self.dragging && self.focus.is_some_and(Target::is_number) {
            let t = self.focus.expect("field");
            if msg == WM_MOUSEMOVE {
                let at = self.output.fields[t.field_index()]
                    .hit(x - self.target_rect(t).x - self.t.n("s-4"));
                let c = self.output.fields[t.field_index()].editor.move_to(at, true);
                self.output_edited(t, c);
            }
            if msg == WM_LBUTTONUP {
                self.dragging = false;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            return true;
        }
        if !matches!(msg, WM_LBUTTONDOWN | WM_LBUTTONDBLCLK) {
            return false;
        }
        let Some(t) = self.hit(x, y).filter(|&t| self.enabled(t)) else {
            return false;
        };
        if !t.is_volume() && !t.is_number() {
            return false;
        }
        self.set_focus(Some(t), false);
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
            SetCapture(self.hwnd);
        }
        let r = self.target_rect(t);
        if t.is_volume() {
            self.output.slider = Some(t);
            let track = self.volume_track(t);
            self.set_volume(
                t,
                range::value_at(((x - track.x) / track.w).into(), 0., 200., 1.),
            );
        } else if x >= r.x + r.w - 26. {
            self.step_output_number(t, y < r.y + r.h / 2.);
            unsafe {
                let _ = ReleaseCapture();
            }
        } else {
            let at = self.output.fields[t.field_index()].hit(x - r.x - self.t.n("s-4"));
            let f = &mut self.output.fields[t.field_index()];
            let c = if msg == WM_LBUTTONDBLCLK {
                let (a, b) = f.editor.word_at(at);
                f.editor.select(a, b)
            } else {
                f.editor.move_to(at, false)
            };
            self.output_edited(t, c);
            self.dragging = true;
        }
        true
    }

    pub(super) fn output_focus(&mut self, target: Option<Target>, visible: bool) {
        if target != self.focus {
            self.commit_output_number();
        }
        if let Some(t) = target.filter(|t| t.is_number()) {
            if visible {
                self.output.fields[t.field_index()].editor.select_all();
            }
            self.output.fields[t.field_index()].caret_moved();
        }
    }

    pub(super) fn output_caret(&self) -> Option<Rect> {
        let t = self.focus.filter(|t| t.is_number())?;
        let r = self.target_rect(t);
        let (x, y, h) = self.output.fields[t.field_index()].caret_box(self.t.n("s-4"), r.h);
        Some(Rect::new(r.x + x, r.y + y, 1., h))
    }

    pub(super) fn output_caret_visible(&self) -> bool {
        self.focus
            .filter(|&t| t.is_number() && self.enabled(t))
            .is_some_and(|t| {
                let (a, b) = self.output.fields[t.field_index()].editor.selection();
                a == b
            })
    }

    pub(super) fn layout_output(&mut self) {
        let track = self.rects[TIMELINE];
        let gap = self.t.n("s-5");
        let pad = self.t.n("s-6") + 1.;
        let h = self.t.n("h-md");
        let mut top = track.y + track.h + gap;
        self.output.cards.clear();
        for t in TARGETS.into_iter().filter(|t| t.is_output()) {
            self.rects[slot(t)] = Rect::default();
        }
        let field_top = pad + 18. + gap + 18. + self.t.n("s-3");
        if self.output_shown(Target::Fps) {
            let ch = field_top + h + pad;
            self.output
                .cards
                .push((Rect::new(track.x, top, track.w, ch), "GIF settings"));
            let w = (track.w - 2. * pad - self.t.n("s-3")) / 2.;
            for (i, t) in [Target::Fps, Target::GifWidth].into_iter().enumerate() {
                self.rects[slot(t)] = Rect::new(
                    track.x + pad + i as f32 * (w + self.t.n("s-3")),
                    top + field_top,
                    w,
                    h,
                );
            }
            top += ch + gap;
        }
        let quality = self.output_shown(Target::Quality);
        let two = quality && self.width > 820.;
        let card_w = if two { (track.w - gap) / 2. } else { track.w };
        let extra = if self.output_shown(Target::Width) {
            self.t.n("s-4") + 16. + self.t.n("s-2") + h
        } else {
            0.
        };
        let ch = field_top + h + extra + pad;
        self.output
            .cards
            .push((Rect::new(track.x, top, card_w, ch), "Crop & size"));
        self.rects[slot(Target::Resolution)] =
            Rect::new(track.x + pad, top + field_top, card_w - 2. * pad, h);
        let w = (card_w - 2. * pad - self.t.n("s-3")) / 2.;
        for (i, t) in [Target::Width, Target::Height].into_iter().enumerate() {
            self.rects[slot(t)] = Rect::new(
                track.x + pad + i as f32 * (w + self.t.n("s-3")),
                top + ch - pad - h,
                w,
                h,
            );
            let _ =
                self.output.fields[i].relayout(&self.g, &self.t, w - 26. - 2. * self.t.n("s-4"));
        }
        if quality {
            let x = if two { track.x + card_w + gap } else { track.x };
            let y = if two { top } else { top + ch + gap };
            let qh = field_top + h + pad;
            self.output.cards.push((
                Rect::new(x, y, card_w, if two { ch } else { qh }),
                "Save quality",
            ));
            self.rects[slot(Target::Quality)] =
                Rect::new(x + pad, y + field_top, (card_w - 2. * pad).min(430.), h);
            if !two {
                top += qh + gap;
            }
        }
        top += ch + gap;
        if self.output_shown(Target::Mono) {
            let mut y = top + pad + 18. + gap;
            for (check, volume) in [
                (Target::System, Target::SystemVolume),
                (Target::Microphone, Target::MicrophoneVolume),
            ] {
                if !self.output_shown(check) {
                    continue;
                }
                self.rects[slot(check)] = Rect::new(track.x + pad, y, 130., 36.);
                self.rects[slot(volume)] = Rect::new(
                    track.x + pad + 130. + gap,
                    y,
                    track.w - 2. * pad - 130. - gap,
                    36.,
                );
                y += 36. + self.t.n("s-4");
            }
            self.rects[slot(Target::Mono)] = Rect::new(track.x + pad, y, track.w - 2. * pad, 24.);
            self.output.cards.push((
                Rect::new(track.x, top, track.w, y + 24. + pad - top),
                "Audio",
            ));
        }
        let bottom = self
            .output
            .cards
            .iter()
            .map(|(r, _)| r.y + r.h)
            .fold(top - gap, f32::max);
        self.rects[OUTPUT] = Rect::new(
            track.x,
            track.y + track.h + gap,
            track.w,
            bottom - (track.y + track.h + gap),
        );
        self.place_caret();
        self.place_output_menu();
        self.dirty[OUTPUT] = true;
    }

    pub(super) fn draw_output(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let t = &self.t;
        let base = self.rects[OUTPUT];
        for (r, title) in &self.output.cards {
            let (x, y) = (r.x - base.x, r.y - base.y);
            let b = gfx::rect(x, y, x + r.w, y + r.h);
            gfx::shadow(dc, b, t.n("r-xl"), t.shadow("shadow-sm"))?;
            gfx::fill_round(dc, b, t.n("r-xl"), t.color("surface-raised"))?;
            gfx::border(dc, b, t.n("r-xl"), 1., t.color("border-subtle"))?;
            self.draw_text(
                dc,
                Style::CardTitle,
                title,
                Rect::new(
                    x + t.n("s-6") + 1.,
                    y + t.n("s-6") + 1.,
                    r.w - 2. * (t.n("s-6") + 1.),
                    18.,
                ),
                "text",
                false,
            )?;
        }
        for (target, label) in [
            (Target::Resolution, "Output resolution"),
            (Target::Width, "Width"),
            (Target::Height, "Height"),
            (Target::Fps, "Frame rate"),
            (Target::GifWidth, "Maximum width"),
            (Target::Quality, "Quality"),
        ] {
            if !self.shown(target) {
                continue;
            }
            let r = self.target_rect(target);
            self.draw_text(
                dc,
                Style::Option,
                label,
                Rect::new(r.x - base.x, r.y - base.y - 18. - t.n("s-3"), r.w, 18.),
                "text-subtle",
                false,
            )?;
        }
        Ok(())
    }

    fn volume_track(&self, target: Target) -> Rect {
        let r = self.target_rect(target);
        Rect::new(r.x + 7., r.y + 24., r.w - 14., 4.)
    }

    pub(super) fn draw_output_control(
        &self,
        dc: &ID2D1DeviceContext,
        target: Target,
    ) -> Result<()> {
        let r = self.target_rect(target);
        let t = &self.t;
        let b = gfx::rect(0., 0., r.w, r.h);
        let focus = self.focus == Some(target);
        if target.is_select() || target.is_number() {
            gfx::fill_round(dc, b, t.n("r-md"), t.color("surface-field"))?;
            gfx::border(
                dc,
                b,
                t.n("r-md"),
                1.,
                t.color(if focus {
                    "theme-accent"
                } else if self.hover == Some(target) {
                    "border-strong"
                } else {
                    "control-border"
                }),
            )?;
            if target.is_number() {
                self.output.fields[target.field_index()].draw(
                    dc,
                    t,
                    (t.n("s-4"), r.w - 26. - 2. * t.n("s-4"), r.h),
                    focus,
                )?;
                gfx::fill(
                    dc,
                    gfx::rect(r.w - 26., 1., r.w - 1., r.h - 1.),
                    t.color("surface-hover"),
                )?;
                gfx::fill(
                    dc,
                    gfx::rect(r.w - 26., 1., r.w - 25., r.h - 1.),
                    t.color("border-subtle"),
                )?;
                gfx::fill(
                    dc,
                    gfx::rect(r.w - 26., r.h / 2., r.w - 1., r.h / 2. + 1.),
                    t.color("border-subtle"),
                )?;
                for (icon, y) in [
                    ("chevron-up", r.h / 4. - 5.),
                    ("chevron-down", r.h * 0.75 - 5.),
                ] {
                    gfx::icon(
                        dc,
                        &self.g.factory,
                        icon,
                        &[],
                        (r.w - 18., y, 10.),
                        1.8,
                        t.color("text-subtle"),
                    )?;
                }
            } else {
                self.draw_text(
                    dc,
                    Style::Option,
                    &self.output_value(target),
                    Rect::new(t.n("s-4"), 0., r.w - 40., r.h),
                    "text",
                    false,
                )?;
                gfx::icon(
                    dc,
                    &self.g.factory,
                    "chevron-down",
                    &[],
                    (r.w - 24., (r.h - 12.) / 2., 12.),
                    1.8,
                    t.color("text-subtle"),
                )?;
            }
        } else if target.is_check() {
            let y = (r.h - 16.) / 2.;
            let checked = self.toggled(target).unwrap_or(false);
            gfx::fill_round(
                dc,
                gfx::rect(0., y, 16., y + 16.),
                t.n("r-xs"),
                t.color(if checked {
                    "theme-accent"
                } else {
                    "surface-field"
                }),
            )?;
            if checked {
                gfx::icon(
                    dc,
                    &self.g.factory,
                    "check",
                    &[],
                    (2., y + 2., 12.),
                    2.,
                    t.color("theme-accent-ink"),
                )?;
            } else {
                gfx::border(
                    dc,
                    gfx::rect(0., y, 16., y + 16.),
                    t.n("r-xs"),
                    1.,
                    t.color("control-border"),
                )?;
            }
            self.draw_text(
                dc,
                Style::Option,
                target.label(),
                Rect::new(16. + t.n("s-3"), 0., r.w - 16. - t.n("s-3"), r.h),
                "text",
                false,
            )?;
        } else if let Some((value, _, _)) = self.volume_range(target) {
            let track = self.volume_track(target);
            let x = track.x - r.x;
            let y = track.y - r.y;
            let head = x + track.w * range::fraction(value, 0., 200.) as f32;
            gfx::fill_round(dc, gfx::rect(0., y, r.w, y + 4.), 2., t.color("n-6"))?;
            gfx::fill_round(
                dc,
                gfx::rect(0., y, r.w * (value / 200.) as f32, y + 4.),
                2.,
                t.color("theme-accent"),
            )?;
            let radius = if self.hover == Some(target) {
                7. * 1.08
            } else {
                7.
            };
            let thumb = gfx::rect(
                head - radius,
                y + 2. - radius,
                head + radius,
                y + 2. + radius,
            );
            gfx::shadow(dc, thumb, radius, t.shadow("shadow-sm"))?;
            gfx::fill_round(dc, thumb, radius, t.color("surface-raised"))?;
            gfx::border(dc, thumb, radius, 1., t.color("border-strong"))?;
            if focus && self.focus_visible {
                gfx::ring(dc, thumb, radius, 2., 4., t.color("theme-accent"))?;
            }
            self.draw_text(
                dc,
                Style::Tiny,
                &format!("{value:.0}%"),
                Rect::new(0., 0., r.w, 14.),
                "text-subtle",
                true,
            )?;
        }
        if focus && self.focus_visible && !target.is_volume() {
            let (bounds, radius) = if target.is_check() {
                let y = (r.h - 16.) / 2.;
                (gfx::rect(0., y, 16., y + 16.), t.n("r-xs"))
            } else {
                (b, t.n("r-md"))
            };
            gfx::ring(dc, bounds, radius, 2., 4., t.color("theme-accent"))?;
        }
        Ok(())
    }
}
