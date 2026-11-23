//! Captures' custom-color picker (`editor_chrome::colors::picker`), as on
//! macOS and GTK: a hue/saturation wheel drawn at the current brightness, a
//! brightness track and, for colors that keep alpha, an opacity track over a
//! checkerboard, on a `--surface-overlay` card. It replaces the system
//! `ChooseColorW` dialog for the editor's custom tile and the Preferences
//! theme colors.
//!
//! [`pick`] opens it as an owned popup near the pointer and runs its own
//! message loop until it closes, like the dialog did, so callers keep calling
//! it with their window released. Escape, Return or a click outside close it;
//! Tab moves between channels and the arrow keys step the focused one. A
//! change is reported when a drag ends, when an arrow key is released, and
//! when it closes, so one gesture is one undo step. The window's name carries
//! the channel values for Narrator.
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use captures_app::editor_chrome::colors::picker::{self, Hsva};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, ValidateRect,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::gfx::{self, Gfx, rect};
use crate::tokens::Tokens;

const CLASS: PCWSTR = w!("CapturesColorPicker");
/// Room around the card for its `--shadow-lg` (18 px down, 44 px blur).
pub const MARGIN: f32 = 44.;
/// The pointer's distance to the card.
const GAP: f32 = 8.;
/// A press outside closes the picker; that same press on the tile that
/// opened it must not open it again.
const REOPEN_GUARD: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Hue,
    Saturation,
    Brightness,
    Opacity,
}

/// The picker's values and drawing, shared by the popup and offscreen renders.
pub struct Picker {
    pub color: Hsva,
    pub alpha: bool,
    pub focused: Channel,
    pub show_focus: bool,
    dragging: Option<Channel>,
    /// The wheel's premultiplied BGRA pixels for (brightness, side in px).
    wheel: RefCell<Option<(f64, u32, Vec<u8>)>>,
}

/// The card's size in points: `picker::width()` × `picker::height(alpha)`.
pub fn card_size(alpha: bool) -> (f32, f32) {
    (picker::width() as f32, picker::height(alpha) as f32)
}

/// The thumb center's travel: the track inset half a thumb at each end.
fn track_span() -> (f64, f64) {
    (
        picker::PADDING + picker::SLIDER / 2.,
        picker::WHEEL - picker::SLIDER,
    )
}

impl Picker {
    /// Seeded from `value` (`#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`);
    /// opaque unless `alpha`.
    pub fn new(value: &str, alpha: bool) -> Self {
        let mut color = Hsva::parse(value).unwrap_or(Hsva {
            h: 0.,
            s: 0.,
            v: 0.,
            a: 1.,
        });
        if !alpha {
            color.a = 1.;
        }
        Self {
            color,
            alpha,
            focused: Channel::Hue,
            show_focus: false,
            dragging: None,
            wheel: RefCell::new(None),
        }
    }

    pub fn hex(&self) -> String {
        self.color.hex(self.alpha)
    }

    pub fn channels(&self) -> Vec<Channel> {
        [
            Channel::Hue,
            Channel::Saturation,
            Channel::Brightness,
            Channel::Opacity,
        ]
        .into_iter()
        .filter(|channel| self.alpha || *channel != Channel::Opacity)
        .collect()
    }

    /// Step `channel` by `steps` keyboard steps (`picker::STEP` of its range).
    pub fn adjust(&mut self, channel: Channel, steps: f64) {
        let delta = steps * picker::STEP;
        let c = &mut self.color;
        match channel {
            Channel::Hue => c.h = (c.h + delta).rem_euclid(1.),
            Channel::Saturation => c.s = (c.s + delta).clamp(0., 1.),
            Channel::Brightness => c.v = (c.v + delta).clamp(0., 1.),
            Channel::Opacity if self.alpha => c.a = (c.a + delta).clamp(0., 1.),
            Channel::Opacity => {}
        }
        self.focused = channel;
    }

