//! "When you drag a preview out": the three mini preview drag styles as a
//! radio group of tiles, each a looping animation of
//! `preview_drag_style::demo` drawn with render nodes, its name below.
//! A tile's demo ticks on the frame clock only while it is mapped, and
//! holds the shared still frame under reduced motion.
use std::cell::Cell;
use std::rc::Rc;

use captures_app::preferences;
use captures_app::preview_drag_style::{self as drag_style, CardPose, DEMO_CARD, DEMO_SIZE};
use captures_settings::{AppSettings, MiniPreviewDragStyle};
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use serde_json::Value;

use super::{Prefs, checked, label, role_button};
use crate::tokens::Tokens;
use crate::widgets::SnapshotArea;

/// The group (dimmed while mini previews are off) and its tiles by value.
pub(super) struct Picker {
    pub group: gtk::Box,
    pub tiles: Vec<(&'static str, gtk::Button)>,
}

/// The persisted `mini_preview_drag_style` value.
pub(super) fn drag_style_id(settings: &AppSettings) -> &'static str {
    match settings.mini_preview_drag_style {
        MiniPreviewDragStyle::Frosted => "frosted",
        MiniPreviewDragStyle::Dock => "dock",
        MiniPreviewDragStyle::Well => "well",
    }
}

fn style_for(id: &str) -> MiniPreviewDragStyle {
    serde_json::from_value(Value::String(id.into())).unwrap_or_default()
}

/// The row and its picker; the caller keeps the tiles in sync.
pub(super) fn row(prefs: &Rc<Prefs>) -> (gtk::Box, Picker) {
    let tokens = prefs.tokens.borrow().clone();
    let row = preferences::row("mini_preview_drag_style");
    let group = glib::Object::builder::<gtk::Box>()
        .property("accessible-role", gtk::AccessibleRole::RadioGroup)
        .property("orientation", gtk::Orientation::Horizontal)
        .property("spacing", tokens.number("s-3") as i32)
        .build();
    group.add_css_class("drag-style-options");
    group.set_halign(gtk::Align::Start);
    group.update_property(&[gtk::accessible::Property::Label(row.title)]);
    let current = drag_style_id(&prefs.settings.borrow());
    let mut tiles = Vec::new();
    for (id, name) in preferences::MINI_PREVIEW_DRAG_STYLES {
        let tile = role_button(gtk::AccessibleRole::Radio);
        tile.add_css_class("drag-style-option");
        tile.set_cursor_from_name(Some("pointer"));
        tile.update_property(&[gtk::accessible::Property::Label(name)]);
        let content = gtk::Box::new(gtk::Orientation::Vertical, tokens.number("s-2") as i32);
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("drag-style-preview");
        frame.set_overflow(gtk::Overflow::Hidden);
        let demo = demo_area(prefs, style_for(id));
        demo.set_size_request(DEMO_SIZE.0 as i32, DEMO_SIZE.1 as i32);
        frame.append(&demo);
        content.append(&frame);
        let name = label(name, "drag-style-name");
        name.set_xalign(0.5);
        content.append(&name);
        tile.set_child(Some(&content));
        checked(&tile, current == id);
        let weak = Rc::downgrade(prefs);
        tile.connect_clicked(move |_| {
            if let Some(prefs) = weak.upgrade() {
                prefs.set("mini_preview_drag_style", Value::String(id.into()));
            }
        });
        group.append(&tile);
        tiles.push((id, tile));
    }
    (
        super::stack_row(row.title, row.description, &group),
        Picker { group, tiles },
    )
}

/// Reduced motion, read as the preview pile does.
fn reduced(widget: &impl IsA<gtk::Widget>) -> bool {
    !widget.settings().is_gtk_enable_animations()
}

