//! Serialized-worker editor handles and independently retained immutable frames.

use super::region::{RegionPixels, response, text};
use captures_app::{
    editor::{
        ElementBase, ElementStyle, Point, ShapeElement, arrow_fill_polygon, smooth_path_centerline,
    },
    editor_render::{MAX_RENDER_DIMENSION, MAX_RENDER_PIXELS},
    editor_session::{EditorSession, ExportOptions, ImportImage, OpenRequest, Request},
    selection::Point as AbiPoint,
};
use image::RgbaImage;
use serde::Deserialize;
use serde_json::json;
use std::{
    ffi::c_char,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
    sync::Arc,
};

pub struct DrawGeometry(Vec<AbiPoint>);

#[repr(C)]
pub struct DrawPoints {
    pub data: *const AbiPoint,
    pub length: usize,
    pub stroke_width: f64,
}

/// Shared transient geometry, independent of editor sessions. Kind 0 returns
/// an arrow outline from exactly two endpoints; kind 1 smooths accepted Pen
/// samples (one point is a dot, two points are also a straight Line preview).
/// A too-short arrow succeeds with an empty outline. No JSON or pixel rendering.
///
/// # Safety
/// Non-null input is aligned and readable for `length` initialized points for
/// this call. Non-null output is writable aligned descriptor storage. Returned
/// points are immutable and live until the returned handle is freed. Nulls,
/// invalid kinds/counts/coordinates or panic return null and leave output intact.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_draw_geometry_v1(
    kind: u32,
    input: *const AbiPoint,
    length: usize,
    output: *mut DrawPoints,
) -> *mut DrawGeometry {
    if input.is_null()
        || output.is_null()
        || length == 0
        || length > isize::MAX as usize / (24 * size_of::<AbiPoint>())
        || kind > 1
        || (kind == 0 && length != 2)
    {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable initialized points; length is bounded above.
        let input = unsafe { std::slice::from_raw_parts(input, length) };
        if !input
            .iter()
            .all(|point| (point.x as f32).is_finite() && (point.y as f32).is_finite())
        {
            return None;
        }
        let style = ElementStyle::default();
        let width = style.stroke_width;
        let points = if kind == 0 {
            arrow_fill_polygon(&ShapeElement {
                base: ElementBase {
                    id: String::new(),
                    x: input[0].x,
                    y: input[0].y,
                    rotation: None,
                    locked: false,
                    visible: true,
                    opacity: 100.,
                    blend_mode: "source-over".into(),
                },
                shape: "arrow".into(),
                end_x: input[1].x,
                end_y: input[1].y,
                controls: Vec::new(),
                style,
                extra: Default::default(),
            })
        } else {
            smooth_path_centerline(
                &input
                    .iter()
                    .map(|point| Point {
                        x: point.x,
                        y: point.y,
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let geometry = Box::new(DrawGeometry(
            points
                .into_iter()
                .map(|point| AbiPoint {
                    x: point.x,
                    y: point.y,
                })
                .collect(),
        ));
        Some((geometry, width))
    }));
    let Ok(Some((geometry, width))) = result else {
        return ptr::null_mut();
    };
    // SAFETY: caller retains writable descriptor storage; handle owns point storage.
    unsafe {
        output.write(DrawPoints {
            data: geometry.0.as_ptr(),
            length: geometry.0.len(),
            stroke_width: width,
        })
    };
    Box::into_raw(geometry)
}

/// # Safety
/// Free a live geometry handle exactly once, after all point borrows end. Null is allowed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_draw_geometry_free_v1(handle: *mut DrawGeometry) {
    if !handle.is_null() {
        // SAFETY: caller transfers the uniquely owned live handle.
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Open from isolated native History/draft roots on a host worker.
///
/// # Safety
/// Input is readable NUL-terminated UTF-8 during the call. Non-null output is
/// aligned writable pointer storage; free its JSON with captures_settings_free_v1.
/// Serialize all returned session calls, including free, on one worker.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_open_v1(
    request_json: *const c_char,
    output: *mut *mut c_char,
) -> *mut EditorSession {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input throughout this call.
        let request = serde_json::from_str::<OpenRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        EditorSession::open(request)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(session) => {
            let value = json!({"ok":true,"result":session.snapshot()});
            (Box::into_raw(Box::new(session)), value)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable pointer storage.
    unsafe { output.write(response(value)) };
    handle
}

#[derive(Deserialize)]
struct SaveNewRequest {
    history_root: PathBuf,
    destination: PathBuf,
    options: ExportOptions,
    mode: captures_capture::CaptureMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportImageRequest {
    name: String,
    selected_id: Option<String>,
    point: Option<Point>,
}

/// Import one host-decoded image and return its stable layer ID plus snapshot.
/// The pixel descriptor and rows are borrowed only for this call; successful
/// validation copies them into storage owned by the serialized editor session.
///
/// # Safety
/// Non-null session is live and exclusively owned for the call. Non-null pixels
/// points to a readable descriptor whose actual top-down straight-alpha sRGB
/// RGBA8 bytes in each row are initialized and readable through the call. Row
/// padding need not be initialized and is never read. Input JSON is readable,
/// NUL-terminated UTF-8. Free the owned response with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_import_image_v1(
    session: *mut EditorSession,
    pixels: *const RegionPixels,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable JSON and pixel metadata for this call.
        let request = serde_json::from_str::<ImportImageRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_mut() }.ok_or("editor handle is null")?;
        let pixels = unsafe { pixels.as_ref() }.ok_or("editor import pixels are null")?;
        // SAFETY: validated layout bounds every byte read from caller storage.
        let pixels = unsafe { copy_import_pixels(pixels) }?;
        let layer_id = session.import_image(ImportImage {
            pixels,
            name: request.name,
            selected_id: request.selected_id,
            point: request.point,
        })?;
        Ok::<_, String>(json!({"layer_id":layer_id,"snapshot":session.snapshot()}))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(imported) => json!({"ok":true,"result":imported}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

unsafe fn copy_import_pixels(pixels: &RegionPixels) -> Result<RgbaImage, String> {
    let width = pixels.width;
    let height = pixels.height;
    let pixel_count = u64::from(width) * u64::from(height);
    if width == 0
        || height == 0
        || width > MAX_RENDER_DIMENSION
        || height > MAX_RENDER_DIMENSION
        || pixel_count > MAX_RENDER_PIXELS
    {
        return Err("Editor images exceed the dimension or total decoded-pixel limit.".into());
    }
    let tight_row = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .ok_or("editor import dimensions overflow")?;
    if pixels.bytes_per_row < tight_row {
        return Err("editor import row stride is too small".into());
    }
    let last_row = usize::try_from(height - 1)
        .ok()
        .and_then(|height| height.checked_mul(pixels.bytes_per_row))
        .ok_or("editor import layout overflows")?;
    let required = last_row
        .checked_add(tight_row)
        .filter(|required| *required <= isize::MAX as usize)
        .ok_or("editor import layout overflows")?;
    if pixels.length < required {
        return Err("editor import pixel buffer is too short".into());
    }
    if pixels.data.is_null() {
        return Err("editor import pixel data is null".into());
    }
    let owned_length = usize::try_from(pixel_count)
        .ok()
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("editor import dimensions overflow")?;
    let mut owned = Vec::with_capacity(owned_length);
    for row in 0..height as usize {
        let start = row
            .checked_mul(pixels.bytes_per_row)
            .ok_or("editor import layout overflows")?;
        // SAFETY: required was checked against the declared length and pointer
        // arithmetic bounds. The caller initializes each tight RGBA row; this
        // slice deliberately excludes possibly uninitialized row padding.
        let source = unsafe { std::slice::from_raw_parts(pixels.data.add(start), tight_row) };
        owned.extend_from_slice(source);
    }
    RgbaImage::from_raw(width, height, owned).ok_or_else(|| "invalid editor image layout".into())
}

/// Publish the edited frame as a new file and distinct History artifact.
/// Does not mutate the session or save its draft. Run on the session worker.
///
/// # Safety
/// Non-null session is live and not accessed/freed concurrently. Input is
/// readable NUL-terminated UTF-8. Free owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_save_new_v1(
    session: *const EditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and a live serialized session.
        let request = serde_json::from_str::<SaveNewRequest>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_ref() }.ok_or("editor handle is null")?;
        captures_app::editor_output::save_new_export(
            &request.history_root,
            &session.pixels(),
            &request.destination,
            request.options,
            request.mode,
        )
        .map_err(|error| error.to_string())
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(saved) => json!({"ok":true,"result":saved}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Apply one command and return a snapshot, never image bytes.
///
/// # Safety
/// Non-null handle is live and exclusively owned for the call. Input is readable
/// NUL-terminated UTF-8. Free the owned JSON with captures_settings_free_v1.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_request_v1(
    handle: *mut EditorSession,
    request_json: *const c_char,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains valid input and exclusive handle ownership.
        let request = serde_json::from_str::<Request>(unsafe { text(request_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { handle.as_mut() }.ok_or("editor handle is null")?;
        session.execute(request)?;
        Ok::<_, String>(json!(session.snapshot()))
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    response(match result {
        Ok(snapshot) => json!({"ok":true,"result":snapshot}),
        Err(error) => json!({"ok":false,"error":error}),
    })
}

/// Encode on the session worker without changing editor state or doing I/O.
/// Returns independently owned bytes and an owned metadata/error JSON response.
///
/// # Safety
/// Non-null session is live and not accessed/freed concurrently. Input is readable
/// NUL-terminated UTF-8. Non-null output is aligned writable pointer storage; free
/// its JSON with captures_settings_free_v1. Null output refuses the operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_encode_v1(
    session: *const EditorSession,
    options_json: *const c_char,
    output: *mut *mut c_char,
) -> *mut Vec<u8> {
    if output.is_null() {
        return ptr::null_mut();
    }
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: caller retains readable input and a live serialized session.
        let options = serde_json::from_str::<ExportOptions>(unsafe { text(options_json) }?)
            .map_err(|error| error.to_string())?;
        let session = unsafe { session.as_ref() }.ok_or("editor handle is null")?;
        session.encode_export(options)
    }))
    .unwrap_or_else(|_| Err("internal panic".into()));
    let (handle, value) = match result {
        Ok(bytes) => {
            let value = json!({"ok":true,"result":{"length":bytes.len()}});
            (Box::into_raw(Box::new(bytes)), value)
        }
        Err(error) => (ptr::null_mut(), json!({"ok":false,"error":error})),
    };
    // SAFETY: caller supplies aligned writable pointer storage.
    unsafe { output.write(response(value)) };
    handle
}

#[repr(C)]
pub struct EditorBytes {
    pub data: *const u8,
    pub length: usize,
}

/// Borrow encoded bytes; false leaves output unchanged.
///
/// # Safety
/// Non-null export is a live handle from captures_editor_encode_v1, retained
/// throughout all reads. Non-null output is aligned writable EditorBytes storage.
/// Never mutate/free the data pointer. The handle may outlive the editor session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_export_bytes_v1(
    export: *const Vec<u8>,
    output: *mut EditorBytes,
) -> bool {
    if export.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains live export storage and aligned writable output.
    let bytes = unsafe { &*export };
    unsafe {
        output.write(EditorBytes {
            data: bytes.as_ptr(),
            length: bytes.len(),
        })
    };
    true
}

/// # Safety
/// Null or a live export from captures_editor_encode_v1, released exactly once
/// after all byte borrows end. May run on a different thread from the session.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_export_free_v1(export: *mut Vec<u8>) {
    if !export.is_null() {
        // SAFETY: caller transfers unique ownership after all reads finish.
        drop(unsafe { Box::from_raw(export) });
    }
}