    /// Tab (or Shift+Tab with `back`) to the next channel.
    pub fn cycle(&mut self, back: bool) {
        let channels = self.channels();
        let index = channels
            .iter()
            .position(|c| *c == self.focused)
            .unwrap_or(0);
        let next = if back {
            index + channels.len() - 1
        } else {
            index + 1
        };
        self.focused = channels[next % channels.len()];
        self.show_focus = true;
    }

    /// The channel a press at card-local `(x, y)` drags, if any.
    pub fn hit(&self, x: f64, y: f64) -> Option<Channel> {
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

    /// Start a drag at card-local `(x, y)`; false off every channel.
    pub fn press(&mut self, x: f64, y: f64) -> bool {
        self.dragging = self.hit(x, y);
        self.show_focus = false;
        self.drag(x, y)
    }

    /// `Hsva::pick_wheel` or `Hsva::track_value` for the dragged channel.
    pub fn drag(&mut self, x: f64, y: f64) -> bool {
        let Some(channel) = self.dragging else {
            return false;
        };
        let radius = picker::WHEEL / 2.;
        let center = picker::PADDING + radius;
        let (left, width) = track_span();
        match channel {
            Channel::Hue | Channel::Saturation => {
                self.color.pick_wheel(x - center, y - center, radius);
            }
            Channel::Brightness => self.color.v = Hsva::track_value(x, left, width),
            Channel::Opacity => self.color.a = Hsva::track_value(x, left, width),
        }
        self.focused = channel;
        true
    }

    /// End a drag; true if one was running.
    pub fn release(&mut self) -> bool {
        self.dragging.take().is_some()
    }

    /// "Hue 210°, Saturation 67%, …" for assistive tech.
    pub fn description(&self) -> String {
        let c = self.color;
        let percent = |value: f64| format!("{}%", (value * 100.).round());
        let mut parts = vec![
            format!("{} {}°", picker::HUE, (c.h * 360.).round()),
            format!("{} {}", picker::SATURATION, percent(c.s)),
            format!("{} {}", picker::BRIGHTNESS, percent(c.v)),
        ];
        if self.alpha {
            parts.push(format!("{} {}", picker::OPACITY, percent(c.a)));
        }
        parts.join(", ")
    }

    /// The card with its shadow, its top-left at `origin` in points; `scale`
    /// is the device scale the wheel's pixels are made for.
    pub fn paint(
        &self,
        dc: &ID2D1DeviceContext,
        t: &Tokens,
        origin: (f32, f32),
        scale: f32,
    ) -> Result<()> {
        let (w, h) = card_size(self.alpha);
        let card = rect(origin.0, origin.1, origin.0 + w, origin.1 + h);
        let radius = t.n("r-lg");
        gfx::shadow(dc, card, radius, t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, card, radius, t.color("surface-overlay"))?;
        gfx::border(dc, card, radius, 1., t.color("border"))?;
        let mut base = Matrix3x2::identity();
        unsafe { dc.GetTransform(&mut base) };
        unsafe { dc.SetTransform(&(Matrix3x2::translation(origin.0, origin.1) * base)) };
        let painted = self.paint_content(dc, t, scale);
        unsafe { dc.SetTransform(&base) };
        painted
    }

    fn paint_content(&self, dc: &ID2D1DeviceContext, t: &Tokens, scale: f32) -> Result<()> {
        let c = self.color;
        let radius = (picker::WHEEL / 2.) as f32;
        let center = picker::PADDING as f32 + radius;
        let ellipse = |cx: f32, cy: f32, r: f32| D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: r,
            radiusY: r,
        };
        let solid = |r: f64, g: f64, b: f64, a: f32| D2D1_COLOR_F {
            r: r as f32,
            g: g as f32,
            b: b as f32,
            a,
        };
        let focus = t.color_alpha("theme-accent", 0.55);

        // The wheel bitmap, rebuilt only when brightness or scale changes.
        let side = ((picker::WHEEL as f32) * scale).round().max(1.) as u32;
        let mut cache = self.wheel.borrow_mut();
        if cache
            .as_ref()
            .is_none_or(|(v, s, _)| *v != c.v || *s != side)
        {
            *cache = Some((c.v, side, wheel_pixels(c.v, side)));
        }
        if let Some((_, side, pixels)) = cache.as_ref() {
            unsafe {
                let bitmap = dc.CreateBitmap(
                    D2D_SIZE_U {
                        width: *side,
                        height: *side,
                    },
                    Some(pixels.as_ptr().cast()),
                    side * 4,
                    &D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.,
                        dpiY: 96.,
                        ..Default::default()
                    },
                )?;
                let p = picker::PADDING as f32;
                dc.DrawBitmap(
                    &bitmap,
                    Some(&rect(p, p, p + 2. * radius, p + 2. * radius)),
                    1.,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    None,
                    None,
                );
            }
        }
        drop(cache);
        unsafe {
            dc.DrawEllipse(
                &ellipse(center, center, radius - 0.5),
                &gfx::brush(dc, t.color("border"))?,
                1.,
                None,
            );
        }

