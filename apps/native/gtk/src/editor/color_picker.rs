//! Captures' custom-color picker (`editor_chrome::colors::picker`): a
//! hue/saturation wheel, a brightness track and, for colors that keep alpha,
//! an opacity track, in a token-styled popover. It replaces GTK's color
//! chooser dialog for the editor's custom tile and the Preferences theme
//! colors. Tab moves between channels and the arrow keys step them.
use std::cell::{Cell, RefCell};
use std::f64::consts::TAU;
use std::rc::Rc;

use captures_app::editor_chrome::colors::picker::{self, Hsva};
use gtk::prelude::*;
use gtk::{cairo, gdk, graphene};

use crate::tokens::{LiveTokens, Tokens};
use crate::widgets::SnapshotArea;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Channel {
    Hue,
    Saturation,
    Brightness,
    Opacity,
}

/// The wheel bitmap's pixels per point; sharp on HiDPI, cheap on 1x.
const WHEEL_SCALE: f64 = 2.;

struct State {
    color: Cell<Hsva>,
    alpha: bool,
    focused: Cell<Channel>,
    show_focus: Cell<bool>,
    dragging: Cell<Option<Channel>>,
    wheel: RefCell<Option<(f64, cairo::ImageSurface)>>,
}

impl State {
    fn channels(&self) -> Vec<Channel> {
        let all = [
            Channel::Hue,
            Channel::Saturation,
            Channel::Brightness,
            Channel::Opacity,
        ];
        all.into_iter()
            .filter(|channel| self.alpha || *channel != Channel::Opacity)
            .collect()
    }

    fn adjust(&self, channel: Channel, steps: f64) {
        let mut color = self.color.get();
        let delta = steps * picker::STEP;
        match channel {
            Channel::Hue => color.h = (color.h + delta).rem_euclid(1.),
            Channel::Saturation => color.s = (color.s + delta).clamp(0., 1.),
            Channel::Brightness => color.v = (color.v + delta).clamp(0., 1.),
            Channel::Opacity if self.alpha => color.a = (color.a + delta).clamp(0., 1.),
            Channel::Opacity => {}
        }
        self.focused.set(channel);
        self.color.set(color);
    }

    /// The channel a press at `(x, y)` drags, if any.
    fn hit(&self, x: f64, y: f64) -> Option<Channel> {
        let radius = picker::WHEEL / 2.;
        let center = picker::PADDING + radius;
        let reach = picker::MARKER / 2.;
        if (x - center).hypot(y - center) <= radius + reach {
            return Some(Channel::Hue);
        }
        let in_row = |index: usize| {
            let top = picker::slider_top(index);
            (picker::PADDING..=picker::PADDING + picker::WHEEL).contains(&x)
                && (top - reach..=top + picker::SLIDER + reach).contains(&y)
        };
        if in_row(0) {
            Some(Channel::Brightness)
        } else if self.alpha && in_row(1) {
            Some(Channel::Opacity)
        } else {
            None
        }
    }

    /// `Hsva::pick_wheel` or `Hsva::track_value` for the dragged channel.
    fn pick(&self, x: f64, y: f64) -> bool {
        let Some(channel) = self.dragging.get() else {
            return false;
        };
        let mut color = self.color.get();
        let radius = picker::WHEEL / 2.;
        let center = picker::PADDING + radius;
        let (left, width) = track_span();
        match channel {
            Channel::Hue | Channel::Saturation => {
                color.pick_wheel(x - center, y - center, radius);
            }
            Channel::Brightness => color.v = Hsva::track_value(x, left, width),
            Channel::Opacity => color.a = Hsva::track_value(x, left, width),
        }
        self.focused.set(channel);
        self.color.set(color);
        true
    }

    fn description(&self) -> String {
        let color = self.color.get();
        let percent = |value: f64| format!("{}%", (value * 100.).round());
        let mut parts = vec![
            format!("{} {}°", picker::HUE, (color.h * 360.).round()),
            format!("{} {}", picker::SATURATION, percent(color.s)),
            format!("{} {}", picker::BRIGHTNESS, percent(color.v)),
        ];
        if self.alpha {
            parts.push(format!("{} {}", picker::OPACITY, percent(color.a)));
        }
        parts.join(", ")
    }
}

/// The thumb center's travel: the track inset half a thumb at each end.
fn track_span() -> (f64, f64) {
    (
        picker::PADDING + picker::SLIDER / 2.,
        picker::WHEEL - picker::SLIDER,
    )
}