/// Retain the current frame without copying pixels; null input returns null.
///
/// # Safety
/// Non-null handle is live and is not accessed/freed concurrently. Release the
/// returned frame once with captures_editor_frame_free_v1. It outlives edits and
/// session destruction and may be transferred to the UI thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_frame_v1(
    handle: *const EditorSession,
) -> *mut Arc<RgbaImage> {
    // SAFETY: caller retains a live session without concurrent mutations.
    unsafe { handle.as_ref() }
        .map(|session| Box::into_raw(Box::new(session.pixels())))
        .unwrap_or(ptr::null_mut())
}

/// Borrow top-down straight-alpha sRGB RGBA8. False leaves output unchanged.
///
/// # Safety
/// Non-null frame is live and remains so throughout every pixel read. Non-null
/// output is aligned writable RegionPixels storage. Never modify/free data.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_frame_pixels_v1(
    frame: *const Arc<RgbaImage>,
    output: *mut RegionPixels,
) -> bool {
    if frame.is_null() || output.is_null() {
        return false;
    }
    // SAFETY: caller retains a valid immutable frame through all pixel reads.
    let image = unsafe { &*frame };
    let pixels = RegionPixels {
        data: image.as_ptr(),
        length: image.len(),
        width: image.width(),
        height: image.height(),
        bytes_per_row: image.width() as usize * 4,
    };
    // SAFETY: output is aligned writable storage.
    unsafe { output.write(pixels) };
    true
}