        // A white ring with a dark edge reads on every color.
        let (dx, dy) = c.wheel_offset(f64::from(radius));
        let (mx, my) = (center + dx as f32, center + dy as f32);
        let marker = (picker::MARKER / 2.) as f32;
        let (r, gr, b) = c.rgb();
        unsafe {
            dc.FillEllipse(
                &ellipse(mx, my, marker),
                &gfx::brush(dc, solid(r, gr, b, 1.))?,
            );
            dc.DrawEllipse(
                &ellipse(mx, my, marker),
                &gfx::brush(dc, solid(1., 1., 1., 1.))?,
                2.,
                None,
            );
            dc.DrawEllipse(
                &ellipse(mx, my, marker + 1.5),
                &gfx::brush(dc, solid(0., 0., 0., 0.35))?,
                1.,
                None,
            );
            if self.show_focus && matches!(self.focused, Channel::Hue | Channel::Saturation) {
                dc.DrawEllipse(
                    &ellipse(center, center, radius + 2.),
                    &gfx::brush(dc, focus)?,
                    2.,
                    None,
                );
            }
        }

        let full = Hsva { v: 1., ..c }.rgb();
        let mut tracks = vec![(0, Channel::Brightness, c.v)];
        if self.alpha {
            tracks.push((1, Channel::Opacity, c.a));
        }
        let (left, travel) = track_span();
        let x = picker::PADDING as f32;
        let width = picker::WHEEL as f32;
        let height = picker::TRACK as f32;
        for (index, channel, value) in tracks {
            let middle = (picker::slider_top(index) + picker::SLIDER / 2.) as f32;
            let bar = rect(x, middle - height / 2., x + width, middle + height / 2.);
            if channel == Channel::Brightness {
                let stops = [solid(0., 0., 0., 1.), solid(full.0, full.1, full.2, 1.)];
                fill_ramp(dc, bar, &stops)?;
            } else {
                opacity_track(dc, bar, (r, gr, b), scale)?;
            }
            gfx::border(dc, bar, height / 2., 1., t.color("border"))?;

            // `RangeSlider`'s raised thumb.
            let tx = (left + value * travel) as f32;
            let thumb = (picker::SLIDER / 2.) as f32;
            unsafe {
                dc.FillEllipse(
                    &ellipse(tx, middle + 1., thumb),
                    &gfx::brush(dc, solid(0., 0., 0., 0.12))?,
                );
                dc.FillEllipse(
                    &ellipse(tx, middle, thumb - 0.5),
                    &gfx::brush(dc, t.color("surface-raised"))?,
                );
                dc.DrawEllipse(
                    &ellipse(tx, middle, thumb - 0.5),
                    &gfx::brush(dc, t.color("border-strong"))?,
                    1.,
                    None,
                );
                if self.show_focus && self.focused == channel {
                    dc.DrawEllipse(
                        &ellipse(tx, middle, thumb + 2.),
                        &gfx::brush(dc, focus)?,
                        2.,
                        None,
                    );
                }
            }
        }
        Ok(())
    }
}