/// Open the picker under `anchor`, seeded from `value`. Each change reports
/// `#rrggbb`, or `#rrggbbaa` when `alpha` allows a translucent color.
pub(crate) fn open(
    anchor: &impl IsA<gtk::Widget>,
    tokens: &LiveTokens,
    label: &str,
    value: &str,
    alpha: bool,
    changed: impl Fn(String) + 'static,
) {
    let mut seed = Hsva::parse(value).unwrap_or(Hsva {
        h: 0.,
        s: 0.,
        v: 0.,
        a: 1.,
    });
    if !alpha {
        seed.a = 1.;
    }
    let state = Rc::new(State {
        color: Cell::new(seed),
        alpha,
        focused: Cell::new(Channel::Hue),
        show_focus: Cell::new(false),
        dragging: Cell::new(None),
        wheel: RefCell::new(None),
    });
    let live = tokens.clone();
    let drawn = state.clone();
    let area = SnapshotArea::new(move |snapshot, w, h| draw(&drawn, &live.get(), snapshot, w, h));
    area.set_size_request(picker::width() as i32, picker::height(alpha) as i32);
    area.set_focusable(true);
    area.set_accessible_role(gtk::AccessibleRole::Group);
    area.update_property(&[
        gtk::accessible::Property::Label(label),
        gtk::accessible::Property::Description(&state.description()),
    ]);
    let changed = Rc::new(changed);
    let publish = {
        let state = state.clone();
        let area = area.downgrade();
        move || {
            let Some(area) = area.upgrade() else { return };
            area.queue_draw();
            area.update_property(&[gtk::accessible::Property::Description(&state.description())]);
            changed(state.color.get().hex(state.alpha));
        }
    };
    let publish = Rc::new(publish);

    let drag = gtk::GestureDrag::new();
    {
        let state = state.clone();
        let publish = publish.clone();
        drag.connect_drag_begin(move |gesture, x, y| {
            if let Some(widget) = gesture.widget() {
                widget.grab_focus();
            }
            state.dragging.set(state.hit(x, y));
            state.show_focus.set(false);
            if state.pick(x, y) {
                publish();
            }
        });
    }
    {
        let state = state.clone();
        let publish = publish.clone();
        drag.connect_drag_update(move |gesture, dx, dy| {
            if let Some((x, y)) = gesture.start_point()
                && state.pick(x + dx, y + dy)
            {
                publish();
            }
        });
    }
    {
        let state = state.clone();
        drag.connect_drag_end(move |_, _, _| state.dragging.set(None));
    }
    area.add_controller(drag);

    let popover = gtk::Popover::new();
    let keys = gtk::EventControllerKey::new();
    {
        let state = state.clone();
        let publish = publish.clone();
        let popover = popover.downgrade();
        let area_weak = area.downgrade();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            use gdk::Key;
            let step = match key {
                Key::Escape | Key::Return | Key::KP_Enter => {
                    if let Some(popover) = popover.upgrade() {
                        popover.popdown();
                    }
                    return gtk::glib::Propagation::Stop;
                }
                Key::Tab | Key::ISO_Left_Tab | Key::KP_Tab => {
                    let channels = state.channels();
                    let index = channels
                        .iter()
                        .position(|channel| *channel == state.focused.get())
                        .unwrap_or(0);
                    let back = key == Key::ISO_Left_Tab
                        || modifiers.contains(gdk::ModifierType::SHIFT_MASK);
                    let next = if back {
                        index + channels.len() - 1
                    } else {
                        index + 1
                    };
                    state.focused.set(channels[next % channels.len()]);
                    state.show_focus.set(true);
                    if let Some(area) = area_weak.upgrade() {
                        area.queue_draw();
                    }
                    return gtk::glib::Propagation::Stop;
                }
                Key::Left | Key::Down | Key::KP_Left | Key::KP_Down => -1.,
                Key::Right | Key::Up | Key::KP_Right | Key::KP_Up => 1.,
                _ => return gtk::glib::Propagation::Proceed,
            };
            state.show_focus.set(true);
            state.adjust(state.focused.get(), step);
            publish();
            gtk::glib::Propagation::Stop
        });
    }
    area.add_controller(keys);

    popover.add_css_class("captures-menu");
    popover.add_css_class("color-picker");
    popover.set_has_arrow(false);
    popover.set_child(Some(&area));
    popover.set_parent(anchor);
    let anchor = anchor.as_ref().clone();
    anchor.update_state(&[gtk::accessible::State::Expanded(Some(true))]);
    popover.connect_closed(move |popover| {
        anchor.update_state(&[gtk::accessible::State::Expanded(Some(false))]);
        let popover = popover.clone();
        gtk::glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
    area.grab_focus();
}

fn source(cr: &cairo::Context, color: &gdk::RGBA) {
    cr.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
}

fn pill(cr: &cairo::Context, x: f64, y: f64, width: f64, height: f64) {
    let radius = height / 2.;
    cr.new_sub_path();
    cr.arc(x + width - radius, y + radius, radius, -TAU / 4., TAU / 4.);
    cr.arc(x + radius, y + radius, radius, TAU / 4., TAU * 3. / 4.);
    cr.close_path();
}

/// The wheel at brightness `v`, antialiased at the rim, with hue and
/// saturation from the same `pick_wheel` the pointer uses.
fn wheel_surface(v: f64) -> Option<cairo::ImageSurface> {
    let pixels = (picker::WHEEL * WHEEL_SCALE).round() as i32;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, pixels, pixels).ok()?;
    let stride = usize::try_from(surface.stride()).ok()?;
    {
        let mut data = surface.data().ok()?;
        let radius = f64::from(pixels) / 2.;
        for y in 0..pixels {
            for x in 0..pixels {
                let (dx, dy) = (f64::from(x) + 0.5 - radius, f64::from(y) + 0.5 - radius);
                let coverage = (radius - dx.hypot(dy) + 0.5).clamp(0., 1.);
                let mut color = Hsva {
                    h: 0.,
                    s: 0.,
                    v,
                    a: 1.,
                };
                color.pick_wheel(dx, dy, radius);
                let (r, g, b) = color.rgb();
                let byte = |channel: f64| u32::from((channel * coverage * 255.).round() as u8);
                // Cairo's ARGB32 is a native-endian, premultiplied u32.
                let pixel = byte(1.) << 24 | byte(r) << 16 | byte(g) << 8 | byte(b);
                let index = y as usize * stride + x as usize * 4;
                data[index..index + 4].copy_from_slice(&pixel.to_ne_bytes());
            }
        }
    }
    surface.set_device_scale(WHEEL_SCALE, WHEEL_SCALE);
    Some(surface)
}

