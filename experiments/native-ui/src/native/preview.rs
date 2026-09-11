//! Native mini-preview stack. Geometry and delete motion intentionally mirror
//! `thumbnailLayout.ts`, `thumbnailExit.ts`, and `mini-preview.css`.
use crate::ui;
use gtk::{cairo, gdk, gdk_pixbuf::Pixbuf, glib, prelude::*};
use image::RgbaImage;
use std::{
    cell::{Cell, RefCell},
    io::Cursor,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

const CARD_W: f64 = 284.;
const CARD_H: f64 = 160.;
const CARD_GAP: f64 = 24.;
const SLOT: f64 = CARD_H + CARD_GAP;
const CARD_X: f64 = 28.;
const STACK_EDGE: f64 = 52.;
const HEIGHT: i32 = 760;
const VISIBLE_CARDS: usize = 3;
const DECODED_PREVIEW_LIMIT: usize = 12;
const DISMISS_MS: f64 = 540.;
const DELETE_MS: f64 = 2_200.;

/// Parent-owned effects. `dismiss` never means delete; `delete` is called only
/// after the user confirms the destructive per-card action.
#[derive(Clone)]
pub struct PreviewCallbacks {
    pub open: Rc<dyn Fn(PathBuf)>,
    pub edit: Rc<dyn Fn(PathBuf)>,
    pub dismiss: Rc<dyn Fn(PathBuf)>,
    pub delete: Rc<dyn Fn(PathBuf)>,
    /// Save an unsaved capture, or reveal an already-saved capture.
    pub save: Rc<dyn Fn(PathBuf)>,
    /// Persist placement after a pile drag: 0 bottom-right, 1 bottom-left,
    /// 2 top-right, 3 top-left.
    pub placement: Rc<dyn Fn(u8)>,
}

/// Metadata used by per-card chrome. `add` derives this for existing callers.
#[derive(Clone, Copy)]
pub struct PreviewMetadata {
    pub width: u32,
    pub height: u32,
    pub size_bytes: u64,
    pub saved: bool,
    pub copied: bool,
}

struct Card {
    id: u64,
    preview: Option<Pixbuf>,
    blurred_preview: Option<Pixbuf>,
    // Small encoded poster survives decoded-cache eviction. This matters for
    // videos whose saved media path cannot itself be decoded as an image.
    poster: Vec<u8>,
    path: PathBuf,
    metadata: PreviewMetadata,
    shell: gtk::EventBox,
    top_actions: gtk::Box,
    edit: gtk::Button,
    hover_chrome: Vec<gtk::Widget>,
    meta: gtk::Label,
    hovered: bool,
    shake_started: Option<Instant>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExitKind {
    Dismiss,
    Delete,
}

struct Exit {
    card: Card,
    kind: ExitKind,
    started: Instant,
    delay_ms: f64,
    slot_index: usize,
    y: f64,
    particles: Vec<Particle>,
}

struct StackDrag {
    root_x: f64,
    root_y: f64,
    window_x: i32,
    window_y: i32,
    moved: bool,
}

struct State {
    cards: Vec<Card>,
    exits: Vec<Exit>,
    expanded: bool,
    expansion: f64,
    hover: f64,
    hovering: bool,
    top: bool,
    right: bool,
    scroll_start: usize,
    drag: Option<StackDrag>,
    next_id: u64,
    ticking: bool,
}

struct Inner {
    window: gtk::Window,
    area: gtk::DrawingArea,
    fixed: gtk::Fixed,
    pile_hit: gtk::Button,
    toolbar: gtk::Box,
    clear: gtk::Button,
    show_less: gtk::Button,
    newer: gtk::Button,
    older: gtk::Button,
    callbacks: PreviewCallbacks,
    state: RefCell<State>,
    visible: Cell<bool>,
}

#[derive(Clone)]
pub struct Preview(Rc<Inner>);

#[derive(Clone, Copy)]
struct Particle {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    dx: f64,
    dy: f64,
    rotation: f64,
    delay: f64,
    duration: f64,
}

fn pose(depth: f64) -> f64 {
    depth.max(0.) * (24. + 0.55 * depth.max(0.)) / (depth.max(0.) + 24.)
}

fn peek_jitter(depth: usize) -> f64 {
    if depth == 0 {
        return 0.;
    }
    let mut hashed = (depth as u32).wrapping_mul(0x9e37_79b1) ^ 0x7f4a_7c15;
    hashed = hashed.wrapping_mul(0x85eb_ca6b);
    (hashed as f64 / 2_f64.powi(32) * 2. - 1.) * 0.4 * 0.58_f64.powi(depth as i32 - 1)
}

#[derive(Clone, Copy, Debug)]
struct CardPose {
    x: f64,
    y: f64,
    scale: f64,
    tilt: f64,
    z: f64,
    origin_y: f64,
    perspective_y: f64,
}

impl CardPose {
    fn flat(x: f64, y: f64) -> Self {
        Self {
            x,
            y,
            scale: 1.,
            tilt: 0.,
            z: 0.,
            origin_y: 0.,
            perspective_y: 0.,
        }
    }

    fn project(self, x: f64, y: f64) -> (f64, f64) {
        let local_y = (y - self.origin_y) * self.scale;
        let perspective = 900. / (900. - self.z - local_y * self.tilt.sin());
        (
            CARD_X + CARD_W / 2. + (self.x - CARD_X + (x - CARD_W / 2.) * self.scale) * perspective,
            self.perspective_y
                + (self.y + self.origin_y + local_y * self.tilt.cos() - self.perspective_y)
                    * perspective,
        )
    }
}

fn collapsed_pose(depth: usize, hover: f64, top: bool) -> CardPose {
    let pile_depth = pose(depth as f64);
    let gravity = if top { -1. } else { 1. };
    let peek = 13. + hover * 3.;
    let x_step = -0.8 + hover * 0.2;
    let scale_step = 0.025 - hover * 0.005;
    CardPose {
        x: CARD_X + pile_depth * x_step,
        // CSS translates from the edge-positioned front card. Importantly,
        // jitter is inside the gravity multiplication, not part of peek size.
        y: if top {
            STACK_EDGE
        } else {
            HEIGHT as f64 - STACK_EDGE - CARD_H
        } + (-pile_depth * peek + peek_jitter(depth)) * gravity,
        scale: 1. - pile_depth * scale_step,
        // Native placements snap to corners, where the source's rotateZ is
        // zero. Its rotateX and translateZ still recede through 900px perspective.
        tilt: (-pile_depth * (0.8 - hover * 0.1) * gravity).to_radians(),
        z: -pile_depth * (24. - hover * 6.),
        origin_y: if top { 0. } else { CARD_H },
        perspective_y: if top {
            STACK_EDGE + CARD_H / 2.
        } else {
            HEIGHT as f64 - STACK_EDGE - CARD_H / 2.
        },
    }
}

fn visual_index_with_slots(card_index: usize, mut occupied: Vec<usize>) -> usize {
    let mut visual = card_index;
    occupied.sort_unstable();
    for slot in occupied {
        if slot <= visual {
            visual += 1;
        }
    }
    visual
}

fn particles(origin_x: f64) -> Vec<Particle> {
    // Fixed seed makes native/browser frame comparisons repeatable.
    let mut seed = 739_u32;
    let mut random = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed as f64 / u32::MAX as f64
    };
    let (cols, rows) = (20, 11); // Exact web soft cap: 220 image chips.
    let (w, h) = (CARD_W / cols as f64, CARD_H / rows as f64);
    let origin_y: f64 = 22.5;
    let max_dist = origin_x
        .max(CARD_W - origin_x)
        .hypot(origin_y.max(CARD_H - origin_y));
    let mut result = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            let (x, y) = (col as f64 * w, row as f64 * h);
            let (cx, cy) = (x + w / 2., y + h / 2.);
            let wave = (cx - origin_x).hypot(cy - origin_y) / max_dist;
            let angle = (cy - origin_y).atan2(cx - origin_x);
            let delay_norm = (wave
                + (angle * 2.7 + wave * 5.5).sin() * 0.07 * wave
                + (random() - 0.5) * 0.34 * wave * wave)
                .clamp(0., 1.12);
            result.push(Particle {
                x,
                y,
                width: w + 0.55,
                height: h + 0.55,
                dx: (cx - origin_x) / max_dist * (12. + random() * 26.) + (random() - 0.5) * 22.,
                dy: -36. - random() * 58.,
                rotation: (random() - 0.5) * 120.,
                delay: delay_norm * 720. + random() * (18. + wave * 140.),
                duration: 780. + random() * 320. + wave * 80.,
            });
        }
    }
    result
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    for (cx, cy, from) in [
        (x + w - r, y + r, -90_f64),
        (x + w - r, y + h - r, 0.),
        (x + r, y + h - r, 90.),
        (x + r, y + r, 180.),
    ] {
        cr.arc(cx, cy, r, from.to_radians(), (from + 90.).to_radians());
    }
    cr.close_path();
}