/// The wheel at brightness `v`, `side` pixels square: premultiplied BGRA,
/// antialiased at the rim, hue and saturation from the same `pick_wheel` the
/// pointer uses.
fn wheel_pixels(v: f64, side: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    let radius = f64::from(side) / 2.;
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (f64::from(x) + 0.5 - radius, f64::from(y) + 0.5 - radius);
            let coverage = (radius - dx.hypot(dy) + 0.5).clamp(0., 1.);
            if coverage <= 0. {
                continue;
            }
            let mut color = Hsva {
                h: 0.,
                s: 0.,
                v,
                a: 1.,
            };
            color.pick_wheel(dx, dy, radius);
            let (r, g, b) = color.rgb();
            let byte = |channel: f64| (channel * coverage * 255.).round() as u8;
            let index = ((y * side + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&[byte(b), byte(g), byte(r), byte(1.)]);
        }
    }
    pixels
}

/// A horizontal ramp across the pill `bar` between two colors with alpha.
fn fill_ramp(dc: &ID2D1DeviceContext, bar: D2D_RECT_F, colors: &[D2D1_COLOR_F; 2]) -> Result<()> {
    let stops = [
        D2D1_GRADIENT_STOP {
            position: 0.,
            color: colors[0],
        },
        D2D1_GRADIENT_STOP {
            position: 1.,
            color: colors[1],
        },
    ];
    unsafe {
        let target: &ID2D1RenderTarget = dc;
        let collection =
            target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let brush = target.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 {
                    X: bar.left,
                    Y: bar.top,
                },
                endPoint: Vector2 {
                    X: bar.right,
                    Y: bar.top,
                },
            },
            None,
            &collection,
        )?;
        let radius = (bar.bottom - bar.top) / 2.;
        dc.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bar,
                radiusX: radius,
                radiusY: radius,
            },
            &brush,
        );
    }
    Ok(())
}

/// The opacity track: the color at rising alpha over the usual transparency
/// checkerboard, computed per pixel and filled into the pill `bar`. Neither
/// gradient-stop alpha nor layer masks are involved, so every Direct2D
/// implementation draws the same pixels.
fn opacity_track(
    dc: &ID2D1DeviceContext,
    bar: D2D_RECT_F,
    (r, g, b): (f64, f64, f64),
    scale: f32,
) -> Result<()> {
    let height = bar.bottom - bar.top;
    let (w, h) = (
        ((bar.right - bar.left) * scale).round().max(1.) as u32,
        (height * scale).round().max(1.) as u32,
    );
    let cell = (f64::from(h) / 2.).max(1.);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let alpha = (f64::from(x) + 0.5) / f64::from(w);
            let dark =
                ((f64::from(x) / cell) as u32 + (f64::from(y) / cell) as u32).is_multiple_of(2);
            let under = if dark { 0.8 } else { 1. };
            let mix =
                |channel: f64| ((channel * alpha + under * (1. - alpha)) * 255.).round() as u8;
            let index = ((y * w + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&[mix(b), mix(g), mix(r), 255]);
        }
    }
    unsafe {
        let target: &ID2D1RenderTarget = dc;
        let bitmap = target.CreateBitmap(
            D2D_SIZE_U {
                width: w,
                height: h,
            },
            Some(pixels.as_ptr().cast()),
            w * 4,
            &D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.,
                dpiY: 96.,
            },
        )?;
        let brush = target.CreateBitmapBrush(
            &bitmap,
            Some(&D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            }),
            Some(&D2D1_BRUSH_PROPERTIES {
                opacity: 1.,
                transform: Matrix3x2 {
                    M11: 1. / scale,
                    M12: 0.,
                    M21: 0.,
                    M22: 1. / scale,
                    M31: bar.left,
                    M32: bar.top,
                },
            }),
        )?;
        dc.FillRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bar,
                radiusX: height / 2.,
                radiusY: height / 2.,
            },
            &brush,
        );
    }
    Ok(())
}

// ---- the popup ----