fn draw(state: &State, tokens: &Tokens, snapshot: &gtk::Snapshot, w: f32, h: f32) {
    let cr = snapshot.append_cairo(&graphene::Rect::new(0., 0., w, h));
    let color = state.color.get();
    let radius = picker::WHEEL / 2.;
    let center = picker::PADDING + radius;
    let focus = |cr: &cairo::Context| {
        let mut ring = tokens.rgba("theme-accent");
        ring.set_alpha(0.55);
        source(cr, &ring);
        cr.set_line_width(2.);
        let _ = cr.stroke();
    };

    let mut cache = state.wheel.borrow_mut();
    if cache.as_ref().is_none_or(|(v, _)| *v != color.v) {
        *cache = wheel_surface(color.v).map(|surface| (color.v, surface));
    }
    if let Some((_, surface)) = cache.as_ref()
        && cr
            .set_source_surface(surface, picker::PADDING, picker::PADDING)
            .is_ok()
    {
        let _ = cr.paint();
    }
    drop(cache);
    cr.arc(center, center, radius - 0.5, 0., TAU);
    source(&cr, &tokens.rgba("border"));
    cr.set_line_width(1.);
    let _ = cr.stroke();

    // A white ring with a dark edge reads on every color.
    let (dx, dy) = color.wheel_offset(radius);
    let (mx, my, marker) = (center + dx, center + dy, picker::MARKER / 2.);
    let (r, g, b) = color.rgb();
    cr.arc(mx, my, marker, 0., TAU);
    cr.set_source_rgb(r, g, b);
    let _ = cr.fill_preserve();
    cr.set_source_rgb(1., 1., 1.);
    cr.set_line_width(2.);
    let _ = cr.stroke();
    cr.arc(mx, my, marker + 1.5, 0., TAU);
    cr.set_source_rgba(0., 0., 0., 0.35);
    cr.set_line_width(1.);
    let _ = cr.stroke();
    let focused = state.focused.get();
    if state.show_focus.get() && matches!(focused, Channel::Hue | Channel::Saturation) {
        cr.arc(center, center, radius + 2., 0., TAU);
        focus(&cr);
    }

    let full = Hsva { v: 1., ..color }.rgb();
    let mut tracks = vec![(0, Channel::Brightness, color.v)];
    if state.alpha {
        tracks.push((1, Channel::Opacity, color.a));
    }
    let (left, travel) = track_span();
    for (index, channel, value) in tracks {
        let middle = picker::slider_top(index) + picker::SLIDER / 2.;
        let (x, y) = (picker::PADDING, middle - picker::TRACK / 2.);
        let (width, height) = (picker::WHEEL, picker::TRACK);
        let gradient = cairo::LinearGradient::new(x, 0., x + width, 0.);
        if channel == Channel::Brightness {
            gradient.add_color_stop_rgb(0., 0., 0., 0.);
            gradient.add_color_stop_rgb(1., full.0, full.1, full.2);
        } else {
            // The usual transparency checkerboard under the color's ramp.
            cr.save().ok();
            pill(&cr, x, y, width, height);
            cr.clip();
            cr.set_source_rgb(1., 1., 1.);
            let _ = cr.paint();
            cr.set_source_rgb(0.8, 0.8, 0.8);
            let cell = height / 2.;
            let mut column = 0.;
            while column * cell < width {
                let row = if (column as i32) % 2 == 0 { 0. } else { cell };
                cr.rectangle(x + column * cell, y + row, cell, cell);
                column += 1.;
            }
            let _ = cr.fill();
            cr.restore().ok();
            gradient.add_color_stop_rgba(0., r, g, b, 0.);
            gradient.add_color_stop_rgba(1., r, g, b, 1.);
        }
        pill(&cr, x, y, width, height);
        let _ = cr.set_source(&gradient);
        let _ = cr.fill_preserve();
        source(&cr, &tokens.rgba("border"));
        cr.set_line_width(1.);
        let _ = cr.stroke();

        // `RangeSlider`'s raised thumb.
        let (tx, thumb) = (left + value * travel, picker::SLIDER / 2.);
        cr.arc(tx, middle + 1., thumb, 0., TAU);
        cr.set_source_rgba(0., 0., 0., 0.12);
        let _ = cr.fill();
        cr.arc(tx, middle, thumb - 0.5, 0., TAU);
        source(&cr, &tokens.rgba("surface-raised"));
        let _ = cr.fill_preserve();
        source(&cr, &tokens.rgba("border-strong"));
        cr.set_line_width(1.);
        let _ = cr.stroke();
        if state.show_focus.get() && focused == channel {
            cr.arc(tx, middle, thumb + 2., 0., TAU);
            focus(&cr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(alpha: bool) -> State {
        State {
            color: Cell::new(Hsva::parse("#336699").unwrap()),
            alpha,
            focused: Cell::new(Channel::Hue),
            show_focus: Cell::new(false),
            dragging: Cell::new(None),
            wheel: RefCell::new(None),
        }
    }

    #[test]
    fn presses_pick_the_channel_under_them_and_opacity_only_when_allowed() {
        let picker = state(true);
        let center = picker::PADDING + picker::WHEEL / 2.;
        let opacity_y = picker::slider_top(1) + picker::SLIDER / 2.;
        assert_eq!(picker.hit(center, center), Some(Channel::Hue));
        assert_eq!(picker.hit(center, opacity_y), Some(Channel::Opacity));
        assert_eq!(state(false).hit(center, opacity_y), None);
        // Dragging brightness to the track's right end, then opacity to its middle.
        let (left, travel) = track_span();
        let brightness_y = picker::slider_top(0) + picker::SLIDER / 2.;
        picker.dragging.set(picker.hit(left + travel, brightness_y));
        assert_eq!(picker.dragging.get(), Some(Channel::Brightness));
        assert!(
            picker.pick(left + travel + 30., brightness_y),
            "drags past the end clamp"
        );
        picker.dragging.set(Some(Channel::Opacity));
        assert!(picker.pick(left + travel / 2., opacity_y));
        assert_eq!(picker.color.get().hex(true), "#55aaff80");
        assert_eq!(
            picker.description(),
            "Hue 210°, Saturation 67%, Brightness 100%, Opacity 50%"
        );
        let opaque = state(false);
        opaque.adjust(Channel::Opacity, -50.);
        assert_eq!(opaque.channels().len(), 3);
        assert_eq!(opaque.color.get().hex(true), "#336699");
    }

    #[test]
    fn wheel_bitmap_matches_pointer_picking() {
        let mut surface = wheel_surface(1.).unwrap();
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        let argb = |x: usize, y: usize| {
            let index = y * stride + x * 4;
            u32::from_ne_bytes(data[index..index + 4].try_into().unwrap()).to_be_bytes()
        };
        let side = (picker::WHEEL * WHEEL_SCALE) as usize;
        let [a, r, g, b] = argb(side - 4, side / 2);
        assert!(a == 255 && r == 255 && g < 8 && b < 8, "3 o'clock is red");
        let [a, r, g, b] = argb(3, side / 2);
        assert!(a == 255 && r < 8 && g > 247 && b > 247, "9 o'clock is cyan");
        assert_eq!(argb(0, 0), [0; 4], "outside the wheel is clear");
    }
}
