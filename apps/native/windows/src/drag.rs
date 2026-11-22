//! Dragging a capture's file out of Captures into another app, shared by
//! History thumbnails and mini preview cards. Shipping used
//! `tauri-plugin-drag` (drag-rs), which on Windows runs an OLE `DoDragDrop`
//! with the shell's data object for the file (`CF_HDROP` plus the shell's
//! own formats), copy only, and a drag image made from the capture's preview
//! resized to fill 284 × 160 with the pointer at its top-left corner
//! (`storage::encode_drag_icon_png`). This does the same.
//!
//! The file comes from the shared `captures_app` `PreparePreviewDrag`
//! request ([`prepare`]), on a worker: a saved export, or a disposable copy
//! that outlives the drag even if the capture leaves History.
//!
//! [`run`] is modal: OLE runs its own message loop until the drop, so call it
//! without holding a window's `RefCell` borrow, or that window's messages
//! fall through to `DefWindowProc` for the whole drag. Start it while the
//! left button is still down, after the pointer moved past [`beyond_threshold`].
use std::path::{Path, PathBuf};

use captures_app::preview::PreviewDropLanding;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Ole::*;
use windows::Win32::System::SystemServices::*;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

/// Shipping's `DRAG_ICON_WIDTH` × `DRAG_ICON_HEIGHT` (physical pixels).
pub const ICON_WIDTH: u32 = 284;
pub const ICON_HEIGHT: u32 = 160;

/// A drag image: premultiplied BGRA, top-down rows.
pub struct Icon {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// What a file drag offers: the file and its drag image.
pub struct Payload {
    pub path: PathBuf,
    pub icon: Option<Icon>,
}

/// How a drag ended. Mini previews read both fields (shipping dismisses a
/// card only for an accepted drop outside Captures); History ignores them.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct Outcome {
    /// A target accepted the copy (`DRAGDROP_S_DROP` with an effect).
    pub accepted: bool,
    /// The pointer at release, in physical screen pixels.
    pub point: POINT,
}

/// Prepare a History artifact for dragging, off the UI thread: the shared
/// export (`captures_app::Request::PreparePreviewDrag`) and the drag image
/// from its preview. Shipping's `prepare_artifact_drag`.
pub fn prepare(root: &Path, id: &str) -> std::result::Result<Payload, String> {
    let request = captures_app::Request::PreparePreviewDrag {
        root: root.to_path_buf(),
        id: id.to_owned(),
    };
    let path = match captures_app::execute(request) {
        Ok(captures_app::Response::PreviewDragPrepared { path, .. }) => path,
        Ok(_) => return Err("unexpected response".into()),
        Err(error) => return Err(error.to_string()),
    };
    let icon = captures_history::read_image(root, id, true)
        .ok()
        .and_then(|png| icon(&png));
    Ok(Payload { path, icon })
}

/// Remove the previous session's disposable drag exports, once at startup.
pub fn clear_previous_exports(root: &Path) {
    let request = captures_app::Request::ClearPreviousPreviewDrags {
        root: root.to_path_buf(),
    };
    if let Err(error) = captures_app::execute(request) {
        eprintln!("couldn't clear old drag exports: {error}");
    }
}

/// Shipping's drag image: the preview PNG resized to fill 284 × 160.
pub fn icon(preview_png: &[u8]) -> Option<Icon> {
    let preview = image::load_from_memory(preview_png).ok()?;
    let rgba = preview
        .resize_to_fill(
            ICON_WIDTH,
            ICON_HEIGHT,
            image::imageops::FilterType::Triangle,
        )
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        let premultiply = |c: u8| ((u16::from(c) * a + 127) / 255) as u8;
        let (r, g, b) = (px[0], px[1], px[2]);
        px[0] = premultiply(b);
        px[1] = premultiply(g);
        px[2] = premultiply(r);
    }
    Some(Icon {
        width,
        height,
        bgra,
    })
}

/// Whether the pointer moved far enough from the press to start a drag
/// (`SM_CXDRAG` × `SM_CYDRAG`, physical pixels).
pub fn beyond_threshold(press: POINT, now: POINT) -> bool {
    let (cx, cy) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };
    (now.x - press.x).abs() > cx.max(1) / 2 || (now.y - press.y).abs() > cy.max(1) / 2
}

/// Whether the left button is still held (the drag is still wanted).
pub fn button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        windows::Win32::UI::Input::KeyboardAndMouse::VK_RBUTTON
    } else {
        VK_LBUTTON
    };
    let state = unsafe { GetAsyncKeyState(button.0 as i32) };
    state < 0
}

/// The top-level Captures window under a screen point, if any.
#[allow(dead_code)] // Mini preview drags (landing).
pub fn own_window_at(point: POINT) -> Option<HWND> {
    unsafe {
        let hwnd = WindowFromPoint(point);
        if hwnd.is_invalid() {
            return None;
        }
        let root = GetAncestor(hwnd, GA_ROOT);
        let mut pid = 0;
        GetWindowThreadProcessId(root, Some(&mut pid));
        (pid == GetCurrentProcessId()).then_some(root)
    }
}

