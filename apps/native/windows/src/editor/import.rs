//! Layers in: shipping's Duplicate (Ctrl+D) and arrow nudges, the layer
//! clipboard (Ctrl+C / Ctrl+V), and image import from Add images, Add layer,
//! dropped files and pasted images (`loadDroppedFiles`).
//!
//! A layer Copy claims the system clipboard with a private marker and keeps
//! the editable copy in the session, never document data on the clipboard;
//! Paste uses it only while the clipboard still holds that marker, otherwise
//! it imports the clipboard's image files or bitmap. Decoding runs on the
//! worker; the clipboard is only read on the UI thread.
//!
//! Drops use OLE (`IDropTarget`): while files are dragged over the window the
//! shared `image_drop_guide` shows where they will land, as shipping's drop
//! snap guide and its "Drop image" toast.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use captures_app::editor::{LayerEdit, Point};
use captures_app::editor_image_decode as decode;
use captures_app::editor_session::{
    self, DROP_IMAGE, DROP_UNSUPPORTED, ImageDropGuide, ImportPlacement, Request,
};
use image::RgbaImage;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::core::*;

use super::{Editor, Job, with_editor};
use crate::gfx::{self, Style, rect};
use crate::scene::Rect;

const CF_HDROP: u16 = 15;
const CF_DIBV5: u32 = 17;
const CLIPBOARD_IMAGE: &str = "Clipboard image";
/// The private clipboard format a layer Copy publishes (the workbench's).
const LAYER_FORMAT: PCWSTR = w!("application/x-captures-editor-layer");

/// What to decode on the worker.
pub enum Source {
    Files(Vec<PathBuf>),
    /// The clipboard's registered `PNG` bytes.
    Png(Vec<u8>),
    /// The clipboard's `CF_DIBV5` bytes.
    Dib(Vec<u8>),
}

impl Source {
    pub fn decode(self) -> std::result::Result<Vec<(String, RgbaImage)>, String> {
        match self {
            Source::Files(paths) => decode::decode_imports(paths),
            Source::Png(bytes) => decode::decode_import_bytes(&bytes, CLIPBOARD_IMAGE)
                .map(|pixels| vec![(CLIPBOARD_IMAGE.into(), pixels)]),
            Source::Dib(bytes) => decode::decode_clipboard_dib(bytes)
                .map(|pixels| vec![(CLIPBOARD_IMAGE.into(), pixels)]),
        }
    }
}

/// A layer ID no document has used: shipping's `editorId()`.
pub(super) fn fresh_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("layer-{t:x}-{n:x}")
}

fn supported(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|path| {
            path.is_absolute() && path.is_file() && editor_session::is_supported_image_path(path)
        })
        .collect()
}

/// The file list in an `HDROP`.
unsafe fn drop_files(drop: HDROP) -> Vec<PathBuf> {
    unsafe {
        let count = DragQueryFileW(drop, u32::MAX, None);
        (0..count)
            .filter_map(|i| {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buffer = vec![0u16; len + 1];
                let got = DragQueryFileW(drop, i, Some(&mut buffer)) as usize;
                (got > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..got])))
            })
            .collect()
    }
}