fn scaled_preview(image: &RgbaImage) -> Option<Pixbuf> {
    let scale = (CARD_W / image.width() as f64).max(CARD_H / image.height() as f64);
    ui::pixbuf(image).scale_simple(
        (image.width() as f64 * scale).ceil() as i32,
        (image.height() as f64 * scale).ceil() as i32,
        gtk::gdk_pixbuf::InterpType::Bilinear,
    )
}

fn load_preview(path: &Path) -> Option<Pixbuf> {
    image::open(path)
        .ok()
        .and_then(|image| scaled_preview(&image.to_rgba8()))
}

fn encode_poster(preview: &Pixbuf) -> Vec<u8> {
    preview.save_to_bufferv("png", &[]).unwrap_or_default()
}

fn decode_poster(poster: &[u8]) -> Option<Pixbuf> {
    Pixbuf::from_read(Cursor::new(poster.to_vec())).ok()
}

fn blur_thumbnail(image: &RgbaImage) -> RgbaImage {
    image::imageops::blur(image, 2.)
}

fn blurred_poster(poster: &[u8]) -> Option<Pixbuf> {
    let image = image::load_from_memory(poster).ok()?.to_rgba8();
    Some(ui::pixbuf(&blur_thumbnail(&image)))
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1_000 {
        format!("{bytes} B")
    } else if bytes < 1_000_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.)
    } else {
        format!("{:.1} MB", bytes as f64 / 1_000_000.)
    }
}

fn card_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("capture")
        .to_owned()
}

fn placement_for_corner(right: bool, top: bool) -> u8 {
    match (right, top) {
        (true, false) => 0,
        (false, false) => 1,
        (true, true) => 2,
        (false, true) => 3,
    }
}

fn set_accessible_name<W: IsA<gtk::Widget>>(widget: &W, name: &str) {
    if let Some(accessible) = widget.as_ref().accessible() {
        accessible.set_name(name);
    }
}

fn install_preview_css() {
    let provider = gtk::CssProvider::new();
    if let Err(error) = provider.load_from_data(include_bytes!("preview.css")) {
        eprintln!("Native preview theme: {error}");
    }
    if let Some(screen) = gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
}

fn icon_button(name: &'static str, accessible: &str, class: &str, size: i32) -> gtk::Button {
    let button = gtk::Button::new();
    button.add(&ui::icon(name, size));
    button.style_context().add_class(class);
    set_accessible_name(&button, accessible);
    button.set_tooltip_text(Some(accessible));
    button
}

fn action_button(name: &'static str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.pack_start(&ui::icon(name, 16), false, false, 0);
    row.pack_start(&gtk::Label::new(Some(label)), false, false, 0);
    button.add(&row);
    button.style_context().add_class("preview-main-action");
    button
}

fn preview_stack_icon() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_size_request(14, 14);
    area.connect_draw(|widget, cr| {
        let color = widget.style_context().color(widget.state_flags());
        cr.set_source_rgba(color.red(), color.green(), color.blue(), color.alpha());
        cr.set_line_width(1.8 * 14. / 16.);
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        cr.scale(14. / 16., 14. / 16.);
        for points in [
            &[(3., 5.5), (8., 2.75), (13., 5.5), (8., 8.25), (3., 5.5)][..],
            &[(3.5, 8.5), (8., 11.), (12.5, 8.5)][..],
            &[(4.5, 11.), (8., 13.), (11.5, 11.)][..],
        ] {
            cr.move_to(points[0].0, points[0].1);
            for &(x, y) in &points[1..] {
                cr.line_to(x, y);
            }
            let _ = cr.stroke();
        }
        glib::Propagation::Proceed
    });
    area
}