/// Shipping `preview_file_drop_landing`: the mini preview stack (`stack`),
/// another Captures window, or anywhere else.
#[allow(dead_code)] // Mini preview drags.
pub fn landing(point: POINT, stack: Option<HWND>) -> PreviewDropLanding {
    match own_window_at(point) {
        Some(hwnd) if Some(hwnd) == stack => PreviewDropLanding::PreviewStack,
        Some(_) => PreviewDropLanding::AppWindow,
        None => PreviewDropLanding::External,
    }
}

/// Drag `payload`'s file until it is dropped or cancelled (Esc, the right
/// button, or release over nothing). Modal; see the module docs.
pub fn run(payload: &Payload) -> Result<Outcome> {
    ensure_ole()?;
    let data = data_object(&payload.path)?;
    if let Some(icon) = &payload.icon {
        set_image(&data, icon);
    }
    let source: IDropSource = DropSource.into();
    let mut effect = DROPEFFECT_NONE;
    let hr = unsafe { DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect) };
    if hr.is_err() {
        return Err(hr.into());
    }
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point)? };
    Ok(Outcome {
        accepted: hr == DRAGDROP_S_DROP && effect != DROPEFFECT_NONE,
        point,
    })
}

thread_local! {
    static OLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `DoDragDrop` needs OLE on this (STA) thread; COM alone is not enough.
fn ensure_ole() -> Result<()> {
    if !OLE.get() {
        unsafe { OleInitialize(None)? };
        OLE.set(true);
    }
    Ok(())
}

/// The shell's data object for the file, as Explorer offers it.
fn data_object(path: &Path) -> Result<IDataObject> {
    unsafe {
        let pidl = ILCreateFromPathW(&HSTRING::from(path.as_os_str()));
        if pidl.is_null() {
            return Err(Error::new(E_INVALIDARG, "the file can't be dragged"));
        }
        let list: [*const ITEMIDLIST; 1] = [pidl.cast_const()];
        let items = SHCreateShellItemArrayFromIDLists(&list);
        ILFree(Some(pidl.cast_const()));
        items?.BindToHandler(None, &BHID_DataObject)
    }
}

/// Attach the drag image; a failure only loses the image.
fn set_image(data: &IDataObject, icon: &Icon) {
    unsafe {
        let Ok(helper) =
            CoCreateInstance::<_, IDragSourceHelper>(&CLSID_DragDropHelper, None, CLSCTX_ALL)
        else {
            return;
        };
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: icon.width as i32,
                // Negative: top-down rows.
                biHeight: -(icon.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return;
        };
        std::ptr::copy_nonoverlapping(icon.bgra.as_ptr(), bits.cast::<u8>(), icon.bgra.len());
        let image = SHDRAGIMAGE {
            sizeDragImage: SIZE {
                cx: icon.width as i32,
                cy: icon.height as i32,
            },
            ptOffset: POINT { x: 0, y: 0 },
            hbmpDragImage: bitmap,
            // CLR_NONE: use the alpha channel, never a color key.
            crColorKey: COLORREF(0xffff_ffff),
        };
        // On success the drag-image manager owns the bitmap.
        if helper.InitializeFromBitmap(&image, data).is_err() {
            let _ = DeleteObject(bitmap.into());
        }
    }
}

#[implement(IDropSource)]
struct DropSource;

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() || keys.0 & MK_RBUTTON.0 != 0 {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_fills_shippings_size_and_premultiplies() {
        // 2:1 red with a half-transparent blue column: fill crops, never letterboxes.
        let mut source = image::RgbaImage::from_pixel(400, 100, image::Rgba([255, 0, 0, 255]));
        for y in 0..100 {
            source.put_pixel(200, y, image::Rgba([0, 0, 255, 128]));
        }
        let mut png = Vec::new();
        source
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let icon = icon(&png).unwrap();
        assert_eq!((icon.width, icon.height), (ICON_WIDTH, ICON_HEIGHT));
        assert_eq!(icon.bgra.len(), (ICON_WIDTH * ICON_HEIGHT * 4) as usize);
        // Corner: opaque red as BGRA.
        assert_eq!(&icon.bgra[..4], &[0, 0, 255, 255]);
        // Every pixel is premultiplied: no channel above alpha.
        assert!(
            icon.bgra
                .chunks_exact(4)
                .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
        );
    }

    /// What a drop target reads: the shell data object's `CF_HDROP` names
    /// exactly the dragged file.
    #[test]
    fn data_object_offers_the_file_as_cf_hdrop() {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
        let dir = std::env::temp_dir().join(format!("captures-drag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Captures_test.png");
        std::fs::write(&file, b"png").unwrap();
        let data = data_object(&file).unwrap();
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        let path = unsafe {
            let mut medium = data.GetData(&format).unwrap();
            let drop = HDROP(medium.u.hGlobal.0);
            assert_eq!(DragQueryFileW(drop, u32::MAX, None), 1);
            let mut name = [0u16; 1024];
            let len = DragQueryFileW(drop, 0, Some(&mut name)) as usize;
            ReleaseStgMedium(&mut medium);
            String::from_utf16_lossy(&name[..len])
        };
        assert_eq!(
            std::fs::canonicalize(&path).unwrap(),
            std::fs::canonicalize(&file).unwrap()
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn landing_outside_captures_is_external() {
        // A point far off every display hits no window.
        let point = POINT {
            x: -1_000_000,
            y: -1_000_000,
        };
        assert_eq!(landing(point, None), PreviewDropLanding::External);
    }
}
