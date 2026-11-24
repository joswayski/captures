//! The drag wake's Win32 side (`captures_app::preview_wake`): while a mini
//! preview's file is dragged out, the card's whole shell leaves its slot,
//! the emptied slot breathes as a glowing imprint, and a ribbon of light
//! with motes runs from it to the carried drag image anywhere on the
//! display. The shared module owns the motion and geometry; this owns the
//! clock's samples, the pointer and the drawing.
//!
//! The wake draws in a click-through overlay over the stack's display
//! (`WS_DISABLED`, layered and `WS_EX_TRANSPARENT`, answering
//! `HTTRANSPARENT`), so `WindowFromPoint`, and with it OLE's drop target
//! search and [`crate::drag::own_window_at`], never finds it. It is created
//! when a file drag begins and destroyed once the wake's end has played.
//! Coordinates are overlay DIPs, y down: the display's top-left is 0, 0.
use captures_app::preview::PreviewDragOutcome;
use captures_app::preview_wake::{
    DragWake, MOTE_HALO_OPACITY, MOTE_HALO_SCALE, SLOT_GLOW_BLUR, SLOT_GLOW_OPACITY,
    SLOT_LINE_OPACITY, SLOT_LINE_WIDTH, SLOT_SHADE_OPACITY, WakeEnd, WakeFrame, WakeInk, WakeRect,
};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use super::Rect;
use crate::gfx::{self, Gfx, rect};
use crate::tokens::Tokens;

/// How a file drag's end shows: an accepted drop elsewhere takes the card
/// with it; a cancel, a refused drop, a drop back on the stack or on
/// another Captures window, a failure, or a drag let go before it began
/// draws the ribbon back and the card re-forms.
pub fn end_for(outcome: Option<PreviewDragOutcome>) -> WakeEnd {
    match outcome {
        Some(PreviewDragOutcome::Dismiss) => WakeEnd::Dropped,
        Some(PreviewDragOutcome::Reject | PreviewDragOutcome::Retain) | None => WakeEnd::Returned,
    }
}

/// The overlay's space: its display's top-left (physical pixels) and the
/// display's DIP scale, which the stack window shares.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    pub origin: POINT,
    pub scale: f64,
}

impl Space {
    /// A card's slot, `card` in the stack window at `window` (pixels).
    pub fn slot(&self, window: POINT, card: Rect, radius: f32) -> WakeRect {
        let x = f64::from(window.x - self.origin.x) / self.scale;
        let y = f64::from(window.y - self.origin.y) / self.scale;
        WakeRect {
            x: x + f64::from(card.left),
            y: y + f64::from(card.top),
            width: f64::from(card.right - card.left),
            height: f64::from(card.bottom - card.top),
            radius: f64::from(radius),
        }
    }

    /// The drag image's centre for the pointer at `cursor` (pixels):
    /// `drag::set_image` hangs the 284 × 160 pixel image from the pointer
    /// by its top-left corner.
    pub fn tip(&self, cursor: POINT) -> (f64, f64) {
        let half = (
            f64::from(crate::drag::ICON_WIDTH) / 2.,
            f64::from(crate::drag::ICON_HEIGHT) / 2.,
        );
        (
            (f64::from(cursor.x - self.origin.x) + half.0) / self.scale,
            (f64::from(cursor.y - self.origin.y) + half.1) / self.scale,
        )
    }
}

/// One drag's wake: the card it left, its motion and the overlay it draws
/// in (`None` when the overlay could not be made; the card still leaves
/// and returns).
pub struct Wake {
    pub id: String,
    pub space: Space,
    pub drag: DragWake,
    overlay: Option<Overlay>,
}

impl Wake {
    pub fn new(g: &Gfx, id: String, monitor: RECT, scale: f64, drag: DragWake) -> Self {
        let overlay = Overlay::open(g, monitor)
            .map_err(|error| eprintln!("Could not open the drag wake overlay: {error}"))
            .ok();
        Self {
            id,
            space: Space {
                origin: POINT {
                    x: monitor.left,
                    y: monitor.top,
                },
                scale,
            },
            drag,
            overlay,
        }
    }