/// Open the clipboard, retrying briefly while another app holds it.
unsafe fn open_clipboard(hwnd: HWND) -> Result<()> {
    let owner = (!hwnd.is_invalid()).then_some(hwnd);
    let mut tries = 0;
    loop {
        match unsafe { OpenClipboard(owner) } {
            Ok(()) => return Ok(()),
            Err(error) if tries >= 10 => return Err(error),
            Err(_) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

/// The clipboard's image files, else its PNG or bitmap; `None` without one.
fn read_clipboard(hwnd: HWND) -> std::result::Result<Option<Source>, String> {
    unsafe {
        open_clipboard(hwnd).map_err(|e| format!("Could not open clipboard: {}", e.message()))?;
        let result = (|| {
            if IsClipboardFormatAvailable(u32::from(CF_HDROP)).is_ok()
                && let Ok(handle) = GetClipboardData(u32::from(CF_HDROP))
            {
                let files = supported(drop_files(HDROP(handle.0)));
                if !files.is_empty() {
                    return Ok(Some(Source::Files(files)));
                }
            }
            let png = RegisterClipboardFormatW(w!("PNG"));
            for format in [png, CF_DIBV5] {
                if IsClipboardFormatAvailable(format).is_err() {
                    continue;
                }
                let handle = GetClipboardData(format)
                    .map_err(|e| format!("Could not read clipboard image: {}", e.message()))?;
                let memory = HGLOBAL(handle.0);
                let size = GlobalSize(memory);
                if size > captures_history::editor_draft::MAX_IMAGE_BYTES {
                    return Err("Clipboard image exceeds the encoded-byte limit.".into());
                }
                let data = GlobalLock(memory) as *const u8;
                if data.is_null() {
                    return Err("Could not read clipboard image.".into());
                }
                let bytes = std::slice::from_raw_parts(data, size).to_vec();
                let _ = GlobalUnlock(memory);
                return Ok(Some(if format == png {
                    Source::Png(bytes)
                } else {
                    Source::Dib(bytes)
                }));
            }
            Ok(None)
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Publish the layer marker; its clipboard sequence number, which changes
/// as soon as anything else is copied.
fn claim_marker(hwnd: HWND) -> Result<u32> {
    unsafe {
        open_clipboard(hwnd)?;
        let result = (|| {
            EmptyClipboard()?;
            let format = RegisterClipboardFormatW(LAYER_FORMAT);
            let token = fresh_id().into_bytes();
            let memory = GlobalAlloc(GMEM_MOVEABLE, token.len())?;
            let target = GlobalLock(memory) as *mut u8;
            if target.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err(Error::from_thread());
            }
            std::ptr::copy_nonoverlapping(token.as_ptr(), target, token.len());
            let _ = GlobalUnlock(memory);
            if let Err(error) = SetClipboardData(format, Some(HANDLE(memory.0))) {
                let _ = GlobalFree(Some(memory));
                return Err(error);
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result?;
        Ok(GetClipboardSequenceNumber())
    }
}

impl Editor {
    /// Shipping `nudgeSelected`: move the selected, unlocked layer. A run
    /// of nudges on one layer is one undo step; nudges made while the
    /// worker is busy add up rather than replacing each other.
    pub(super) fn nudge_selected(&mut self, dx: f64, dy: f64) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let locked = self
            .presented
            .as_ref()
            .is_some_and(|p| p.layers.iter().any(|l| l.id == id && l.locked));
        if locked {
            return;
        }
        let key = format!("nudge:{id}");
        if let Some(Request::Live {
            key: queued,
            request,
        }) = self.pending.as_mut()
            && *queued == key
            && let Request::Layer {
                edit: LayerEdit::Translate { delta_x, delta_y },
                ..
            } = request.as_mut()
        {
            *delta_x += dx;
            *delta_y += dy;
            return;
        }
        self.submit_live(Request::Live {
            key,
            request: Box::new(Request::Layer {
                id,
                edit: LayerEdit::Translate {
                    delta_x: dx,
                    delta_y: dy,
                },
            }),
        });
    }

    /// Shipping `duplicateLayer`: the copy goes right above the layer and
    /// becomes the selection, under Select.
    pub(super) fn duplicate_layer(&mut self, id: String) {
        if self.inline.is_some() {
            self.finish_text();
        }
        let new_id = fresh_id();
        self.select_after = Some(new_id.clone());
        self.submit(Request::Layer {
            id,
            edit: LayerEdit::Duplicate { new_id },
        });
        self.select_tool("v");
    }

    pub(super) fn duplicate_selected(&mut self) {
        if let Some(id) = self.selected.clone() {
            self.duplicate_layer(id);
        }
    }

    /// Ctrl+C with a layer selected: shipping `copySelectedLayer`.
    pub(super) fn copy_layer(&mut self) -> bool {
        let Some(id) = self.selected.clone() else {
            return false;
        };
        match claim_marker(self.hwnd) {
            Ok(marker) => {
                self.submit(Request::CopyLayer { id });
                self.layer_marker = Some(marker);
            }
            Err(error) => {
                self.layer_marker = None;
                self.set_notice(Some((format!("Couldn’t copy: {}", error.message()), true)));
            }
        }
        true
    }

    /// Ctrl+V: this editor's copied layer while the clipboard still holds
    /// its marker, else the clipboard's images.
    pub(super) fn paste(&mut self) {
        if self.presented.is_none() {
            return;
        }
        if self.layer_marker.is_some()
            && self.layer_marker == Some(unsafe { GetClipboardSequenceNumber() })
        {
            if self.inline.is_some() {
                self.finish_text();
            }
            let new_id = fresh_id();
            self.select_after = Some(new_id.clone());
            self.submit(Request::PasteLayer {
                new_id,
                after_id: self.selected.clone(),
            });
            self.select_tool("v");
            return;
        }
        match read_clipboard(self.hwnd) {
            Ok(Some(source)) => self.import(source, None),
            Ok(None) => {}
            Err(error) => self.set_notice(Some((error, true))),
        }
    }

    /// Decode and add images on the worker; the last becomes the selection.
    pub(super) fn import(&mut self, source: Source, point: Option<Point>) {
        if self.presented.is_none() {
            return;
        }
        if self.inline.is_some() {
            self.finish_text();
        }
        self.busy = true;
        self.notice = None;
        let _ = self.jobs.send(Job::Import {
            source,
            selected: self.selected.clone(),
            point,
        });
        self.export_changed();
    }

    /// Add images' and Add layer's picker answer.
    pub fn add_images(&mut self, paths: Vec<PathBuf>) {
        let files = supported(paths);
        if files.is_empty() {
            self.set_notice(Some((DROP_UNSUPPORTED.into(), true)));
        } else {
            self.import(Source::Files(files), None);
        }
    }

    // ---- drops ----

    /// The guide for a drag at client point `(x, y)` (none: the default).
    fn drop_guide_at(&self, at: Option<(f32, f32)>) -> Option<ImageDropGuide> {
        let document = &self.presented.as_ref()?.document;
        let point = at.and_then(|(x, y)| self.to_document(x, y));
        Some(editor_session::image_drop_guide(
            document,
            self.selected.as_deref(),
            point,
        ))
    }

    fn set_drop_guide(&mut self, guide: Option<ImageDropGuide>) {
        if guide != self.drop_guide {
            self.drop_guide = guide;
            self.drop_active = true;
            self.overlay_changed();
        }
    }

    /// Files entered the window at a client point (none: shipping's
    /// `defaultImageDropGuide`).
    pub(super) fn drag_enter(&mut self, at: Option<(f32, f32)>) {
        self.drop_active = true;
        let guide = self.drop_guide_at(at);
        self.drop_guide = None;
        self.set_drop_guide(guide);
        self.overlay_changed();
    }

    fn drag_leave(&mut self) {
        self.drop_active = false;
        self.drop_guide = None;
        self.overlay_changed();
    }

    fn dropped(&mut self, files: Vec<PathBuf>, at: (f32, f32)) {
        let guide = self.drop_guide_at(Some(at)).or(self.drop_guide.take());
        self.drag_leave();
        let files = supported(files);
        if files.is_empty() {
            self.set_notice(Some((DROP_UNSUPPORTED.into(), true)));
            return;
        }
        self.import(Source::Files(files), guide.map(|g| g.point));
    }

    /// The drop guide and toast, well-relative, for the overlay's bounds.
    pub(super) fn drop_bounds(&self) -> Option<Rect> {
        if !self.drop_active {
            return None;
        }
        let toast = self.drop_toast_rect();
        let guide = self
            .drop_guide
            .as_ref()
            .and_then(|g| self.doc_rect(g.target));
        Some(match guide {
            // Room for the bloom (96 px) and the stack light's pool.
            Some(r) => super::cover(
                toast,
                Rect::new(
                    r.x - 0.7 * r.w - 4.,
                    r.y - 0.6 * r.h - 100.,
                    r.w * 2.4 + 8.,
                    r.h * 2.4 + 200.,
                ),
            ),
            None => toast,
        })
    }

    /// A document rect in well-relative DIPs.
    pub(super) fn doc_rect(&self, r: captures_app::editor::Rect) -> Option<Rect> {
        let a = self.to_well(Point { x: r.x, y: r.y })?;
        let b = self.to_well(Point {
            x: r.x + r.width,
            y: r.y + r.height,
        })?;
        Some(Rect::new(a.0, a.1, b.0 - a.0, b.1 - a.1))
    }

    fn drop_label(&self) -> &'static str {
        self.drop_guide.as_ref().map_or(DROP_IMAGE, |g| g.label)
    }

    /// `.screenshot-drop-overlay`, centred 18 px below the viewport's top.
    fn drop_toast_rect(&self) -> Rect {
        let (s4, s5) = (self.t.n("s-4"), self.t.n("s-5"));
        let width = self
            .g
            .text(&self.t, Style::Button, self.drop_label(), 1e4)
            .map_or(120., |t| t.width.ceil());
        let w = (44. + width + s5 + 2.).min(280.);
        let h = 2. * s4 + 20. + 2.;
        Rect::new((self.l.well.w - w) / 2., 18., w, h)
    }

    /// Shipping's drop snap guide (edge bar and bloom, or the stack light)
    /// and the "Drop image" toast, at rest.
    pub(super) fn draw_drop(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        if !self.drop_active {
            return Ok(());
        }
        let t = &self.t;
        if let Some(guide) = &self.drop_guide
            && let Some(r) = self.doc_rect(guide.target)
        {
            let b = rect(r.x, r.y, r.x + r.w, r.y + r.h);
            match guide.placement {
                ImportPlacement::Stack => {
                    if let Some(f) = self.doc_rect(guide.focus) {
                        let focus = rect(f.x, f.y, f.x + f.w, f.y + f.h);
                        let radius = t.n("r-md");
                        let warm = D2D1_COLOR_F {
                            r: 1.,
                            g: 250. / 255.,
                            b: 240. / 255.,
                            a: 0.12,
                        };
                        gfx::glow(dc, focus, radius, 28., warm)?;
                        // The contact shadow, outside the footprint only.
                        for (blur, alpha) in [(4., 0.2), (18., 0.3), (40., 0.24)] {
                            let black = D2D1_COLOR_F {
                                r: 0.,
                                g: 0.,
                                b: 0.,
                                a: alpha,
                            };
                            gfx::glow(dc, focus, radius, blur, black)?;
                        }
                        let rim = D2D1_COLOR_F {
                            r: 1.,
                            g: 1.,
                            b: 1.,
                            a: 0.36,
                        };
                        gfx::border(dc, focus, radius, 1., rim)?;
                    }
                }
                edge => {
                    gfx::glow(dc, b, 3., 22., t.color_alpha("theme-accent", 0.28))?;
                    gfx::fill_round(dc, b, 3., t.color_alpha("theme-accent", 0.1))?;
                    gfx::border(dc, b, 3., 1., t.color_alpha("theme-accent", 0.78))?;
                    edge_glow(dc, t, b, edge)?;
                }
            }
        }
        // The toast.
        let toast = self.drop_toast_rect();
        let tr = rect(toast.x, toast.y, toast.x + toast.w, toast.y + toast.h);
        let radius = t.n("r-lg");
        gfx::shadow(dc, tr, radius, t.shadow("shadow-lg"))?;
        gfx::fill_round(dc, tr, radius, t.color("glass-strong"))?;
        gfx::border(dc, tr, radius, 1., t.color_alpha("theme-accent", 0.72))?;
        gfx::icon(
            dc,
            &self.g.factory,
            "image",
            &[],
            (tr.left + 13., (tr.top + tr.bottom - 22.) / 2., 22.),
            1.7,
            t.color("theme-accent"),
        )?;
        let text = self.g.text(t, Style::Button, self.drop_label(), 1e4)?;
        self.g.ellipsis(&text, toast.w - 44. - t.n("s-5"))?;
        gfx::text(
            dc,
            &text,
            tr.left + 44.,
            ((tr.top + tr.bottom - text.height) / 2.).round(),
            t.color("glass-text"),
        )
    }
}

/// `.screenshot-drop-snap-guide.edge-*::after` and its outward bloom: a
/// 5 px accent bar on the snapped edge with a glow, and a gradient fading
/// away from it (`min(96px, 42%)` deep, 8% wider each side).
fn edge_glow(
    dc: &ID2D1DeviceContext,
    t: &crate::tokens::Tokens,
    b: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    edge: ImportPlacement,
) -> Result<()> {
    let (w, h) = (b.right - b.left, b.bottom - b.top);
    let deep = |side: f32| (side * 0.42).min(96.);
    let (bar, bloom, angle) = match edge {
        ImportPlacement::Top => (
            rect(b.left - 1., b.top - 2., b.right + 1., b.top + 3.),
            rect(
                b.left - 0.08 * w,
                b.top - deep(h),
                b.right + 0.08 * w,
                b.top,
            ),
            0.,
        ),
        ImportPlacement::Bottom => (
            rect(b.left - 1., b.bottom - 3., b.right + 1., b.bottom + 2.),
            rect(
                b.left - 0.08 * w,
                b.bottom,
                b.right + 0.08 * w,
                b.bottom + deep(h),
            ),
            180.,
        ),
        ImportPlacement::Left => (
            rect(b.left - 2., b.top - 1., b.left + 3., b.bottom + 1.),
            rect(
                b.left - deep(w),
                b.top - 0.08 * h,
                b.left,
                b.bottom + 0.08 * h,
            ),
            270.,
        ),
        _ => (
            rect(b.right - 3., b.top - 1., b.right + 2., b.bottom + 1.),
            rect(
                b.right,
                b.top - 0.08 * h,
                b.right + deep(w),
                b.bottom + 0.08 * h,
            ),
            90.,
        ),
    };
    hint_bloom(dc, t, bloom, angle, 0.55)?;
    for (blur, alpha) in [(36., 0.4), (20., 0.65), (8., 0.95)] {
        gfx::glow(dc, bar, 2.5, blur, t.color_alpha("theme-accent", alpha))?;
    }
    gfx::fill_round(dc, bar, 2.5, t.color("theme-accent"))
}

/// `.screenshot-drop-snap-bloom` (and the Expand/Trim blooms): the accent
/// at `alpha` fading out across `r` along `angle` (CSS degrees; 0 starts at
/// the bottom edge), with shipping's 0.55 / 0.22 / 0.06 / 0 stops.
pub(super) fn hint_bloom(
    dc: &ID2D1DeviceContext,
    t: &crate::tokens::Tokens,
    r: windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F,
    angle: f32,
    alpha: f32,
) -> Result<()> {
    use windows::Win32::Graphics::Direct2D::Common::D2D1_GRADIENT_STOP;
    let accent = t.color("theme-accent");
    let stop = |position: f32, share: f32| D2D1_GRADIENT_STOP {
        position,
        color: D2D1_COLOR_F {
            a: accent.a * alpha * share,
            ..accent
        },
    };
    let stops = [
        stop(0., 1.),
        stop(0.38, 0.4),
        stop(0.72, 0.11),
        stop(1., 0.),
    ];
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let a = angle.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let half = (w * dx.abs() + h * dy.abs()) / 2.;
    let (cx, cy) = (r.left + w / 2., r.top + h / 2.);
    let point = |x: f32, y: f32| windows_numerics::Vector2 { X: x, Y: y };
    unsafe {
        let target: &ID2D1RenderTarget = dc;
        let linear = |stops: &[D2D1_GRADIENT_STOP], from, to| -> Result<_> {
            let collection = target.CreateGradientStopCollection(
                stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            target.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: from,
                    endPoint: to,
                },
                None,
                &collection,
            )
        };
        let brush = linear(
            &stops,
            point(cx - dx * half, cy - dy * half),
            point(cx + dx * half, cy + dy * half),
        )?;
        // `mask-image: linear-gradient(to right, transparent, #000 12%,
        // #000 88%, transparent)` across the bloom.
        let ink = |a: f32| D2D1_COLOR_F {
            r: 0.,
            g: 0.,
            b: 0.,
            a,
        };
        let mask_stops = [
            D2D1_GRADIENT_STOP {
                position: 0.,
                color: ink(0.),
            },
            D2D1_GRADIENT_STOP {
                position: 0.12,
                color: ink(1.),
            },
            D2D1_GRADIENT_STOP {
                position: 0.88,
                color: ink(1.),
            },
            D2D1_GRADIENT_STOP {
                position: 1.,
                color: ink(0.),
            },
        ];
        let across = (dy.abs(), dx.abs());
        let reach = (w * across.0 + h * across.1) / 2.;
        let mask = linear(
            &mask_stops,
            point(cx - across.0 * reach, cy - across.1 * reach),
            point(cx + across.0 * reach, cy + across.1 * reach),
        )?;
        let mask: ID2D1Brush = mask.cast()?;
        dc.PushLayer(
            &D2D1_LAYER_PARAMETERS1 {
                contentBounds: r,
                opacity: 1.,
                opacityBrush: std::mem::ManuallyDrop::new(Some(mask)),
                maskTransform: windows_numerics::Matrix3x2::identity(),
                ..Default::default()
            },
            None,
        );
        dc.FillRectangle(&r, &brush);
        dc.PopLayer();
    }
    Ok(())
}

// ---- OLE drop target ----

/// The editor window's `IDropTarget`: forwards drags to its editor.
#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
}

impl DropTarget {
    fn client(&self, pt: &POINTL) -> (f32, f32) {
        let mut point = POINT { x: pt.x, y: pt.y };
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut point);
        }
        let scale = with_editor(self.hwnd, |e| e.scale).unwrap_or(1.);
        (point.x as f32 / scale, point.y as f32 / scale)
    }
}

fn has_files(data: Option<&IDataObject>) -> bool {
    data.is_some_and(|data| unsafe { data.QueryGetData(&hdrop_format()) } == S_OK)
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn data_files(data: &IDataObject) -> Vec<PathBuf> {
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else {
            return Vec::new();
        };
        let files = drop_files(HDROP(medium.u.hGlobal.0));
        ReleaseStgMedium(&mut medium);
        files
    }
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let accept = has_files(data.as_ref());
        if accept {
            let at = self.client(pt);
            with_editor(self.hwnd, |e| e.drag_enter(Some(at)));
        }
        unsafe {
            *effect = if accept {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let at = self.client(pt);
        let active = with_editor(self.hwnd, |e| {
            if e.drop_active {
                let guide = e.drop_guide_at(Some(at));
                e.set_drop_guide(guide);
            }
            e.drop_active
        })
        .unwrap_or(false);
        unsafe {
            *effect = if active {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            };
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        with_editor(self.hwnd, |e| e.drag_leave());
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let files = data.as_ref().map(data_files).unwrap_or_default();
        let at = self.client(pt);
        with_editor(self.hwnd, |e| e.dropped(files, at));
        unsafe {
            *effect = DROPEFFECT_COPY;
        }
        Ok(())
    }
}

/// Make `hwnd` a drop target; OLE is initialized on first use.
pub fn register_drop(hwnd: HWND) {
    unsafe {
        // Already-initialized COM in this apartment makes this S_FALSE.
        let _ = OleInitialize(None);
        let target: IDropTarget = DropTarget { hwnd }.into();
        if let Err(error) = RegisterDragDrop(hwnd, &target) {
            eprintln!("editor drop target: {error}");
        }
    }
}

pub fn revoke_drop(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}