/// One style's looping demo. It redraws on the frame clock while mapped
/// and animating; unmapped, its tick callback stops.
fn demo_area(prefs: &Rc<Prefs>, style: MiniPreviewDragStyle) -> SnapshotArea {
    let elapsed = Rc::new(Cell::new(0.));
    let still = Rc::new(Cell::new(false));
    let (weak, shown, held) = (Rc::downgrade(prefs), elapsed.clone(), still.clone());
    let area = SnapshotArea::new(move |snapshot, _, _| {
        if let Some(prefs) = weak.upgrade() {
            let tokens = prefs.tokens.borrow().clone();
            draw_demo(snapshot, &tokens, style, shown.get(), held.get());
        }
    });
    area.set_can_target(false);
    area.update_state(&[gtk::accessible::State::Hidden(true)]);
    // The page restyles it with the look.
    prefs.swatches.borrow_mut().push(area.clone());
    let ticking = Rc::new(Cell::new(false));
    area.connect_map(move |area| {
        still.set(reduced(area));
        area.queue_draw();
        if still.get() || ticking.replace(true) {
            return;
        }
        let (origin, elapsed, ticking) = (Cell::new(None::<f64>), elapsed.clone(), ticking.clone());
        area.add_tick_callback(move |area, clock| {
            if !area.is_mapped() {
                ticking.set(false);
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time() as f64 / 1000.;
            let start = origin.get().unwrap_or(now);
            origin.set(Some(start));
            elapsed.set(now - start);
            area.queue_draw();
            glib::ControlFlow::Continue
        });
    });
    area
}

fn with_alpha(color: gdk::RGBA, alpha: f64) -> gdk::RGBA {
    gdk::RGBA::new(
        color.red(),
        color.green(),
        color.blue(),
        color.alpha() * alpha as f32,
    )
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> graphene::Rect {
    graphene::Rect::new(x as f32, y as f32, w as f32, h as f32)
}

/// One frame: a small desktop, three cards in a column (the middle one
/// under the style's pose) and the drag image above everything.
fn draw_demo(
    snapshot: &gtk::Snapshot,
    tokens: &Tokens,
    style: MiniPreviewDragStyle,
    elapsed: f64,
    reduced: bool,
) {
    let frame = drag_style::demo(style, elapsed, reduced);
    let (w, h) = DEMO_SIZE;
    // The desktop the stack floats over: previews always use the fixed
    // glass palette, so the tile is the same dark glass in either appearance.
    snapshot.append_color(&tokens.rgba("glass-raised-solid"), &rect(0., 0., w, h));
    let left = drag_style::demo_card_left();
    for (index, top) in frame.card_tops.iter().enumerate() {
        let dragged = index == 1;
        let pose = if dragged { frame.pose } else { CardPose::REST };
        draw_card(snapshot, tokens, (left, *top), &pose, dragged);
    }
    if frame.ghost_opacity > 0. {
        snapshot.push_opacity(frame.ghost_opacity);
        draw_card(snapshot, tokens, frame.ghost, &CardPose::REST, true);
        snapshot.pop();
    }
}

/// A miniature card: its shadow and size chip (chrome), glass fill and a
/// two-tone "screenshot", under `pose`.
fn draw_card(
    snapshot: &gtk::Snapshot,
    tokens: &Tokens,
    (x, y): (f64, f64),
    pose: &CardPose,
    tinted: bool,
) {
    if pose.opacity <= 0. {
        return;
    }
    let (cw, ch) = DEMO_CARD;
    let radius = 4_f32;
    let corner = graphene::Size::new(radius, radius);
    let outline = gsk::RoundedRect::new(rect(0., 0., cw, ch), corner, corner, corner, corner);
    let dark = tokens.rgba("glass-strong-solid");
    snapshot.save();
    snapshot.translate(&graphene::Point::new(
        (x + cw / 2.) as f32,
        (y + ch / 2.) as f32,
    ));
    snapshot.scale(pose.scale as f32, pose.scale as f32);
    snapshot.translate(&graphene::Point::new((-cw / 2.) as f32, (-ch / 2.) as f32));
    snapshot.push_opacity(pose.opacity);
    if pose.chrome_opacity > 0. {
        snapshot.append_outset_shadow(
            &outline,
            &with_alpha(gdk::RGBA::BLACK, 0.4 * pose.chrome_opacity),
            0.,
            2.,
            0.,
            5.,
        );
    }
    snapshot.push_rounded_clip(&outline);
    if pose.fill_opacity > 0. {
        snapshot.append_color(&with_alpha(dark, pose.fill_opacity), &rect(0., 0., cw, ch));
    }
    if pose.media_opacity > 0. {
        snapshot.save();
        snapshot.translate(&graphene::Point::new((cw / 2.) as f32, (ch / 2.) as f32));
        snapshot.scale(pose.media_scale as f32, pose.media_scale as f32);
        snapshot.translate(&graphene::Point::new((-cw / 2.) as f32, (-ch / 2.) as f32));
        snapshot.push_opacity(pose.media_opacity);
        let blurred = pose.media_blur > 0.;
        if blurred {
            snapshot.push_blur(pose.media_blur);
        }
        // A light page with a title bar, a sidebar and one coloured block.
        snapshot.append_color(&tokens.rgba("glass-text"), &rect(0., 0., cw, ch));
        snapshot.append_color(&with_alpha(dark, 0.14), &rect(0., 0., cw, 5.));
        snapshot.append_color(&with_alpha(dark, 0.06), &rect(0., 5., cw * 0.24, ch - 5.));
        let block = if tinted {
            tokens.rgba("theme-accent")
        } else {
            with_alpha(dark, 0.3)
        };
        snapshot.append_color(&block, &rect(cw * 0.34, ch * 0.44, cw * 0.56, ch * 0.34));
        if blurred {
            snapshot.pop();
        }
        snapshot.pop();
        snapshot.restore();
    }
    // The size chip, with the rest of the chrome.
    if pose.chrome_opacity > 0. {
        snapshot.append_color(
            &with_alpha(tokens.rgba("glass-veil-heavy"), pose.chrome_opacity),
            &rect(3., ch - 7., 11., 4.),
        );
    }
    crate::previews::draw_imprint(
        snapshot,
        (cw as f32, ch as f32),
        radius,
        (tokens.rgba("glass-text"), tokens.rgba("glass-veil")),
        pose,
        0.5,
    );
    snapshot.pop();
    snapshot.pop();
    snapshot.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_tile_selects_and_saves_its_style() {
        let settings = AppSettings::default();
        // Frosted imprint is the default and the first tile.
        assert_eq!(drag_style_id(&settings), "frosted");
        assert_eq!(preferences::MINI_PREVIEW_DRAG_STYLES[0].0, "frosted");
        for (id, _) in preferences::MINI_PREVIEW_DRAG_STYLES {
            // A tile's click goes through `Prefs::set`'s path.
            let chosen = super::super::with_setting(
                &AppSettings {
                    mini_preview_drag_style: if id == "dock" {
                        MiniPreviewDragStyle::Well
                    } else {
                        MiniPreviewDragStyle::Dock
                    },
                    ..settings.clone()
                },
                "mini_preview_drag_style",
                Value::String(id.into()),
            )
            .expect("a new style");
            assert_eq!(drag_style_id(&chosen), id);
            assert_eq!(chosen.mini_preview_drag_style, style_for(id));
            // Choosing the selected tile again changes nothing.
            assert!(
                super::super::with_setting(
                    &chosen,
                    "mini_preview_drag_style",
                    Value::String(id.into())
                )
                .is_none()
            );
        }
        assert!(
            super::super::with_setting(
                &settings,
                "mini_preview_drag_style",
                Value::String("sparkle".into())
            )
            .is_none()
        );
    }
}
