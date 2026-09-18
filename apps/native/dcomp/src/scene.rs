use crate::options::{Options, Scene};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}
pub enum Node {
    Box {
        rect: Rect,
        fill: [f32; 4],
        radius: f32,
    },
    Text {
        rect: Rect,
        text: String,
        size: f32,
        bold: bool,
        color: [f32; 4],
    },
    Image {
        rect: Rect,
    },
    PushClip(Rect),
    PopClip,
}

#[derive(Clone, Deserialize)]
pub struct Tokens {
    colors: BTreeMap<String, [f32; 4]>,
    numbers: BTreeMap<String, f32>,
}
impl Tokens {
    pub fn color(&self, name: &str) -> [f32; 4] {
        self.colors[name]
    }
    pub fn number(&self, name: &str) -> f32 {
        self.numbers[name]
    }
    pub fn load(options: &Options, system_light: bool) -> Self {
        static VARIANTS: OnceLock<BTreeMap<String, Tokens>> = OnceLock::new();
        let variants = VARIANTS.get_or_init(|| {
            serde_json::from_str(include_str!("../resources/tokens.json"))
                .expect("generated tokens")
        });
        let appearance = if options.appearance == "system" {
            if system_light { "light" } else { "dark" }
        } else {
            &options.appearance
        };
        variants[&format!("{appearance}-{}", options.theme)].clone()
    }
}