/// # Safety
/// Null or a live frame from captures_editor_frame_v1, released exactly once
/// after every borrow has ended. May run on the UI thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_frame_free_v1(frame: *mut Arc<RgbaImage>) {
    if !frame.is_null() {
        // SAFETY: caller transfers unique box ownership; shared pixels use Arc.
        drop(unsafe { Box::from_raw(frame) });
    }
}

/// No implicit save on close. Hosts explicitly flush drafts before freeing.
///
/// # Safety
/// Null or live exclusive session from open, released once on its owner worker.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_editor_free_v1(handle: *mut EditorSession) {
    if !handle.is_null() {
        // SAFETY: caller transfers unique ownership after all calls complete.
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::{CStr, CString},
        mem::MaybeUninit,
    };

    #[test]
    fn drawing_geometry_is_owned_and_uses_shared_quadratics_and_arrow_outline() {
        let mut input = [
            AbiPoint { x: 2., y: 3. },
            AbiPoint { x: 10., y: 19. },
            AbiPoint { x: 26., y: 7. },
        ];
        let mut output = MaybeUninit::uninit();
        // SAFETY: all buffers are initialized/live; borrows end before their owner is freed.
        unsafe {
            let pen = captures_editor_draw_geometry_v1(
                1,
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
            );
            assert!(!pen.is_null());
            let output = output.assume_init();
            input[0].x = 99.;
            let points = std::slice::from_raw_parts(output.data, output.length);
            assert_eq!(output.stroke_width, 8.);
            assert_eq!(points.len(), 26);
            assert_ne!(points[0].x, input[0].x, "geometry owns a copy of its input");
            assert_eq!((points[0].x, points[0].y), (2., 3.));
            // Quadratic at t=.5, control (10,19), midpoint endpoint (18,13).
            assert_eq!((points[12].x, points[12].y), (10., 13.5));
            assert_eq!((points[25].x, points[25].y), (26., 7.));
            captures_editor_draw_geometry_free_v1(pen);

            let endpoints = [AbiPoint { x: 5., y: 9. }, AbiPoint { x: 85., y: 9. }];
            let mut output = MaybeUninit::uninit();
            let arrow =
                captures_editor_draw_geometry_v1(0, endpoints.as_ptr(), 2, output.as_mut_ptr());
            assert!(!arrow.is_null());
            let output = output.assume_init();
            let points = std::slice::from_raw_parts(output.data, output.length);
            assert!(
                points.len() > 6,
                "tapered outline includes the rounded tail, not a triangle"
            );
            assert!(points.iter().any(|point| point.x == 85. && point.y == 9.));
            assert!(points.iter().any(|point| point.y > 9.));
            assert!(points.iter().any(|point| point.y < 9.));
            captures_editor_draw_geometry_free_v1(arrow);
        }
    }

    #[test]
    fn drawing_geometry_rejects_invalid_inputs_and_retains_dots_and_minimum_arrows() {
        let input = [AbiPoint { x: 7., y: 11. }, AbiPoint { x: 8.49, y: 11. }];
        let mut output = DrawPoints {
            data: ptr::null(),
            length: 123,
            stroke_width: -1.,
        };
        // SAFETY: invalid metadata is rejected before dereference; all other buffers are live.
        unsafe {
            for (kind, points, count) in [
                (2, input.as_ptr(), 2),
                (0, input.as_ptr(), 1),
                (1, ptr::null(), 1),
                (1, input.as_ptr(), 0),
                (1, input.as_ptr(), usize::MAX),
            ] {
                assert!(
                    captures_editor_draw_geometry_v1(kind, points, count, &mut output).is_null()
                );
                assert_eq!(output.length, 123);
                assert_eq!(output.stroke_width, -1.);
                assert!(output.data.is_null());
            }
            assert!(
                captures_editor_draw_geometry_v1(1, input.as_ptr(), 1, ptr::null_mut()).is_null()
            );
            let invalid = [AbiPoint {
                x: f64::INFINITY,
                y: 0.,
            }];
            assert!(
                captures_editor_draw_geometry_v1(1, invalid.as_ptr(), 1, &mut output).is_null()
            );
            assert_eq!(output.length, 123);
            let dot = captures_editor_draw_geometry_v1(1, input.as_ptr(), 1, &mut output);
            assert!(!dot.is_null());
            assert_eq!(output.length, 1);
            assert_eq!(((*output.data).x, (*output.data).y), (7., 11.));
            captures_editor_draw_geometry_free_v1(dot);
            for (length, empty) in [(1.49, true), (1.5, false)] {
                let endpoints = [
                    input[0],
                    AbiPoint {
                        x: 7. + length,
                        y: 11.,
                    },
                ];
                let arrow = captures_editor_draw_geometry_v1(0, endpoints.as_ptr(), 2, &mut output);
                assert!(!arrow.is_null());
                assert_eq!(output.length == 0, empty);
                captures_editor_draw_geometry_free_v1(arrow);
            }
            captures_editor_draw_geometry_free_v1(ptr::null_mut());
        }
    }

    unsafe fn take_json(value: *mut c_char) -> serde_json::Value {
        // SAFETY: tests pass only live Rust-owned response strings.
        let json = serde_json::from_slice(unsafe { CStr::from_ptr(value) }.to_bytes()).unwrap();
        unsafe { crate::captures_settings_free_v1(value) };
        json
    }

    fn editor_fixture() -> (tempfile::TempDir, CString, RgbaImage) {
        let data = tempfile::tempdir().unwrap();
        let original = RgbaImage::from_fn(7, 3, |x, y| {
            image::Rgba([19 + x as u8 * 29, 31 + y as u8 * 67, 83, 255])
        });
        let capture = captures_app::persist_screenshot(
            &data.path().join("history"),
            &original,
            captures_capture::CaptureMode::Region,
        )
        .unwrap();
        let request = CString::new(
            json!({
                "history_root":data.path().join("history"),
                "drafts_root":data.path().join("drafts"),
                "artifact_id":capture.entry.id,
            })
            .to_string(),
        )
        .unwrap();
        (data, request, original)
    }

    unsafe fn open_editor(request: &CString) -> *mut EditorSession {
        let mut output = ptr::null_mut();
        // SAFETY: request and output remain readable/writable for the call.
        let session = unsafe { captures_editor_open_v1(request.as_ptr(), &mut output) };
        assert!(!session.is_null());
        assert_eq!(unsafe { take_json(output) }["ok"], true);
        session
    }

    unsafe fn import_response(
        session: *mut EditorSession,
        pixels: *const RegionPixels,
        request: *const c_char,
    ) -> serde_json::Value {
        // SAFETY: each caller documents live handle/input storage or intentional nulls.
        unsafe { take_json(captures_editor_import_image_v1(session, pixels, request)) }
    }

    #[test]
    fn import_copies_padded_pixels_and_round_trips_undo_redo_and_draft() {
        let (data, open_request, original) = editor_fixture();
        let imported = RgbaImage::from_raw(
            3,
            2,
            vec![
                201, 17, 91, 255, 33, 149, 207, 255, 117, 61, 5, 255, 8, 222, 47, 255, 173, 99,
                211, 255, 64, 13, 159, 255,
            ],
        )
        .unwrap();
        let mut caller = [MaybeUninit::<u8>::uninit(); 32];
        for (output, input) in caller[..12].iter_mut().zip(&imported.as_raw()[..12]) {
            output.write(*input);
        }
        for (output, input) in caller[16..28].iter_mut().zip(&imported.as_raw()[12..]) {
            output.write(*input);
        }
        let pixels = RegionPixels {
            data: caller.as_ptr().cast(),
            length: caller.len(),
            width: 3,
            height: 2,
            bytes_per_row: 16,
        };

        // SAFETY: all C inputs and handles remain live and serialized; owned JSON is freed.
        unsafe {
            let session = open_editor(&open_request);
            let initial = (&*session).pixels();
            assert_eq!(initial.as_ref(), &original);
            let response = import_response(
                session,
                &pixels,
                c"{\"name\":\"asymmetric.png\",\"selected_id\":\"capture-background\"}".as_ptr(),
            );
            assert_eq!(response["ok"], true);
            let layer_id = response["result"]["layer_id"].as_str().unwrap().to_owned();
            assert!(!layer_id.is_empty());
            assert_eq!(response["result"]["snapshot"]["document"]["width"], 7.);
            assert_eq!(response["result"]["snapshot"]["document"]["height"], 5.);
            assert_eq!(
                response["result"]["snapshot"]["document"]["elements"][1]["id"],
                layer_id
            );
            let imported_frame = (&*session).pixels();
            for y in 0..2 {
                for x in 0..3 {
                    assert_eq!(
                        imported_frame.get_pixel(x + 2, y + 3),
                        imported.get_pixel(x, y)
                    );
                }
            }

            caller.fill(MaybeUninit::new(0));
            assert_eq!((&*session).pixels(), imported_frame);
            let undo = take_json(captures_editor_request_v1(
                session,
                c"{\"operation\":\"undo\"}".as_ptr(),
            ));
            assert_eq!(undo["ok"], true);
            assert_eq!((&*session).pixels().as_ref(), &original);
            assert_eq!(undo["result"]["can_redo"], true);
            let redo = take_json(captures_editor_request_v1(
                session,
                c"{\"operation\":\"redo\"}".as_ptr(),
            ));
            assert_eq!(redo["result"]["document"]["elements"][1]["id"], layer_id);
            assert_eq!((&*session).pixels(), imported_frame);
            let saved = take_json(captures_editor_request_v1(
                session,
                c"{\"operation\":\"save_draft\",\"updated_at_ms\":91}".as_ptr(),
            ));
            assert_eq!(saved["ok"], true);
            captures_editor_free_v1(session);

            let reopened = open_editor(&open_request);
            let snapshot = take_json(captures_editor_request_v1(
                reopened,
                c"{\"operation\":\"snapshot\"}".as_ptr(),
            ));
            assert_eq!(
                snapshot["result"]["document"]["elements"][1]["id"],
                layer_id
            );
            assert_eq!(snapshot["result"]["has_draft"], true);
            assert_eq!((&*reopened).pixels(), imported_frame);
            captures_editor_free_v1(reopened);
        }
        assert!(data.path().join("drafts").exists());
    }

    #[test]
    fn malformed_imports_preserve_frame_document_redo_assets_and_files() {
        let (data, open_request, _) = editor_fixture();
        let bytes = [17, 91, 203, 255];
        let valid = RegionPixels {
            data: bytes.as_ptr(),
            length: bytes.len(),
            width: 1,
            height: 1,
            bytes_per_row: 4,
        };
        let request = c"{\"name\":\"one.png\"}";

        // SAFETY: valid buffers remain live; fake pointers are paired only with
        // metadata that must be rejected before any pixel read.
        unsafe {
            let session = open_editor(&open_request);
            assert_eq!(
                import_response(session, &valid, request.as_ptr())["ok"],
                true
            );
            assert_eq!(
                take_json(captures_editor_request_v1(
                    session,
                    c"{\"operation\":\"undo\"}".as_ptr()
                ))["result"]["can_redo"],
                true
            );
            let before = json!((&*session).snapshot());
            let frame = (&*session).pixels();
            let dangling = ptr::NonNull::<u8>::dangling().as_ptr();
            let invalid = [
                RegionPixels {
                    data: ptr::null(),
                    ..valid
                },
                RegionPixels { width: 0, ..valid },
                RegionPixels { length: 3, ..valid },
                RegionPixels {
                    bytes_per_row: 3,
                    ..valid
                },
                RegionPixels {
                    data: dangling,
                    length: usize::MAX,
                    width: 1,
                    height: 2,
                    bytes_per_row: usize::MAX,
                },
                RegionPixels {
                    data: dangling,
                    length: usize::MAX,
                    width: MAX_RENDER_DIMENSION + 1,
                    height: 1,
                    bytes_per_row: 0,
                },
            ];
            assert_eq!(
                import_response(session, ptr::null(), request.as_ptr())["ok"],
                false
            );
            for pixels in &invalid {
                assert_eq!(
                    import_response(session, pixels, request.as_ptr())["ok"],
                    false
                );
            }
            for metadata in [
                ptr::null(),
                c"{bad".as_ptr(),
                c"{\"name\":\"one.png\",\"src\":\"file:///tmp/not-owned\"}".as_ptr(),
                c"{\"name\":\"one.png\",\"pixels\":[1,2,3,4]}".as_ptr(),
            ] {
                assert_eq!(import_response(session, &valid, metadata)["ok"], false);
            }
            assert_eq!(
                import_response(ptr::null_mut(), &valid, request.as_ptr())["ok"],
                false
            );
            assert_eq!(json!((&*session).snapshot()), before);
            assert!((&*session).snapshot().can_redo);
            assert!(Arc::ptr_eq(&frame, &(&*session).pixels()));
            assert!(!data.path().join("drafts").exists());
            assert_eq!(
                take_json(captures_editor_request_v1(
                    session,
                    c"{\"operation\":\"redo\"}".as_ptr()
                ))["ok"],
                true
            );
            assert_eq!((&*session).pixels().get_pixel(3, 3).0, bytes);
            captures_editor_free_v1(session);
        }
    }

    #[test]
    fn save_new_reports_publication_collision_and_partial_success_without_changing_session() {
        let data = tempfile::tempdir().unwrap();
        let root = data.path().join("history");
        let capture = captures_app::persist_screenshot(
            &root,
            &RgbaImage::from_fn(7, 3, |x, y| {
                image::Rgba([x as u8 * 31, y as u8 * 71, 9, 255])
            }),
            captures_capture::CaptureMode::Window,
        )
        .unwrap();
        let mut session = EditorSession::open(OpenRequest {
            history_root: root.clone(),
            drafts_root: data.path().join("drafts"),
            artifact_id: capture.entry.id,
        })
        .unwrap();
        session
            .execute(Request::Crop {
                rect: captures_app::editor::Rect {
                    x: 2.,
                    y: 1.,
                    width: 4.,
                    height: 2.,
                },
            })
            .unwrap();
        let before = json!(session.snapshot());
        let pixels = session.pixels();
        let destination = data.path().join("copy.png");
        let mut request = json!({"history_root":root,"destination":destination,"mode":"window","options":{"format":"png","quality":"preserve","quality_value":80,"png":{}}});
        let input = CString::new(request.to_string()).unwrap();
        // SAFETY: stack session and C strings remain live and serialized; every owned response is freed.
        unsafe {
            assert_eq!(
                take_json(captures_editor_save_new_v1(ptr::null(), input.as_ptr()))["ok"],
                false
            );
            assert_eq!(
                take_json(captures_editor_save_new_v1(&session, ptr::null()))["ok"],
                false
            );
            assert_eq!(
                take_json(captures_editor_save_new_v1(&session, c"{}".as_ptr()))["ok"],
                false
            );
            assert!(!destination.exists());
            let saved = take_json(captures_editor_save_new_v1(&session, input.as_ptr()));
            assert_eq!(saved["ok"], true);
            assert_eq!(saved["result"]["status"], "saved");
            assert_eq!(saved["result"]["path"], json!(destination));
            assert_eq!(saved["result"]["artifact"]["entry"]["mode"], "window");
            let bytes = std::fs::read(&destination).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().into_rgba8();
            assert_eq!(decoded.dimensions(), (4, 2));
            assert_eq!(decoded.get_pixel(0, 0).0, [62, 71, 9, 255]);
            assert_eq!(
                take_json(captures_editor_save_new_v1(&session, input.as_ptr()))["ok"],
                false
            );
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);

            let blocked = data.path().join("blocked-history");
            std::fs::write(&blocked, b"blocked").unwrap();
            request["history_root"] = json!(blocked);
            request["destination"] = json!(data.path().join("recovered.png"));
            let input = CString::new(request.to_string()).unwrap();
            let saved = take_json(captures_editor_save_new_v1(&session, input.as_ptr()));
            assert_eq!(saved["ok"], true);
            assert_eq!(saved["result"]["status"], "saved_without_history");
            assert!(!saved["result"]["warning"].as_str().unwrap().is_empty());
            assert_eq!(
                std::fs::read(data.path().join("recovered.png")).unwrap(),
                bytes
            );
        }
        assert_eq!(json!(session.snapshot()), before);
        assert!(Arc::ptr_eq(&pixels, &session.pixels()));
        assert!(!data.path().join("drafts").exists());
    }

    #[test]
    fn frames_survive_edit_and_close_and_failed_requests_preserve_state() {
        let data = tempfile::tempdir().unwrap();
        let original = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 40, y as u8 * 80, 9, 255])
        });
        let capture = captures_app::persist_screenshot(
            &data.path().join("history"),
            &original,
            captures_capture::CaptureMode::Region,
        )
        .unwrap();
        let request = CString::new(
            json!({
                "history_root":data.path().join("history"),
                "drafts_root":data.path().join("drafts"),
                "artifact_id":capture.entry.id,
            })
            .to_string(),
        )
        .unwrap();
        // SAFETY: test retains all input, handle, frame and response ownership.
        unsafe {
            let mut output = ptr::null_mut();
            let session = captures_editor_open_v1(request.as_ptr(), &mut output);
            assert!(!session.is_null());
            assert_eq!(take_json(output)["ok"], true);
            let old = captures_editor_frame_v1(session);
            let request =
                c"{\"operation\":\"crop\",\"rect\":{\"x\":1,\"y\":0,\"width\":2,\"height\":1}}";
            assert_eq!(
                take_json(captures_editor_request_v1(session, request.as_ptr()))["result"]["document"]
                    ["width"],
                2.
            );
            assert_eq!(
                take_json(captures_editor_request_v1(session, c"{bad".as_ptr()))["ok"],
                false
            );
            let current = captures_editor_frame_v1(session);
            let options =
                c"{\"format\":\"png\",\"quality\":\"preserve\",\"quality_value\":100,\"png\":{}}";
            let exported = captures_editor_encode_v1(session, options.as_ptr(), &mut output);
            assert!(!exported.is_null());
            let encoded_response = take_json(output);
            assert_eq!(encoded_response["ok"], true);
            assert_eq!(encoded_response["result"].as_object().unwrap().len(), 1);
            let before = take_json(captures_editor_request_v1(
                session,
                c"{\"operation\":\"snapshot\"}".as_ptr(),
            ));
            for invalid in [
                c"{bad",
                c"{\"format\":\"gif\",\"quality\":\"preserve\",\"quality_value\":100,\"png\":{}}",
                c"{\"format\":\"png\",\"quality\":\"unknown\",\"quality_value\":100,\"png\":{}}",
                c"{\"format\":\"png\",\"quality\":\"preserve\",\"quality_value\":256,\"png\":{}}",
                c"{\"format\":\"png\",\"quality\":\"maximum\",\"quality_value\":100,\"max_size_bytes\":0,\"png\":{}}",
            ] {
                assert!(captures_editor_encode_v1(session, invalid.as_ptr(), &mut output).is_null());
                assert_eq!(take_json(output)["ok"], false);
                assert_eq!(take_json(captures_editor_request_v1(session, c"{\"operation\":\"snapshot\"}".as_ptr())), before);
            }
            assert_eq!(
                take_json(captures_editor_request_v1(
                    session,
                    c"{\"operation\":\"undo\"}".as_ptr()
                ))["ok"],
                true
            );
            captures_editor_free_v1(session);
            let mut bytes = EditorBytes {
                data: ptr::null(),
                length: 0,
            };
            assert!(!captures_editor_export_bytes_v1(exported, ptr::null_mut()));
            assert!(captures_editor_export_bytes_v1(exported, &mut bytes));
            assert_eq!(encoded_response["result"]["length"], bytes.length);
            let decoded =
                image::load_from_memory(std::slice::from_raw_parts(bytes.data, bytes.length))
                    .unwrap()
                    .into_rgba8();
            assert_eq!(decoded.dimensions(), (2, 1));
            assert_eq!(decoded.as_raw(), &[40, 0, 9, 255, 80, 0, 9, 255]);
            captures_editor_export_free_v1(exported);
            let mut pixels = RegionPixels {
                data: ptr::null(),
                length: 0,
                width: 0,
                height: 0,
                bytes_per_row: 0,
            };
            assert!(captures_editor_frame_pixels_v1(old, &mut pixels));
            assert_eq!(
                (pixels.width, pixels.height, pixels.bytes_per_row),
                (3, 2, 12)
            );
            assert_eq!(
                std::slice::from_raw_parts(pixels.data, pixels.length),
                original.as_raw()
            );
            assert!(captures_editor_frame_pixels_v1(current, &mut pixels));
            assert_eq!((pixels.width, pixels.height), (2, 1));
            assert_eq!(
                std::slice::from_raw_parts(pixels.data, pixels.length),
                &[40, 0, 9, 255, 80, 0, 9, 255]
            );
            captures_editor_frame_free_v1(old);
            captures_editor_frame_free_v1(current);
        }
    }

    #[test]
    fn null_handles_and_bad_open_have_owned_errors_and_leave_pixel_output_unchanged() {
        // SAFETY: null pointers are explicitly accepted; output lives throughout.
        unsafe {
            assert!(captures_editor_open_v1(ptr::null(), ptr::null_mut()).is_null());
            let mut output = ptr::null_mut();
            assert!(captures_editor_encode_v1(ptr::null(), ptr::null(), ptr::null_mut()).is_null());
            for options in [
                ptr::null(),
                c"{\"format\":\"png\",\"quality\":\"preserve\",\"quality_value\":100,\"png\":{}}"
                    .as_ptr(),
            ] {
                assert!(captures_editor_encode_v1(ptr::null(), options, &mut output).is_null());
                assert_eq!(take_json(output)["ok"], false);
            }
            let mut bytes = EditorBytes {
                data: ptr::null(),
                length: 91,
            };
            assert!(!captures_editor_export_bytes_v1(ptr::null(), &mut bytes));
            assert_eq!(bytes.length, 91);
            assert!(bytes.data.is_null());
            captures_editor_export_free_v1(ptr::null_mut());
            assert!(captures_editor_open_v1(ptr::null(), &mut output).is_null());
            assert_eq!(take_json(output)["ok"], false);
            assert_eq!(
                take_json(captures_editor_request_v1(
                    ptr::null_mut(),
                    c"{\"operation\":\"snapshot\"}".as_ptr()
                ))["ok"],
                false
            );
            assert!(captures_editor_frame_v1(ptr::null()).is_null());
            let mut pixels = RegionPixels {
                data: ptr::null(),
                length: 91,
                width: 7,
                height: 13,
                bytes_per_row: 28,
            };
            assert!(!captures_editor_frame_pixels_v1(ptr::null(), &mut pixels));
            assert_eq!((pixels.length, pixels.width, pixels.height), (91, 7, 13));
            captures_editor_frame_free_v1(ptr::null_mut());
            captures_editor_free_v1(ptr::null_mut());
        }
    }
}
