//! Native composited image stack. Layout and dust motion follow thumbnailLayout.ts / thumbnailExit.ts.
use crate::ui;
use gtk::{cairo, gdk, gdk_pixbuf::Pixbuf, glib, prelude::*};
use image::RgbaImage;
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

const CARD_W: f64 = 284.;
const CARD_H: f64 = 160.;
const PAD: f64 = 120.;
const HEIGHT: i32 = 760;

struct Card {
    image: RgbaImage,
    preview: Pixbuf,
    path: PathBuf,
}
struct Dust {
    card: Card,
    started: Instant,
    particles: Vec<Particle>,
}
struct State {
    cards: Vec<Card>,
    expanded: bool,
    expansion: f64,
    hover: f64,
    hovering: bool,
    down: bool,
    dust: Option<Dust>,
    ticking: bool,
}
struct Inner {
    window: gtk::Window,
    area: gtk::DrawingArea,
    actions: gtk::Box,
    state: RefCell<State>,
    open: Rc<dyn Fn(PathBuf)>,
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
    depth * (24. + 0.55 * depth) / (depth + 24.)
}
fn particles() -> Vec<Particle> {
    // Fixed seed makes frame-by-frame native/browser comparisons repeatable.
    let mut seed = 739_u32;
    let mut random = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        seed as f64 / u32::MAX as f64
    };
    let (cols, rows) = (20, 11); // 220 chips, same cap as the web renderer.
    let (w, h) = (CARD_W / cols as f64, CARD_H / rows as f64);
    let max_dist = (CARD_W - 22.5).hypot(CARD_H - 22.5);
    let mut result = vec![];
    for row in 0..rows {
        for col in 0..cols {
            let (x, y) = (col as f64 * w, row as f64 * h);
            let (cx, cy) = (x + w / 2., y + h / 2.);
            let wave = (cx - 22.5).hypot(cy - 22.5) / max_dist;
            let angle = (cy - 22.5).atan2(cx - 22.5);
            let delay = (wave
                + (angle * 2.7 + wave * 5.5).sin() * 0.07 * wave
                + (random() - 0.5) * 0.34 * wave * wave)
                .clamp(0., 1.12);
            result.push(Particle {
                x,
                y,
                width: w + 0.55,
                height: h + 0.55,
                dx: (cx - 22.5) / max_dist * (12. + random() * 26.) + (random() - 0.5) * 22.,
                dy: -36. - random() * 58.,
                rotation: (random() - 0.5) * 120.,
                delay: delay * 720. + random() * (18. + wave * 140.),
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

impl Preview {
    pub fn new(open: Rc<dyn Fn(PathBuf)>) -> Self {
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("Captures — Mini previews");
        window.set_decorated(false);
        window.set_keep_above(true);
        window.set_accept_focus(false);
        window.set_skip_taskbar_hint(true);
        window.set_app_paintable(true);
        window.set_resizable(false);
        if let Some(screen) = gdk::Screen::default() {
            window.set_visual(screen.rgba_visual().as_ref());
        }
        window.set_default_size((CARD_W + PAD * 2.) as i32, HEIGHT);
        let overlay = gtk::Overlay::new();
        let area = gtk::DrawingArea::new();
        area.add_events(
            gdk::EventMask::BUTTON_PRESS_MASK
                | gdk::EventMask::ENTER_NOTIFY_MASK
                | gdk::EventMask::LEAVE_NOTIFY_MASK,
        );
        overlay.add(&area);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.style_context().add_class("glass");
        actions.set_halign(gtk::Align::Start);
        actions.set_valign(gtk::Align::End);
        actions.set_margin_start(PAD as i32);
        actions.set_margin_bottom(60);
        overlay.add_overlay(&actions);
        window.add(&overlay);
        let inner = Rc::new(Inner {
            window,
            area,
            actions,
            open,
            state: RefCell::new(State {
                cards: vec![],
                expanded: false,
                expansion: 0.,
                hover: 0.,
                hovering: false,
                down: false,
                dust: None,
                ticking: false,
            }),
        });
        let this = Self(inner);
        for (name, action) in [("Expand", 0), ("Edit", 1), ("Copy", 2), ("Dismiss", 3)] {
            let button = ui::button(name);
            this.0.actions.pack_start(&button, false, false, 0);
            let this = this.clone();
            button.connect_clicked(move |button| match action {
                0 => {
                    let expanded = !this.0.state.borrow().expanded;
                    this.0.state.borrow_mut().expanded = expanded;
                    button.set_label(if expanded { "Collapse" } else { "Expand" });
                    this.animate();
                }
                1 => {
                    if let Some(card) = this.0.state.borrow().cards.first() {
                        (this.0.open)(card.path.clone());
                    }
                }
                2 => {
                    if let Some(card) = this.0.state.borrow().cards.first() {
                        gtk::Clipboard::get(&gdk::SELECTION_CLIPBOARD)
                            .set_image(&ui::pixbuf(&card.image));
                    }
                }
                _ => this.dismiss(),
            });
        }
        {
            let this = this.clone();
            this.0.area.clone().connect_draw(move |_, cr| {
                this.draw(cr);
                glib::Propagation::Proceed
            });
        }
        {
            let this = this.clone();
            this.0.area.clone().connect_enter_notify_event(move |_, _| {
                this.0.state.borrow_mut().hovering = true;
                this.animate();
                glib::Propagation::Proceed
            });
        }
        {
            let this = this.clone();
            this.0.area.clone().connect_leave_notify_event(move |_, _| {
                this.0.state.borrow_mut().hovering = false;
                this.animate();
                glib::Propagation::Proceed
            });
        }
        {
            let this = this.clone();
            this.0
                .area
                .clone()
                .connect_button_press_event(move |_, event| {
                    if event.button() == 3 {
                        let (x, y) = event.root();
                        this.0
                            .window
                            .begin_move_drag(3, x as i32, y as i32, event.time());
                    } else if event.button() == 1 {
                        let state = this.0.state.borrow();
                        let (_, y) = event.position();
                        if state.expanded {
                            if let Some(card) =
                                state.cards.iter().take(3).enumerate().find_map(|(i, c)| {
                                    let top = Self::card_y(&state, i);
                                    (y >= top && y <= top + CARD_H).then_some(c)
                                })
                            {
                                (this.0.open)(card.path.clone());
                            }
                        } else {
                            drop(state);
                            this.0.state.borrow_mut().expanded = true;
                            this.animate();
                        }
                    }
                    glib::Propagation::Stop
                });
        }
        // Native file drag; right-button drag moves the entire floating stack.
        this.0.area.drag_source_set(
            gdk::ModifierType::BUTTON1_MASK,
            &[gtk::TargetEntry::new(
                "text/uri-list",
                gtk::TargetFlags::OTHER_APP,
                0,
            )],
            gdk::DragAction::COPY,
        );
        {
            let this = this.clone();
            this.0
                .area
                .clone()
                .connect_drag_data_get(move |_, _, data, _, _| {
                    if let Some(card) = this.0.state.borrow().cards.first()
                        && let Ok(uri) = glib::filename_to_uri(&card.path, None)
                    {
                        data.set_uris(&[uri.as_str()]);
                    }
                });
        }
        this
    }

    pub fn add(&self, image: RgbaImage, path: PathBuf) {
        let scale = (CARD_W / image.width() as f64).max(CARD_H / image.height() as f64);
        let preview = ui::pixbuf(&image)
            .scale_simple(
                (image.width() as f64 * scale).ceil() as i32,
                (image.height() as f64 * scale).ceil() as i32,
                gtk::gdk_pixbuf::InterpType::Bilinear,
            )
            .unwrap();
        self.0.state.borrow_mut().cards.insert(
            0,
            Card {
                image,
                preview,
                path,
            },
        );
        // Retain a bounded number of decoded images; all files remain in History.
        self.0.state.borrow_mut().cards.truncate(12);
        self.0.window.show_all();
        self.home(false, false);
        self.0.area.queue_draw();
    }
    pub fn hide(&self) {
        self.0.window.hide();
    }
    pub fn show(&self) {
        if !self.0.state.borrow().cards.is_empty() {
            self.0.window.show_all();
        }
    }
    pub fn home(&self, right: bool, top: bool) {
        self.0.state.borrow_mut().down = top;
        self.0.actions.set_valign(if top {
            gtk::Align::Start
        } else {
            gtk::Align::End
        });
        self.0.actions.set_margin_top(if top { 60 } else { 0 });
        self.0.actions.set_margin_bottom(if top { 0 } else { 60 });
        if let Some(display) = gdk::Display::default()
            && let Some(monitor) = display.primary_monitor().or_else(|| display.monitor(0))
        {
            let r = monitor.workarea();
            self.0.window.move_(
                if right {
                    r.x() + r.width() - (CARD_W + PAD * 2.) as i32
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
        self.0.area.queue_draw();
    }
    pub fn dismiss(&self) {
        let mut state = self.0.state.borrow_mut();
        if state.cards.is_empty() || state.dust.is_some() {
            return;
        }
        let card = state.cards.remove(0);
        state.dust = Some(Dust {
            card,
            started: Instant::now(),
            particles: particles(),
        });
        drop(state);
        self.animate();
    }
    fn animate(&self) {
        if self.0.state.borrow().ticking {
            return;
        }
        self.0.state.borrow_mut().ticking = true;
        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(16), move || {
            let mut state = this.0.state.borrow_mut();
            let expansion = if state.expanded { 1. } else { 0. };
            let hover = if state.hovering { 1. } else { 0. };
            state.expansion += (expansion - state.expansion) * 0.23;
            state.hover += (hover - state.hover) * 0.23;
            if state
                .dust
                .as_ref()
                .is_some_and(|d| d.started.elapsed().as_millis() > 2200)
            {
                state.dust = None;
            }
            let done = (state.expansion - expansion).abs() < 0.001
                && (state.hover - hover).abs() < 0.001
                && state.dust.is_none();
            if done {
                state.expansion = expansion;
                state.hover = hover;
                state.ticking = false;
            }
            let empty = state.cards.is_empty() && state.dust.is_none();
            drop(state);
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
    fn card_y(state: &State, index: usize) -> f64 {
        let step = pose(index as f64) * (13. + state.hover * 3.) * (1. - state.expansion)
            + index as f64 * 184. * state.expansion;
        if state.down {
            PAD + step
        } else {
            HEIGHT as f64 - PAD - CARD_H - step
        }
    }
    fn draw(&self, cr: &cairo::Context) {
        cr.set_operator(cairo::Operator::Source);
        cr.set_source_rgba(0., 0., 0., 0.);
        let _ = cr.paint();
        cr.set_operator(cairo::Operator::Over);
        let state = self.0.state.borrow();
        let input = cairo::Region::create();
        for (i, card) in state.cards.iter().take(3).enumerate().rev() {
            let y = Self::card_y(&state, i);
            let _ = input.union_rectangle(&cairo::RectangleInt::new(
                PAD as i32 - 12,
                y as i32 - 12,
                CARD_W as i32 + 24,
                CARD_H as i32 + 24,
            ));
            let _ = cr.save();
            cr.translate(PAD + CARD_W / 2., y + CARD_H / 2.);
            let angle = if i % 2 == 0 { -2.8_f64 } else { 2.9_f64 };
            cr.rotate((angle * (1. - state.expansion) * (i > 0) as u8 as f64).to_radians());
            cr.translate(-CARD_W / 2., -CARD_H / 2.);
            rounded(cr, -2., -2., CARD_W + 4., CARD_H + 4., 14.);
            cr.set_source_rgba(1., 1., 1., 0.65);
            let _ = cr.fill();
            rounded(cr, 0., 0., CARD_W, CARD_H, 12.);
            cr.clip();
            cr.set_source_pixbuf(
                &card.preview,
                (CARD_W - card.preview.width() as f64) / 2.,
                (CARD_H - card.preview.height() as f64) / 2.,
            );
            let _ = cr.paint();
            let _ = cr.restore();
        }
        if let Some(dust) = &state.dust {
            let y = if state.down {
                PAD
            } else {
                HEIGHT as f64 - PAD - CARD_H
            };
            let time = dust.started.elapsed().as_secs_f64() * 1000.;
            for p in &dust.particles {
                let progress = ((time - p.delay) / p.duration).clamp(0., 1.);
                if progress >= 1. {
                    continue;
                }
                let eased = 1. - (1. - progress).powi(3);
                let _ = cr.save();
                cr.translate(
                    PAD + p.x + p.width / 2. + p.dx * eased,
                    y + p.y + p.height / 2. + p.dy * eased,
                );
                cr.rotate((p.rotation * eased).to_radians());
                cr.scale(1. - progress * 0.65, 1. - progress * 0.65);
                cr.rectangle(-p.width / 2., -p.height / 2., p.width, p.height);
                cr.clip();
                cr.set_source_pixbuf(
                    &dust.card.preview,
                    -p.x - p.width / 2. + (CARD_W - dust.card.preview.width() as f64) / 2.,
                    -p.y - p.height / 2. + (CARD_H - dust.card.preview.height() as f64) / 2.,
                );
                let _ = cr.paint_with_alpha(1. - progress);
                let _ = cr.restore();
            }
        }
        let actions = self.0.actions.allocation();
        let _ = input.union_rectangle(&cairo::RectangleInt::new(
            actions.x(),
            actions.y(),
            actions.width(),
            actions.height(),
        ));
        if let Some(window) = self.0.window.window() {
            window.input_shape_combine_region(&input, 0, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receding_stack_uses_web_layout_formula() {
        assert!((pose(1.) - 0.982).abs() < 1e-9);
        assert!((pose(4.) - 3.742857142857143).abs() < 1e-9);
        assert!(pose(10.) - pose(9.) < pose(2.) - pose(1.));
    }
    #[test]
    fn dust_covers_image_with_upward_flights_and_delayed_wave() {
        let p = particles();
        assert_eq!(p.len(), 220);
        assert_eq!((p[0].x, p[0].y), (0., 0.));
        assert!(p.iter().all(|p| p.dy < 0. && p.duration >= 780.));
        assert!(p.last().unwrap().delay > p.first().unwrap().delay + 300.);
        assert!(p.last().unwrap().x + p.last().unwrap().width >= CARD_W);
    }
}
