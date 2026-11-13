//! wlr-layer-shell stack placement (wlroots compositors such as Sway, and
//! KDE): the preview window becomes a layer surface on the Top layer,
//! anchored to shipping's corner with `preview::thumbnail_geometry`'s gaps as
//! margins, so it stays above windows on every workspace without taking
//! focus until clicked. Margins count from the compositor's usable area,
//! which already excludes panels' exclusive zones. Used only when the
//! compositor offers the protocol; GNOME has none and places the toplevel
//! itself, as before.
use captures_app::preview::{self, ThumbnailMonitorBounds, ThumbnailStackOrigin};
use captures_settings::MiniPreviewPlacement;
use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// Make `window` a layer surface when the compositor supports it. Must run
/// before the window is realized; returns whether it did.
pub fn init(window: &gtk::ApplicationWindow) -> bool {
    // The protocol check itself asserts a Wayland display.
    let wayland = WidgetExt::display(window).type_().name() == "GdkWaylandDisplay";
    if !wayland || !gtk4_layer_shell::is_supported() {
        return false;
    }
    window.init_layer_shell();
    window.set_namespace(Some("captures-previews"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    true
}

/// The output the stack is on (or the first one before it maps), in
/// physical pixels at the GDK scale like the X11 placer's bounds.
pub fn bounds(window: &gtk::ApplicationWindow) -> Option<ThumbnailMonitorBounds> {
    let display = WidgetExt::display(window);
    let monitor = window
        .monitor()
        .or_else(|| display.monitor_at_surface(&window.surface()?))
        .or_else(|| display.monitors().item(0).and_downcast::<gdk::Monitor>())?;
    let scale = monitor.scale_factor();
    let full = monitor.geometry();
    let device = |logical: i32| logical * scale;
    let size = |logical: i32| device(logical).max(0) as u32;
    // Wayland exposes no work area: the compositor's usable area applies
    // through the anchor, and the shared auto-hide reserve keeps the gap.
    Some(ThumbnailMonitorBounds {
        work_x: device(full.x()),
        work_y: device(full.y()),
        work_width: size(full.width()),
        work_height: size(full.height()),
        full_x: device(full.x()),
        full_y: device(full.y()),
        full_width: size(full.width()),
        full_height: size(full.height()),
        scale_factor: f64::from(scale),
    })
}

/// Anchor the stack to its corner, or to a carried `origin`. The anchored
/// edge stays put as the window's height follows the stack.
pub fn place(
    window: &gtk::ApplicationWindow,
    count: usize,
    collapsed: bool,
    (placement, origin): (MiniPreviewPlacement, Option<ThumbnailStackOrigin>),
) {
    let Some(bounds) = bounds(window) else {
        return;
    };
    let scale = bounds.scale_factor;
    let (left, top) = (
        f64::from(bounds.full_x) / scale,
        f64::from(bounds.full_y) / scale,
    );
    let (right, bottom) = (
        left + f64::from(bounds.full_width) / scale,
        top + f64::from(bounds.full_height) / scale,
    );
    let geometry = preview::thumbnail_geometry(bounds, count.max(1), collapsed, origin, placement);
    let top_anchor = geometry.anchor.is_top();
    let right_anchor = placement.is_right();
    let margin = |logical: f64| logical.round().max(0.) as i32;
    window.set_anchor(Edge::Top, top_anchor);
    window.set_anchor(Edge::Bottom, !top_anchor);
    window.set_anchor(Edge::Left, !right_anchor);
    window.set_anchor(Edge::Right, right_anchor);
    if top_anchor {
        window.set_margin(Edge::Top, margin(geometry.y - top));
    } else {
        window.set_margin(Edge::Bottom, margin(bottom - geometry.y - geometry.height));
    }
    if right_anchor {
        window.set_margin(
            Edge::Right,
            margin(right - geometry.x - preview::THUMBNAIL_WIDTH),
        );
    } else {
        window.set_margin(Edge::Left, margin(geometry.x - left));
    }
}

/// Where the stack's window starts on its output, in logical pixels: the
/// layer surface's anchored margins, which this host set.
pub fn origin(window: &gtk::ApplicationWindow) -> Option<(f64, f64)> {
    let bounds = bounds(window)?;
    let scale = bounds.scale_factor;
    let (left, top) = (
        f64::from(bounds.full_x) / scale,
        f64::from(bounds.full_y) / scale,
    );
    let (width, height) = (
        f64::from(bounds.full_width) / scale,
        f64::from(bounds.full_height) / scale,
    );
    let x = if window.is_anchor(Edge::Right) {
        left + width - f64::from(window.margin(Edge::Right)) - f64::from(window.width())
    } else {
        left + f64::from(window.margin(Edge::Left))
    };
    let y = if window.is_anchor(Edge::Bottom) {
        top + height - f64::from(window.margin(Edge::Bottom)) - f64::from(window.height())
    } else {
        top + f64::from(window.margin(Edge::Top))
    };
    Some((x, y))
}

/// A tray notice as an Overlay layer surface in the tray corner: anchored
/// right and to the `top` or bottom edge, `margins` (right, vertical) from
/// the compositor's usable area. Keyboard focus only when clicked. Must run
/// before the window is realized; returns whether it did.
pub fn notice(window: &impl IsA<gtk::Window>, top: bool, (right, vertical): (i32, i32)) -> bool {
    let window = window.as_ref();
    let wayland = WidgetExt::display(window).type_().name() == "GdkWaylandDisplay";
    if !wayland || !gtk4_layer_shell::is_supported() {
        return false;
    }
    window.init_layer_shell();
    window.set_namespace(Some("captures-notice"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_anchor(Edge::Right, true);
    window.set_anchor(if top { Edge::Top } else { Edge::Bottom }, true);
    window.set_margin(Edge::Right, right);
    window.set_margin(if top { Edge::Top } else { Edge::Bottom }, vertical);
    true
}

/// A capture selector as an Overlay layer surface covering the whole
/// output, above panels, with the keyboard (Escape) while it is up. Must
/// run before the window is realized; returns whether it did.
pub fn cover(window: &impl IsA<gtk::Window>) -> bool {
    let window = window.as_ref();
    let wayland = WidgetExt::display(window).type_().name() == "GdkWaylandDisplay";
    if !wayland || !gtk4_layer_shell::is_supported() {
        return false;
    }
    window.init_layer_shell();
    window.set_namespace(Some("captures-selector"));
    window.set_layer(Layer::Overlay);
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, true);
    }
    true
}

/// Shipping's HUD bottom gap: the controls sit this far above the bottom of
/// the usable area, centred.
pub const RECORDING_CONTROLS_GAP: i32 = 24;

/// The recording controls as a Top layer surface anchored bottom centre,
/// [`RECORDING_CONTROLS_GAP`] above the compositor's usable area, above
/// windows on every workspace, with the keyboard only when clicked. Must run
/// before the window is realized; returns whether it did.
pub fn recording_controls(window: &impl IsA<gtk::Window>) -> bool {
    let window = window.as_ref();
    let wayland = WidgetExt::display(window).type_().name() == "GdkWaylandDisplay";
    if !wayland || !gtk4_layer_shell::is_supported() {
        return false;
    }
    window.init_layer_shell();
    window.set_namespace(Some("captures-recording-controls"));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(Edge::Bottom, RECORDING_CONTROLS_GAP);
    true
}

/// The hidden-controls notice as a click-through Overlay layer surface
/// centred where the anchored controls were: `controls_height` and
/// `height` are the two windows' heights. Never takes the keyboard.
pub fn recording_hidden(window: &impl IsA<gtk::Window>, controls_height: i32, height: i32) -> bool {
    let window = window.as_ref();
    let wayland = WidgetExt::display(window).type_().name() == "GdkWaylandDisplay";
    if !wayland || !gtk4_layer_shell::is_supported() {
        return false;
    }
    window.init_layer_shell();
    window.set_namespace(Some("captures-notice"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(
        Edge::Bottom,
        (RECORDING_CONTROLS_GAP + (controls_height - height) / 2).max(0),
    );
    true
}