#[derive(Clone, Copy)]
pub enum Action {
    Scene(Scene),
    Appearance,
    Pause,
    Mute,
    Preview,
    Search,
    Row(usize),
}
pub struct State {
    pub options: Options,
    pub query: String,
    pub editing: bool,
    pub paused: bool,
    pub muted: bool,
    pub deleted: bool,
    pub first_row: usize,
    pub selected_row: Option<usize>,
    pub zoom: f32,
    pub pan: [f32; 2],
    pub hits: Vec<(Rect, Action)>,
}
impl State {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            query: String::new(),
            editing: false,
            paused: false,
            muted: false,
            deleted: false,
            first_row: 0,
            selected_row: None,
            zoom: 1.,
            pan: [0.; 2],
            hits: vec![],
        }
    }

    pub fn activate(&mut self, action: Action) {
        match action {
            Action::Scene(scene) => {
                self.options.scene = scene;
                self.editing = false;
            }
            Action::Appearance => {
                self.options.appearance = if self.options.appearance == "light" {
                    "dark"
                } else {
                    "light"
                }
                .into()
            }
            Action::Pause => self.paused = !self.paused,
            Action::Mute => self.muted = !self.muted,
            Action::Preview => self.deleted = !self.deleted,
            Action::Search => self.editing = true,
            Action::Row(row) => self.selected_row = Some(row),
        }
    }
    pub fn scroll(&mut self, amount: f32) {
        if self.options.scene == Scene::History {
            self.first_row = (self.first_row as isize - amount as isize)
                .clamp(0, self.options.history_count.saturating_sub(1) as isize)
                as usize;
        } else if self.options.scene == Scene::Editor {
            self.zoom = (self.zoom + amount * 0.05).clamp(0.25, 4.);
        }
    }

    pub fn nodes(&mut self, width: f32, height: f32, t: &Tokens) -> Vec<Node> {
        self.hits.clear();
        let gap = t.number("s-5");
        let margin = t.number("s-8");
        let mut p = Painter {
            nodes: vec![],
            hits: &mut self.hits,
            tokens: t,
        };
        let floating = self.options.floating;
        if !floating {
            p.boxed(Rect::new(0., 0., width, height), "surface-canvas", 0.);
            p.boxed(Rect::new(0., 0., 196., height), "surface-sunken", 0.);
            p.text(
                Rect::new(margin, margin, 160., 32.),
                "Captures",
                t.number("text-2xl"),
                true,
                "text",
            );
            for (index, scene) in Scene::VISIBLE.iter().enumerate() {
                p.button(
                    Rect::new(
                        gap,
                        90. + index as f32 * (t.number("h-lg") + gap),
                        196. - gap * 2.,
                        t.number("h-lg"),
                    ),
                    scene.title(),
                    Action::Scene(*scene),
                    *scene == self.options.scene,
                    false,
                );
            }
            p.text(
                Rect::new(gap, height - 64., 180., 48.),
                "Renderer candidate\nCapture engine not connected",
                t.number("text-sm"),
                false,
                "text-muted",
            );
        }
        let left = if floating { margin } else { 196. + margin };
        let w = width - left - margin;
        if !floating {
            p.text(
                Rect::new(left, margin, w, 36.),
                self.options.scene.title(),
                t.number("text-3xl"),
                true,
                "text",
            );
            p.text(
                Rect::new(left, margin + 42., w, 32.),
                "DirectComposition / Direct2D · synthetic renderer probe",
                t.number("text-md"),
                false,
                "text-muted",
            );
        }
        let top = if floating { margin } else { 112. };
        match self.options.scene {
            Scene::Preferences => {
                p.button(
                    Rect::new(left, top, w, 40.),
                    &format!(
                        "Find a setting: {}{}",
                        self.query,
                        if self.editing { " │" } else { "" }
                    ),
                    Action::Search,
                    self.editing,
                    false,
                );
                let sections = [
                    ("Appearance", "Light / Dark", Action::Appearance),
                    ("Capture", "Screenshot defaults · PNG", Action::Appearance),
                    ("Recording", "Video and GIF defaults", Action::Appearance),
                    (
                        "Shortcuts",
                        "Platform integrations not connected",
                        Action::Appearance,
                    ),
                ];
                let mut y = top + 64.;
                for (title, detail, action) in sections {
                    if !format!("{title} {detail}")
                        .to_lowercase()
                        .contains(&self.query.to_lowercase())
                    {
                        continue;
                    }
                    p.boxed(
                        Rect::new(left, y, w, 96.),
                        "surface-raised",
                        t.number("r-lg"),
                    );
                    p.text(
                        Rect::new(left + gap, y + gap, w - 180., 32.),
                        title,
                        t.number("text-xl"),
                        true,
                        "text",
                    );
                    p.text(
                        Rect::new(left + gap, y + 48., w - 180., 30.),
                        detail,
                        t.number("text-md"),
                        false,
                        "text-muted",
                    );
                    if title == "Appearance" {
                        p.button(
                            Rect::new(left + w - 150., y + gap, 130., 40.),
                            &self.options.appearance,
                            action,
                            false,
                            false,
                        );
                    }
                    y += 112.;
                }
            }
            Scene::History => {
                p.text(
                    Rect::new(left, top, w, 32.),
                    &format!(
                        "{} synthetic captures · scroll to virtualize",
                        self.options.history_count
                    ),
                    t.number("text-md"),
                    false,
                    "text-muted",
                );
                let row_height = 76.;
                let visible = ((height - top - 48.) / row_height).max(0.) as usize;
                for row in
                    self.first_row..(self.first_row + visible).min(self.options.history_count)
                {
                    let y = top + 42. + (row - self.first_row) as f32 * row_height;
                    let rect = Rect::new(left, y, w, row_height - gap);
                    p.boxed(
                        rect,
                        if self.selected_row == Some(row) {
                            "surface-selected"
                        } else {
                            "surface-raised"
                        },
                        t.number("r-md"),
                    );
                    p.nodes.push(Node::Image {
                        rect: Rect::new(left + gap, y + gap, 72., 40.),
                    });
                    p.text(
                        Rect::new(left + 100., y + gap, w - 110., 40.),
                        &format!(
                            "Capture {:04} · {}",
                            row + 1,
                            ["Video", "Screenshot", "GIF"][row % 3]
                        ),
                        t.number("text-lg"),
                        false,
                        "text",
                    );
                    p.hits.push((rect, Action::Row(row)));
                }
            }
            Scene::Hud => {
                p.boxed(
                    Rect::new(left, top, w, 180.),
                    "glass-strong",
                    t.number("r-xl"),
                );
                p.text(
                    Rect::new(left + gap, top + gap, w - 2. * gap, 40.),
                    if self.paused {
                        "Paused · 00:42"
                    } else {
                        "Recording · 00:42"
                    },
                    t.number("text-2xl"),
                    true,
                    "glass-text",
                );
                p.button(
                    Rect::new(left + gap, top + 76., 130., 40.),
                    if self.paused { "Resume" } else { "Pause" },
                    Action::Pause,
                    false,
                    true,
                );
                p.button(
                    Rect::new(left + gap + 146., top + 76., 130., 40.),
                    if self.muted { "Unmute mic" } else { "Mute mic" },
                    Action::Mute,
                    false,
                    true,
                );
            }
            Scene::Preview => {
                p.boxed(
                    Rect::new(left, top, w, 360.),
                    "glass-strong",
                    t.number("r-xl"),
                );
                // DWM animates the existing surface in floating mode. Redraws
                // from keyboard, DPI, or expose must not remove its source pixels.
                if !self.deleted || floating {
                    p.nodes.push(Node::Image {
                        rect: Rect::new(left + gap, top + gap, w - 2. * gap, 230.),
                    });
                }
                p.button(
                    Rect::new(left + gap, top + 260., 180., 40.),
                    if floating {
                        "Space: fade / restore"
                    } else if self.deleted {
                        "Restore fixture"
                    } else {
                        "Dismiss fixture"
                    },
                    Action::Preview,
                    false,
                    true,
                );
                p.text(
                    Rect::new(left + gap, top + 316., w - 2. * gap, 30.),
                    "Composition fade probe, not dust parity",
                    t.number("text-md"),
                    false,
                    "glass-text-muted",
                );
            }
            Scene::Editor => {
                p.button(
                    Rect::new(left, top, w, 40.),
                    &format!(
                        "Annotation: {}{}",
                        self.query,
                        if self.editing { " │" } else { "" }
                    ),
                    Action::Search,
                    self.editing,
                    false,
                );
                let image_width = (w - 32.) * self.zoom;
                p.nodes.push(Node::PushClip(Rect::new(
                    left,
                    top + 52.,
                    w,
                    height - top - 112.,
                )));
                p.nodes.push(Node::Image {
                    rect: Rect::new(
                        left + 16. + self.pan[0],
                        top + 64. + self.pan[1],
                        image_width,
                        image_width * 1152. / 2048.,
                    ),
                });
                p.text(
                    Rect::new(
                        left + 40. + self.pan[0],
                        top + 110. + self.pan[1],
                        w - 64.,
                        48.,
                    ),
                    &self.query,
                    t.number("text-2xl"),
                    true,
                    "glass-text",
                );
                p.nodes.push(Node::PopClip);
                p.text(
                    Rect::new(left, height - 48., w, 32.),
                    &format!(
                        "Zoom {:.0}% · wheel to zoom · drag to pan",
                        self.zoom * 100.
                    ),
                    t.number("text-md"),
                    false,
                    "text-muted",
                );
            }
            Scene::Countdown => {
                p.boxed(
                    Rect::new(left, top, w, height - top - margin),
                    "glass-countdown-scrim",
                    0.,
                );
                p.text(
                    Rect::new(left + w * 0.3, top + 60., w * 0.6, 40.),
                    "SCREENSHOT IN",
                    t.number("countdown-label-max"),
                    true,
                    "glass-text-muted",
                );
                p.text(
                    Rect::new(left + w * 0.38, top + 110., w * 0.6, 340.),
                    "3",
                    (height * 0.4).clamp(
                        t.number("countdown-number-min"),
                        t.number("countdown-number-max"),
                    ),
                    true,
                    "glass-text",
                );
                p.text(
                    Rect::new(left + w * 0.3, height - 90., w * 0.6, 40.),
                    "Press Esc to cancel",
                    t.number("text-lg"),
                    false,
                    "glass-text-muted",
                );
            }
            Scene::Idle => {}
        }
        p.nodes
    }
}