struct Popup {
    hwnd: HWND,
    owner: HWND,
    g: Gfx,
    t: Tokens,
    label: String,
    picker: Picker,
    scale: f32,
    composition: Option<(IDCompositionTarget, IDCompositionVisual2)>,
    /// The last value reported to the caller.
    reported: String,
    /// A finished gesture's value, waiting for the loop to report it.
    pending: Option<String>,
    /// Closed, and whether by a key (focus goes back to the owner).
    closed: Option<bool>,
}

thread_local! {
    static POPUP: RefCell<Option<Popup>> = const { RefCell::new(None) };
    static CLOSED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn with_popup<R>(f: impl FnOnce(&mut Popup) -> R) -> Option<R> {
    POPUP.with(|p| p.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Open the picker for `label` near the pointer over `owner`, seeded from
/// `value`, and return once it closes. `changed` gets `#rrggbb`, or
/// `#rrggbbaa` when `alpha` allows a translucent color, after each gesture.
pub fn pick(
    owner: HWND,
    t: &Tokens,
    label: &str,
    value: &str,
    alpha: bool,
    mut changed: impl FnMut(String),
) -> Result<()> {
    let recently_closed = CLOSED_AT
        .with(Cell::get)
        .is_some_and(|at| at.elapsed() < REOPEN_GUARD);
    if recently_closed || POPUP.with(|p| p.borrow().is_some()) {
        return Ok(());
    }
    let g = Gfx::shared(t)?;
    let picker = Picker::new(value, alpha);
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        });
        let scale = GetDpiForWindow(owner).max(96) as f32 / 96.;
        let (w, h) = card_size(alpha);
        let px = (
            ((w + 2. * MARGIN) * scale).round() as i32,
            ((h + 2. * MARGIN) * scale).round() as i32,
        );
        let origin = place(owner, px, scale);
        let title = format!("{label}: {}", picker.description());
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            &HSTRING::from(title),
            WS_POPUP,
            origin.x,
            origin.y,
            px.0,
            px.1,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        )?;
        let reported = picker.hex();
        POPUP.with(|p| {
            *p.borrow_mut() = Some(Popup {
                hwnd,
                owner,
                g,
                t: t.clone(),
                label: label.to_owned(),
                picker,
                scale,
                composition: None,
                reported,
                pending: None,
                closed: None,
            })
        });
        if let Some(Err(error)) = with_popup(Popup::render) {
            eprintln!("failed to draw the color picker: {error}");
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));

        let mut msg = MSG::default();
        loop {
            if with_popup(|p| p.closed.is_some()).unwrap_or(true)
                || !IsWindow(Some(owner)).as_bool()
            {
                break;
            }
            match GetMessageW(&mut msg, None, 0, 0).0 {
                0 => {
                    // Hand WM_QUIT back to the outer loop.
                    PostQuitMessage(msg.wParam.0 as i32);
                    break;
                }
                -1 => break,
                _ => {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if let Some(Some(hex)) = with_popup(|p| p.pending.take()) {
                changed(hex);
            }
        }
        let closed = POPUP.with(|p| p.borrow_mut().take());
        if let Some(mut popup) = closed {
            popup.finish();
            if let Some(hex) = popup.pending.take() {
                changed(hex);
            }
            let _ = DestroyWindow(popup.hwnd);
            match popup.closed {
                Some(true) => {
                    let _ = SetForegroundWindow(popup.owner);
                }
                _ => CLOSED_AT.with(|c| c.set(Some(Instant::now()))),
            }
        }
    }
    Ok(())
}