    /// The overlay window, for keeping the stack just below it.
    pub fn hwnd(&self) -> Option<HWND> {
        self.overlay.as_ref().map(|o| o.hwnd)
    }

    /// Show the overlay while the stack shows (it hides with the stack
    /// under capture UI unless Include mini previews in captures keeps it).
    pub fn show(&self, visible: bool) {
        if let Some(overlay) = &self.overlay {
            unsafe {
                let _ = ShowWindow(
                    overlay.hwnd,
                    if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
                );
            }
        }
    }

    /// Sample the wake at `now_ms` and draw it; `None` once it finished.
    pub fn tick(&mut self, g: &Gfx, t: &Tokens, now_ms: f64) -> Option<WakeFrame> {
        let frame = self.drag.frame(now_ms)?;
        if let Some(overlay) = &mut self.overlay
            && let Err(error) = overlay.draw(g, t, self.space.scale, &frame)
        {
            eprintln!("Could not draw the drag wake: {error}");
            self.overlay = None;
        }
        Some(frame)
    }
}

/// The click-through overlay over the stack's display.
struct Overlay {
    hwnd: HWND,
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual2,
    surface: IDCompositionSurface,
    px: (i32, i32),
    /// The pixels the last frame drew, cleared by the next; `None` until
    /// the first frame, which draws the whole surface.
    drawn: Option<RECT>,
}