impl Preview {
    pub fn with_callbacks(callbacks: PreviewCallbacks) -> Self {
        install_preview_css();
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Captures — Mini previews");
        window.set_decorated(false);
        window.set_keep_above(true);
        window.set_accept_focus(false);
        window.set_skip_taskbar_hint(true);
        window.set_app_paintable(true);
        window.style_context().add_class("floating");
        window.set_resizable(false);
        window.add_events(gdk::EventMask::POINTER_MOTION_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        if let Some(screen) = gdk::Screen::default() {
            window.set_visual(screen.rgba_visual().as_ref());
        }
        window.set_default_size((CARD_W + CARD_X * 2.) as i32, HEIGHT);

        let overlay = gtk::Overlay::new();
        let area = gtk::DrawingArea::new();
        overlay.add(&area);
        let fixed = gtk::Fixed::new();
        fixed.set_hexpand(true);
        fixed.set_vexpand(true);
        overlay.add_overlay(&fixed);

        let pile_hit = gtk::Button::new();
        set_accessible_name(&pile_hit, "Expand preview");
        pile_hit.style_context().add_class("preview-hit");
        pile_hit.set_size_request(CARD_W as i32, CARD_H as i32);
        fixed.put(&pile_hit, CARD_X as i32, STACK_EDGE as i32);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        toolbar.style_context().add_class("preview-toolbar");
        let newer = icon_button(
            "chevron-up",
            "Show newer captures",
            "preview-overflow-cue",
            16,
        );
        let older = icon_button(
            "chevron-down",
            "Show older captures",
            "preview-overflow-cue",
            16,
        );
        let clear = icon_button("close", "Clear all", "preview-stack-control", 14);
        let show_less = gtk::Button::new();
        let show_less_content = gtk::Stack::new();
        show_less_content.set_transition_type(gtk::StackTransitionType::Crossfade);
        show_less_content.set_transition_duration(180);
        show_less_content.add_named(&preview_stack_icon(), "icon");
        let show_less_label = gtk::Label::new(Some("Show less"));
        show_less_label
            .style_context()
            .add_class("preview-show-less-label");
        show_less_content.add_named(&show_less_label, "label");
        show_less_content.set_visible_child_name("icon");
        show_less.add(&show_less_content);
        show_less.style_context().add_class("preview-stack-control");
        set_accessible_name(&show_less, "Show less");
        {
            let content = show_less_content.clone();
            show_less.connect_enter_notify_event(move |button, _| {
                button.set_size_request(92, 28);
                content.set_visible_child_name("label");
                glib::Propagation::Proceed
            });
        }
        {
            let content = show_less_content;
            show_less.connect_leave_notify_event(move |button, _| {
                button.set_size_request(28, 28);
                content.set_visible_child_name("icon");
                glib::Propagation::Proceed
            });
        }
        toolbar.pack_start(&clear, false, false, 0);
        toolbar.pack_start(&show_less, false, false, 0);
        fixed.put(&toolbar, CARD_X as i32, HEIGHT - 44);
        fixed.put(&newer, 147, 6);
        fixed.put(&older, 147, HEIGHT - 28);

        window.add(&overlay);
        let this = Self(Rc::new(Inner {
            window,
            area,
            fixed,
            pile_hit,
            toolbar,
            clear,
            show_less,
            newer,
            older,
            callbacks,
            visible: Cell::new(true),
            state: RefCell::new(State {
                cards: vec![],
                exits: vec![],
                expanded: false,
                expansion: 0.,
                hover: 0.,
                hovering: false,
                top: false,
                right: false,
                scroll_start: 0,
                drag: None,
                next_id: 1,
                ticking: false,
            }),
        }));

        {
            let this = this.clone();
            this.0.area.clone().connect_draw(move |_, cr| {
                this.draw(cr);
                glib::Propagation::Proceed
            });
        }
        {
            let this = this.clone();
            this.0
                .window
                .clone()
                .connect_motion_notify_event(move |_, event| {
                    let (x, y) = event.root();
                    this.update_card_hover(x, y);
                    glib::Propagation::Proceed
                });
        }
        {
            // GTK3 child input windows do not consistently propagate motion to
            // the transparent toplevel. Poll the seat like Tauri's native
            // preview tracking so media, icons, and gaps share one hover truth.
            let weak = Rc::downgrade(&this.0);
            glib::timeout_add_local(Duration::from_millis(32), move || {
                let Some(inner) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let preview = Preview(inner);
                if preview.0.window.is_visible()
                    && let Some((_, x, y)) = gdk::Display::default()
                        .and_then(|display| display.default_seat())
                        .and_then(|seat| seat.pointer())
                        .map(|device| device.position())
                {
                    preview.update_card_hover(x as f64, y as f64);
                }
                glib::ControlFlow::Continue
            });
        }
        {
            let this = this.clone();
            this.0
                .window
                .clone()
                .connect_leave_notify_event(move |_, event| {
                    if event.detail() != gdk::NotifyType::Inferior {
                        this.update_card_hover(f64::NEG_INFINITY, f64::NEG_INFINITY);
                    }
                    glib::Propagation::Proceed
                });
        }
        {
            let this = this.clone();
            this.0
                .pile_hit
                .clone()
                .connect_enter_notify_event(move |_, _| {
                    this.0.state.borrow_mut().hovering = true;
                    this.animate();
                    glib::Propagation::Proceed
                });
        }
        {
            let this = this.clone();
            this.0
                .pile_hit
                .clone()
                .connect_leave_notify_event(move |_, _| {
                    this.0.state.borrow_mut().hovering = false;
                    this.animate();
                    glib::Propagation::Proceed
                });
        }
        this.connect_pile_drag();
        {
            let this = this.clone();
            this.0
                .pile_hit
                .clone()
                .connect_clicked(move |_| this.set_expanded(true));
        }
        {
            let this = this.clone();
            this.0
                .pile_hit
                .clone()
                .connect_activate(move |_| this.set_expanded(true));
        }
        {
            let this = this.clone();
            this.0
                .show_less
                .clone()
                .connect_clicked(move |_| this.set_expanded(false));
        }
        {
            let this = this.clone();
            this.0
                .toolbar
                .clone()
                .connect_size_allocate(move |toolbar, allocation| {
                    let state = this.0.state.borrow();
                    let x = if state.right {
                        (CARD_X + CARD_W) as i32 - allocation.width()
                    } else {
                        CARD_X as i32
                    };
                    let y = if state.top { 16 } else { HEIGHT - 44 };
                    this.0.fixed.move_(toolbar, x, y);
                });
        }
        {
            let this = this.clone();
            this.0
                .clear
                .clone()
                .connect_clicked(move |_| this.clear_all());
        }
        {
            let this = this.clone();
            this.0
                .older
                .clone()
                .connect_clicked(move |_| this.scroll(1));
        }
        {
            let this = this.clone();
            this.0
                .newer
                .clone()
                .connect_clicked(move |_| this.scroll(-1));
        }
        this.reflow();
        this
    }

    pub fn add(&self, image: RgbaImage, path: PathBuf) {
        let size_bytes = std::fs::metadata(&path).map_or(image.len() as u64, |m| m.len());
        let metadata = PreviewMetadata {
            width: image.width(),
            height: image.height(),
            size_bytes,
            saved: path.exists(),
            copied: false,
        };
        self.add_with_metadata(image, path, metadata);
    }

    pub fn add_with_metadata(&self, image: RgbaImage, path: PathBuf, metadata: PreviewMetadata) {
        let preview = scaled_preview(&image);
        let poster = preview.as_ref().map(encode_poster).unwrap_or_default();
        let blurred_preview = blurred_poster(&poster);
        let id = {
            let mut state = self.0.state.borrow_mut();
            let id = state.next_id;
            state.next_id += 1;
            id
        };
        let (shell, top_actions, edit, hover_chrome, meta) =
            self.build_card_controls(id, &path, metadata);
        self.0.fixed.put(&shell, CARD_X as i32, STACK_EDGE as i32);
        self.0.state.borrow_mut().cards.insert(
            0,
            Card {
                id,
                preview,
                blurred_preview,
                poster,
                path,
                metadata,
                shell,
                top_actions,
                edit,
                hover_chrome,
                meta,
                hovered: false,
                shake_started: None,
            },
        );
        self.bound_decoded_previews();
        if self.0.visible.get() {
            self.0.window.show_all();
        }
        let (right, top) = {
            let state = self.0.state.borrow();
            (state.right, state.top)
        };
        self.home(right, top);
        self.reflow();
        self.0.area.queue_draw();
    }

    pub fn hide(&self) {
        self.0.visible.set(false);
        self.0.window.hide();
    }

    pub fn show(&self) {
        self.0.visible.set(true);
        if !self.0.state.borrow().cards.is_empty() {
            self.0.window.show_all();
            self.reflow();
        }
    }

    pub fn home(&self, right: bool, top: bool) {
        {
            let mut state = self.0.state.borrow_mut();
            state.top = top;
            state.right = right;
        }
        if let Some(display) = gdk::Display::default()
            && let Some(monitor) = display.primary_monitor().or_else(|| display.monitor(0))
        {
            let r = monitor.workarea();
            self.0.window.move_(
                if right {
                    r.x() + r.width() - (CARD_W + CARD_X * 2.) as i32
                } else {
                    r.x()
                },
                if top {
                    r.y()
                } else {
                    r.y() + r.height() - HEIGHT
                },
            );
        }
        self.reflow();
    }

    fn build_card_controls(
        &self,
        id: u64,
        path: &Path,
        metadata: PreviewMetadata,
    ) -> (
        gtk::EventBox,
        gtk::Box,
        gtk::Button,
        Vec<gtk::Widget>,
        gtk::Label,
    ) {
        let shell = gtk::EventBox::new();
        shell.set_visible_window(false);
        shell.add_events(gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        shell.set_size_request(CARD_W as i32, CARD_H as i32);
        set_accessible_name(&shell, &format!("Preview {}", card_name(path)));
        let overlay = gtk::Overlay::new();
        let spacer = gtk::DrawingArea::new();
        spacer.set_size_request(CARD_W as i32, CARD_H as i32);
        overlay.add(&spacer);

        // Add the full media drag surface before chrome so actual controls
        // remain the topmost pointer targets.
        let fill = gtk::EventBox::new();
        fill.set_visible_window(false);
        fill.set_hexpand(true);
        fill.set_vexpand(true);
        fill.set_halign(gtk::Align::Fill);
        fill.set_valign(gtk::Align::Fill);
        fill.add_events(gdk::EventMask::ENTER_NOTIFY_MASK | gdk::EventMask::LEAVE_NOTIFY_MASK);
        overlay.add_overlay(&fill);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        top.set_halign(if self.0.state.borrow().right {
            gtk::Align::End
        } else {
            gtk::Align::Start
        });
        top.set_valign(gtk::Align::Start);
        top.set_margin_start(8);
        top.set_margin_end(8);
        top.set_margin_top(8);
        top.set_no_show_all(true);
        let close = icon_button("close", "Close", "preview-icon-button", 16);
        let delete = icon_button("trash", "Delete", "preview-icon-button", 16);
        delete.style_context().add_class("delete");
        if metadata.saved {
            top.pack_start(&close, false, false, 0);
        }
        top.pack_start(&delete, false, false, 0);
        overlay.add_overlay(&top);

        let edit = icon_button("edit", "Edit", "preview-edit-button", 16);
        edit.set_halign(if self.0.state.borrow().right {
            gtk::Align::Start
        } else {
            gtk::Align::End
        });
        edit.set_valign(gtk::Align::Start);
        edit.set_margin_start(8);
        edit.set_margin_end(8);
        edit.set_margin_top(8);
        edit.set_no_show_all(true);
        overlay.add_overlay(&edit);

        let actions = gtk::Box::new(gtk::Orientation::Vertical, 6);
        actions.set_halign(gtk::Align::Center);
        actions.set_valign(gtk::Align::Center);
        actions.set_size_request(140, -1);
        actions.set_no_show_all(true);
        let copy = action_button("copy", "Copy");
        copy.set_sensitive(!metadata.copied);
        let save_label = if metadata.saved {
            "Show in Folder"
        } else {
            "Save file"
        };
        let save = action_button(if metadata.saved { "folder" } else { "save" }, save_label);
        if !metadata.copied {
            actions.pack_start(&copy, false, false, 0);
        }
        actions.pack_start(&save, false, false, 0);
        overlay.add_overlay(&actions);

        let meta = gtk::Label::new(Some(&format!(
            "{} × {} · {}",
            metadata.width,
            metadata.height,
            format_bytes(metadata.size_bytes)
        )));
        meta.set_halign(gtk::Align::Start);
        meta.set_valign(gtk::Align::End);
        meta.set_margin_start(8);
        meta.set_margin_bottom(8);
        meta.style_context().add_class("preview-meta-chip");
        overlay.add_overlay(&meta);

        let copied = gtk::Label::new(Some("✓  Copied to clipboard"));
        copied.set_halign(gtk::Align::End);
        copied.set_valign(gtk::Align::End);
        copied.set_margin_end(8);
        copied.set_margin_bottom(8);
        copied.style_context().add_class("preview-copied-chip");
        copied.set_visible(metadata.copied);
        copied.set_no_show_all(!metadata.copied);
        overlay.add_overlay(&copied);

        shell.add(&overlay);

        let name = card_name(path);
        for (button, action) in [
            (&close, "Close"),
            (&delete, "Delete"),
            (&edit, "Edit"),
            (&copy, "Copy"),
            (
                &save,
                if metadata.saved {
                    "Show in Folder"
                } else {
                    "Save"
                },
            ),
        ] {
            set_accessible_name(button, &format!("{action} {name}"));
        }
        {
            let this = self.clone();
            close.connect_clicked(move |_| this.exit(id, ExitKind::Dismiss, 0.));
        }
        {
            let this = self.clone();
            delete.connect_clicked(move |_| this.confirm_delete(id));
        }
        {
            let this = self.clone();
            edit.connect_clicked(move |_| {
                this.run_for_card(id, |callbacks, path| (callbacks.edit)(path))
            });
        }
        {
            let this = self.clone();
            let copied = copied.clone();
            let actions = actions.clone();
            copy.connect_clicked(move |button| {
                if this.copy_card(id) {
                    actions.remove(button);
                    copied.set_no_show_all(false);
                    copied.show();
                }
            });
        }
        {
            let this = self.clone();
            save.connect_clicked(move |_| {
                this.run_for_card(id, |callbacks, path| (callbacks.save)(path))
            });
        }
        let hover_widgets: Vec<gtk::Widget> = vec![
            top.clone().upcast(),
            edit.clone().upcast(),
            actions.clone().upcast(),
        ];
        {
            let this = self.clone();
            let hover_widgets = hover_widgets.clone();
            let meta = meta.clone();
            shell.connect_enter_notify_event(move |_, _| {
                for widget in &hover_widgets {
                    widget.set_no_show_all(false);
                    widget.show_all();
                }
                meta.hide();
                if let Some(card) = this
                    .0
                    .state
                    .borrow_mut()
                    .cards
                    .iter_mut()
                    .find(|card| card.id == id)
                {
                    card.hovered = true;
                }
                this.0.area.queue_draw();
                glib::Propagation::Proceed
            });
        }
        {
            // The transparent URI drag source owns the media input window, so
            // entering the visible image does not reliably cross `shell` on X11.
            let this = self.clone();
            let hover_widgets = hover_widgets.clone();
            let meta = meta.clone();
            fill.connect_enter_notify_event(move |_, _| {
                for widget in &hover_widgets {
                    widget.set_no_show_all(false);
                    widget.show_all();
                }
                meta.hide();
                if let Some(card) = this
                    .0
                    .state
                    .borrow_mut()
                    .cards
                    .iter_mut()
                    .find(|card| card.id == id)
                {
                    card.hovered = true;
                }
                this.0.area.queue_draw();
                glib::Propagation::Proceed
            });
        }
        {
            let this = self.clone();
            let hover_widgets = hover_widgets.clone();
            let meta = meta.clone();
            shell.connect_leave_notify_event(move |_, event| {
                if event.detail() == gdk::NotifyType::Inferior {
                    return glib::Propagation::Proceed;
                }
                for widget in &hover_widgets {
                    widget.hide();
                    widget.set_no_show_all(true);
                }
                meta.show();
                if let Some(card) = this
                    .0
                    .state
                    .borrow_mut()
                    .cards
                    .iter_mut()
                    .find(|card| card.id == id)
                {
                    card.hovered = false;
                }
                this.0.area.queue_draw();
                glib::Propagation::Proceed
            });
        }

        fill.drag_source_set(
            gdk::ModifierType::BUTTON1_MASK,
            &[gtk::TargetEntry::new(
                "text/uri-list",
                gtk::TargetFlags::empty(),
                0,
            )],
            gdk::DragAction::COPY,
        );
        {
            let this = self.clone();
            fill.connect_button_press_event(move |_, event| {
                if event.event_type() == gdk::EventType::DoubleButtonPress {
                    this.run_for_card(id, |callbacks, path| (callbacks.open)(path));
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
        }
        {
            let this = self.clone();
            fill.connect_drag_data_get(move |_, _, data, _, _| {
                if let Some(path) = this.card_path(id)
                    && let Ok(uri) = glib::filename_to_uri(path, None)
                {
                    data.set_uris(&[uri.as_str()]);
                }
            });
        }
        {
            let this = self.clone();
            fill.connect_drag_end(move |_, context| this.finish_file_drag(id, context));
        }
        (shell, top, edit, hover_widgets, meta)
    }

    fn update_card_hover(&self, root_x: f64, root_y: f64) {
        let (window_x, window_y) = self.0.window.position();
        let (x, y) = (root_x - window_x as f64, root_y - window_y as f64);
        let mut changed = false;
        let mut state = self.0.state.borrow_mut();
        let scroll_start = state.scroll_start;
        let expanded = state.expanded && state.expansion > 0.94;
        let slots: Vec<(u64, f64)> = state
            .cards
            .iter()
            .enumerate()
            .filter_map(|(index, card)| {
                let visual = Self::visual_index(&state, index);
                let local = visual.saturating_sub(scroll_start);
                (expanded && visual >= scroll_start && local < VISIBLE_CARDS)
                    .then(|| (card.id, Self::card_y(&state, local)))
            })
            .collect();
        for card in &mut state.cards {
            let hovered = slots.iter().any(|(id, card_y)| {
                *id == card.id
                    && (CARD_X..CARD_X + CARD_W).contains(&x)
                    && y >= *card_y
                    && y < *card_y + CARD_H
            });
            if hovered == card.hovered {
                continue;
            }
            card.hovered = hovered;
            changed = true;
            for widget in &card.hover_chrome {
                if hovered {
                    widget.set_no_show_all(false);
                    widget.show_all()
                } else {
                    widget.hide();
                    widget.set_no_show_all(true)
                }
            }
            if hovered {
                card.meta.hide()
            } else {
                card.meta.show()
            }
        }
        drop(state);
        if changed {
            self.0.area.queue_draw();
        }
    }

    fn connect_pile_drag(&self) {
        self.0.pile_hit.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::BUTTON_RELEASE_MASK
                | gdk::EventMask::POINTER_MOTION_MASK,
        );
        {
            let this = self.clone();
            this.0
                .pile_hit
                .clone()
                .connect_button_press_event(move |_, event| {
                    if event.button() != 1 {
                        return glib::Propagation::Proceed;
                    }
                    let (root_x, root_y) = event.root();
                    let (window_x, window_y) = this.0.window.position();
                    this.0.state.borrow_mut().drag = Some(StackDrag {
                        root_x,
                        root_y,
                        window_x,
                        window_y,
                        moved: false,
                    });
                    glib::Propagation::Stop
                });
        }
        {
            let this = self.clone();
            this.0
                .pile_hit
                .clone()
                .connect_motion_notify_event(move |_, event| {
                    let (root_x, root_y) = event.root();
                    let mut state = this.0.state.borrow_mut();
                    let Some(drag) = state.drag.as_mut() else {
                        return glib::Propagation::Proceed;
                    };
                    let dx = root_x - drag.root_x;
                    let dy = root_y - drag.root_y;
                    if dx.hypot(dy) >= 5. {
                        drag.moved = true;
                    }
                    if drag.moved {
                        this.0
                            .window
                            .move_(drag.window_x + dx as i32, drag.window_y + dy as i32);
                    }
                    glib::Propagation::Stop
                });
        }
        {
            let this = self.clone();
            this.0
                .pile_hit
                .clone()
                .connect_button_release_event(move |_, event| {
                    if event.button() != 1 {
                        return glib::Propagation::Proceed;
                    }
                    let moved = this
                        .0
                        .state
                        .borrow_mut()
                        .drag
                        .take()
                        .is_some_and(|drag| drag.moved);
                    if moved {
                        this.update_anchor_after_drag();
                    } else {
                        this.set_expanded(true);
                    }
                    glib::Propagation::Stop
                });
        }
    }

    fn update_anchor_after_drag(&self) {
        let mut placement = None;
        if let Some(display) = gdk::Display::default()
            && let Some(monitor) = display.monitor_at_window(&self.0.window.window().unwrap())
        {
            let work = monitor.workarea();
            let (x, y) = self.0.window.position();
            let mut state = self.0.state.borrow_mut();
            let previous_card_y = Self::card_y(&state, 0);
            state.right = x + (CARD_W + CARD_X * 2.) as i32 / 2 > work.x() + work.width() / 2;
            state.top =
                y + previous_card_y as i32 + CARD_H as i32 / 2 < work.y() + work.height() / 2;
            let next_card_y = Self::card_y(&state, 0);
            // Changing fan direction must not teleport the physical front card.
            self.0
                .window
                .move_(x, y + (previous_card_y - next_card_y).round() as i32);
            placement = Some(placement_for_corner(state.right, state.top));
        }
        if let Some(placement) = placement {
            (self.0.callbacks.placement)(placement);
        }
        self.reflow();
    }

    fn set_expanded(&self, expanded: bool) {
        {
            let mut state = self.0.state.borrow_mut();
            if state.expanded == expanded {
                return;
            }
            state.expanded = expanded;
            state.hovering = false;
            state.scroll_start = state.scroll_start.min(state.cards.len().saturating_sub(1));
        }
        // The collapsed pile never takes focus. An explicit expand enables real
        // keyboard traversal of native controls; collapse restores no-focus.
        self.0.window.set_accept_focus(expanded);
        self.animate();
        self.reflow();
    }

    fn scroll(&self, direction: isize) {
        let mut state = self.0.state.borrow_mut();
        let max = state.cards.len().saturating_sub(VISIBLE_CARDS);
        state.scroll_start =
            (state.scroll_start as isize + direction).clamp(0, max as isize) as usize;
        drop(state);
        self.ensure_visible_previews();
        self.reflow();
        self.0.area.queue_draw();
    }

    fn clear_all(&self) {
        let ids: Vec<u64> = self
            .0
            .state
            .borrow()
            .cards
            .iter()
            .map(|card| card.id)
            .collect();
        for (index, id) in ids.into_iter().enumerate() {
            self.exit(id, ExitKind::Dismiss, (index.min(6) * 55) as f64);
        }
    }

    fn confirm_delete(&self, id: u64) {
        let Some(path) = self.card_path(id) else {
            return;
        };
        let prompt = format!("Delete {} permanently?", card_name(&path));
        let dialog = gtk::MessageDialog::new(
            Some(&self.0.window),
            gtk::DialogFlags::MODAL,
            gtk::MessageType::Warning,
            gtk::ButtonsType::None,
            &prompt,
        );
        dialog.add_buttons(&[
            ("Cancel", gtk::ResponseType::Cancel),
            ("Delete", gtk::ResponseType::Accept),
        ]);
        dialog.set_secondary_text(Some(
            "Close keeps the saved file and Capture History entry.",
        ));
        let confirmed = dialog.run() == gtk::ResponseType::Accept;
        dialog.close();
        if confirmed {
            self.exit(id, ExitKind::Delete, 0.);
        }
    }

    fn exit(&self, id: u64, kind: ExitKind, delay_ms: f64) {
        let mut state = self.0.state.borrow_mut();
        let Some(index) = state.cards.iter().position(|card| card.id == id) else {
            return;
        };
        let slot_index = Self::visual_index(&state, index);
        let y = Self::card_y(&state, slot_index.saturating_sub(state.scroll_start));
        let card = state.cards.remove(index);
        self.0.fixed.remove(&card.shell);
        let origin = if state.right {
            CARD_W - if card.metadata.saved { 57.5 } else { 22.5 }
        } else if card.metadata.saved {
            57.5
        } else {
            22.5
        };
        state.exits.push(Exit {
            card,
            kind,
            started: Instant::now(),
            delay_ms,
            slot_index,
            y,
            particles: if kind == ExitKind::Delete {
                particles(origin)
            } else {
                vec![]
            },
        });
        let max = state.cards.len().saturating_sub(VISIBLE_CARDS);
        state.scroll_start = state.scroll_start.min(max);
        drop(state);
        self.reflow();
        self.animate();
    }

    fn run_for_card(&self, id: u64, action: impl FnOnce(&PreviewCallbacks, PathBuf)) {
        if let Some(path) = self.card_path(id) {
            action(&self.0.callbacks, path);
        }
    }

    fn card_path(&self, id: u64) -> Option<PathBuf> {
        self.0
            .state
            .borrow()
            .cards
            .iter()
            .find(|card| card.id == id)
            .map(|card| card.path.clone())
    }

    fn copy_card(&self, id: u64) -> bool {
        let Some(path) = self.card_path(id) else {
            return false;
        };
        let Ok(image) = Pixbuf::from_file(path) else {
            return false;
        };
        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD).set_image(&image);
        if let Some(card) = self
            .0
            .state
            .borrow_mut()
            .cards
            .iter_mut()
            .find(|card| card.id == id)
        {
            card.metadata.copied = true;
        }
        true
    }

    fn finish_file_drag(&self, id: u64, context: &gdk::DragContext) {
        let pointer = gdk::Display::default()
            .and_then(|display| display.default_seat())
            .and_then(|seat| seat.pointer())
            .map(|device| device.position())
            .map(|(_, x, y)| (x, y));
        let (wx, wy) = self.0.window.position();
        let inside = pointer.is_some_and(|(x, y)| {
            x >= wx && x < wx + (CARD_W + CARD_X * 2.) as i32 && y >= wy && y < wy + HEIGHT
        });
        if inside {
            if let Some(card) = self
                .0
                .state
                .borrow_mut()
                .cards
                .iter_mut()
                .find(|card| card.id == id)
            {
                card.shake_started = Some(Instant::now());
            }
            self.animate();
        } else if !context.selected_action().is_empty() {
            self.exit(id, ExitKind::Dismiss, 0.);
        }
    }

    fn bound_decoded_previews(&self) {
        let mut state = self.0.state.borrow_mut();
        for card in state.cards.iter_mut().skip(DECODED_PREVIEW_LIMIT) {
            // Keep every card/path/action accessible; evict only decoded pixels.
            card.preview = None;
            card.blurred_preview = None;
        }
    }

    fn ensure_visible_previews(&self) {
        let (start, expanded) = {
            let state = self.0.state.borrow();
            (state.scroll_start, state.expanded)
        };
        let mut state = self.0.state.borrow_mut();
        let range = if expanded {
            start..(start + VISIBLE_CARDS).min(state.cards.len())
        } else {
            0..state.cards.len().min(DECODED_PREVIEW_LIMIT)
        };
        for index in range {
            if state.cards[index].preview.is_none() {
                state.cards[index].preview = decode_poster(&state.cards[index].poster)
                    .or_else(|| load_preview(&state.cards[index].path));
            }
            if state.cards[index].blurred_preview.is_none() {
                state.cards[index].blurred_preview = blurred_poster(&state.cards[index].poster);
            }
        }
        // Bound decoded thumbnails without removing accessible card records.
        let keep_start = if expanded { start } else { 0 };
        let keep_end = if expanded {
            (start + VISIBLE_CARDS).min(state.cards.len())
        } else {
            state.cards.len().min(DECODED_PREVIEW_LIMIT)
        };
        for (index, card) in state.cards.iter_mut().enumerate() {
            if index < keep_start || index >= keep_end {
                card.preview = None;
                card.blurred_preview = None;
            }
        }
    }

    fn reflow(&self) {
        self.ensure_visible_previews();
        let state = self.0.state.borrow();
        let controls_visible = state.expanded && state.expansion > 0.94;
        let end = (state.scroll_start + VISIBLE_CARDS).min(state.cards.len());
        for (index, card) in state.cards.iter().enumerate() {
            let visual_index = Self::visual_index(&state, index);
            let local = visual_index.saturating_sub(state.scroll_start);
            let visible =
                controls_visible && visual_index >= state.scroll_start && local < VISIBLE_CARDS;
            card.shell.set_visible(visible);
            card.top_actions.set_halign(if state.right {
                gtk::Align::End
            } else {
                gtk::Align::Start
            });
            card.edit.set_halign(if state.right {
                gtk::Align::Start
            } else {
                gtk::Align::End
            });
            if visible {
                self.0.fixed.move_(
                    &card.shell,
                    CARD_X as i32,
                    Self::card_y(&state, local).round() as i32,
                );
            }
        }
        self.0
            .pile_hit
            .set_visible(!state.expanded && !state.cards.is_empty());
        set_accessible_name(
            &self.0.pile_hit,
            &format!(
                "Expand {}",
                if state.cards.len() == 1 {
                    "preview".to_owned()
                } else {
                    format!("{} previews", state.cards.len())
                }
            ),
        );
        self.0.fixed.move_(
            &self.0.pile_hit,
            CARD_X as i32,
            Self::card_y(&state, 0).round() as i32,
        );
        self.0.toolbar.set_visible(controls_visible);
        self.0.toolbar.set_direction(if state.right {
            gtk::TextDirection::Rtl
        } else {
            gtk::TextDirection::Ltr
        });
        self.0.clear.set_visible(state.cards.len() >= 2);
        self.0
            .newer
            .set_visible(controls_visible && state.scroll_start > 0);
        self.0
            .older
            .set_visible(controls_visible && end < state.cards.len());
        let toolbar_y = if state.top { 16 } else { HEIGHT - 44 };
        let toolbar_x = if state.right {
            (CARD_X + CARD_W) as i32 - self.0.toolbar.allocated_width().max(28)
        } else {
            CARD_X as i32
        };
        self.0.fixed.move_(&self.0.toolbar, toolbar_x, toolbar_y);
        self.0.fixed.move_(&self.0.newer, 147, 6);
        self.0.fixed.move_(&self.0.older, 147, HEIGHT - 28);
        drop(state);
        self.update_input_shape();
    }

    fn update_input_shape(&self) {
        let state = self.0.state.borrow();
        let input = cairo::Region::create();
        if state.expanded {
            let end = (state.scroll_start + VISIBLE_CARDS).min(state.cards.len());
            for index in state.scroll_start..end {
                let visual_index = Self::visual_index(&state, index);
                let local = visual_index.saturating_sub(state.scroll_start);
                if local >= VISIBLE_CARDS {
                    continue;
                }
                let y = Self::card_y(&state, local);
                let _ = input.union_rectangle(&cairo::RectangleInt::new(
                    CARD_X as i32 - 4,
                    y as i32 - 4,
                    CARD_W as i32 + 8,
                    CARD_H as i32 + 8,
                ));
            }
            if self.0.toolbar.is_visible() {
                let allocation = self.0.toolbar.allocation();
                let _ = input.union_rectangle(&cairo::RectangleInt::new(
                    allocation.x(),
                    allocation.y(),
                    allocation.width(),
                    allocation.height(),
                ));
            }
            for cue in [&self.0.newer, &self.0.older] {
                if cue.is_visible() {
                    let allocation = cue.allocation();
                    let _ = input.union_rectangle(&cairo::RectangleInt::new(
                        allocation.x(),
                        allocation.y(),
                        allocation.width(),
                        allocation.height(),
                    ));
                }
            }
        } else if !state.cards.is_empty() {
            for depth in 0..state.cards.len().min(DECODED_PREVIEW_LIMIT) {
                let y = Self::card_y(&state, depth);
                let _ = input.union_rectangle(&cairo::RectangleInt::new(
                    CARD_X as i32 - 12,
                    y as i32 - 12,
                    CARD_W as i32 + 24,
                    CARD_H as i32 + 24,
                ));
            }
        }
        if let Some(window) = self.0.window.window() {
            window.input_shape_combine_region(&input, 0, 0);
        }
    }

    fn animate(&self) {
        if self.0.state.borrow().ticking {
            return;
        }
        self.0.state.borrow_mut().ticking = true;
        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let mut completed = vec![];
            let done;
            let empty;
            {
                let mut state = this.0.state.borrow_mut();
                let expansion_target = if state.expanded { 1. } else { 0. };
                let hover_target = if state.hovering { 1. } else { 0. };
                state.expansion += (expansion_target - state.expansion) * 0.23;
                state.hover += (hover_target - state.hover) * 0.23;
                state.exits.retain(|exit| {
                    let duration = if exit.kind == ExitKind::Delete {
                        DELETE_MS
                    } else {
                        DISMISS_MS
                    } + exit.delay_ms;
                    if exit.started.elapsed().as_secs_f64() * 1_000. >= duration {
                        completed.push((exit.kind, exit.card.path.clone()));
                        false
                    } else {
                        true
                    }
                });
                for card in &mut state.cards {
                    if card
                        .shake_started
                        .is_some_and(|at| at.elapsed() >= Duration::from_millis(420))
                    {
                        card.shake_started = None;
                    }
                }
                done = (state.expansion - expansion_target).abs() < 0.001
                    && (state.hover - hover_target).abs() < 0.001
                    && state.exits.is_empty()
                    && state.cards.iter().all(|card| card.shake_started.is_none());
                if done {
                    state.expansion = expansion_target;
                    state.hover = hover_target;
                    state.ticking = false;
                }
                empty = state.cards.is_empty() && state.exits.is_empty();
            }
            for (kind, path) in completed {
                match kind {
                    ExitKind::Dismiss => (this.0.callbacks.dismiss)(path),
                    ExitKind::Delete => (this.0.callbacks.delete)(path),
                }
            }
            this.reflow();
            this.0.area.queue_draw();
            if empty {
                this.hide();
            }
            if done {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn card_y(state: &State, depth: usize) -> f64 {
        let compact = pose(depth as f64) * (13. + state.hover * 3.) + peek_jitter(depth);
        let step = compact * (1. - state.expansion) + depth as f64 * SLOT * state.expansion;
        if state.top {
            STACK_EDGE + step
        } else {
            HEIGHT as f64 - STACK_EDGE - CARD_H - step
        }
    }

    fn visual_index(state: &State, card_index: usize) -> usize {
        visual_index_with_slots(
            card_index,
            state.exits.iter().map(|exit| exit.slot_index).collect(),
        )
    }

    fn draw_card(
        cr: &cairo::Context,
        card: &Card,
        pose: CardPose,
        alpha: f64,
        dim: f64,
        hover_blur: bool,
    ) {
        let shake = card.shake_started.map_or(0., |started| {
            let t = (started.elapsed().as_secs_f64() / 0.42).clamp(0., 1.);
            let envelope = 1. - t;
            (t * 9. * std::f64::consts::PI).sin() * 12. * envelope
        });
        let _ = cr.save();
        cr.translate(shake, 0.);
        if pose.z == 0. && pose.tilt == 0. {
            cr.translate(pose.x, pose.y);
            Self::paint_card(cr, card, alpha, dim, hover_blur);
        } else if let Ok(surface) =
            cairo::ImageSurface::create(cairo::Format::ARgb32, CARD_W as i32 + 4, CARD_H as i32 + 4)
            && let Ok(painter) = cairo::Context::new(&surface)
        {
            painter.translate(2., 2.);
            Self::paint_card(&painter, card, 1., dim, hover_blur);
            // Cairo has affine transforms only. Project one source-pixel strip
            // at a time to preserve CSS's perspective/rotateX trapezoid, rather
            // than approximating receding cards with a uniform 2D scale.
            for row in -2..CARD_H as i32 + 2 {
                let (_, top) = pose.project(0., row as f64);
                let (_, bottom) = pose.project(0., row as f64 + 1.);
                let (left, _) = pose.project(-2., row as f64 + 0.5);
                let (right, _) = pose.project(CARD_W + 2., row as f64 + 0.5);
                let _ = cr.save();
                cr.set_antialias(cairo::Antialias::None);
                cr.rectangle(0., top, 340., bottom - top);
                cr.clip();
                cr.translate(left, top);
                cr.scale((right - left) / (CARD_W + 4.), bottom - top);
                let _ = cr.set_source_surface(&surface, 0., -(row as f64 + 2.));
                let _ = cr.paint_with_alpha(alpha);
                let _ = cr.restore();
            }
        }
        let _ = cr.restore();
    }

    fn paint_card(cr: &cairo::Context, card: &Card, alpha: f64, dim: f64, hover_blur: bool) {
        let _ = cr.save();
        rounded(cr, -1., -1., CARD_W + 2., CARD_H + 2., 13.);
        cr.set_source_rgba(1., 1., 1., 0.08 * alpha);
        let _ = cr.fill();
        rounded(cr, 0., 0., CARD_W, CARD_H, 12.);
        cr.clip();
        if let Some(preview) = if hover_blur {
            card.blurred_preview.as_ref().or(card.preview.as_ref())
        } else {
            card.preview.as_ref()
        } {
            cr.set_source_pixbuf(
                preview,
                (CARD_W - preview.width() as f64) / 2.,
                (CARD_H - preview.height() as f64) / 2.,
            );
            let _ = cr.paint_with_alpha(alpha);
        } else {
            cr.set_source_rgba(0.09, 0.09, 0.11, alpha);
            let _ = cr.paint();
        }
        if dim > 0. {
            cr.set_source_rgba(0.05, 0.05, 0.07, dim * alpha);
            let _ = cr.paint();
        }
        let _ = cr.restore();
    }

    fn draw(&self, cr: &cairo::Context) {
        cr.set_operator(cairo::Operator::Source);
        cr.set_source_rgba(0., 0., 0., 0.);
        let _ = cr.paint();
        cr.set_operator(cairo::Operator::Over);
        let state = self.0.state.borrow();
        if state.expansion < 0.999 {
            for (depth, card) in state
                .cards
                .iter()
                .take(DECODED_PREVIEW_LIMIT)
                .enumerate()
                .rev()
            {
                let compact = collapsed_pose(depth, state.hover, state.top);
                let expanded_step = depth as f64 * SLOT;
                let expanded_y = if state.top {
                    STACK_EDGE + expanded_step
                } else {
                    HEIGHT as f64 - STACK_EDGE - CARD_H - expanded_step
                };
                let progress = state.expansion;
                let fade = (1. - depth.saturating_sub(4) as f64 * 0.12).max(0.28);
                Self::draw_card(
                    cr,
                    card,
                    CardPose {
                        x: compact.x + (CARD_X - compact.x) * progress,
                        y: compact.y + (expanded_y - compact.y) * progress,
                        scale: compact.scale + (1. - compact.scale) * progress,
                        tilt: compact.tilt * (1. - progress),
                        z: compact.z * (1. - progress),
                        ..compact
                    },
                    fade,
                    (depth as f64 * 0.055).min(0.42),
                    false,
                );
            }
        } else {
            let end = (state.scroll_start + VISIBLE_CARDS).min(state.cards.len());
            for index in (state.scroll_start..end).rev() {
                let visual_index = Self::visual_index(&state, index);
                let local = visual_index.saturating_sub(state.scroll_start);
                if local >= VISIBLE_CARDS {
                    continue;
                }
                let y = Self::card_y(&state, local);
                let card = &state.cards[index];
                Self::draw_card(
                    cr,
                    card,
                    CardPose::flat(CARD_X, y),
                    1.,
                    if card.hovered { 0.5 } else { 0. },
                    card.hovered,
                );
            }
        }
        for exit in &state.exits {
            let elapsed = exit.started.elapsed().as_secs_f64() * 1_000. - exit.delay_ms;
            if elapsed < 0. {
                continue;
            }
            match exit.kind {
                ExitKind::Dismiss => {
                    let t = (elapsed / DISMISS_MS).clamp(0., 1.);
                    let eased = 1. - (1. - t).powi(3);
                    let direction = if state.right { 1. } else { -1. };
                    Self::draw_card(
                        cr,
                        &exit.card,
                        CardPose::flat(CARD_X + direction * eased * 150., exit.y),
                        1. - t,
                        t * 0.3,
                        false,
                    );
                }
                ExitKind::Delete => self.draw_dust(cr, exit, exit.y, elapsed),
            }
        }
        drop(state);
        // The first reflow can precede realization. Reapply on paint so the
        // native X11 input shape never falls back to the full transparent frame.
        self.update_input_shape();
    }

    fn draw_dust(&self, cr: &cairo::Context, exit: &Exit, y: f64, elapsed: f64) {
        let Some(preview) = &exit.card.preview else {
            return;
        };
        for particle in &exit.particles {
            let linear = ((elapsed - particle.delay) / particle.duration).clamp(0., 1.);
            if linear >= 1. {
                continue;
            }
            let t = cubic_bezier_progress(0.28, 0., 0.12, 1., linear);
            let opacity = if t <= 0.14 {
                1.
            } else if t <= 0.5 {
                1. - (t - 0.14) / 0.36 * 0.28
            } else if t <= 0.82 {
                0.72 * (1. - (t - 0.5) / 0.32)
            } else {
                0.
            };
            if opacity <= 0. {
                continue;
            }
            let (dx, dy, rotation, scale) = if t <= 0.14 {
                let mix = t / 0.14;
                (
                    particle.dx * 0.06 * mix,
                    particle.dy * 0.06 * mix,
                    particle.rotation * 0.08 * mix,
                    1. - 0.02 * mix,
                )
            } else {
                let mix = (t - 0.14) / 0.86;
                (
                    particle.dx * (0.06 + 0.94 * mix),
                    particle.dy * (0.06 + 0.94 * mix),
                    particle.rotation * (0.08 + 0.92 * mix),
                    0.98 - 0.8 * mix,
                )
            };
            let _ = cr.save();
            cr.translate(
                CARD_X + particle.x + particle.width / 2. + dx,
                y + particle.y + particle.height / 2. + dy,
            );
            cr.rotate(rotation.to_radians());
            cr.scale(scale, scale);
            cr.rectangle(
                -particle.width / 2.,
                -particle.height / 2.,
                particle.width,
                particle.height,
            );
            cr.clip();
            cr.set_source_pixbuf(
                preview,
                -particle.x - particle.width / 2. + (CARD_W - preview.width() as f64) / 2.,
                -particle.y - particle.height / 2. + (CARD_H - preview.height() as f64) / 2.,
            );
            let _ = cr.paint_with_alpha(opacity);
            let _ = cr.restore();
        }
    }
}

fn sample_bezier(t: f64, a: f64, b: f64) -> f64 {
    let inverse = 1. - t;
    3. * inverse * inverse * t * a + 3. * inverse * t * t * b + t * t * t
}

fn sample_bezier_derivative(t: f64, a: f64, b: f64) -> f64 {
    let inverse = 1. - t;
    3. * inverse * inverse * a + 6. * inverse * t * (b - a) + 3. * t * t * (1. - b)
}

fn cubic_bezier_progress(x1: f64, y1: f64, x2: f64, y2: f64, x: f64) -> f64 {
    if x <= 0. {
        return 0.;
    }
    if x >= 1. {
        return 1.;
    }
    let mut t = x;
    for _ in 0..8 {
        let delta = sample_bezier(t, x1, x2) - x;
        if delta.abs() < 1e-6 {
            return sample_bezier(t, y1, y2);
        }
        let derivative = sample_bezier_derivative(t, x1, x2);
        if derivative.abs() < 1e-6 {
            break;
        }
        t = (t - delta / derivative).clamp(0., 1.);
    }
    let (mut low, mut high) = (0., 1.);
    for _ in 0..12 {
        if sample_bezier(t, x1, x2) < x {
            low = t;
        } else {
            high = t;
        }
        t = (low + high) / 2.;
    }
    sample_bezier(t, y1, y2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receding_stack_matches_independent_web_cases() {
        assert!((pose(1.) - 0.982).abs() < 1e-9);
        assert!((pose(4.) - 3.742_857_142_857_143).abs() < 1e-9);
        assert!((pose(10.) - 8.676_470_588_235_293).abs() < 1e-9);
        assert!(pose(10.) - pose(9.) < pose(2.) - pose(1.));
    }

    #[test]
    fn jitter_is_deterministic_and_decays_like_web_layout() {
        assert!((peek_jitter(1) - -0.177_334_425_598_382_97).abs() < 1e-12);
        assert!(peek_jitter(4).abs() < peek_jitter(1).abs());
        assert_eq!(peek_jitter(0), 0.);
    }

    #[test]
    fn collapsed_corner_geometry_matches_independent_css_values() {
        let bottom_idle = collapsed_pose(2, 0., false);
        assert!((bottom_idle.x - 26.455_384_615_384_617).abs() < 1e-12);
        assert!((bottom_idle.y - 522.868_683_839_235_5).abs() < 1e-12);
        assert!((bottom_idle.scale - 0.951_730_769_230_769_3).abs() < 1e-12);
        assert_eq!(bottom_idle.origin_y, CARD_H);

        let top_hover = collapsed_pose(5, 1., true);
        assert!((top_hover.x - 25.232_758_620_689_655).abs() < 1e-12);
        assert!((top_hover.y - 125.784_521_298_047_9).abs() < 1e-12);
        assert!((top_hover.scale - 0.907_758_620_689_655_2).abs() < 1e-12);
        assert_eq!(top_hover.origin_y, 0.);

        // Jitter follows CSS gravity: it moves in opposite screen directions
        // for otherwise equivalent top and bottom placements.
        let top_idle = collapsed_pose(2, 0., true);
        assert!(
            (bottom_idle.y - (HEIGHT as f64 - STACK_EDGE - CARD_H) + (top_idle.y - STACK_EDGE))
                .abs()
                < 1e-12
        );
    }

    #[test]
    fn collapsed_projection_matches_browser_measured_bounds() {
        // Chromium's real mini-preview.css, depth 1 at bottom-left, 340×760.
        // A scale-only implementation misses the width by almost seven pixels.
        let pose = collapsed_pose(1, 0., false);
        let top_left = pose.project(0., 0.);
        let top_right = pose.project(CARD_W, 0.);
        let bottom = pose.project(0., CARD_H);
        assert!((top_left.0 - 33.939_960).abs() < 0.001);
        assert!((top_left.1 - 541.069_092).abs() < 0.001);
        assert!((top_right.0 - 304.525_379).abs() < 0.001);
        assert!((bottom.1 - 693.345_459).abs() < 0.001);
        assert_eq!(collapsed_pose(0, 1., true).project(0., 0.), (28., 52.));
    }

    #[test]
    fn delete_particles_cover_card_and_only_fly_up() {
        let left = particles(57.5);
        let right = particles(CARD_W - 57.5);
        assert_eq!(left.len(), 220);
        assert_eq!((left[0].x, left[0].y), (0., 0.));
        assert!(
            left.iter()
                .all(|particle| particle.dy < 0. && particle.duration >= 780.)
        );
        assert!(left.last().unwrap().x + left.last().unwrap().width >= CARD_W);
        assert_ne!(left[0].delay, right[0].delay);
    }

    #[test]
    fn dust_easing_matches_css_curve_boundaries() {
        assert_eq!(cubic_bezier_progress(0.28, 0., 0.12, 1., 0.), 0.);
        assert_eq!(cubic_bezier_progress(0.28, 0., 0.12, 1., 1.), 1.);
        assert!((cubic_bezier_progress(0.28, 0., 0.12, 1., 0.5) - 0.833_663).abs() < 0.000_01);
    }

    #[test]
    fn exiting_cards_hold_their_stack_slots() {
        assert_eq!(visual_index_with_slots(0, vec![]), 0);
        assert_eq!(visual_index_with_slots(0, vec![0]), 1);
        assert_eq!(visual_index_with_slots(0, vec![1]), 0);
        assert_eq!(visual_index_with_slots(1, vec![1]), 2);
        assert_eq!(visual_index_with_slots(0, vec![1, 0]), 2);
    }

    #[test]
    fn dragged_corner_matches_settings_placement_values() {
        assert_eq!(placement_for_corner(true, false), 0);
        assert_eq!(placement_for_corner(false, false), 1);
        assert_eq!(placement_for_corner(true, true), 2);
        assert_eq!(placement_for_corner(false, true), 3);
    }

    #[test]
    fn encoded_poster_restores_media_that_cannot_be_image_decoded() {
        let preview = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, true, 8, 9, 7).unwrap();
        preview.fill(0x35_71_c9_ff);
        let restored = decode_poster(&encode_poster(&preview)).unwrap();
        assert_eq!((restored.width(), restored.height()), (9, 7));
        assert_eq!(
            restored.pixel_bytes().unwrap().as_ref()[0..4],
            [0x35, 0x71, 0xc9, 0xff]
        );
    }

    #[test]
    fn hover_blur_spreads_asymmetric_edge_colors_without_swapping_sides() {
        let mut source = RgbaImage::from_pixel(9, 5, image::Rgba([0, 0, 0, 255]));
        for y in 0..5 {
            source.put_pixel(0, y, image::Rgba([255, 0, 0, 255]));
            source.put_pixel(8, y, image::Rgba([0, 0, 255, 255]));
        }
        let blurred = blur_thumbnail(&source);
        let near_left = blurred.get_pixel(1, 2).0;
        let near_right = blurred.get_pixel(7, 2).0;
        assert!(near_left[0] > near_left[2] && near_left[0] > 0);
        assert!(near_right[2] > near_right[0] && near_right[2] > 0);
        assert_eq!(blurred.dimensions(), source.dimensions());
    }
}