/// The popup's top-left in physical pixels: centered under the pointer when
/// it is over `owner` (else over the owner's center), above it when there is
/// no room below, kept on the monitor's work area.
fn place(owner: HWND, px: (i32, i32), scale: f32) -> POINT {
    unsafe {
        let mut window = RECT::default();
        let _ = GetWindowRect(owner, &mut window);
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let inside = cursor.x >= window.left
            && cursor.x < window.right
            && cursor.y >= window.top
            && cursor.y < window.bottom;
        if !inside {
            cursor = POINT {
                x: (window.left + window.right) / 2,
                y: (window.top + window.bottom) / 2,
            };
        }
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let margin = (MARGIN * scale).round() as i32;
        let gap = (GAP * scale).round() as i32;
        let mut x = cursor.x - px.0 / 2;
        let mut y = cursor.y + gap - margin;
        if y + px.1 - margin > work.bottom {
            y = cursor.y - gap - px.1 + margin;
        }
        x = x.clamp(
            work.left - margin,
            (work.right - px.0 + margin).max(work.left - margin),
        );
        y = y.clamp(
            work.top - margin,
            (work.bottom - px.1 + margin).max(work.top - margin),
        );
        POINT { x, y }
    }
}

impl Popup {
    fn render(&mut self) -> Result<()> {
        let dcomp = self.g.dcomp.clone().ok_or_else(Error::empty)?;
        let (w, h) = card_size(self.picker.alpha);
        let px = (
            ((w + 2. * MARGIN) * self.scale).round().max(1.) as u32,
            ((h + 2. * MARGIN) * self.scale).round().max(1.) as u32,
        );
        let surface = self.g.surface(px.0, px.1)?;
        unsafe {
            if self.composition.is_none() {
                let target = dcomp.CreateTargetForHwnd(self.hwnd, true)?;
                let visual = dcomp.CreateVisual()?;
                target.SetRoot(&visual)?;
                self.composition = Some((target, visual));
            }
            if let Some((_, visual)) = &self.composition {
                visual.SetContent(&surface)?;
            }
        }
        let (picker, t, scale) = (&self.picker, &self.t, self.scale);
        gfx::draw_surface(&surface, px, (scale, scale), 0., |dc| {
            picker.paint(dc, t, (MARGIN, MARGIN), scale)
        })?;
        unsafe { dcomp.Commit() }
    }

    /// Redraw and rename after a change.
    fn changed(&mut self) {
        let title = format!("{}: {}", self.label, self.picker.description());
        unsafe {
            let _ = SetWindowTextW(self.hwnd, &HSTRING::from(title));
        }
        if let Err(error) = self.render() {
            eprintln!("failed to draw the color picker: {error}");
        }
    }

    /// Queue the current value if it differs from the last report.
    fn finish(&mut self) {
        let hex = self.picker.hex();
        if hex != self.reported {
            self.reported.clone_from(&hex);
            self.pending = Some(hex);
        }
    }