impl Overlay {
    fn open(g: &Gfx, monitor: RECT) -> Result<Self> {
        let dcomp = g.dcomp.clone().ok_or_else(Error::empty)?;
        let px = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        let hwnd = unsafe { window(monitor)? };
        let parts = (|| unsafe {
            let surface = g.surface(px.0.max(1) as u32, px.1.max(1) as u32)?;
            let target = dcomp.CreateTargetForHwnd(hwnd, true)?;
            let visual = dcomp.CreateVisual()?;
            visual.SetContent(&surface)?;
            target.SetRoot(&visual)?;
            Ok((target, visual, surface))
        })();
        match parts {
            Ok((target, visual, surface)) => Ok(Self {
                hwnd,
                _target: target,
                _visual: visual,
                surface,
                px,
                drawn: None,
            }),
            Err(error) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                Err(error)
            }
        }
    }

    /// Redraw what changed: the last frame's pixels and this one's.
    fn draw(&mut self, g: &Gfx, t: &Tokens, scale: f64, frame: &WakeFrame) -> Result<()> {
        let surface = RECT {
            left: 0,
            top: 0,
            right: self.px.0,
            bottom: self.px.1,
        };
        let (x, y, w, h) = frame.bounds;
        let now = clamp(
            RECT {
                left: (x * scale).floor() as i32 - 2,
                top: (y * scale).floor() as i32 - 2,
                right: ((x + w) * scale).ceil() as i32 + 2,
                bottom: ((y + h) * scale).ceil() as i32 + 2,
            },
            surface,
        );
        let dirty = match self.drawn {
            Some(before) => union(before, now),
            None => surface,
        };
        self.drawn = Some(now);
        if empty(dirty) {
            return Ok(());
        }
        let s = scale as f32;
        unsafe {
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = self.surface.BeginDraw(Some(&dirty), &mut offset)?;
            dc.SetDpi(96., 96.);
            // Atlased surfaces share a texture: clip to the update before
            // clearing it.
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            dc.PushAxisAlignedClip(
                &rect(
                    0.,
                    0.,
                    (dirty.right - dirty.left) as f32,
                    (dirty.bottom - dirty.top) as f32,
                ),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
            dc.Clear(Some(&D2D1_COLOR_F::default()));
            dc.SetTransform(&Matrix3x2 {
                M11: s,
                M12: 0.,
                M21: 0.,
                M22: s,
                M31: (offset.x - dirty.left) as f32,
                M32: (offset.y - dirty.top) as f32,
            });
            let painted = paint(&dc, t, frame);
            dc.PopAxisAlignedClip();
            self.surface.EndDraw()?;
            painted?;
            g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit()
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn empty(r: RECT) -> bool {
    r.right <= r.left || r.bottom <= r.top
}

fn clamp(r: RECT, to: RECT) -> RECT {
    RECT {
        left: r.left.max(to.left),
        top: r.top.max(to.top),
        right: r.right.min(to.right),
        bottom: r.bottom.min(to.bottom),
    }
}

fn union(a: RECT, b: RECT) -> RECT {
    match (empty(a), empty(b)) {
        (true, _) => b,
        (_, true) => a,
        _ => RECT {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        },
    }
}

/// A wake ink token at `alpha` (times the token's own alpha).
fn ink(t: &Tokens, ink: WakeInk, alpha: f64) -> D2D1_COLOR_F {
    t.color_alpha(
        match ink {
            WakeInk::Accent => "theme-accent",
            WakeInk::Light => "glass-text",
        },
        alpha as f32,
    )
}

/// One wake frame, per [`WakeFrame`]'s docs: the slot's accent glow,
/// `--glass-veil` pocket and light hairline; the ribbon's passes, outermost
/// first; the motes' accent halos and light cores; an accepted drop's bloom.
pub fn paint(dc: &ID2D1DeviceContext, t: &Tokens, frame: &WakeFrame) -> Result<()> {
    let slot = frame.slot;
    let o = frame.slot_opacity;
    if o > 0.001 {
        let r = rect(
            slot.x as f32,
            slot.y as f32,
            (slot.x + slot.width) as f32,
            (slot.y + slot.height) as f32,
        );
        let radius = slot.radius as f32;
        gfx::glow(
            dc,
            r,
            radius,
            SLOT_GLOW_BLUR as f32,
            ink(t, WakeInk::Accent, SLOT_GLOW_OPACITY * o),
        )?;
        // `--glass-veil` is rgb(10, 10, 14) at its own 0.4; the pocket
        // takes its colour at SLOT_SHADE_OPACITY.
        let veil = D2D1_COLOR_F {
            a: (SLOT_SHADE_OPACITY * o) as f32,
            ..t.color("glass-veil")
        };
        gfx::fill_round(dc, r, radius, veil)?;
        gfx::ring(
            dc,
            r,
            radius,
            -SLOT_LINE_WIDTH as f32,
            0.,
            ink(t, WakeInk::Light, SLOT_LINE_OPACITY * o),
        )?;
    }
    let factory = unsafe { dc.GetFactory()? };
    let target: &ID2D1RenderTarget = dc;
    for ribbon in &frame.ribbons {
        let Some((first, rest)) = ribbon.outline.split_first() else {
            continue;
        };
        let point = |&(x, y): &(f64, f64)| Vector2 {
            X: x as f32,
            Y: y as f32,
        };
        let rest: Vec<Vector2> = rest.iter().map(point).collect();
        let stops = ribbon.stops.map(|(offset, alpha)| D2D1_GRADIENT_STOP {
            position: offset as f32,
            color: ink(t, ribbon.ink, alpha),
        });
        unsafe {
            let geometry = factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            sink.SetFillMode(D2D1_FILL_MODE_WINDING);
            sink.BeginFigure(point(first), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLines(&rest);
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            let collection = target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let brush = target.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: point(&ribbon.from),
                    endPoint: point(&ribbon.to),
                },
                None,
                &collection,
            )?;
            dc.FillGeometry(&geometry, &brush, None);
        }
    }
    for mote in &frame.motes {
        halo(
            dc,
            (mote.x, mote.y),
            mote.radius * MOTE_HALO_SCALE,
            ink(t, WakeInk::Accent, mote.opacity * MOTE_HALO_OPACITY),
        )?;
        let r = mote.radius as f32;
        unsafe {
            dc.FillEllipse(
                &D2D1_ELLIPSE {
                    point: Vector2 {
                        X: mote.x as f32,
                        Y: mote.y as f32,
                    },
                    radiusX: r,
                    radiusY: r,
                },
                &gfx::brush(dc, ink(t, WakeInk::Light, mote.opacity))?,
            );
        }
    }
    if let Some(bloom) = frame.bloom {
        halo(
            dc,
            (bloom.x, bloom.y),
            bloom.radius,
            ink(t, WakeInk::Accent, bloom.opacity),
        )?;
    }
    Ok(())
}

/// A radial glow of `radius` fading from `color` at its centre to clear.
fn halo(
    dc: &ID2D1DeviceContext,
    (x, y): (f64, f64),
    radius: f64,
    color: D2D1_COLOR_F,
) -> Result<()> {
    if radius <= 0. || color.a <= 0. {
        return Ok(());
    }
    let center = Vector2 {
        X: x as f32,
        Y: y as f32,
    };
    let r = radius as f32;
    let target: &ID2D1RenderTarget = dc;
    unsafe {
        let collection = target.CreateGradientStopCollection(
            &[
                D2D1_GRADIENT_STOP {
                    position: 0.,
                    color,
                },
                D2D1_GRADIENT_STOP {
                    position: 1.,
                    color: D2D1_COLOR_F { a: 0., ..color },
                },
            ],
            D2D1_GAMMA_2_2,
            D2D1_EXTEND_MODE_CLAMP,
        )?;
        let brush = target.CreateRadialGradientBrush(
            &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                center,
                gradientOriginOffset: Vector2::default(),
                radiusX: r,
                radiusY: r,
            },
            None,
            &collection,
        )?;
        dc.FillEllipse(
            &D2D1_ELLIPSE {
                point: center,
                radiusX: r,
                radiusY: r,
            },
            &brush,
        );
    }
    Ok(())
}

extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Never a hit-test target, even where a layered window would be one.
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// The overlay's window over `monitor` (pixels), shown without activating.
unsafe fn window(monitor: RECT) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("CapturesPreviewWake");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(overlay_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let hwnd = CreateWindowExW(
            // Layered and transparent: input and hit tests pass through.
            WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT,
            class,
            w!("Captures Drag Wake"),
            WS_POPUP | WS_DISABLED,
            monitor.left,
            monitor.top,
            monitor.right - monitor.left,
            monitor.bottom - monitor.top,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        Ok(hwnd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_accepted_drop_elsewhere_takes_the_card_with_it() {
        assert_eq!(end_for(Some(PreviewDragOutcome::Dismiss)), WakeEnd::Dropped);
        // A drop back on the stack (shaken), on another Captures window or
        // refused, a cancel, a failure or a drag let go before it began.
        assert_eq!(end_for(Some(PreviewDragOutcome::Reject)), WakeEnd::Returned);
        assert_eq!(end_for(Some(PreviewDragOutcome::Retain)), WakeEnd::Returned);
        assert_eq!(end_for(None), WakeEnd::Returned);
    }

    #[test]
    fn the_tip_is_the_drag_images_centre_in_display_dips() {
        // A display at -1920, 0 scaled 150 %.
        let space = Space {
            origin: POINT { x: -1920, y: 0 },
            scale: 1.5,
        };
        // The 284 × 160 px image hangs from the pointer by its top-left.
        assert_eq!(space.tip(POINT { x: -1920, y: 0 }), (142. / 1.5, 80. / 1.5));
        assert_eq!(
            space.tip(POINT { x: -1620, y: 300 }),
            (442. / 1.5, 380. / 1.5)
        );
        // The stack window at 30, 600 px on that display, a card 8 DIPs in.
        let slot = space.slot(POINT { x: -1890, y: 600 }, rect(8., 12., 248., 147.), 14.);
        assert_eq!(
            (slot.x, slot.y, slot.width, slot.height, slot.radius),
            (28., 412., 240., 135., 14.)
        );
    }

    #[test]
    fn hit_tests_and_drop_targets_never_find_the_overlay() {
        let monitor = RECT {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        let hwnd = unsafe { window(monitor) }.unwrap();
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        let point = POINT { x: 200, y: 150 };
        assert_ne!(crate::drag::own_window_at(point), Some(hwnd));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}