struct Painter<'a> {
    nodes: Vec<Node>,
    hits: &'a mut Vec<(Rect, Action)>,
    tokens: &'a Tokens,
}
impl Painter<'_> {
    fn boxed(&mut self, rect: Rect, fill: &str, radius: f32) {
        self.nodes.push(Node::Box {
            rect,
            fill: self.tokens.color(fill),
            radius,
        });
    }
    fn text(&mut self, rect: Rect, text: &str, size: f32, bold: bool, color: &str) {
        self.nodes.push(Node::Text {
            rect,
            text: text.into(),
            size,
            bold,
            color: self.tokens.color(color),
        });
    }
    fn button(&mut self, rect: Rect, label: &str, action: Action, selected: bool, glass: bool) {
        self.boxed(
            rect,
            if glass {
                "glass-raised"
            } else if selected {
                "surface-selected"
            } else {
                "control"
            },
            self.tokens.number("r-md"),
        );
        self.text(
            Rect::new(rect.x + 12., rect.y + 8., rect.w - 24., rect.h - 8.),
            label,
            self.tokens.number("text-md"),
            false,
            if glass { "glass-text" } else { "text" },
        );
        self.hits.push((rect, action));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(scene: &str) -> State {
        State::new(Options::parse(["--scene".into(), scene.into()]).unwrap())
    }
    #[test]
    fn history_virtualization_keeps_original_ids_and_respects_the_tail() {
        let mut s = state("history");
        let t = Tokens::load(&s.options, false);
        s.first_row = 991;
        let nodes = s.nodes(1000., 720., &t);
        assert_eq!(
            nodes
                .iter()
                .filter(|n| matches!(n, Node::Image { .. }))
                .count(),
            7
        );
        assert!(s.hits.iter().any(|(_, a)| matches!(a, Action::Row(991))));
        assert!(!s.hits.iter().any(|(_, a)| matches!(a, Action::Row(998))));
        s.first_row = 999;
        assert_eq!(
            s.nodes(1000., 720., &t)
                .iter()
                .filter(|n| matches!(n, Node::Image { .. }))
                .count(),
            1
        );
        s.options.history_count = 0;
        assert_eq!(
            s.nodes(1000., 720., &t)
                .iter()
                .filter(|n| matches!(n, Node::Image { .. }))
                .count(),
            0
        );
    }
    #[test]
    fn search_filters_content_and_glass_is_independent_of_appearance() {
        let mut s = state("preferences");
        let dark = Tokens::load(&s.options, false);
        s.query = "rEcOrD".into();
        let nodes = s.nodes(1000., 720., &dark);
        let labels: Vec<_> = nodes
            .iter()
            .filter_map(|n| {
                if let Node::Text { text, .. } = n {
                    Some(text.as_str())
                } else {
                    None
                }
            })
            .collect();
        assert!(labels.contains(&"Recording"));
        assert!(!labels.contains(&"Capture"));
        s.options.appearance = "light".into();
        let light = Tokens::load(&s.options, false);
        assert_eq!(dark.color("glass-strong"), light.color("glass-strong"));
        assert_ne!(dark.color("surface-canvas"), light.color("surface-canvas"));
    }
    #[test]
    fn floating_fade_preserves_source_pixels_across_redraws() {
        let mut s = state("preview");
        s.activate(Action::Preview);
        let t = Tokens::load(&s.options, false);
        assert!(
            !s.nodes(1000., 720., &t)
                .iter()
                .any(|n| matches!(n, Node::Image { .. }))
        );
        s.options.floating = true;
        assert!(
            s.nodes(640., 620., &t)
                .iter()
                .any(|n| matches!(n, Node::Image { .. }))
        );
    }
    #[test]
    fn editor_clips_image_and_annotation_but_not_toolbar_or_footer() {
        let mut s = state("editor");
        s.query = "Selected layer".into();
        s.zoom = 3.;
        s.pan = [-300., -120.];
        let t = Tokens::load(&s.options, false);
        let nodes = s.nodes(1000., 720., &t);
        let start = nodes
            .iter()
            .position(|n| matches!(n, Node::PushClip(_)))
            .unwrap();
        let end = nodes
            .iter()
            .position(|n| matches!(n, Node::PopClip))
            .unwrap();
        assert!(
            nodes[start + 1..end]
                .iter()
                .any(|n| matches!(n, Node::Image { .. }))
        );
        assert!(
            nodes[start + 1..end]
                .iter()
                .any(|n| matches!(n,Node::Text{text,..} if text=="Selected layer"))
        );
        assert!(matches!(nodes.last().unwrap(),Node::Text{text,..} if text.contains("Zoom 300%")));
        let r = Rect::new(3., 5., 11., 13.);
        assert!(r.contains(3., 5.));
        assert!(!r.contains(14., 5.));
        assert!(!r.contains(4., 18.));
    }
}