    /// Card-local points from client pixels in `lp`.
    fn local(&self, lp: LPARAM) -> (f64, f64) {
        let x = lp.0 as u16 as i16 as f32 / self.scale - MARGIN;
        let y = (lp.0 >> 16) as u16 as i16 as f32 / self.scale - MARGIN;
        (f64::from(x), f64::from(y))
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_LBUTTONDOWN => {
                let _ = SetCapture(hwnd);
                with_popup(|p| {
                    let (x, y) = p.local(lp);
                    if p.picker.press(x, y) {
                        p.changed();
                    } else {
                        // A press on the card's padding or shadow; a press
                        // past the card closes it like a click outside.
                        let (w, h) = card_size(p.picker.alpha);
                        let on_card =
                            (0.0..f64::from(w)).contains(&x) && (0.0..f64::from(h)).contains(&y);
                        if !on_card {
                            p.closed = Some(false);
                        }
                    }
                });
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                with_popup(|p| {
                    let (x, y) = p.local(lp);
                    if p.picker.drag(x, y) {
                        p.changed();
                    }
                });
                LRESULT(0)
            }
            WM_LBUTTONUP | WM_CAPTURECHANGED => {
                if msg == WM_LBUTTONUP {
                    let _ = ReleaseCapture();
                }
                with_popup(|p| {
                    if p.picker.release() {
                        p.finish();
                    }
                });
                LRESULT(0)
            }
            WM_KEYDOWN => {
                let key = VIRTUAL_KEY(wp.0 as u16);
                let step = match key {
                    VK_LEFT | VK_DOWN => -1.,
                    VK_RIGHT | VK_UP => 1.,
                    _ => 0.,
                };
                with_popup(|p| match key {
                    VK_ESCAPE | VK_RETURN => p.closed = Some(true),
                    VK_TAB => {
                        let back = GetKeyState(i32::from(VK_SHIFT.0)) < 0;
                        p.picker.cycle(back);
                        p.finish();
                        p.changed();
                    }
                    _ if step != 0. => {
                        p.picker.show_focus = true;
                        let channel = p.picker.focused;
                        p.picker.adjust(channel, step);
                        p.changed();
                    }
                    _ => {}
                });
                LRESULT(0)
            }
            // One held arrow is one change.
            WM_KEYUP => {
                let key = VIRTUAL_KEY(wp.0 as u16);
                if matches!(key, VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN) {
                    with_popup(Popup::finish);
                }
                LRESULT(0)
            }
            WM_ACTIVATE if (wp.0 & 0xffff) as u32 == WA_INACTIVE => {
                with_popup(|p| {
                    if p.closed.is_none() {
                        p.closed = Some(false);
                    }
                });
                LRESULT(0)
            }
            WM_SETCURSOR => {
                let mut point = POINT::default();
                let _ = GetCursorPos(&mut point);
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
                let over = with_popup(|p| {
                    let lp = LPARAM(((point.y as u16 as isize) << 16) | point.x as u16 as isize);
                    let (x, y) = p.local(lp);
                    p.picker.hit(x, y).is_some()
                })
                .unwrap_or(false);
                SetCursor(LoadCursorW(None, if over { IDC_HAND } else { IDC_ARROW }).ok());
                LRESULT(1)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presses_drag_the_channel_under_them_and_opacity_only_when_allowed() {
        let mut picker = Picker::new("#336699", true);
        let center = picker::PADDING + picker::WHEEL / 2.;
        let (left, travel) = track_span();
        let brightness_y = picker::slider_top(0) + picker::SLIDER / 2.;
        let opacity_y = picker::slider_top(1) + picker::SLIDER / 2.;
        assert_eq!(picker.hit(center, center), Some(Channel::Hue));
        assert_eq!(Picker::new("#336699", false).hit(center, opacity_y), None);
        assert!(picker.press(left + travel, brightness_y));
        assert!(
            picker.drag(left + travel + 30., brightness_y),
            "past the end clamps"
        );
        assert!(picker.release());
        assert!(picker.press(left + travel / 2., opacity_y));
        assert_eq!(picker.hex(), "#55aaff80");
        assert_eq!(
            picker.description(),
            "Hue 210°, Saturation 67%, Brightness 100%, Opacity 50%"
        );
        // 12 o'clock half way out: hue 90°, saturation 0.5.
        assert!(picker.press(center, center - picker::WHEEL / 4.));
        assert_eq!(picker.hex(), "#bfff8080");
        picker.cycle(false);
        assert_eq!(picker.focused, Channel::Saturation);
        picker.cycle(true);
        picker.cycle(true);
        assert_eq!(picker.focused, Channel::Opacity, "Shift+Tab wraps");

        let mut opaque = Picker::new("#33669980", false);
        assert_eq!(opaque.hex(), "#336699", "opaque fields drop alpha");
        opaque.adjust(Channel::Opacity, -50.);
        assert_eq!(opaque.channels().len(), 3);
        assert_eq!(opaque.hex(), "#336699");
    }

    #[test]
    fn wheel_pixels_match_pointer_picking() {
        let side = 400;
        let pixels = wheel_pixels(1., side);
        let bgra = |x: u32, y: u32| {
            let i = ((y * side + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        let [b, g, r, a] = bgra(side - 4, side / 2);
        assert!(a == 255 && r == 255 && g < 8 && b < 8, "3 o'clock is red");
        let [b, g, r, a] = bgra(3, side / 2);
        assert!(a == 255 && r < 8 && g > 247 && b > 247, "9 o'clock is cyan");
        assert_eq!(bgra(0, 0), [0; 4], "outside the wheel is clear");
    }
}
