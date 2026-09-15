mod draft_worker;
mod feedback_worker;
mod image_worker;
mod media_worker;
mod renderer;

use captures_capture::{CaptureMode, DisplayDescriptor, XcapBackend};
use captures_feedback::FeedbackDraft;
use captures_media::{AudioEdit, EditSpec, ExportFormat, ExportSpec, QualityPreset};
use captures_recording::{
    AudioOptions, GifOptions, RecordingKind, RecordingOptions, RecordingSegmentInfo,
    RecordingState, RecordingTarget,
};
use captures_recording_xcap::XcapRecordingSegment;
use captures_windows_native::{
    async_state::{
        SaveCompletion, SaveTracker, accepts_document_request, classify_save_completion,
    },
    draft::{DraftIdentity, DraftSession, DraftStore, retire_capture_session},
    editor::{
        BlendMode, FreehandGesture, ImageTarget, Layer, RemoveBackgroundMode, resize_from_corner,
    },
    geometry::{
        Point, Rect, SelectionDrag, editor_layer_lock_button, editor_layer_visibility_button,
        editor_shape_flyout_index, recording_editor_timeline_track, rounded_contains,
        screenshot_editor_canvas, screenshot_editor_properties_y, screenshot_editor_viewport,
        update_selection,
    },
    history::{Artifact, History, move_to_trash, restore_from_trash, safe_delete},
    preview_motion::{MEDIA_WIDTH, PAD},
    settings::{Settings, data_dir, profile_id},
    state::{
        AppState, EditorExportSize, EditorInputField, EditorQualityMode, PreferencesPage,
        RecordingEditorState, RecordingUi, Surface, backspace_feedback, can_replace_editor_source,
        delete_feedback, replace_feedback_selection, sanitize_editor_filename,
    },
    theme::{palette, theme_colors},
};
use draft_worker::DraftWorker;
use feedback_worker::FeedbackWorker;
use image::RgbaImage;
use image_worker::{EncodeSpec as ImageEncodeSpec, Event as ImageEvent, ImageWorker};
use media_worker::{ComparisonSpec, Event as MediaEvent, ExportJob, MediaWorker, PlaybackSpec};
use renderer::{Frame, Renderer};
use std::os::windows::ffi::OsStrExt;
use std::{
    fs::{self, OpenOptions},
    io::BufWriter,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
            ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HANDLE, HWND,
            LPARAM, LRESULT, POINT, RECT, WPARAM,
        },
        Graphics::Gdi::{
            BeginPaint, CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, EndPaint,
            HGDIOBJ, InvalidateRect, PAINTSTRUCT, RGN_OR, ScreenToClient, SetWindowRgn,
        },
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, IDataObject,
            },
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
            },
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{
                CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource,
                IDropSource_Impl, OleInitialize, OleUninitialize,
            },
            Registry::{
                HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
                RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW,
            },
            SystemServices::{MK_LBUTTON, MK_RBUTTON, MODIFIERKEYS_FLAGS},
            Threading::CreateMutexW,
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                GetKeyState, HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_SHIFT, RegisterHotKey,
                ReleaseCapture, SetCapture, UnregisterHotKey, VK_CONTROL, VK_DELETE, VK_END,
                VK_ESCAPE, VK_HOME, VK_LEFT, VK_RETURN, VK_RIGHT,
            },
            Shell::{
                Common::{COMDLG_FILTERSPEC, ITEMIDLIST},
                FOS_ALLOWMULTISELECT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FileOpenDialog,
                IFileOpenDialog, ILClone, ILCreateFromPathW, ILFindLastID, ILRemoveLastID,
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                SHCreateDataObject, SIGDN_FILESYSPATH, Shell_NotifyIconW,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, HRESULT, HSTRING, PCWSTR, implement, w},
};

const CLASS: PCWSTR = w!("CapturesWindowsNativeWindow");
const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATE_INSTANCE: u32 = WM_APP + 2;
const WM_FIXTURE_EXIT: u32 = WM_APP + 3;
const TIMER_ANIMATION: usize = 1;
const HOTKEY_CAPTURE: i32 = 100;
const HOTKEY_DISPLAY: i32 = 101;

type EditorEstimateKey = (
    u64,
    u64,
    String,
    EditorExportSize,
    EditorQualityMode,
    u8,
    u64,
    u32,
    u32,
);

struct RecordingSession {
    options: RecordingOptions,
    display: DisplayDescriptor,
    active: Option<XcapRecordingSegment>,
    segment_paths: Vec<PathBuf>,
    completed: Vec<RecordingSegmentInfo>,
    directory: PathBuf,
}

#[implement(IDropSource)]
struct FileDropSource;

impl IDropSource_Impl for FileDropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & (MK_LBUTTON.0 | MK_RBUTTON.0) == 0 {
            DRAGDROP_S_DROP
        } else {
            HRESULT(0)
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

struct App {
    hwnd: HWND,
    renderer: Option<Renderer>,
    state: AppState,
    settings: Settings,
    history: History,
    recording: Option<RecordingSession>,
    recording_mode: CaptureMode,
    editor_drag: Option<Point>,
    editor_freehand: Option<FreehandGesture>,
    editor_remove_stroke: Option<(ImageTarget, Vec<Point>)>,
    editor_transform: Option<EditorTransform>,
    editor_text_origin: Option<Point>,
    editor_text: String,
    next_session_check: Instant,
    width: u32,
    height: u32,
    dpi: f32,
    tray: NOTIFYICONDATAW,
    instance_mutex: HANDLE,
    delete_return: Surface,
    start_hidden: bool,
    fixture_mode: bool,
    draft_fixture_phase: Option<DraftFixturePhase>,
    media: MediaWorker,
    media_epoch: u64,
    frame_request: u64,
    comparison_request: u64,
    export_request: u64,
    feedback: FeedbackWorker,
    feedback_request: u64,
    feedback_high_surrogate: Option<u16>,
    images: ImageWorker,
    editor_render_requested: (u64, u64),
    editor_render_inflight: bool,
    editor_estimate_request: u64,
    editor_estimate_key: Option<EditorEstimateKey>,
    editor_copy_request: u64,
    editor_save_request: u64,
    editor_saves: SaveTracker,
    drafts: DraftWorker,
    editor_draft: Option<DraftSession>,
    editor_pan_drag: Option<(Point, Point)>,
    editor_input_select_all: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DraftFixturePhase {
    Create,
    Restore,
}

#[derive(Clone)]
enum EditorTransform {
    Move {
        anchor: Point,
        original: Layer,
    },
    Resize {
        corner: usize,
        original: Layer,
    },
    Rotate {
        center: Point,
        pointer_offset: f32,
        original: Layer,
    },
}

impl EditorTransform {
    fn original(&self) -> &Layer {
        match self {
            Self::Move { original, .. }
            | Self::Resize { original, .. }
            | Self::Rotate { original, .. } => original,
        }
    }

    fn update(&self, pointer: Point) -> Option<Layer> {
        match self {
            Self::Move { anchor, original } => Some(original.translated(Point {
                x: pointer.x - anchor.x,
                y: pointer.y - anchor.y,
            })),
            Self::Resize { corner, original } => resize_from_corner(original, *corner, pointer),
            Self::Rotate {
                center,
                pointer_offset,
                original,
            } => {
                let mut layer = original.clone();
                layer.rotation_degrees = (pointer.y - center.y)
                    .atan2(pointer.x - center.x)
                    .to_degrees()
                    - pointer_offset;
                Some(layer)
            }
        }
    }
}

pub fn run() -> Result<(), String> {
    unsafe {
        let id = profile_id(&data_dir());
        let mutex_name = HSTRING::from(format!("Local\\CapturesWindowsNativeExperiment-{id}"));
        let window_title = HSTRING::from(format!("Captures [{id}]"));
        let instance_mutex = CreateMutexW(None, false, &mutex_name).map_err(win_error)?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            for _ in 0..100 {
                if let Ok(existing) = FindWindowW(CLASS, &window_title) {
                    PostMessageW(Some(existing), WM_ACTIVATE_INSTANCE, WPARAM(0), LPARAM(0))
                        .map_err(win_error)?;
                    CloseHandle(instance_mutex).map_err(win_error)?;
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(30));
            }
            CloseHandle(instance_mutex).map_err(win_error)?;
            return Err("primary Captures instance did not become ready".into());
        }
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            .map_err(win_error)?;
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(win_error)?;
        OleInitialize(None).map_err(win_error)?;
        let module = GetModuleHandleW(None).map_err(win_error)?;
        let cursor = LoadCursorW(None, IDC_ARROW).map_err(win_error)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: module.into(),
            hCursor: cursor,
            lpszClassName: CLASS,
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(last_error("register window class"));
        }
        let mut settings = Settings::load()?;
        if let Some(appearance) = argument_value("--appearance") {
            settings.appearance = appearance;
            settings.validate()?;
        }
        fs::create_dir_all(&settings.output_directory).map_err(|e| e.to_string())?;
        let mut history = History::load(&data_dir())?;
        let mut state = AppState::default();
        let requested_view = argument_value("--view");
        let draft_fixture_phase = match requested_view.as_deref() {
            Some("editor-draft-create") => Some(DraftFixturePhase::Create),
            Some("editor-draft-restore") => Some(DraftFixturePhase::Restore),
            _ => None,
        };
        let mut draft_fixture_open = None;
        if let Some(view) = requested_view.as_deref() {
            let fixture_path = argument_value("--fixture-image").map(PathBuf::from);
            let fixture_image = fixture_path
                .as_ref()
                .map(image::open)
                .transpose()
                .map_err(|error| format!("cannot open fixture image: {error}"))?
                .map(|image| image.to_rgba8());
            if draft_fixture_phase.is_some() {
                draft_fixture_open = Some((
                    fixture_image
                        .clone()
                        .ok_or("draft fixture requires --fixture-image")?,
                    fixture_path.ok_or("draft fixture requires a source path")?,
                ));
            }
            prepare_fixture(
                &mut state,
                view,
                fixture_image,
                argument_value("--fixture-video").map(PathBuf::from),
            );
            if view == "preview-delete-input" {
                settings.output_directory = data_dir().join("fixture-captures");
                fs::create_dir_all(&settings.output_directory).map_err(|e| e.to_string())?;
                let preview = &mut state.previews[0];
                preview.artifact.path = settings.output_directory.join("delete-input.png");
                preview
                    .image
                    .save(&preview.artifact.path)
                    .map_err(|e| e.to_string())?;
                history.add(preview.artifact.clone())?;
            }
        }
        let mut app = Box::new(App {
            hwnd: HWND::default(),
            renderer: None,
            state,
            settings,
            history,
            recording: None,
            recording_mode: CaptureMode::Region,
            editor_drag: None,
            editor_freehand: None,
            editor_remove_stroke: None,
            editor_transform: None,
            editor_text_origin: None,
            editor_text: String::new(),
            next_session_check: Instant::now(),
            width: 420,
            height: 430,
            dpi: 96.0,
            tray: NOTIFYICONDATAW::default(),
            instance_mutex,
            delete_return: Surface::Menu,
            start_hidden: std::env::args().any(|argument| argument == "--background"),
            fixture_mode: requested_view.is_some(),
            draft_fixture_phase,
            media: MediaWorker::new(),
            media_epoch: 0,
            frame_request: 0,
            comparison_request: 0,
            export_request: 0,
            feedback: FeedbackWorker::new(),
            feedback_request: 0,
            feedback_high_surrogate: None,
            images: ImageWorker::new(),
            editor_render_requested: (0, 0),
            editor_render_inflight: false,
            editor_estimate_request: 0,
            editor_estimate_key: None,
            editor_copy_request: 0,
            editor_save_request: 0,
            editor_saves: SaveTracker::default(),
            drafts: DraftWorker::new(data_dir()),
            editor_draft: None,
            editor_pan_drag: None,
            editor_input_select_all: false,
        });
        if let Some((image, path)) = draft_fixture_open {
            app.open_image_editor(image, Some(path.clone()), DraftIdentity::capture(path));
        }
        let raw = Box::into_raw(app);
        let _hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            &window_title,
            WS_POPUP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            420,
            430,
            None,
            None,
            Some(module.into()),
            Some(raw.cast()),
        )
        .map_err(win_error)?;
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if draft_fixture_phase.is_some()
                && message.message == WM_FIXTURE_EXIT
                && message.hwnd == _hwnd
            {
                let _ = DestroyWindow(_hwnd);
                continue;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        OleUninitialize();
        CoUninitialize();
    }
    Ok(())
}

pub fn fatal(error: &str) {
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(data_dir().join("startup-error.txt"), error);
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam.0 as *const CREATESTRUCTW);
            let app = create.lpCreateParams.cast::<App>();
            (*app).hwnd = hwnd;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as isize);
        }
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
        if pointer.is_null() {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        let app = &mut *pointer;
        match message {
            WM_CREATE => {
                if let Err(error) = app.initialize() {
                    app.set_error(error);
                }
                LRESULT(0)
            }
            WM_PAINT => {
                let mut paint = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut paint);
                app.paint();
                let _ = EndPaint(hwnd, &paint);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE
                if matches!(app.state.surface, Surface::Preview | Surface::RecordingHud) =>
            {
                LRESULT(MA_NOACTIVATE as isize)
            }
            WM_NCHITTEST if app.state.surface == Surface::Preview => {
                let mut cursor = POINT {
                    x: point(lparam.0).x as i32,
                    y: point(lparam.0).y as i32,
                };
                let _ = ScreenToClient(hwnd, &mut cursor);
                if app.preview_contains(Point {
                    x: cursor.x as f32 * 96.0 / app.dpi,
                    y: cursor.y as f32 * 96.0 / app.dpi,
                }) {
                    LRESULT(HTCLIENT as isize)
                } else {
                    LRESULT(HTTRANSPARENT as isize)
                }
            }
            WM_SIZE => {
                app.width = loword(lparam.0) as u32;
                app.height = hiword(lparam.0) as u32;
                if let Some(renderer) = &mut app.renderer
                    && let Err(error) = renderer.resize(app.width, app.height, app.dpi)
                {
                    app.set_error(error.to_string());
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                app.dpi = hiword(wparam.0 as isize) as f32;
                let rect = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                LRESULT(0)
            }
            WM_LBUTTONDOWN => {
                app.pointer_down(app.logical_point(lparam.0));
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                app.pointer_move(app.logical_point(lparam.0));
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                app.pointer_up(app.logical_point(lparam.0));
                LRESULT(0)
            }
            WM_KEYDOWN => {
                app.key(wparam.0 as u32);
                LRESULT(0)
            }
            WM_CHAR => {
                app.character_utf16(wparam.0 as u16);
                LRESULT(0)
            }
            WM_PASTE => {
                app.paste_feedback();
                LRESULT(0)
            }
            WM_HOTKEY => {
                let mode = if wparam.0 as i32 == HOTKEY_DISPLAY {
                    CaptureMode::Display
                } else {
                    CaptureMode::Region
                };
                app.start_capture(mode, false);
                LRESULT(0)
            }
            WM_TIMER => {
                let now = Instant::now();
                if now >= app.next_session_check {
                    app.next_session_check = now + Duration::from_secs(1);
                    let recording = app
                        .state
                        .recording
                        .as_ref()
                        .is_some_and(|ui| ui.state == RecordingState::Recording);
                    if recording && !captures_session::capture_session_available() {
                        app.toggle_pause();
                        if let Some(ui) = app.state.recording.as_mut() {
                            ui.warning = Some(
                                "Recording paused because the Windows session became unavailable"
                                    .into(),
                            );
                        }
                    }
                }
                let had_previews = !app.state.previews.is_empty();
                let changed = app.state.tick(now);
                app.tick_recording_editor(now);
                app.process_media_events();
                app.process_feedback_events();
                app.tick_image_editor();
                app.process_image_events();
                app.process_draft_events();
                app.tick_editor_draft(now);
                if had_previews
                    && app.state.previews.is_empty()
                    && app.state.surface == Surface::Preview
                {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    if app.fixture_mode {
                        let _ =
                            fs::write(data_dir().join("preview-exit-finished.txt"), "empty:hidden");
                    }
                }
                let animating =
                    app.state.surface == Surface::Preview
                        && app.state.previews.iter().any(|preview| {
                            preview.dismissing.is_some() || preview.deletion.is_some()
                        });
                if app.state.surface == Surface::Preview && (changed || animating) {
                    app.update_preview_region();
                }
                // Retained previews must not repaint an unrelated editor or
                // Preferences at 60Hz; settled cards have no animation work.
                if changed || app.state.surface == Surface::RecordingHud || animating {
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }
            WM_TRAY if lparam.0 as u32 == WM_LBUTTONUP || lparam.0 as u32 == WM_RBUTTONUP => {
                app.show_surface(Surface::Menu, 420, 430, false);
                LRESULT(0)
            }
            WM_ACTIVATE_INSTANCE => {
                app.show_surface(Surface::Menu, 420, 430, false);
                LRESULT(0)
            }
            WM_CLOSE => {
                app.flush_editor_draft();
                app.media.cancel_jobs();
                app.media_epoch = app.media_epoch.wrapping_add(1);
                if let Some(phase) = app.draft_fixture_phase {
                    app.write_draft_flush_marker(phase);
                    let _ = PostMessageW(Some(hwnd), WM_FIXTURE_EXIT, WPARAM(0), LPARAM(0));
                    return LRESULT(0);
                }
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_DESTROY => {
                app.flush_editor_draft();
                app.remove_tray();
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_CAPTURE);
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_DISPLAY);
                let _ = CloseHandle(app.instance_mutex);
                PostQuitMessage(0);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(pointer));
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

impl App {
    fn write_draft_flush_marker(&self, phase: DraftFixturePhase) {
        let expected = self
            .state
            .editor
            .as_ref()
            .map(|document| document.render_key());
        let persisted = self
            .editor_draft
            .as_ref()
            .map(|session| session.persisted_key);
        let value = if expected.is_some() && expected == persisted {
            "flush:exact"
        } else {
            "flush:mismatch"
        };
        let name = match phase {
            DraftFixturePhase::Create => "editor-draft-create-flushed.txt",
            DraftFixturePhase::Restore => "editor-draft-restore-flushed.txt",
        };
        let _ = fs::write(data_dir().join(name), value);
    }

    unsafe fn initialize(&mut self) -> Result<(), String> {
        unsafe {
            self.dpi = GetDpiForWindow(self.hwnd) as f32;
            self.renderer = Some(
                Renderer::new(self.hwnd, self.width, self.height, self.dpi).map_err(win_error)?,
            );
            if let Some(renderer) = &self.renderer {
                let _ = fs::create_dir_all(data_dir());
                let _ = fs::write(data_dir().join("render-driver.txt"), renderer.driver_name());
            }
            SetTimer(Some(self.hwnd), TIMER_ANIMATION, 16, None);
            RegisterHotKey(
                Some(self.hwnd),
                HOTKEY_CAPTURE,
                MOD_CONTROL | MOD_SHIFT,
                0x20,
            )
            .map_err(win_error)?;
            RegisterHotKey(Some(self.hwnd), HOTKEY_DISPLAY, HOT_KEY_MODIFIERS(0), 0x2c)
                .map_err(win_error)?;
            self.add_tray()?;
            if self.state.surface == Surface::RecordingEditor
                && let Some(source) = self
                    .state
                    .recording
                    .as_ref()
                    .and_then(|recording| recording.source.clone())
            {
                self.open_recording_editor(source);
            }
            let (surface, width, height) = match self.state.surface {
                Surface::ScreenshotEditor => (Surface::ScreenshotEditor, 1100, 720),
                Surface::RecordingSelector => (Surface::RecordingSelector, 460, 450),
                Surface::RecordingHud => (Surface::RecordingHud, 500, 82),
                Surface::RecordingEditor => (Surface::RecordingEditor, 1000, 680),
                Surface::Preview => (
                    Surface::Preview,
                    (MEDIA_WIDTH + PAD * 2.0) as i32,
                    200 + PAD as i32 * 2,
                ),
                Surface::History => (Surface::History, 720, 430),
                Surface::Preferences => (Surface::Preferences, 760, 520),
                Surface::Feedback => (Surface::Feedback, 620, 500),
                Surface::DeleteConfirmation => (Surface::DeleteConfirmation, 480, 300),
                _ => (Surface::Menu, 420, 430),
            };
            if self.start_hidden {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            } else {
                self.show_surface(surface, width, height, surface == Surface::Preview);
            }
            Ok(())
        }
    }

    fn logical_point(&self, value: isize) -> Point {
        let point = point(value);
        let scale = 96.0 / self.dpi.max(96.0);
        Point {
            x: point.x * scale,
            y: point.y * scale,
        }
    }

    unsafe fn paint(&mut self) {
        let (accent, signal) = theme_colors(
            &self.settings.theme,
            &self.settings.custom_accent,
            &self.settings.custom_signal,
        );
        let light = self.settings.appearance == "light";
        if let Some(renderer) = &mut self.renderer {
            let rendered = renderer
                .draw(Frame {
                    state: &self.state,
                    settings: &self.settings,
                    history: self.history.entries(),
                    recording_mode: self.recording_mode,
                    palette: palette(light, accent, signal),
                    width: self.width as f32 * 96.0 / self.dpi,
                    height: self.height as f32 * 96.0 / self.dpi,
                })
                .is_ok();
            if rendered
                && self.fixture_mode
                && self.state.surface == Surface::Preview
                && self
                    .state
                    .previews
                    .iter()
                    .any(|preview| preview.deletion.is_some())
            {
                let _ = fs::write(
                    data_dir().join("preview-deletion-presented.txt"),
                    "dust:presented",
                );
            }
            if rendered
                && self.fixture_mode
                && self.state.surface == Surface::RecordingEditor
                && self.state.recording_editor.is_some()
                && let Some(preview) = &self.state.recording_preview
            {
                let _ = fs::write(
                    data_dir().join("recording-frame-presented.txt"),
                    format!(
                        "{}x{} decoded paused frame",
                        preview.width(),
                        preview.height()
                    ),
                );
            }
        }
    }

    unsafe fn show_surface(
        &mut self,
        surface: Surface,
        width: i32,
        height: i32,
        no_activate: bool,
    ) {
        unsafe {
            if self.state.surface == Surface::RecordingEditor && surface != Surface::RecordingEditor
            {
                self.media.cancel_jobs();
                self.media_epoch = self.media_epoch.wrapping_add(1);
                if let Some(editor) = self.state.recording_editor.as_mut() {
                    editor.playing = false;
                }
            }
            self.state.surface = surface;
            let mut extended = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) as u32;
            if matches!(surface, Surface::Preview | Surface::RecordingHud) {
                extended |= WS_EX_NOACTIVATE.0;
            } else {
                extended &= !WS_EX_NOACTIVATE.0;
            }
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, extended as isize);
            let mut flags = SWP_SHOWWINDOW;
            let insert = if matches!(
                surface,
                Surface::Overlay | Surface::Preview | Surface::RecordingHud
            ) {
                Some(HWND_TOPMOST)
            } else {
                Some(HWND_NOTOPMOST)
            };
            if no_activate {
                flags |= SWP_NOACTIVATE;
            }
            let _ = SetWindowPos(
                self.hwnd,
                insert,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                scale(width, self.dpi),
                scale(height, self.dpi),
                flags | SWP_NOMOVE,
            );
            if surface == Surface::Preview {
                self.update_preview_region();
            } else {
                let _ = SetWindowRgn(self.hwnd, None, true);
            }
            let excluded = !self.fixture_mode
                && match surface {
                    Surface::Overlay => true,
                    Surface::Preview => !self.settings.include_mini_previews_in_captures,
                    Surface::RecordingHud => !self.settings.include_recording_controls_in_captures,
                    _ => false,
                };
            let _ = SetWindowDisplayAffinity(
                self.hwnd,
                if excluded {
                    WDA_EXCLUDEFROMCAPTURE
                } else {
                    WDA_NONE
                },
            );
            let _ = InvalidateRect(Some(self.hwnd), None, false);
            if !no_activate {
                let _ = SetForegroundWindow(self.hwnd);
            }
        }
    }

    unsafe fn start_capture(&mut self, mode: CaptureMode, recording: bool) {
        unsafe {
            if !captures_session::capture_session_available() {
                self.set_error(
                    "Capture unavailable: Windows session is locked, inactive, or unverifiable",
                );
                return;
            }
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            captures_session::dismiss_transient_shell_ui_before_capture();
            std::thread::sleep(Duration::from_millis(120));
            let backend = XcapBackend;
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let result = backend
                .capture_display_at_point(Some((cursor.x, cursor.y)))
                .and_then(|frame| backend.windows().map(|windows| (frame, windows)));
            match result {
                Ok((frame, windows)) => {
                    let d = frame.descriptor.clone();
                    self.state.begin_overlay(mode, frame, windows, recording);
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        d.x,
                        d.y,
                        d.width as i32,
                        d.height as i32,
                        SWP_SHOWWINDOW,
                    );
                    let _ = SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE);
                    let _ = SetForegroundWindow(self.hwnd);
                }
                Err(error) => {
                    self.set_error(error.to_string());
                    self.show_surface(Surface::Menu, 420, 430, false);
                }
            }
        }
    }

    unsafe fn pointer_down(&mut self, p: Point) {
        unsafe {
            match self.state.surface {
                Surface::Menu => match p.y {
                    y if (82.0..146.0).contains(&y) => {
                        self.start_capture(CaptureMode::Region, false)
                    }
                    y if (154.0..218.0).contains(&y) => {
                        self.start_capture(CaptureMode::Window, false)
                    }
                    y if (226.0..290.0).contains(&y) => {
                        self.start_capture(CaptureMode::Display, false)
                    }
                    y if (302.0..366.0).contains(&y) => {
                        self.show_surface(Surface::RecordingSelector, 460, 450, false)
                    }
                    y if y >= 378.0 && p.x < 210.0 => {
                        self.show_surface(Surface::History, 720, 430, false)
                    }
                    y if y >= 378.0 => self.show_surface(Surface::Preferences, 760, 520, false),
                    _ => {}
                },
                Surface::Overlay => {
                    if let Some(overlay) = &mut self.state.overlay
                        && overlay.mode == CaptureMode::Region
                    {
                        overlay.drag = Some(
                            if let Some(rect) = overlay.selection.filter(|r| r.contains(p)) {
                                SelectionDrag::Move {
                                    original: rect,
                                    anchor: p,
                                }
                            } else {
                                SelectionDrag::Create { anchor: p }
                            },
                        );
                        SetCapture(self.hwnd);
                    }
                }
                Surface::RecordingSelector => {
                    if (78.0..140.0).contains(&p.y) {
                        self.recording_mode = CaptureMode::Region;
                    } else if (150.0..212.0).contains(&p.y) {
                        self.recording_mode = CaptureMode::Window;
                    } else if (222.0..284.0).contains(&p.y) {
                        self.recording_mode = CaptureMode::Display;
                    } else if (326.0..370.0).contains(&p.y) {
                        match (p.x / 104.0).floor() as usize {
                            0 => {
                                self.settings.recording.show_cursor =
                                    !self.settings.recording.show_cursor
                            }
                            1 => {
                                self.settings.recording.highlight_clicks =
                                    !self.settings.recording.highlight_clicks
                            }
                            2 => {
                                self.settings.recording.show_keystrokes =
                                    !self.settings.recording.show_keystrokes
                            }
                            3 => {
                                self.settings.recording.capture_system_audio =
                                    !self.settings.recording.capture_system_audio
                            }
                            _ => return,
                        }
                        if let Err(error) = self.settings.save() {
                            self.set_error(error);
                        }
                    } else if p.y > 375.0 {
                        self.start_capture(self.recording_mode, true)
                    }
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
                Surface::Preview => {
                    let p = Point {
                        x: p.x - PAD,
                        y: p.y - PAD,
                    };
                    if self.state.previews.first().is_none_or(|preview| {
                        preview.deletion.is_some() || preview.dismissing.is_some()
                    }) {
                        return;
                    }
                    if p.y > self.height as f32 * 96.0 / self.dpi - PAD * 2.0 - 42.0 {
                        if p.x < 85.0 {
                            if let Some((image, path)) =
                                self.state.previews.first().map(|preview| {
                                    (preview.image.clone(), preview.artifact.path.clone())
                                })
                            {
                                self.open_image_editor(
                                    image,
                                    Some(path.clone()),
                                    DraftIdentity::capture(path),
                                );
                                self.show_surface(Surface::ScreenshotEditor, 1100, 720, false)
                            }
                        } else if p.x < 170.0 {
                            if let Some(preview) = self.state.previews.first()
                                && let Err(error) = copy_image(self.hwnd, &preview.image)
                            {
                                self.set_error(error);
                            }
                        } else if p.x < 250.0 {
                            if let Some(path) = self
                                .state
                                .previews
                                .first()
                                .map(|preview| preview.artifact.path.clone())
                                && let Err(error) = drag_file(&path)
                            {
                                self.set_error(error);
                            }
                        } else if p.x > 250.0
                            && let Some(preview) = self.state.previews.first()
                        {
                            self.state.pending_delete = Some(preview.artifact.clone());
                            self.delete_return = Surface::Preview;
                            self.show_surface(Surface::DeleteConfirmation, 480, 300, false)
                        }
                    }
                }
                Surface::DeleteConfirmation => {
                    let w = self.width as f32 * 96.0 / self.dpi;
                    let h = self.height as f32 * 96.0 / self.dpi;
                    if p.y > h / 2.0 + 30.0 && p.x > w / 2.0 {
                        self.confirm_delete()
                    } else if p.y > h / 2.0 + 30.0 {
                        self.state.pending_delete = None;
                        self.show_delete_return()
                    }
                }
                Surface::RecordingHud => self.hud_click(p),
                Surface::Preferences => self.preferences_click(p),
                Surface::Feedback => self.feedback_click(p),
                Surface::History => self.history_click(p),
                Surface::ScreenshotEditor => {
                    self.editor_pointer_down(p);
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
                Surface::RecordingEditor => self.recording_editor_click(p),
            }
        }
    }

    unsafe fn pointer_move(&mut self, p: Point) {
        unsafe {
            if let Some((start, original)) = self.editor_pan_drag {
                self.state.editor_pan = Point {
                    x: original.x + p.x - start.x,
                    y: original.y + p.y - start.y,
                };
                let _ = InvalidateRect(Some(self.hwnd), None, false);
                return;
            }
            if self.state.surface == Surface::ScreenshotEditor
                && self.editor_remove_stroke.is_some()
                && let Some(source) = self.editor_source_point_unbounded(p)
            {
                if let Some((_, points)) = &mut self.editor_remove_stroke
                    && points
                        .last()
                        .is_none_or(|last| (last.x - source.x).hypot(last.y - source.y) >= 0.5)
                {
                    points.push(source);
                }
                return;
            }
            if self.state.surface == Surface::ScreenshotEditor
                && let Some(source) = self.editor_source_point_unbounded(p)
                && let Some(transform) = &self.editor_transform
                && let Some(layer) = transform.update(source)
                && let Some(document) = self.state.editor.as_mut()
            {
                document.preview_layer(layer);
                let _ = InvalidateRect(Some(self.hwnd), None, false);
                return;
            }
            if self.state.surface == Surface::ScreenshotEditor
                && let Some(source) = self.editor_source_point(p)
                && let Some(gesture) = self.editor_freehand.as_mut()
            {
                gesture.sample(source);
                let _ = InvalidateRect(Some(self.hwnd), None, false);
                return;
            }
            let hovered_window = self.state.hit_window(p);
            if let Some(overlay) = &mut self.state.overlay {
                match overlay.mode {
                    CaptureMode::Region => {
                        if let Some(drag) = overlay.drag {
                            let (w, h) = overlay.frame.descriptor.overlay_size();
                            overlay.selection = Some(update_selection(
                                drag,
                                p,
                                Rect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: w as f32,
                                    height: h as f32,
                                },
                            ));
                        }
                    }
                    CaptureMode::Window => overlay.hovered_window = hovered_window,
                    CaptureMode::Display => {}
                }
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }
    unsafe fn pointer_up(&mut self, _p: Point) {
        unsafe {
            if self.editor_pan_drag.take().is_some() {
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            } else if let Some(overlay) = &mut self.state.overlay {
                overlay.drag = None;
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            } else if self.state.surface == Surface::ScreenshotEditor {
                self.editor_pointer_up(_p);
            }
        }
    }

    unsafe fn preferences_click(&mut self, p: Point) {
        if p.x < 184.0 {
            if p.y >= 430.0 {
                unsafe { self.show_surface(Surface::Feedback, 620, 500, false) };
                return;
            }
            let index = ((p.y - 72.0) / 44.0).floor() as usize;
            if let Some(page) = PreferencesPage::ALL.get(index) {
                self.state.preferences_page = *page;
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            }
            return;
        }
        if p.y < 78.0 {
            return;
        }
        let row = ((p.y - 84.0) / 64.0).floor() as i32;
        match (self.state.preferences_page, row) {
            (PreferencesPage::General, 0) => {
                let enabled = !self.settings.launch_at_login;
                if let Err(error) = set_autostart(enabled) {
                    self.set_error(error);
                    return;
                }
                self.settings.launch_at_login = enabled;
            }
            (PreferencesPage::General, 1) => {
                self.settings.auto_copy_to_clipboard = !self.settings.auto_copy_to_clipboard
            }
            (PreferencesPage::General, 2) => {
                self.settings.show_mini_previews = !self.settings.show_mini_previews
            }
            (PreferencesPage::General, 3) => {
                self.settings.freeze_screen = !self.settings.freeze_screen
            }
            (PreferencesPage::Capture, 0) => {
                self.settings.screenshot_countdown_seconds =
                    match self.settings.screenshot_countdown_seconds {
                        0 => 3,
                        3 => 5,
                        5 => 10,
                        _ => 0,
                    }
            }
            (PreferencesPage::Capture, 1) => {
                self.settings.show_cursor_in_screenshots = !self.settings.show_cursor_in_screenshots
            }
            (PreferencesPage::Capture, 2) => {
                self.settings.freeze_screen = !self.settings.freeze_screen
            }
            (PreferencesPage::Capture, 3) => {
                self.settings.screenshot_format = match self.settings.screenshot_format.as_str() {
                    "png" => "jpeg",
                    "jpeg" => "webp",
                    _ => "png",
                }
                .into()
            }
            (PreferencesPage::Recording, 0) => {
                self.settings.recording.video_format =
                    if self.settings.recording.video_format == "mp4" {
                        "gif"
                    } else {
                        "mp4"
                    }
                    .into()
            }
            (PreferencesPage::Recording, 1) => {
                self.settings.recording.video_fps = match self.settings.recording.video_fps {
                    15 => 30,
                    30 => 60,
                    _ => 15,
                }
            }
            (PreferencesPage::Recording, 2) => {
                self.settings.recording.countdown_seconds =
                    match self.settings.recording.countdown_seconds {
                        0 => 3,
                        3 => 5,
                        5 => 10,
                        _ => 0,
                    }
            }
            (PreferencesPage::Recording, 3) => {
                self.settings.recording.show_cursor = !self.settings.recording.show_cursor
            }
            (PreferencesPage::Recording, 4) => {
                self.settings.recording.capture_system_audio =
                    !self.settings.recording.capture_system_audio
            }
            (PreferencesPage::Recording, 5) => {
                self.settings.recording.open_editor_after_recording =
                    !self.settings.recording.open_editor_after_recording
            }
            (PreferencesPage::Appearance, 0) => {
                self.settings.appearance = match self.settings.appearance.as_str() {
                    "system" => "light",
                    "light" => "dark",
                    _ => "system",
                }
                .into()
            }
            (PreferencesPage::Appearance, 1) => {
                self.settings.theme = match self.settings.theme.as_str() {
                    "mustard" => "cobalt",
                    "cobalt" => "mint",
                    _ => "mustard",
                }
                .into()
            }
            (PreferencesPage::Appearance, 2) => {
                self.settings.include_mini_previews_in_captures =
                    !self.settings.include_mini_previews_in_captures
            }
            (PreferencesPage::Appearance, 3) => {
                self.settings.include_recording_controls_in_captures =
                    !self.settings.include_recording_controls_in_captures
            }
            _ => return,
        }
        if let Err(error) = self.settings.save() {
            self.set_error(error);
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    unsafe fn feedback_click(&mut self, p: Point) {
        if self.state.feedback_submitting {
            return;
        }
        if (92.0..258.0).contains(&p.y) {
            self.state.feedback_field = 0;
            self.state.feedback_caret = self.state.feedback_message.chars().count();
            self.state.feedback_select_all = false;
        } else if (274.0..320.0).contains(&p.y) {
            self.state.feedback_field = 1;
            self.state.feedback_caret = self.state.feedback_contact.chars().count();
            self.state.feedback_select_all = false;
        } else if (338.0..378.0).contains(&p.y) {
            let index = ((p.x - 32.0) / 112.0).floor() as usize;
            if let Some(category) = ["bug", "idea", "other"].get(index) {
                self.state.feedback_category = (*category).into();
            }
        } else if p.y >= 430.0 && p.x < 300.0 {
            unsafe { self.show_surface(Surface::Preferences, 760, 520, false) };
            return;
        } else if p.y >= 430.0 && p.x >= 430.0 {
            self.feedback_request = self.feedback_request.wrapping_add(1);
            let draft = FeedbackDraft {
                message: self.state.feedback_message.clone(),
                contact: Some(self.state.feedback_contact.clone()),
                category: self.state.feedback_category.clone(),
            };
            match self.feedback.submit(self.feedback_request, draft) {
                Ok(()) => self.state.feedback_submitting = true,
                Err(error) => self.set_error(error),
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn process_feedback_events(&mut self) {
        while let Some(event) = self.feedback.try_recv() {
            if event.request != self.feedback_request {
                continue;
            }
            self.state.feedback_submitting = false;
            match event.result {
                Ok(()) => {
                    self.state.feedback_message.clear();
                    self.state.status = Some(("Feedback sent. Thank you.".into(), Instant::now()));
                }
                Err(error) => self.set_error(error),
            }
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    unsafe fn history_click(&mut self, p: Point) {
        if p.y < 104.0 || p.y > 254.0 {
            return;
        }
        let index = ((p.x - 28.0) / 210.0).floor().max(0.0) as usize;
        let Some(artifact) = self.history.entries().get(index).cloned() else {
            return;
        };
        let local_x = p.x - (36.0 + index as f32 * 210.0);
        let button = ((218.0..246.0).contains(&p.y) && local_x >= 0.0)
            .then(|| (local_x / 57.0).floor() as usize);
        match button {
            Some(0) if !artifact.is_trashed() => {
                if artifact.kind == captures_windows_native::history::ArtifactKind::Image {
                    match image::open(&artifact.path) {
                        Ok(image) => {
                            self.open_image_editor(
                                image.to_rgba8(),
                                Some(artifact.path.clone()),
                                DraftIdentity::capture(artifact.path.clone()),
                            );
                            unsafe {
                                self.show_surface(Surface::ScreenshotEditor, 1100, 720, false)
                            };
                        }
                        Err(error) => self.set_error(error.to_string()),
                    }
                } else {
                    self.open_recording_editor(artifact.path);
                    unsafe { self.show_surface(Surface::RecordingEditor, 1000, 680, false) };
                }
            }
            Some(1) if artifact.is_trashed() => {
                let old = artifact.path.clone();
                match restore_from_trash(
                    &artifact,
                    &self.settings.output_directory,
                    &data_dir().join("trash"),
                ) {
                    Ok(restored) => {
                        if let Err(error) = self.history.replace_entry(&old, restored) {
                            self.set_error(error);
                        }
                    }
                    Err(error) => self.set_error(error),
                }
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            }
            Some(2) => {
                self.state.pending_delete = Some(artifact);
                self.delete_return = Surface::History;
                unsafe { self.show_surface(Surface::DeleteConfirmation, 480, 300, false) };
            }
            None if !artifact.is_trashed() => {
                if let Err(error) = drag_file(&artifact.path) {
                    self.set_error(error);
                }
            }
            _ => {}
        }
    }

    unsafe fn editor_pointer_down(&mut self, p: Point) {
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let footer_y = height - 92.0;
        let sidebar_x = width - 320.0;
        let sidebar_layer = (p.x > sidebar_x && (104.0..260.0).contains(&p.y))
            .then(|| ((p.y - 104.0) / 52.0).floor() as usize)
            .filter(|index| *index < 3);
        if self.state.editor_export_settings_open && (footer_y - 132.0..footer_y).contains(&p.y) {
            if p.y < footer_y - 96.0 {
                if self.state.editor_export_size == EditorExportSize::Custom && p.x >= 300.0 {
                    let (field, value) = if p.x < 390.0 {
                        (
                            EditorInputField::ExportWidth,
                            self.state.editor_custom_export_width,
                        )
                    } else if p.x < 480.0 {
                        (
                            EditorInputField::ExportHeight,
                            self.state.editor_custom_export_height,
                        )
                    } else {
                        self.state.editor_export_aspect_locked =
                            !self.state.editor_export_aspect_locked;
                        return;
                    };
                    self.begin_editor_input(field, value.to_string());
                } else {
                    self.state.editor_export_size = match self.state.editor_export_size {
                        EditorExportSize::Original => EditorExportSize::Percent75,
                        EditorExportSize::Percent75 => EditorExportSize::Percent50,
                        EditorExportSize::Percent50 => {
                            if let Some(document) = self.state.editor.as_ref() {
                                self.state.editor_custom_export_width = document.canvas_width;
                                self.state.editor_custom_export_height = document.canvas_height;
                            }
                            EditorExportSize::Custom
                        }
                        EditorExportSize::Custom => EditorExportSize::Original,
                    };
                }
            } else if p.y < footer_y - 60.0 {
                if p.x < 340.0 {
                    self.state.editor_quality_mode = match self.state.editor_quality_mode {
                        EditorQualityMode::Preserve => EditorQualityMode::Compress,
                        EditorQualityMode::Compress => EditorQualityMode::Maximum,
                        EditorQualityMode::Maximum => EditorQualityMode::Preserve,
                    };
                } else if self.state.editor_quality_mode == EditorQualityMode::Compress {
                    self.state.editor_quality = match self.state.editor_quality {
                        0..=55 => 70,
                        56..=70 => 85,
                        71..=85 => 92,
                        86..=92 => 98,
                        _ => 55,
                    };
                } else if self.state.editor_quality_mode == EditorQualityMode::Maximum {
                    self.begin_editor_input(
                        EditorInputField::MaximumKilobytes,
                        self.state.editor_maximum_kilobytes.to_string(),
                    );
                }
            }
        } else if self.state.editor_layer_menu_open {
            let menu_button = Rect {
                x: sidebar_x + 230.0,
                y: 64.0,
                width: 40.0,
                height: 36.0,
            };
            let menu = Rect {
                x: sidebar_x + 18.0,
                y: 102.0,
                width: 284.0,
                height: 148.0,
            };
            if menu_button.contains(p) {
                self.state.editor_layer_menu_open = false;
            } else if menu.contains(p) {
                let action = (0..3).find(|index| {
                    Rect {
                        x: sidebar_x + 30.0,
                        y: 140.0 + *index as f32 * 34.0,
                        width: 260.0,
                        height: 30.0,
                    }
                    .contains(p)
                });
                let result = match action {
                    Some(0) => self.state.selected_layer.map_or(Ok(None), |id| {
                        self.state
                            .editor
                            .as_mut()
                            .map_or(Ok(None), |document| document.merge_layer_down(id))
                    }),
                    Some(1) => self
                        .state
                        .editor
                        .as_mut()
                        .map_or(Ok(None), |document| document.merge_visible_layers()),
                    Some(2) => self.state.editor.as_mut().map_or(Ok(None), |document| {
                        document
                            .flatten_layers()
                            .map(|changed| changed.then_some(0))
                    }),
                    _ => Ok(None),
                };
                match result {
                    Ok(Some(0)) => {
                        self.state.selected_layer = None;
                        self.state.editor_layer_menu_open = false;
                        self.state.status =
                            Some(("Flattened visible layers".into(), Instant::now()));
                        if self.fixture_mode
                            && action == Some(2)
                            && self.state.editor.as_ref().is_some_and(|document| {
                                document.source_present
                                    && document.layers.is_empty()
                                    && document.background.is_none()
                                    && document.crop.x == 0.0
                                    && document.crop.y == 0.0
                            })
                        {
                            let _ = fs::write(
                                data_dir().join("editor-flatten-source-applied.txt"),
                                "flatten:source-only-background",
                            );
                        }
                    }
                    Ok(Some(id)) => {
                        self.state.selected_layer = Some(id);
                        self.state.editor_layer_menu_open = false;
                        self.state.status = Some(("Combined image layers".into(), Instant::now()));
                        if self.fixture_mode
                            && action == Some(0)
                            && self.state.editor.as_ref().is_some_and(|document| {
                                document.layers.len() == 1
                                    && document.layers.iter().any(|layer| {
                                        layer.id == id
                                            && matches!(
                                                layer.shape,
                                                captures_windows_native::editor::Shape::Image { .. }
                                            )
                                    })
                            })
                        {
                            let _ = fs::write(
                                data_dir().join("editor-merge-applied.txt"),
                                "merge-down:one-image-layer",
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(error) => self.set_error(error),
                }
            } else {
                self.state.editor_layer_menu_open = false;
            }
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        } else if p.y >= footer_y {
            if p.x < 208.0 {
                self.state.editor_export_settings_open = !self.state.editor_export_settings_open;
            } else if (216.0..416.0).contains(&p.x) {
                self.state.editor_editing_filename = true;
                self.state.status = Some(("Editing export filename".into(), Instant::now()));
            } else if (416.0..496.0).contains(&p.x) {
                self.state.editor_format = match self.state.editor_format.as_str() {
                    "png" => "jpeg",
                    "jpeg" => "webp",
                    _ => "png",
                }
                .into();
                if !can_replace_editor_source(
                    self.state.editor_source.as_deref(),
                    &self.state.editor_format,
                    &self.state.editor_filename,
                ) {
                    self.state.editor_save_as_new = true;
                }
            } else if (width - 492.0..width - 360.0).contains(&p.x) {
                if let (Some(document), Some(dimensions)) =
                    (self.state.editor.clone(), self.editor_output_dimensions())
                {
                    self.editor_copy_request = self.editor_copy_request.wrapping_add(1);
                    self.state.status = Some(("Preparing copy…".into(), Instant::now()));
                    self.images
                        .copy(self.editor_copy_request, document, dimensions);
                }
            } else if (width - 360.0..width - 180.0).contains(&p.x) {
                if can_replace_editor_source(
                    self.state.editor_source.as_deref(),
                    &self.state.editor_format,
                    &self.state.editor_filename,
                ) {
                    self.state.editor_save_as_new = !self.state.editor_save_as_new;
                } else {
                    self.state.editor_save_as_new = true;
                    self.state.status = Some((
                        "Source format and filename must match before replacing the original"
                            .into(),
                        Instant::now(),
                    ));
                }
            } else if p.x >= width - 176.0 {
                self.save_editor_document();
            }
        } else if p.y < 52.0 {
            if p.x >= width - 214.0 {
                match unsafe { choose_editor_images(self.hwnd) } {
                    Ok(paths) if !paths.is_empty() => {
                        self.media_epoch = self.media_epoch.wrapping_add(1);
                        self.media.import_images(self.media_epoch, paths);
                        self.state.status = Some(("Loading image layers…".into(), Instant::now()));
                    }
                    Ok(_) => {}
                    Err(error) => self.set_error(error),
                }
            } else if (232.0..340.0).contains(&p.x) {
                if let Some(document) = self.state.editor.as_mut() {
                    match document.trim_to_visible_content() {
                        Ok(trimmed) => {
                            self.state.status = Some((
                                if trimmed {
                                    "Trimmed to visible layers"
                                } else {
                                    "Canvas already fits visible layers"
                                }
                                .into(),
                                Instant::now(),
                            ));
                            if self.fixture_mode && trimmed {
                                let _ = fs::write(
                                    data_dir().join("editor-trim-applied.txt"),
                                    format!(
                                        "trim:{}x{}",
                                        document.canvas_width, document.canvas_height
                                    ),
                                );
                            }
                        }
                        Err(error) => self.set_error(error),
                    }
                }
            } else if (78.0..154.0).contains(&p.x) {
                let value = self
                    .state
                    .editor
                    .as_ref()
                    .map_or(1, |document| document.canvas_width);
                self.begin_editor_input(EditorInputField::CanvasWidth, value.to_string());
            } else if (154.0..232.0).contains(&p.x) {
                let value = self
                    .state
                    .editor
                    .as_ref()
                    .map_or(1, |document| document.canvas_height);
                self.begin_editor_input(EditorInputField::CanvasHeight, value.to_string());
            } else if (340.0..368.0).contains(&p.x) {
                let value = self
                    .state
                    .editor
                    .as_ref()
                    .and_then(|document| document.background)
                    .map_or_else(|| "transparent".to_owned(), format_color);
                self.begin_editor_input(EditorInputField::Background, value);
            } else if (368.0..462.0).contains(&p.x) {
                if let Some(document) = self.state.editor.as_mut() {
                    let next = document
                        .background
                        .is_none()
                        .then_some([247, 247, 245, 255]);
                    document.set_background(next);
                }
            } else if (width - 414.0..width - 380.0).contains(&p.x) {
                self.state.editor_zoom_fit = true;
                self.state.editor_pan = Point::default();
            } else if (width - 380.0..width - 344.0).contains(&p.x) {
                self.state.editor_zoom_fit = false;
                self.state.editor_zoom_percent =
                    (self.state.editor_zoom_percent.saturating_mul(4) / 5).max(5);
            } else if (width - 292.0..width - 264.0).contains(&p.x) {
                self.state.editor_zoom_fit = false;
                self.state.editor_zoom_percent =
                    (self.state.editor_zoom_percent.saturating_mul(5) / 4).min(800);
            } else if (width - 498.0..width - 460.0).contains(&p.x) {
                if let Some(document) = self.state.editor.as_mut() {
                    document.undo();
                    self.state.selected_layer = None;
                }
            } else if (width - 460.0..width - 422.0).contains(&p.x)
                && let Some(document) = self.state.editor.as_mut()
            {
                document.redo();
                self.state.selected_layer = None;
            }
        } else if (Rect {
            x: sidebar_x + 230.0,
            y: 64.0,
            width: 40.0,
            height: 36.0,
        })
        .contains(p)
        {
            self.state.editor_layer_menu_open =
                self.state.editor.as_ref().is_some_and(|document| {
                    self.state
                        .selected_layer
                        .is_some_and(|id| document.can_merge_layer_down(id))
                        || document.can_merge_visible_layers()
                        || document.can_flatten_layers()
                });
        } else if p.x < 56.0 && (64.0..400.0).contains(&p.y) {
            let index = ((p.y - 64.0) / 48.0).floor() as usize;
            if index == 3 {
                self.state.editor_shapes_open = !self.state.editor_shapes_open;
                if !matches!(
                    self.state.editor_tool,
                    captures_windows_native::editor::Tool::Line
                        | captures_windows_native::editor::Tool::Rectangle
                        | captures_windows_native::editor::Tool::Ellipse
                        | captures_windows_native::editor::Tool::Triangle
                        | captures_windows_native::editor::Tool::Diamond
                        | captures_windows_native::editor::Tool::Star
                ) {
                    self.state.editor_tool = captures_windows_native::editor::Tool::Rectangle;
                }
            } else if let Some(tool) = [
                captures_windows_native::editor::Tool::Select,
                captures_windows_native::editor::Tool::Crop,
                captures_windows_native::editor::Tool::Text,
                captures_windows_native::editor::Tool::Rectangle,
                captures_windows_native::editor::Tool::Arrow,
                captures_windows_native::editor::Tool::Pen,
                captures_windows_native::editor::Tool::Eraser,
            ]
            .get(index)
            {
                self.state.editor_tool = *tool;
                self.state.editor_shapes_open = false;
            }
        } else if self.state.editor_shapes_open
            && let Some(index) = editor_shape_flyout_index(p)
        {
            if let Some(tool) = [
                captures_windows_native::editor::Tool::Line,
                captures_windows_native::editor::Tool::Rectangle,
                captures_windows_native::editor::Tool::Ellipse,
                captures_windows_native::editor::Tool::Triangle,
                captures_windows_native::editor::Tool::Diamond,
                captures_windows_native::editor::Tool::Star,
            ]
            .get(index)
            {
                self.state.editor_tool = *tool;
                self.state.editor_shapes_open = false;
            }
        } else if self.state.editor.as_ref().is_some_and(|document| {
            let row_y = 104.0 + document.layers.len().min(3) as f32 * 52.0;
            document.source_present && editor_layer_visibility_button(sidebar_x, row_y).contains(p)
        }) {
            if let Some(document) = self.state.editor.as_mut() {
                document.toggle_source_visibility();
                self.state.selected_layer = None;
            }
        } else if p.x > sidebar_x
            && (104.0..260.0).contains(&p.y)
            && let Some(index) = sidebar_layer
            && let Some(id) = self
                .state
                .editor
                .as_ref()
                .and_then(|document| document.layers.iter().rev().nth(index))
                .map(|layer| layer.id)
        {
            let was_selected = self.state.selected_layer == Some(id);
            self.state.selected_layer = Some(id);
            self.state.editor_layer_menu_open = false;
            self.state.editor_tool = captures_windows_native::editor::Tool::Select;
            if let Some(layer) = self
                .state
                .editor
                .as_ref()
                .and_then(|document| document.layers.iter().find(|layer| layer.id == id))
            {
                self.state.editor_color_hex = format_color(layer.color);
            }
            let row_y = 104.0 + index as f32 * 52.0;
            if let Some(document) = self.state.editor.as_mut() {
                if editor_layer_visibility_button(sidebar_x, row_y).contains(p) {
                    document.toggle_visibility(id);
                } else if editor_layer_lock_button(sidebar_x, row_y).contains(p) {
                    document.toggle_locked(id);
                } else if was_selected
                    && (sidebar_x + 52.0..sidebar_x + 220.0).contains(&p.x)
                    && document.layers.iter().any(|layer| {
                        layer.id == id
                            && !layer.locked
                            && matches!(
                                layer.shape,
                                captures_windows_native::editor::Shape::Image { .. }
                            )
                    })
                {
                    let name = document
                        .layers
                        .iter()
                        .find(|layer| layer.id == id)
                        .map(|layer| layer.name.clone())
                        .unwrap_or_default();
                    self.begin_editor_input(EditorInputField::LayerName, name);
                }
            }
        } else if p.x > sidebar_x && {
            let count = self
                .state
                .editor
                .as_ref()
                .map_or(0, |document| document.layers.len().min(3));
            let source_present = self
                .state
                .editor
                .as_ref()
                .is_none_or(|document| document.source_present);
            let properties_y = screenshot_editor_properties_y(height, count, source_present);
            (properties_y - 4.0..properties_y + 228.0).contains(&p.y)
        } {
            let count = self
                .state
                .editor
                .as_ref()
                .map_or(0, |document| document.layers.len().min(3));
            let source_present = self
                .state
                .editor
                .as_ref()
                .is_none_or(|document| document.source_present);
            let properties_y = screenshot_editor_properties_y(height, count, source_present);
            if self.state.editor_tool == captures_windows_native::editor::Tool::Eraser {
                if let Some(mode) = [
                    RemoveBackgroundMode::Wand,
                    RemoveBackgroundMode::Erase,
                    RemoveBackgroundMode::Restore,
                ]
                .into_iter()
                .enumerate()
                .find_map(|(index, mode)| {
                    Rect {
                        x: sidebar_x + 20.0 + index as f32 * 92.0,
                        y: properties_y + 28.0,
                        width: 86.0,
                        height: 32.0,
                    }
                    .contains(p)
                    .then_some(mode)
                }) {
                    self.state.editor_remove_mode = mode;
                } else if (Rect {
                    x: sidebar_x + 184.0,
                    y: properties_y + 74.0,
                    width: 52.0,
                    height: 32.0,
                })
                .contains(p)
                {
                    if self.state.editor_remove_mode == RemoveBackgroundMode::Wand {
                        self.state.editor_wand_tolerance =
                            self.state.editor_wand_tolerance.saturating_sub(8);
                    } else {
                        self.state.editor_remove_brush_size =
                            self.state.editor_remove_brush_size.saturating_sub(4).max(4);
                    }
                } else if (Rect {
                    x: sidebar_x + 240.0,
                    y: properties_y + 74.0,
                    width: 52.0,
                    height: 32.0,
                })
                .contains(p)
                {
                    if self.state.editor_remove_mode == RemoveBackgroundMode::Wand {
                        self.state.editor_wand_tolerance =
                            self.state.editor_wand_tolerance.saturating_add(8).min(120);
                    } else {
                        self.state.editor_remove_brush_size = self
                            .state
                            .editor_remove_brush_size
                            .saturating_add(4)
                            .min(120);
                    }
                } else if self.state.editor_remove_mode == RemoveBackgroundMode::Wand
                    && (Rect {
                        x: sidebar_x + 254.0,
                        y: properties_y + 124.0,
                        width: 30.0,
                        height: 18.0,
                    })
                    .contains(p)
                {
                    self.state.editor_wand_contiguous = !self.state.editor_wand_contiguous;
                } else if self.state.editor_remove_mode != RemoveBackgroundMode::Wand {
                    let adjustment = if (Rect {
                        x: sidebar_x + 184.0,
                        y: properties_y + 114.0,
                        width: 52.0,
                        height: 32.0,
                    })
                    .contains(p)
                    {
                        Some(false)
                    } else if (Rect {
                        x: sidebar_x + 240.0,
                        y: properties_y + 114.0,
                        width: 52.0,
                        height: 32.0,
                    })
                    .contains(p)
                    {
                        Some(true)
                    } else {
                        None
                    };
                    if let Some(increase) = adjustment {
                        self.state.editor_remove_softness = if increase {
                            self.state
                                .editor_remove_softness
                                .saturating_add(10)
                                .min(100)
                        } else {
                            self.state.editor_remove_softness.saturating_sub(10)
                        };
                    }
                }
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
                return;
            }
            if let Some(id) = self.state.selected_layer
                && (properties_y - 4.0..properties_y + 28.0).contains(&p.y)
            {
                if p.x < sidebar_x + 112.0 {
                    if let Some(document) = self.state.editor.as_mut()
                        && let Some(mode) = document
                            .layers
                            .iter()
                            .find(|layer| layer.id == id)
                            .map(|layer| layer.blend_mode)
                    {
                        document.set_layer_blend_mode(id, next_blend_mode(mode));
                    }
                } else {
                    match ((p.x - sidebar_x - 112.0) / 46.0).floor() as usize {
                        0 => {
                            if let Some(document) = self.state.editor.as_mut() {
                                document.move_layer(id, isize::MAX);
                            }
                        }
                        1 => {
                            if let Some(document) = self.state.editor.as_mut() {
                                document.move_layer(id, isize::MIN);
                            }
                        }
                        2 => {
                            if let Some(document) = self.state.editor.as_mut()
                                && let Some(duplicate) = document.duplicate(id)
                            {
                                self.state.selected_layer = Some(duplicate);
                            }
                        }
                        3 => {
                            if self
                                .state
                                .editor
                                .as_mut()
                                .is_some_and(|document| document.delete(id))
                            {
                                self.state.selected_layer = None;
                            }
                        }
                        _ => {}
                    }
                }
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
                return;
            }
            let row = ((p.y - properties_y - 28.0) / 40.0).floor() as usize;
            let selected = self.state.selected_layer;
            let selected_image = selected.is_some_and(|id| {
                self.state.editor.as_ref().is_some_and(|document| {
                    document.layers.iter().any(|layer| {
                        layer.id == id
                            && matches!(
                                layer.shape,
                                captures_windows_native::editor::Shape::Image { .. }
                            )
                    })
                })
            });
            if selected_image {
                let id = selected.unwrap_or_default();
                if (properties_y + 28.0..properties_y + 62.0).contains(&p.y) {
                    if let Some(document) = self.state.editor.as_mut()
                        && let Some(mode) = document
                            .layers
                            .iter()
                            .find(|layer| layer.id == id)
                            .map(|layer| layer.blend_mode)
                    {
                        document.set_layer_blend_mode(id, next_blend_mode(mode));
                    }
                } else if (properties_y + 78.0..properties_y + 122.0).contains(&p.y) {
                    let delta = if p.x < sidebar_x + 236.0 { -26 } else { 26 };
                    if let Some(document) = self.state.editor.as_mut()
                        && let Some(opacity) = document
                            .layers
                            .iter()
                            .find(|layer| layer.id == id)
                            .map(|layer| layer.opacity)
                    {
                        let next = opacity.saturating_add_signed(delta);
                        if document.set_layer_opacity(id, next)
                            && self.fixture_mode
                            && !document.source_present
                            && next == 229
                        {
                            let _ = fs::write(
                                data_dir().join("editor-source-consumed-input.txt"),
                                "opacity:229",
                            );
                        }
                    }
                } else if (properties_y + 122.0..properties_y + 174.0).contains(&p.y) {
                    let delta = if p.x < sidebar_x + 236.0 { -15.0 } else { 15.0 };
                    if let Some(document) = self.state.editor.as_mut()
                        && let Some(rotation) = document
                            .layers
                            .iter()
                            .find(|layer| layer.id == id)
                            .map(|layer| layer.rotation_degrees)
                    {
                        document.set_layer_rotation(id, rotation + delta);
                    }
                }
            } else {
                match row {
                    0 if (sidebar_x + 116.0..sidebar_x + 296.0).contains(&p.x) => {
                        self.state.editor_editing_color = true;
                        if let Some(id) = selected
                            && let Some(layer) = self.state.editor.as_ref().and_then(|document| {
                                document.layers.iter().find(|layer| layer.id == id)
                            })
                        {
                            self.state.editor_color_hex = format_color(layer.color);
                        }
                        self.state.status = Some((
                            "Type a #RRGGBB color and press Enter".into(),
                            Instant::now(),
                        ));
                    }
                    1 => {
                        let delta = if p.x < sidebar_x + 236.0 { -1.0 } else { 1.0 };
                        if let Some(id) = selected {
                            if let Some(document) = self.state.editor.as_mut()
                                && let Some(stroke) = document
                                    .layers
                                    .iter()
                                    .find(|layer| layer.id == id)
                                    .map(|layer| layer.stroke)
                            {
                                document.set_layer_stroke(id, stroke + delta);
                            }
                        } else {
                            self.state.editor_stroke =
                                (self.state.editor_stroke + delta).clamp(1.0, 48.0);
                        }
                    }
                    2 => {
                        if let Some(id) = selected {
                            if let Some(document) = self.state.editor.as_mut()
                                && let Some(layer) =
                                    document.layers.iter().find(|layer| layer.id == id)
                                && layer.supports_fill()
                            {
                                let fill = layer.fill.is_none().then_some(layer.color);
                                document.set_layer_fill(id, fill);
                            }
                        } else {
                            self.state.editor_fill = self
                                .state
                                .editor_fill
                                .is_none()
                                .then_some(self.state.editor_color);
                        }
                    }
                    3 if selected.is_some() => {
                        let id = selected.unwrap_or_default();
                        let delta = if p.x < sidebar_x + 236.0 { -15.0 } else { 15.0 };
                        if let Some(document) = self.state.editor.as_mut()
                            && let Some(rotation) = document
                                .layers
                                .iter()
                                .find(|layer| layer.id == id)
                                .map(|layer| layer.rotation_degrees)
                        {
                            document.set_layer_rotation(id, rotation + delta);
                        }
                    }
                    4 if selected.is_some() => {
                        let id = selected.unwrap_or_default();
                        let delta = if p.x < sidebar_x + 236.0 { -26 } else { 26 };
                        if let Some(document) = self.state.editor.as_mut()
                            && let Some(opacity) = document
                                .layers
                                .iter()
                                .find(|layer| layer.id == id)
                                .map(|layer| layer.opacity)
                        {
                            document.set_layer_opacity(id, opacity.saturating_add_signed(delta));
                        }
                    }
                    _ => {}
                }
            }
        } else if screenshot_editor_canvas(width, height).contains(p)
            && unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
        {
            self.editor_pan_drag = Some((p, self.state.editor_pan));
            unsafe { SetCapture(self.hwnd) };
        } else if self.state.editor_tool == captures_windows_native::editor::Tool::Select
            && let Some(source) = self.editor_source_point_unbounded(p)
            && let Some(transform) = self.editor_transform_at(p, source)
        {
            self.editor_transform = Some(transform);
            unsafe { SetCapture(self.hwnd) };
        } else if let Some(source) = self.editor_source_point(p) {
            if self.state.editor_tool == captures_windows_native::editor::Tool::Select {
                self.state.selected_layer = self
                    .state
                    .editor
                    .as_ref()
                    .and_then(|document| document.hit_test(source, 6.0));
                if let Some(id) = self.state.selected_layer
                    && let Some(layer) =
                        self.state.editor.as_ref().and_then(|document| {
                            document.layers.iter().find(|layer| layer.id == id)
                        })
                {
                    self.state.editor_color_hex = format_color(layer.color);
                }
                if let Some(id) = self.state.selected_layer
                    && let Some(original) = self
                        .state
                        .editor
                        .as_ref()
                        .and_then(|document| document.layers.iter().find(|layer| layer.id == id))
                        .cloned()
                {
                    self.editor_transform = Some(EditorTransform::Move {
                        anchor: source,
                        original,
                    });
                    unsafe { SetCapture(self.hwnd) };
                }
                unsafe {
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            } else if self.state.editor_tool == captures_windows_native::editor::Tool::Text {
                self.editor_text_origin = Some(source);
                self.editor_text.clear();
                self.state.status = Some((
                    "Type annotation text and press Enter".into(),
                    Instant::now(),
                ));
            } else if self.state.editor_tool == captures_windows_native::editor::Tool::Pen {
                self.editor_freehand = Some(FreehandGesture::begin(source));
            } else if self.state.editor_tool == captures_windows_native::editor::Tool::Eraser {
                let target = self
                    .state
                    .editor
                    .as_ref()
                    .and_then(|document| document.hit_test_image(source));
                let Some(target) = target else {
                    self.state.status = Some((
                        "Click an image layer to remove or restore its background".into(),
                        Instant::now(),
                    ));
                    return;
                };
                if self.state.editor_remove_mode == RemoveBackgroundMode::Wand {
                    if let Some(document) = self.state.editor.as_mut() {
                        match document.remove_background_wand(
                            target,
                            source,
                            self.state.editor_wand_tolerance,
                            self.state.editor_wand_contiguous,
                        ) {
                            Ok(true) => {}
                            Ok(false) => {
                                self.state.status = Some((
                                    "No matching pixels found; increase tolerance".into(),
                                    Instant::now(),
                                ))
                            }
                            Err(error) => self.set_error(error),
                        }
                    }
                } else {
                    self.editor_remove_stroke = Some((target, vec![source]));
                    unsafe { SetCapture(self.hwnd) };
                }
            } else {
                self.editor_drag = Some(source);
            }
        }
    }

    unsafe fn editor_pointer_up(&mut self, p: Point) {
        if let Some((target, mut points)) = self.editor_remove_stroke.take() {
            if let Some(end) = self.editor_source_point_unbounded(p) {
                points.push(end);
            }
            if let Some(document) = self.state.editor.as_mut() {
                let result = document.remove_background_stroke(
                    target,
                    &points,
                    self.state.editor_remove_brush_size as f32,
                    self.state.editor_remove_softness as f32,
                    self.state.editor_remove_mode == RemoveBackgroundMode::Restore,
                );
                match result {
                    Ok(true)
                        if self.fixture_mode
                            && self.state.editor_remove_mode == RemoveBackgroundMode::Erase =>
                    {
                        let _ = fs::write(
                            data_dir().join("editor-eraser-applied.txt"),
                            "erase:alpha-changed",
                        );
                    }
                    Ok(_) => {}
                    Err(error) => self.set_error(error),
                }
            }
            unsafe {
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            return;
        }
        if let Some(transform) = self.editor_transform.take() {
            if let Some(source) = self.editor_source_point_unbounded(p)
                && let Some(layer) = transform.update(source)
                && let Some(document) = self.state.editor.as_mut()
            {
                document.preview_layer(layer);
            }
            if let Some(document) = self.state.editor.as_mut() {
                document.commit_layer_preview(transform.original().clone());
            }
            unsafe {
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            return;
        }
        if let Some(gesture) = self.editor_freehand.take() {
            let Some(end) = self.editor_source_point(p) else {
                return;
            };
            if let Some(document) = self.state.editor.as_mut() {
                self.state.selected_layer = Some(document.add(
                    captures_windows_native::editor::Shape::Stroke(gesture.finish(end)),
                    self.state.editor_color,
                    self.state.editor_stroke,
                ));
            }
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            return;
        }
        let Some(start) = self.editor_drag.take() else {
            return;
        };
        let Some(end) = self.editor_source_point(p) else {
            return;
        };
        let Some(document) = &mut self.state.editor else {
            return;
        };
        if self.state.editor_tool == captures_windows_native::editor::Tool::Crop {
            document.set_crop(Rect::from_points(start, end));
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            return;
        }
        let shape = match self.state.editor_tool {
            captures_windows_native::editor::Tool::Arrow => {
                captures_windows_native::editor::Shape::Arrow(start, end)
            }
            captures_windows_native::editor::Tool::Line => {
                captures_windows_native::editor::Shape::Line(start, end)
            }
            captures_windows_native::editor::Tool::Ellipse => {
                captures_windows_native::editor::Shape::Ellipse(Rect::from_points(start, end))
            }
            captures_windows_native::editor::Tool::Triangle => {
                captures_windows_native::editor::Shape::Polygon(polygon_points(
                    Rect::from_points(start, end),
                    3,
                    -90.0,
                ))
            }
            captures_windows_native::editor::Tool::Diamond => {
                captures_windows_native::editor::Shape::Polygon(polygon_points(
                    Rect::from_points(start, end),
                    4,
                    -90.0,
                ))
            }
            captures_windows_native::editor::Tool::Star => {
                let bounds = Rect::from_points(start, end);
                let center = Point {
                    x: bounds.x + bounds.width / 2.0,
                    y: bounds.y + bounds.height / 2.0,
                };
                captures_windows_native::editor::Shape::Polygon(
                    (0..10)
                        .map(|index| {
                            let angle = (-90.0 + index as f32 * 36.0).to_radians();
                            let scale = if index % 2 == 0 { 1.0 } else { 0.42 };
                            Point {
                                x: center.x + angle.cos() * bounds.width / 2.0 * scale,
                                y: center.y + angle.sin() * bounds.height / 2.0 * scale,
                            }
                        })
                        .collect(),
                )
            }
            _ => captures_windows_native::editor::Shape::Rectangle(Rect::from_points(start, end)),
        };
        let id = document.add(shape, self.state.editor_color, self.state.editor_stroke);
        if self.state.editor_fill.is_some()
            && let Some(layer) = document.layers.iter_mut().find(|layer| layer.id == id)
            && layer.supports_fill()
        {
            layer.fill = self.state.editor_fill;
        }
        self.state.selected_layer = Some(id);
        if self.draft_fixture_phase == Some(DraftFixturePhase::Create)
            && document.layers.iter().any(|layer| {
                layer.id == id
                    && matches!(&layer.shape, captures_windows_native::editor::Shape::Polygon(points) if points.len() == 10)
            })
        {
            let _ = fs::write(
                data_dir().join("editor-draft-edited.txt"),
                "edit:polygon-10",
            );
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn editor_source_point(&self, point: Point) -> Option<Point> {
        let document = self.state.editor.as_ref()?;
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let viewport = screenshot_editor_viewport(
            (document.canvas_width, document.canvas_height),
            screenshot_editor_canvas(width, height),
            self.state.editor_zoom_fit,
            self.state.editor_zoom_percent,
            self.state.editor_pan,
        );
        if !viewport.contains(point) {
            return None;
        }
        self.editor_source_point_unbounded(point)
    }

    fn editor_source_point_unbounded(&self, point: Point) -> Option<Point> {
        let document = self.state.editor.as_ref()?;
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let viewport = screenshot_editor_viewport(
            (document.canvas_width, document.canvas_height),
            screenshot_editor_canvas(width, height),
            self.state.editor_zoom_fit,
            self.state.editor_zoom_percent,
            self.state.editor_pan,
        );
        Some(Point {
            x: document.crop.x
                + (point.x - viewport.x) / viewport.width * document.canvas_width as f32,
            y: document.crop.y
                + (point.y - viewport.y) / viewport.height * document.canvas_height as f32,
        })
    }

    fn editor_screen_point(&self, point: Point) -> Option<Point> {
        let document = self.state.editor.as_ref()?;
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let viewport = screenshot_editor_viewport(
            (document.canvas_width, document.canvas_height),
            screenshot_editor_canvas(width, height),
            self.state.editor_zoom_fit,
            self.state.editor_zoom_percent,
            self.state.editor_pan,
        );
        Some(Point {
            x: viewport.x
                + (point.x - document.crop.x) / document.canvas_width as f32 * viewport.width,
            y: viewport.y
                + (point.y - document.crop.y) / document.canvas_height as f32 * viewport.height,
        })
    }

    fn editor_transform_at(&self, screen: Point, source: Point) -> Option<EditorTransform> {
        let id = self.state.selected_layer?;
        let original = self
            .state
            .editor
            .as_ref()?
            .layers
            .iter()
            .find(|layer| layer.id == id)?
            .clone();
        if original.locked {
            return None;
        }
        let corners = original.selection_corners()?;
        let screen_corners =
            corners.map(|point| self.editor_screen_point(point).unwrap_or_default());
        if let Some(corner) = original
            .resize_handles()
            .into_iter()
            .find_map(|(handle, point)| {
                let point = self.editor_screen_point(point)?;
                ((point.x - screen.x).hypot(point.y - screen.y) <= 9.0).then_some(handle)
            })
        {
            return Some(EditorTransform::Resize { corner, original });
        }
        let top = Point {
            x: (screen_corners[0].x + screen_corners[1].x) / 2.0,
            y: (screen_corners[0].y + screen_corners[1].y) / 2.0,
        };
        let center_screen = Point {
            x: screen_corners.iter().map(|point| point.x).sum::<f32>() / 4.0,
            y: screen_corners.iter().map(|point| point.y).sum::<f32>() / 4.0,
        };
        let mut direction = Point {
            x: top.x - center_screen.x,
            y: top.y - center_screen.y,
        };
        let mut length = direction.x.hypot(direction.y);
        if length < 1.0 {
            let axis = Point {
                x: screen_corners[2].x - screen_corners[0].x,
                y: screen_corners[2].y - screen_corners[0].y,
            };
            length = axis.x.hypot(axis.y).max(1.0);
            direction = Point {
                x: axis.y,
                y: -axis.x,
            };
        }
        let rotation_screen = Point {
            x: top.x + direction.x / length * 28.0,
            y: top.y + direction.y / length * 28.0,
        };
        if (rotation_screen.x - screen.x).hypot(rotation_screen.y - screen.y) <= 10.0 {
            let bounds = original.geometry_bounds()?;
            let center = Point {
                x: bounds.x + bounds.width / 2.0,
                y: bounds.y + bounds.height / 2.0,
            };
            let pointer_angle = (source.y - center.y)
                .atan2(source.x - center.x)
                .to_degrees();
            let pointer_offset = pointer_angle - original.rotation_degrees;
            return Some(EditorTransform::Rotate {
                center,
                pointer_offset,
                original,
            });
        }
        None
    }

    unsafe fn recording_editor_click(&mut self, point: Point) {
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let Some(editor) = self.state.recording_editor.as_mut() else {
            return;
        };
        let mut rebuild_comparison = false;
        let mut playback_changed = false;
        if point.y > height - 88.0 && point.x > width - 190.0 {
            self.export_recording_editor();
            return;
        }
        if editor.quality_menu_open && point.x > width - 236.0 && (320.0..500.0).contains(&point.y)
        {
            let index = ((point.y - 320.0) / 30.0).floor() as usize;
            if let Some(quality) = [
                QualityPreset::Preserve,
                QualityPreset::Highest,
                QualityPreset::High,
                QualityPreset::Standard,
                QualityPreset::Small,
                QualityPreset::Tiny,
            ]
            .get(index)
            {
                editor.quality = *quality;
                editor.quality_menu_open = false;
                rebuild_comparison = true;
            }
        } else if point.x > width - 236.0 && (280.0..330.0).contains(&point.y) {
            editor.quality_menu_open = !editor.quality_menu_open;
        } else if point.y > height - 88.0 && (width - 390.0..width - 190.0).contains(&point.x) {
            editor.save_as_new = !editor.save_as_new;
        } else if (height - 196.0..height - 100.0).contains(&point.y) {
            let track = recording_editor_timeline_track(width, height);
            let track_start = track.x;
            let track_width = track.width;
            let ratio = ((point.x - track_start) / track_width).clamp(0.0, 1.0);
            let at = (ratio * editor.duration_ms as f32).round() as u64;
            let start_x =
                track_start + editor.trim_start_ms as f32 / editor.duration_ms as f32 * track_width;
            let end_x =
                track_start + editor.trim_end_ms as f32 / editor.duration_ms as f32 * track_width;
            if (point.x - start_x).abs() < 16.0 {
                editor.set_trim_start(at);
            } else if (point.x - end_x).abs() < 16.0 {
                editor.set_trim_end(at);
            } else if point.x < 80.0 {
                editor.toggle_playback(Instant::now());
                playback_changed = true;
            } else {
                editor.seek(at);
                playback_changed = true;
            }
        }
        if playback_changed {
            self.restart_playback_worker();
        }
        if rebuild_comparison && let Err(error) = self.build_compression_comparison() {
            self.set_error(error);
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn open_recording_editor(&mut self, source: PathBuf) {
        self.media.cancel_jobs();
        self.media_epoch = self.media_epoch.wrapping_add(1);
        self.frame_request = 0;
        self.comparison_request = 0;
        self.export_request = 0;
        self.state.recording_editor = None;
        self.state.recording_preview = None;
        self.state.status = Some(("Opening recording…".into(), Instant::now()));
        self.media.probe(self.media_epoch, source);
    }

    fn editor_output_dimensions(&self) -> Option<(u32, u32)> {
        let document = self.state.editor.as_ref()?;
        Some(self.state.editor_export_size.dimensions(
            document,
            (
                self.state.editor_custom_export_width,
                self.state.editor_custom_export_height,
            ),
        ))
    }

    fn editor_encode_spec(&self) -> Option<ImageEncodeSpec> {
        let (width, height) = self.editor_output_dimensions()?;
        Some(ImageEncodeSpec {
            format: self.state.editor_format.clone(),
            quality_mode: self.state.editor_quality_mode,
            quality: self.state.editor_quality,
            maximum_bytes: self.state.editor_maximum_kilobytes.saturating_mul(1_000),
            width,
            height,
        })
    }

    fn tick_image_editor(&mut self) {
        if self.state.surface != Surface::ScreenshotEditor {
            return;
        }
        let Some(document) = self.state.editor.as_ref() else {
            return;
        };
        let render_key = document.render_key();
        if render_key != self.editor_render_requested && !self.editor_render_inflight {
            self.editor_render_requested = render_key;
            self.editor_render_inflight = true;
            self.images.render_preview(render_key, document.clone());
        }
        if !self.state.editor_export_settings_open {
            self.editor_estimate_key = None;
            self.state.editor_estimate_pending = false;
            return;
        }
        let Some(spec) = self.editor_encode_spec() else {
            return;
        };
        let key = (
            render_key.0,
            render_key.1,
            spec.format.clone(),
            self.state.editor_export_size,
            spec.quality_mode,
            spec.quality,
            spec.maximum_bytes,
            spec.width,
            spec.height,
        );
        if self.editor_estimate_key.as_ref() != Some(&key) {
            self.editor_estimate_key = Some(key);
            self.editor_estimate_request = self.editor_estimate_request.wrapping_add(1);
            self.state.editor_estimate_pending = true;
            self.state.editor_estimated_bytes = None;
            self.images.estimate(
                render_key,
                self.editor_estimate_request,
                document.clone(),
                spec,
            );
        }
    }

    fn tick_editor_draft(&mut self, now: Instant) {
        if self.state.surface != Surface::ScreenshotEditor {
            return;
        }
        let Some(document) = self.state.editor.as_ref() else {
            return;
        };
        let key = document.render_key();
        let Some(session) = self.editor_draft.as_mut() else {
            return;
        };
        if session.observed_key != key {
            session.observed_key = key;
            session.changed_at = Some(now);
        }
        if session.in_flight.is_none()
            && key != session.persisted_key
            && session
                .changed_at
                .is_some_and(|changed| now.duration_since(changed) >= Duration::from_millis(700))
        {
            match self.drafts.save(
                session.identity.clone(),
                self.state.editor_source.clone(),
                document.clone(),
            ) {
                Ok(()) => session.in_flight = Some(key),
                Err(error) => self.set_error(error),
            }
        }
    }

    fn process_draft_events(&mut self) {
        while let Some(event) = self.drafts.try_recv() {
            let Some(session) = self
                .editor_draft
                .as_mut()
                .filter(|session| session.identity == event.identity)
            else {
                continue;
            };
            if session.in_flight == Some(event.document_key) {
                session.in_flight = None;
            }
            match event.result {
                Ok(()) => {
                    session.persisted_key = event.document_key;
                    if session.observed_key == event.document_key {
                        session.changed_at = None;
                    }
                }
                Err(error) => self.set_error(format!("Could not save screenshot draft: {error}")),
            }
        }
    }

    fn flush_editor_draft(&mut self) {
        let Some(document) = self.state.editor.clone() else {
            return;
        };
        let Some(session) = self.editor_draft.as_mut() else {
            return;
        };
        let key = document.render_key();
        if key == session.persisted_key {
            return;
        }
        match self.drafts.save_and_wait(
            session.identity.clone(),
            self.state.editor_source.clone(),
            document,
        ) {
            Ok(()) => {
                session.persisted_key = key;
                session.observed_key = key;
                session.changed_at = None;
                session.in_flight = None;
            }
            Err(error) => self.set_error(format!("Could not save screenshot draft: {error}")),
        }
    }

    fn open_image_editor(
        &mut self,
        image: RgbaImage,
        source: Option<PathBuf>,
        identity: DraftIdentity,
    ) {
        self.flush_editor_draft();
        let dimensions = image.dimensions();
        let restored = DraftStore::new(&data_dir()).load(&identity, source.as_deref());
        let (document, did_restore) = match restored {
            Ok(Some(document)) => (document, true),
            Ok(None) => (captures_windows_native::editor::Document::new(image), false),
            Err(error) => {
                self.set_error(format!("Could not restore screenshot draft: {error}"));
                (captures_windows_native::editor::Document::new(image), false)
            }
        };
        self.state.edit_document_from(document, source, dimensions);
        let key = self
            .state
            .editor
            .as_ref()
            .expect("editor document installed")
            .render_key();
        self.editor_draft = Some(DraftSession {
            identity,
            persisted_key: key,
            observed_key: key,
            changed_at: None,
            in_flight: None,
        });
        if did_restore {
            self.state.status = Some(("Restored unsaved screenshot edits".into(), Instant::now()));
        }
        if self.draft_fixture_phase == Some(DraftFixturePhase::Restore) {
            let marker = self.verify_restored_draft_fixture(did_restore);
            let _ = fs::write(data_dir().join("editor-draft-restored.txt"), marker);
        }
    }

    fn verify_restored_draft_fixture(&mut self, did_restore: bool) -> &'static str {
        if !did_restore {
            return "restore:missing";
        }
        let Some(document) = self.state.editor.as_mut() else {
            return "restore:no-document";
        };
        let Some(layer) = document.layers.first() else {
            return "restore:no-layer";
        };
        if document.layers.len() != 1
            || !matches!(&layer.shape, captures_windows_native::editor::Shape::Polygon(points) if points.len() == 10)
        {
            return "restore:wrong-layers";
        }
        let id = layer.id;
        let opacity = layer.opacity;
        if !document.set_layer_opacity(id, opacity.saturating_sub(1)) || !document.undo() {
            return "restore:not-editable";
        }
        if document
            .layers
            .first()
            .is_none_or(|layer| layer.id != id || layer.opacity != opacity)
        {
            return "restore:undo-failed";
        }
        "restore:polygon-10:editable-undo"
    }

    fn reset_editor_draft_after_save(&mut self, destination: &Path, completion: SaveCompletion) {
        let Some(previous) = self.editor_draft.take() else {
            return;
        };
        let identity = if previous.identity.matches_source_path(destination) {
            previous.identity.clone()
        } else {
            DraftIdentity::capture(destination.to_path_buf())
        };
        let document = self.state.editor.clone();
        let key = document
            .as_ref()
            .map_or((0, 0), |document| document.render_key());
        let persisted = if completion == SaveCompletion::NewerRevision {
            document.is_some_and(|document| {
                match self.drafts.preserve_newer_export(
                    previous.identity.clone(),
                    identity.clone(),
                    destination.to_path_buf(),
                    document,
                ) {
                    Ok(()) => true,
                    Err(error) => {
                        self.set_error(format!(
                            "Could not preserve newer screenshot edits: {error}"
                        ));
                        false
                    }
                }
            })
        } else {
            false
        };
        let discard_previous = completion == SaveCompletion::ExportedRevision;
        if discard_previous && let Err(error) = self.drafts.discard_and_wait(previous.identity) {
            self.set_error(format!("Could not clear screenshot draft: {error}"));
        }
        while self.drafts.try_recv().is_some() {}
        self.editor_draft = Some(DraftSession {
            identity,
            persisted_key: if persisted || completion == SaveCompletion::ExportedRevision {
                key
            } else {
                (0, 0)
            },
            observed_key: key,
            changed_at: (!persisted && completion == SaveCompletion::NewerRevision)
                .then(Instant::now),
            in_flight: None,
        });
    }

    fn process_image_events(&mut self) {
        while let Some(event) = self.images.try_recv() {
            match event {
                ImageEvent::Preview { key, result } => {
                    self.editor_render_inflight = false;
                    if self
                        .state
                        .editor
                        .as_ref()
                        .is_some_and(|document| document.render_key() == key)
                    {
                        match result {
                            Ok(image) => self.state.editor_preview = Some(image),
                            Err(error) => self.set_error(error),
                        }
                        unsafe {
                            let _ = InvalidateRect(Some(self.hwnd), None, false);
                        }
                    }
                }
                ImageEvent::Estimate {
                    key,
                    request,
                    result,
                } if request == self.editor_estimate_request
                    && self
                        .state
                        .editor
                        .as_ref()
                        .is_some_and(|document| document.render_key() == key) =>
                {
                    self.state.editor_estimate_pending = false;
                    match result {
                        Ok(bytes) => self.state.editor_estimated_bytes = Some(bytes),
                        Err(error) => {
                            self.state.editor_estimated_bytes = None;
                            self.set_error(error);
                        }
                    }
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
                ImageEvent::Copy {
                    document_id,
                    request,
                    result,
                } if accepts_document_request(
                    self.state
                        .editor
                        .as_ref()
                        .map(|document| document.render_key()),
                    self.editor_copy_request,
                    document_id,
                    request,
                ) =>
                {
                    match result.and_then(|image| copy_image(self.hwnd, &image)) {
                        Ok(()) => {
                            self.state.status = Some(("Copied edited image".into(), Instant::now()))
                        }
                        Err(error) => self.set_error(error),
                    }
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
                ImageEvent::Save {
                    document_key,
                    request,
                    destination,
                    dimensions,
                    result,
                } => {
                    self.editor_saves.finish(request);
                    let completion = classify_save_completion(
                        self.state
                            .editor
                            .as_ref()
                            .map(|document| document.render_key()),
                        self.editor_save_request,
                        document_key,
                        request,
                    );
                    let current_document = completion != SaveCompletion::Stale;
                    match result {
                        Ok(()) => {
                            let artifact = Artifact::from_path(
                                destination.clone(),
                                dimensions.0,
                                dimensions.1,
                            );
                            let history_result = self.history.add(artifact);
                            if current_document {
                                if let Err(error) = history_result {
                                    self.set_error(error);
                                } else {
                                    self.state.editor_source = Some(destination.clone());
                                    self.state.editor_save_as_new = false;
                                    self.state.status = Some((
                                        format!("Saved {}", destination.display()),
                                        Instant::now(),
                                    ));
                                    self.reset_editor_draft_after_save(&destination, completion);
                                }
                            }
                        }
                        Err(error) if current_document => self.set_error(error),
                        Err(_) => {}
                    }
                    if current_document {
                        unsafe {
                            let _ = InvalidateRect(Some(self.hwnd), None, false);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn tick_recording_editor(&mut self, now: Instant) {
        let changed = self
            .state
            .recording_editor
            .as_mut()
            .is_some_and(|editor| editor.tick(now));
        if changed {
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    fn restart_playback_worker(&mut self) {
        let Some(editor) = self.state.recording_editor.as_ref() else {
            return;
        };
        self.frame_request = self.frame_request.wrapping_add(1);
        if editor.playing {
            self.media.play(PlaybackSpec {
                epoch: self.media_epoch,
                request: self.frame_request,
                source: editor.source.clone(),
                start_ms: editor.position_ms,
                end_ms: editor.trim_end_ms,
                width: editor.width,
                height: editor.height,
            });
        } else {
            self.media.cancel_playback();
            self.media.still(
                self.media_epoch,
                self.frame_request,
                editor.source.clone(),
                editor.position_ms.min(editor.duration_ms.saturating_sub(1)),
            );
        }
    }

    fn process_media_events(&mut self) {
        while let Some(event) = self.media.try_recv() {
            match event {
                MediaEvent::Images { epoch, result }
                    if epoch == self.media_epoch
                        && self.state.surface == Surface::ScreenshotEditor =>
                {
                    match result {
                        Ok(images) => {
                            let mut selected = None;
                            if let Some(document) = self.state.editor.as_mut() {
                                let images = images
                                    .into_iter()
                                    .map(|(path, image)| {
                                        let name = path
                                            .file_name()
                                            .and_then(|value| value.to_str())
                                            .unwrap_or("Image")
                                            .to_owned();
                                        (image, name)
                                    })
                                    .collect();
                                selected = document.add_images(images, 0).last().copied();
                            }
                            self.state.selected_layer = selected;
                            self.state.editor_tool = captures_windows_native::editor::Tool::Select;
                            self.state.status = Some(("Added image layers".into(), Instant::now()));
                        }
                        Err(error) => self.set_error(format!("Image could not be added: {error}")),
                    }
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
                MediaEvent::Probe {
                    epoch,
                    source,
                    result,
                } if epoch == self.media_epoch => match result {
                    Ok(probe) => {
                        let result = RecordingEditorState::new(
                            source,
                            probe.metadata.duration_ms.unwrap_or(0),
                            probe.metadata.width,
                            probe.metadata.height,
                            probe.has_audio,
                        );
                        match result {
                            Ok(mut editor) => {
                                if self.fixture_mode {
                                    editor.seek(editor.duration_ms / 3);
                                }
                                self.state.recording_editor = Some(editor);
                                self.state.status = None;
                                self.restart_playback_worker();
                            }
                            Err(error) => self.set_error(error),
                        }
                    }
                    Err(error) => self.set_error(error),
                },
                MediaEvent::Frame {
                    epoch,
                    request,
                    result,
                } if captures_windows_native::async_state::accepts(
                    self.media_epoch,
                    self.frame_request,
                    epoch,
                    request,
                ) =>
                {
                    match result {
                        Ok(image) => {
                            self.state.recording_preview = Some(image);
                            if self.comparison_request == 0 {
                                let _ = self.build_compression_comparison();
                            }
                            unsafe {
                                let _ = InvalidateRect(Some(self.hwnd), None, false);
                            }
                        }
                        Err(error) => self.set_error(error),
                    }
                }
                MediaEvent::Comparison {
                    epoch,
                    request,
                    result,
                } if epoch == self.media_epoch && request == self.comparison_request => {
                    match result {
                        Ok((image, estimated)) => {
                            self.state.recording_preview = Some(image);
                            if let Some(editor) = self.state.recording_editor.as_mut() {
                                editor.comparison_estimated_bytes = Some(estimated);
                            }
                            self.state.status = None;
                        }
                        Err(error) if error.contains("cancelled") => {}
                        Err(error) => self.set_error(error),
                    }
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                }
                MediaEvent::Export {
                    epoch,
                    request,
                    source,
                    destination,
                    save_as_new,
                    result,
                } if epoch == self.media_epoch && request == self.export_request => {
                    self.finish_editor_export(source, destination, save_as_new, result);
                }
                MediaEvent::Export { destination, .. } => {
                    let _ = fs::remove_file(destination);
                }
                _ => {}
            }
        }
    }

    fn export_recording_editor(&mut self) {
        self.media.cancel_playback();
        if let Some(editor) = self.state.recording_editor.as_mut() {
            editor.playing = false;
        }
        let Some(editor) = self.state.recording_editor.clone() else {
            return;
        };
        let extension = editor
            .source
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("mp4")
            .to_ascii_lowercase();
        let format = if extension == "gif" {
            ExportFormat::Gif
        } else {
            ExportFormat::Mp4
        };
        let destination = unique_path(
            &self.settings.output_directory,
            if editor.save_as_new {
                "Recording edit"
            } else {
                ".Captures replacement"
            },
            &extension,
        );
        self.export_request = self.export_request.wrapping_add(1);
        self.state.status = Some(("Exporting recording…".into(), Instant::now()));
        self.media.export(ExportJob {
            epoch: self.media_epoch,
            request: self.export_request,
            source: editor.source,
            destination,
            save_as_new: editor.save_as_new,
            edit: EditSpec {
                trim_start_ms: editor.trim_start_ms,
                trim_end_ms: Some(editor.trim_end_ms),
                audio: AudioEdit {
                    source_has_system_audio: editor.has_audio,
                    ..Default::default()
                },
                ..Default::default()
            },
            export: ExportSpec {
                format,
                quality: editor.quality,
                max_size_bytes: None,
                frames_per_second: Some(if format == ExportFormat::Gif {
                    self.settings.recording.gif_fps
                } else {
                    self.settings.recording.video_fps
                }),
                gif_max_colors: (format == ExportFormat::Gif)
                    .then_some(self.settings.recording.gif_max_colors),
            },
        });
    }

    fn build_compression_comparison(&mut self) -> Result<(), String> {
        self.media.cancel_playback();
        if let Some(editor) = self.state.recording_editor.as_mut() {
            editor.playing = false;
        }
        let editor = self
            .state
            .recording_editor
            .clone()
            .ok_or("recording editor is unavailable")?;
        let before = self
            .state
            .recording_preview
            .clone()
            .ok_or("original comparison frame is unavailable")?;
        if let Some(editor) = self.state.recording_editor.as_mut() {
            editor.comparison_estimated_bytes = None;
        }
        self.comparison_request = self.comparison_request.wrapping_add(1);
        self.state.status = Some(("Building compression comparison…".into(), Instant::now()));
        let gif = editor
            .source
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("gif"));
        self.media.compare(ComparisonSpec {
            epoch: self.media_epoch,
            request: self.comparison_request,
            source: editor.source,
            before,
            position_ms: editor.position_ms,
            trim_start_ms: editor.trim_start_ms,
            trim_end_ms: editor.trim_end_ms,
            quality: editor.quality,
            has_audio: editor.has_audio,
            frames_per_second: if gif {
                self.settings.recording.gif_fps
            } else {
                self.settings.recording.video_fps
            },
            gif_max_colors: self.settings.recording.gif_max_colors,
        });
        Ok(())
    }

    fn finish_editor_export(
        &mut self,
        source: PathBuf,
        destination: PathBuf,
        save_as_new: bool,
        result: Result<captures_media::ExportOutcome, String>,
    ) {
        let result = result.and_then(|outcome| {
            if save_as_new {
                let editor = self
                    .state
                    .recording_editor
                    .as_ref()
                    .ok_or("recording editor closed during export")?;
                self.history.add(Artifact::from_path(
                    outcome.path,
                    editor.width,
                    editor.height,
                ))?;
                Ok("Exported as a new recording".to_owned())
            } else {
                replace_file_safely(&destination, &source)?;
                Ok("Updated the original recording".to_owned())
            }
        });
        match result {
            Ok(message) => self.state.status = Some((message, Instant::now())),
            Err(error) if error.contains("cancelled") => {
                let _ = fs::remove_file(destination);
            }
            Err(error) => {
                let _ = fs::remove_file(destination);
                self.set_error(error);
            }
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
    unsafe fn key(&mut self, key: u32) {
        unsafe {
            let editor = self.state.surface == Surface::ScreenshotEditor;
            let control = GetKeyState(VK_CONTROL.0 as i32) < 0;
            if self.state.surface == Surface::Feedback {
                if self.state.feedback_submitting {
                    return;
                }
                let length = if self.state.feedback_field == 0 {
                    self.state.feedback_message.chars().count()
                } else {
                    self.state.feedback_contact.chars().count()
                };
                match key {
                    0x41 if control => self.state.feedback_select_all = true,
                    0x56 if control => {
                        self.paste_feedback();
                        return;
                    }
                    value if value == VK_ESCAPE.0 as u32 => {
                        self.show_surface(Surface::Preferences, 760, 520, false);
                        return;
                    }
                    value if value == VK_RETURN.0 as u32 && self.state.feedback_field == 0 => {
                        replace_feedback_selection(
                            &mut self.state.feedback_message,
                            &mut self.state.feedback_caret,
                            &mut self.state.feedback_select_all,
                            "\n",
                            8_000,
                        );
                    }
                    value if value == VK_LEFT.0 as u32 => {
                        self.state.feedback_caret = self.state.feedback_caret.saturating_sub(1);
                        self.state.feedback_select_all = false;
                    }
                    value if value == VK_RIGHT.0 as u32 => {
                        self.state.feedback_caret = (self.state.feedback_caret + 1).min(length);
                        self.state.feedback_select_all = false;
                    }
                    value if value == VK_HOME.0 as u32 => {
                        self.state.feedback_caret = 0;
                        self.state.feedback_select_all = false;
                    }
                    value if value == VK_END.0 as u32 => {
                        self.state.feedback_caret = length;
                        self.state.feedback_select_all = false;
                    }
                    value if value == VK_DELETE.0 as u32 => {
                        let field = if self.state.feedback_field == 0 {
                            &mut self.state.feedback_message
                        } else {
                            &mut self.state.feedback_contact
                        };
                        delete_feedback(
                            field,
                            &mut self.state.feedback_caret,
                            &mut self.state.feedback_select_all,
                        );
                    }
                    _ => {}
                }
                let _ = InvalidateRect(Some(self.hwnd), None, false);
                return;
            }
            if editor && key == VK_RETURN.0 as u32 && self.state.editor_input_field.is_some() {
                self.commit_editor_input();
            } else if editor && key == VK_RETURN.0 as u32 && self.editor_text_origin.is_some() {
                self.commit_editor_text();
            } else if editor && key == VK_RETURN.0 as u32 && self.state.editor_editing_filename {
                self.state.editor_filename = sanitize_editor_filename(&self.state.editor_filename);
                self.state.editor_editing_filename = false;
                if !can_replace_editor_source(
                    self.state.editor_source.as_deref(),
                    &self.state.editor_format,
                    &self.state.editor_filename,
                ) {
                    self.state.editor_save_as_new = true;
                }
            } else if editor && key == VK_RETURN.0 as u32 && self.state.editor_editing_color {
                match parse_hex_color(&self.state.editor_color_hex) {
                    Some(color) => {
                        if let Some(id) = self.state.selected_layer {
                            if let Some(document) = self.state.editor.as_mut() {
                                document.set_layer_color(id, color);
                            }
                        } else {
                            self.state.editor_color = color;
                            if self.state.editor_fill.is_some() {
                                self.state.editor_fill = Some(color);
                            }
                        }
                        self.state.editor_editing_color = false;
                    }
                    None => self.set_error("Color must be exactly #RRGGBB"),
                }
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            } else if editor && key == VK_DELETE.0 as u32 {
                if let Some(id) = self.state.selected_layer.take()
                    && let Some(document) = self.state.editor.as_mut()
                {
                    document.delete(id);
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            } else if editor && control && key == 0x44 {
                if let Some(id) = self.state.selected_layer
                    && let Some(document) = self.state.editor.as_mut()
                    && let Some(duplicate) = document.duplicate(id)
                {
                    self.state.selected_layer = Some(duplicate);
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            } else if editor && control && key == 0x5a {
                if let Some(document) = self.state.editor.as_mut() {
                    document.undo();
                    self.state.selected_layer = None;
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            } else if editor && control && key == 0x59 {
                if let Some(document) = self.state.editor.as_mut() {
                    document.redo();
                    self.state.selected_layer = None;
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                }
            } else if editor && control && key == 0x53 {
                self.save_editor_document();
            } else if key == VK_ESCAPE.0 as u32 {
                if self.state.editor_shapes_open {
                    self.state.editor_shapes_open = false;
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                } else if self.state.editor_input_field.take().is_some() {
                    self.state.editor_input_text.clear();
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                } else if self.editor_text_origin.take().is_some()
                    || self.state.editor_editing_color
                    || self.state.editor_editing_filename
                {
                    self.editor_text.clear();
                    self.state.editor_editing_color = false;
                    self.state.editor_editing_filename = false;
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                } else if self.state.surface == Surface::Overlay {
                    self.state.cancel_overlay();
                    self.show_surface(Surface::Menu, 420, 430, false)
                } else if self.state.surface == Surface::Preview {
                    if let Some(preview) = self.state.previews.first_mut()
                        && preview.deletion.is_none()
                        && preview.dismissing.is_none()
                    {
                        preview.dismissing = Some(Instant::now());
                    }
                } else if self.state.surface == Surface::RecordingEditor {
                    self.media.cancel_jobs();
                    self.media_epoch = self.media_epoch.wrapping_add(1);
                    if let Some(editor) = self.state.recording_editor.as_mut() {
                        editor.playing = false;
                    }
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                } else {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
            } else if key == VK_RETURN.0 as u32 && self.state.surface == Surface::Overlay {
                self.finish_overlay()
            } else {
                match key {
                    0x52 => self.start_capture(CaptureMode::Region, false),
                    0x57 => self.start_capture(CaptureMode::Window, false),
                    0x44 => self.start_capture(CaptureMode::Display, false),
                    _ => {}
                }
            }
        }
    }

    fn character_utf16(&mut self, unit: u16) {
        if self.state.surface == Surface::Feedback && self.state.feedback_submitting {
            self.feedback_high_surrogate = None;
            return;
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.feedback_high_surrogate = Some(unit);
            return;
        }
        if let Some(high) = self.feedback_high_surrogate.take() {
            if (0xdc00..=0xdfff).contains(&unit) {
                if let Some(character) =
                    char::decode_utf16([high, unit]).next().and_then(Result::ok)
                {
                    self.character(character);
                }
                return;
            }
            self.character(char::REPLACEMENT_CHARACTER);
        }
        self.character(char::from_u32(u32::from(unit)).unwrap_or(char::REPLACEMENT_CHARACTER));
    }

    fn character(&mut self, character: char) {
        if self.state.surface == Surface::Feedback {
            if self.state.feedback_submitting {
                return;
            }
            let field = if self.state.feedback_field == 0 {
                &mut self.state.feedback_message
            } else {
                &mut self.state.feedback_contact
            };
            let maximum = if self.state.feedback_field == 0 {
                8_000
            } else {
                200
            };
            match character {
                '\u{8}' => backspace_feedback(
                    field,
                    &mut self.state.feedback_caret,
                    &mut self.state.feedback_select_all,
                ),
                value if !value.is_control() => replace_feedback_selection(
                    field,
                    &mut self.state.feedback_caret,
                    &mut self.state.feedback_select_all,
                    &value.to_string(),
                    maximum,
                ),
                _ => {}
            }
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
            return;
        }
        if self.state.surface != Surface::ScreenshotEditor {
            return;
        }
        if self.state.editor_input_field.is_some() {
            let maximum = if self.state.editor_input_field == Some(EditorInputField::LayerName) {
                80
            } else {
                16
            };
            match character {
                '\u{8}' => {
                    if self.editor_input_select_all {
                        self.state.editor_input_text.clear();
                    } else {
                        self.state.editor_input_text.pop();
                    }
                    self.editor_input_select_all = false;
                }
                value
                    if !value.is_control()
                        && (self.editor_input_select_all
                            || self.state.editor_input_text.chars().count() < maximum)
                        && (self.state.editor_input_field == Some(EditorInputField::LayerName)
                            || value.is_ascii_hexdigit()
                            || (self.state.editor_input_field
                                == Some(EditorInputField::Background)
                                && value.is_ascii_alphabetic())
                            || matches!(value, '#' | '.')) =>
                {
                    if self.editor_input_select_all {
                        self.state.editor_input_text.clear();
                    }
                    self.state.editor_input_text.push(value);
                    self.editor_input_select_all = false;
                }
                _ => {}
            }
        } else if self.state.editor_editing_color {
            match character {
                '\u{8}' => {
                    self.state.editor_color_hex.pop();
                }
                '#' | '0'..='9' | 'a'..='f' | 'A'..='F'
                    if self.state.editor_color_hex.len() < 7 =>
                {
                    self.state
                        .editor_color_hex
                        .push(character.to_ascii_lowercase());
                }
                _ => {}
            }
        } else if self.state.editor_editing_filename {
            match character {
                '\u{8}' => {
                    self.state.editor_filename.pop();
                }
                value
                    if !value.is_control() && self.state.editor_filename.chars().count() < 120 =>
                {
                    self.state.editor_filename.push(value)
                }
                _ => {}
            }
            if !can_replace_editor_source(
                self.state.editor_source.as_deref(),
                &self.state.editor_format,
                &self.state.editor_filename,
            ) {
                self.state.editor_save_as_new = true;
            }
        } else if self.editor_text_origin.is_some() {
            match character {
                '\u{8}' => {
                    self.editor_text.pop();
                }
                '\r' | '\n' => {}
                value if !value.is_control() && self.editor_text.chars().count() < 240 => {
                    self.editor_text.push(value)
                }
                _ => {}
            }
            self.state.status = Some((format!("Text: {}", self.editor_text), Instant::now()));
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn paste_feedback(&mut self) {
        if self.state.surface != Surface::Feedback || self.state.feedback_submitting {
            return;
        }
        let result = unsafe {
            match OpenClipboard(Some(self.hwnd)) {
                Err(error) => Err(error.to_string()),
                Ok(()) => {
                    let result = (|| {
                        let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0))
                            .map_err(|error| error.to_string())?;
                        let global = windows::Win32::Foundation::HGLOBAL(handle.0);
                        let bytes = GlobalSize(global);
                        if bytes < 2 {
                            return Err("Clipboard text is unavailable.".into());
                        }
                        let pointer = GlobalLock(global).cast::<u16>();
                        if pointer.is_null() {
                            return Err("Clipboard text is unavailable.".into());
                        }
                        let words = std::slice::from_raw_parts(pointer, bytes / 2);
                        let length = words
                            .iter()
                            .position(|word| *word == 0)
                            .unwrap_or(words.len());
                        let text = String::from_utf16_lossy(&words[..length]);
                        let _ = GlobalUnlock(global);
                        Ok(text)
                    })();
                    let _ = CloseClipboard();
                    result
                }
            }
        };
        match result {
            Ok(mut text) => {
                let maximum = if self.state.feedback_field == 0 {
                    8_000
                } else {
                    text = text.replace(['\r', '\n'], " ");
                    200
                };
                let field = if self.state.feedback_field == 0 {
                    &mut self.state.feedback_message
                } else {
                    &mut self.state.feedback_contact
                };
                replace_feedback_selection(
                    field,
                    &mut self.state.feedback_caret,
                    &mut self.state.feedback_select_all,
                    &text,
                    maximum,
                );
            }
            Err(error) => self.set_error(error),
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn begin_editor_input(&mut self, field: EditorInputField, value: String) {
        self.state.editor_input_field = Some(field);
        self.state.editor_input_text = value;
        self.editor_input_select_all = true;
        self.state.status = Some(("Type a value and press Enter".into(), Instant::now()));
    }

    fn commit_editor_input(&mut self) {
        let Some(field) = self.state.editor_input_field.take() else {
            return;
        };
        let text = std::mem::take(&mut self.state.editor_input_text);
        let result = match field {
            EditorInputField::CanvasWidth | EditorInputField::CanvasHeight => text
                .parse::<u32>()
                .map_err(|_| "Canvas dimensions must be whole pixels".to_owned())
                .and_then(|value| {
                    let document = self.state.editor.as_mut().ok_or("Editor is unavailable")?;
                    let dimensions = if field == EditorInputField::CanvasWidth {
                        (value, document.canvas_height)
                    } else {
                        (document.canvas_width, value)
                    };
                    document
                        .set_canvas_size(dimensions.0, dimensions.1)
                        .map(|_| ())
                        .map_err(str::to_owned)
                }),
            EditorInputField::Background => {
                let color = if text.trim().eq_ignore_ascii_case("transparent") {
                    Some(None)
                } else {
                    parse_hex_color(text.trim()).map(Some)
                };
                color
                    .ok_or_else(|| "Background must be transparent or #RRGGBB".to_owned())
                    .and_then(|color| {
                        self.state
                            .editor
                            .as_mut()
                            .ok_or_else(|| "Editor is unavailable".to_owned())?
                            .set_background(color);
                        Ok(())
                    })
            }
            EditorInputField::ExportWidth | EditorInputField::ExportHeight => text
                .parse::<u32>()
                .map_err(|_| "Output dimensions must be whole pixels".to_owned())
                .and_then(|value| {
                    if !(1..=16_384).contains(&value) {
                        return Err("Output dimensions must be between 1 and 16384".into());
                    }
                    let document = self.state.editor.as_ref().ok_or("Editor is unavailable")?;
                    if field == EditorInputField::ExportWidth {
                        self.state.editor_custom_export_width = value;
                        if self.state.editor_export_aspect_locked {
                            self.state.editor_custom_export_height =
                                ((u64::from(value) * u64::from(document.canvas_height)
                                    + u64::from(document.canvas_width) / 2)
                                    / u64::from(document.canvas_width))
                                .clamp(1, 16_384) as u32;
                        }
                    } else {
                        self.state.editor_custom_export_height = value;
                        if self.state.editor_export_aspect_locked {
                            self.state.editor_custom_export_width =
                                ((u64::from(value) * u64::from(document.canvas_width)
                                    + u64::from(document.canvas_height) / 2)
                                    / u64::from(document.canvas_height))
                                .clamp(1, 16_384) as u32;
                        }
                    }
                    self.state.editor_export_size = EditorExportSize::Custom;
                    Ok(())
                }),
            EditorInputField::MaximumKilobytes => text
                .parse::<u64>()
                .map_err(|_| "Maximum file size must be a whole number of KB".to_owned())
                .and_then(|value| {
                    if value < 10 {
                        Err("Maximum file size must be at least 10 KB".into())
                    } else {
                        self.state.editor_maximum_kilobytes = value;
                        Ok(())
                    }
                }),
            EditorInputField::LayerName => {
                if let Some(id) = self.state.selected_layer {
                    if self
                        .state
                        .editor
                        .as_mut()
                        .is_some_and(|document| document.rename_layer(id, text))
                    {
                        Ok(())
                    } else {
                        Err("Layer name cannot be empty or the layer is locked".into())
                    }
                } else {
                    Err("Select a layer to rename".into())
                }
            }
        };
        if let Err(error) = result {
            self.set_error(error);
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn commit_editor_text(&mut self) {
        let Some(origin) = self.editor_text_origin.take() else {
            return;
        };
        if self.editor_text.trim().is_empty() {
            return;
        }
        let font = std::env::var_os("WINDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
            .join("Fonts")
            .join("segoeui.ttf");
        let bytes = match fs::read(&font) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.set_error(format!(
                    "Could not load Segoe UI for text annotation: {error}"
                ));
                return;
            }
        };
        if let Some(document) = self.state.editor.as_mut() {
            self.state.selected_layer = Some(document.add(
                captures_windows_native::editor::Shape::Text {
                    origin,
                    value: std::mem::take(&mut self.editor_text),
                    font_size: 32.0,
                    font_data: bytes.into(),
                },
                self.state.editor_color,
                1.0,
            ));
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    unsafe fn finish_overlay(&mut self) {
        unsafe {
            if !captures_session::capture_session_available() {
                self.state.cancel_overlay();
                self.set_error("Capture cancelled: desktop session became unavailable");
                self.show_surface(Surface::Menu, 420, 430, false);
                return;
            }
            let Some(overlay) = self.state.overlay.take() else {
                return;
            };
            if overlay.recording {
                let target = match overlay.mode {
                    CaptureMode::Display => RecordingTarget::Display {
                        display_id: overlay.frame.descriptor.id.clone(),
                    },
                    CaptureMode::Region => {
                        let Some(rect) = overlay.selection else {
                            self.state.overlay = Some(overlay);
                            return;
                        };
                        RecordingTarget::Region {
                            display_id: overlay.frame.descriptor.id.clone(),
                            rect: captures_recording::CaptureRect {
                                x: rect.x.round() as i32,
                                y: rect.y.round() as i32,
                                width: rect.width.round() as u32,
                                height: rect.height.round() as u32,
                            },
                        }
                    }
                    CaptureMode::Window => {
                        let Some(index) = overlay.hovered_window else {
                            self.state.overlay = Some(overlay);
                            return;
                        };
                        RecordingTarget::Window {
                            window_id: overlay.windows[index].id.clone(),
                        }
                    }
                };
                self.start_recording(target, overlay.frame.descriptor);
                return;
            }
            let image = match overlay.mode {
                CaptureMode::Display => overlay.frame.image,
                CaptureMode::Region => {
                    let Some(rect) = overlay.selection else {
                        self.state.overlay = Some(overlay);
                        return;
                    };
                    let physical = rect.to_logical().to_physical(
                        overlay.frame.descriptor.overlay_to_buffer_scale(
                            overlay.frame.image.width(),
                            overlay.frame.image.height(),
                        ),
                        overlay.frame.image.width(),
                        overlay.frame.image.height(),
                    );
                    let Some(image) = overlay.frame.crop(physical) else {
                        return;
                    };
                    image
                }
                CaptureMode::Window => {
                    let Some(index) = overlay.hovered_window else {
                        return;
                    };
                    match XcapBackend.capture_window(&overlay.windows[index].id) {
                        Ok(image) => image,
                        Err(error) => {
                            self.set_error(error.to_string());
                            self.show_surface(Surface::Menu, 420, 430, false);
                            return;
                        }
                    }
                }
            };
            match self.save_image(&image) {
                Ok(artifact) => {
                    let _ = copy_image(self.hwnd, &image);
                    let _ = self.history.add(artifact.clone());
                    self.state.add_preview(artifact, image, Instant::now());
                    self.show_preview()
                }
                Err(error) => {
                    self.set_error(error);
                    self.show_surface(Surface::Menu, 420, 430, false)
                }
            }
        }
    }

    fn save_image(&self, image: &RgbaImage) -> Result<Artifact, String> {
        let extension = &self.settings.screenshot_format;
        let path = unique_path(&self.settings.output_directory, "Capture", extension);
        write_image_file(&path, image, extension)?;
        Ok(Artifact::from_path(path, image.width(), image.height()))
    }

    fn save_editor_document(&mut self) {
        let Some(document) = self.state.editor.clone() else {
            return;
        };
        let Some(spec) = self.editor_encode_spec() else {
            return;
        };
        if self.state.editor_quality_mode == EditorQualityMode::Maximum
            && self.state.editor_maximum_kilobytes < 10
        {
            self.set_error("Maximum file size must be at least 10 KB");
            return;
        }
        let extension = self.state.editor_format.clone();
        let source = self.state.editor_source.clone();
        let replace = !self.state.editor_save_as_new
            && can_replace_editor_source(
                source.as_deref(),
                &extension,
                &self.state.editor_filename,
            );
        let result: Result<(PathBuf, Option<PathBuf>), String> = (|| {
            let (path, replace_source) = if replace {
                let original = source.as_ref().expect("source checked above");
                let staged = unique_path(
                    original
                        .parent()
                        .ok_or_else(|| "source has no parent directory".to_owned())?,
                    ".Captures image edit",
                    &extension,
                );
                (staged, Some(original.clone()))
            } else {
                let stem = sanitize_editor_filename(&self.state.editor_filename);
                let path = available_named_path(&self.settings.output_directory, &stem, &extension);
                (path, None)
            };
            Ok((path, replace_source))
        })();
        match result {
            Ok((destination, replace_source)) => {
                let request = self.editor_save_request.wrapping_add(1);
                let document_id = document.render_key().0;
                let final_destination = replace_source.as_ref().unwrap_or(&destination);
                if !self
                    .editor_saves
                    .try_start(request, document_id, final_destination)
                {
                    self.state.status = Some((
                        "A save for this document or destination is already in progress".into(),
                        Instant::now(),
                    ));
                    unsafe {
                        let _ = InvalidateRect(Some(self.hwnd), None, false);
                    }
                    return;
                }
                self.editor_save_request = request;
                self.state.status = Some(("Saving edited image…".into(), Instant::now()));
                self.images
                    .save(request, document, spec, destination, replace_source);
            }
            Err(error) => self.set_error(error),
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
}

fn write_image_file(path: &Path, image: &RgbaImage, extension: &str) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    let bytes = match extension {
        "jpeg" => captures_windows_native::encoder::encode_jpeg(image, 92),
        "webp" => captures_windows_native::encoder::encode_webp(image, None),
        _ => captures_windows_native::encoder::encode_png(image, None),
    };
    let result = bytes.and_then(|bytes| {
        let mut writer = BufWriter::new(file);
        std::io::Write::write_all(&mut writer, &bytes).map_err(|e| e.to_string())?;
        std::io::Write::flush(&mut writer).map_err(|e| e.to_string())?;
        writer.get_ref().sync_all().map_err(|e| e.to_string())
    });
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

unsafe fn choose_editor_images(owner: HWND) -> Result<Vec<PathBuf>, String> {
    let dialog: IFileOpenDialog = unsafe {
        CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?
    };
    let options = unsafe { dialog.GetOptions() }.map_err(|error| error.to_string())?;
    let filters = [COMDLG_FILTERSPEC {
        pszName: w!("Images"),
        pszSpec: w!("*.png;*.jpg;*.jpeg;*.webp;*.gif"),
    }];
    unsafe { dialog.SetFileTypes(&filters) }.map_err(|error| error.to_string())?;
    unsafe {
        dialog.SetOptions(options | FOS_ALLOWMULTISELECT | FOS_FILEMUSTEXIST | FOS_FORCEFILESYSTEM)
    }
    .map_err(|error| error.to_string())?;
    if let Err(error) = unsafe { dialog.Show(Some(owner)) } {
        return if error.code().0 as u32 == 0x8007_04c7 {
            Ok(Vec::new())
        } else {
            Err(error.to_string())
        };
    }
    let items = unsafe { dialog.GetResults() }.map_err(|error| error.to_string())?;
    let count = unsafe { items.GetCount() }.map_err(|error| error.to_string())?;
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let item = unsafe { items.GetItemAt(index) }.map_err(|error| error.to_string())?;
        let value =
            unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(|error| error.to_string())?;
        let path = unsafe { value.to_string() }.map_err(|error| error.to_string());
        unsafe { CoTaskMemFree(Some(value.0.cast())) };
        paths.push(PathBuf::from(path?));
    }
    Ok(paths)
}

fn available_named_path(root: &Path, stem: &str, extension: &str) -> PathBuf {
    let first = root.join(format!("{stem}.{extension}"));
    if !first.exists() {
        return first;
    }
    for suffix in 2..1000 {
        let path = root.join(format!("{stem}-{suffix}.{extension}"));
        if !path.exists() {
            return path;
        }
    }
    root.join(format!("{stem}-{}.{}", uuid::Uuid::new_v4(), extension))
}

impl App {
    unsafe fn show_preview(&mut self) {
        unsafe {
            let work = work_area();
            let logical_height =
                200 + self.state.previews.len().saturating_sub(1).min(4) as i32 * 28;
            let x = work.right - scale(368 + PAD as i32, self.dpi);
            let y = work.bottom - scale(logical_height + 28 + PAD as i32, self.dpi);
            self.show_surface(
                Surface::Preview,
                (MEDIA_WIDTH + PAD * 2.0) as i32,
                logical_height + PAD as i32 * 2,
                true,
            );
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                scale((MEDIA_WIDTH + PAD * 2.0) as i32, self.dpi),
                scale(logical_height + PAD as i32 * 2, self.dpi),
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
            self.update_preview_region();
            let _ = SetWindowDisplayAffinity(
                self.hwnd,
                if self.fixture_mode || self.settings.include_mini_previews_in_captures {
                    WDA_NONE
                } else {
                    WDA_EXCLUDEFROMCAPTURE
                },
            );
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn preview_contains(&self, point: Point) -> bool {
        let count = self.state.previews.len().min(5);
        (0..count).any(|index| {
            if self.state.previews[index].deletion.is_some()
                || self.state.previews[index].dismissing.is_some()
            {
                return false;
            }
            let y = PAD + (count - 1 - index) as f32 * 28.0;
            rounded_contains(
                point,
                Rect {
                    x: PAD,
                    y,
                    width: MEDIA_WIDTH,
                    height: 200.0,
                },
                14.0,
            )
        })
    }

    unsafe fn update_preview_region(&self) {
        unsafe {
            let count = self.state.previews.len().min(5);
            let diameter = scale(28, self.dpi);
            let combined = CreateRectRgn(0, 0, 0, 0);
            if combined.is_invalid() {
                return;
            }
            for (index, preview) in self.state.previews.iter().take(count).enumerate() {
                let y = PAD + (count - 1 - index) as f32 * 28.0;
                if let Some(deletion) = &preview.deletion {
                    let elapsed = deletion.elapsed_ms(Instant::now());
                    for particle in &deletion.particles {
                        let pose = particle.visual(elapsed);
                        if pose.opacity <= 0.0 {
                            continue;
                        }
                        let angle = pose.rotate.to_radians();
                        let a = deletion.atlas.cell_width as f64 / 2.0;
                        let b = deletion.atlas.cell_height as f64 / 2.0;
                        let rx = ((a * angle.cos().abs() + b * angle.sin().abs()) * pose.scale
                            + 2.0) as f32;
                        let ry = ((a * angle.sin().abs() + b * angle.cos().abs()) * pose.scale
                            + 2.0) as f32;
                        let cx =
                            PAD + (particle.source_left + particle.width / 2.0 + pose.dx) as f32;
                        let cy = y + (particle.source_top + particle.height / 2.0 + pose.dy) as f32;
                        let fragment = CreateRectRgn(
                            scale((cx - rx).floor() as i32, self.dpi),
                            scale((cy - ry).floor() as i32, self.dpi),
                            scale((cx + rx).ceil() as i32, self.dpi),
                            scale((cy + ry).ceil() as i32, self.dpi),
                        );
                        if !fragment.is_invalid() {
                            let _ =
                                CombineRgn(Some(combined), Some(combined), Some(fragment), RGN_OR);
                            let _ = DeleteObject(HGDIOBJ(fragment.0));
                        }
                    }
                    continue;
                }
                let y = scale(y as i32, self.dpi);
                let card = CreateRoundRectRgn(
                    scale(PAD as i32, self.dpi),
                    y,
                    scale((PAD + MEDIA_WIDTH) as i32, self.dpi) + 1,
                    y + scale(200, self.dpi) + 1,
                    diameter,
                    diameter,
                );
                if !card.is_invalid() {
                    let _ = CombineRgn(Some(combined), Some(combined), Some(card), RGN_OR);
                    let _ = DeleteObject(HGDIOBJ(card.0));
                }
            }
            if SetWindowRgn(self.hwnd, Some(combined), true) == 0 {
                let _ = DeleteObject(HGDIOBJ(combined.0));
            }
        }
    }

    unsafe fn start_recording(&mut self, target: RecordingTarget, display: DisplayDescriptor) {
        unsafe {
            if !captures_session::capture_session_available() {
                self.set_error("Recording unavailable: desktop session cannot be verified");
                return;
            }
            let r = &self.settings.recording;
            let kind = if r.video_format == "gif" {
                RecordingKind::Gif
            } else {
                RecordingKind::Video
            };
            let options = RecordingOptions {
                kind,
                target,
                frames_per_second: if kind == RecordingKind::Gif {
                    r.gif_fps
                } else {
                    r.video_fps
                },
                max_resolution: r.video_max_resolution,
                countdown_seconds: 0,
                show_cursor: r.show_cursor,
                highlight_clicks: r.highlight_clicks,
                show_keystrokes: r.show_keystrokes,
                audio: AudioOptions {
                    capture_system_audio: r.capture_system_audio,
                    microphone_device_id: r.microphone_device_id.clone(),
                    mono_output: r.mono_audio,
                    ..Default::default()
                },
                gif: GifOptions {
                    max_width: r.gif_max_width,
                    max_colors: r.gif_max_colors,
                    optimize: true,
                },
            };
            let directory = data_dir()
                .join("recording-drafts")
                .join(uuid::Uuid::new_v4().to_string());
            let _ = fs::create_dir_all(&directory);
            let path = directory.join("segment-000.mp4");
            match XcapRecordingSegment::start(&options, &path, &display) {
                Ok(segment) => {
                    self.recording = Some(RecordingSession {
                        options: options.clone(),
                        display,
                        active: Some(segment),
                        segment_paths: vec![path],
                        completed: vec![],
                        directory,
                    });
                    self.state.recording = Some(RecordingUi {
                        state: RecordingState::Recording,
                        kind,
                        started: Instant::now(),
                        elapsed_before_pause: Duration::ZERO,
                        muted: false,
                        hidden: false,
                        warning: None,
                        source: None,
                    });
                    self.state.surface = Surface::RecordingHud;
                    let work = work_area();
                    let _ = SetWindowPos(
                        self.hwnd,
                        Some(HWND_TOPMOST),
                        work.left + (work.right - work.left - scale(500, self.dpi)) / 2,
                        work.bottom - scale(112, self.dpi),
                        scale(500, self.dpi),
                        scale(82, self.dpi),
                        SWP_SHOWWINDOW,
                    );
                    let _ = SetWindowDisplayAffinity(
                        self.hwnd,
                        if self.settings.include_recording_controls_in_captures {
                            WDA_NONE
                        } else {
                            WDA_EXCLUDEFROMCAPTURE
                        },
                    );
                }
                Err(error) => {
                    self.set_error(error.to_string());
                    self.show_surface(Surface::Menu, 420, 430, false)
                }
            }
        }
    }

    unsafe fn hud_click(&mut self, p: Point) {
        unsafe {
            if p.y < 20.0 {
                return;
            }
            let index = ((p.x - 126.0) / 52.0).floor() as i32;
            match index {
                0 => self.stop_recording(false),
                1 => self.toggle_pause(),
                2 => self.restart_recording(),
                5 => self.stop_recording(true),
                6 => {
                    let _ = ShowWindow(self.hwnd, SW_HIDE);
                }
                _ => {}
            }
        }
    }
    unsafe fn toggle_pause(&mut self) {
        unsafe {
            let Some(session) = &mut self.recording else {
                return;
            };
            let Some(ui) = &mut self.state.recording else {
                return;
            };
            if ui.state == RecordingState::Recording {
                if let Some(active) = session.active.take() {
                    match active.stop() {
                        Ok(info) => {
                            session.completed.push(info);
                            let _ = ui.transition(RecordingState::Paused, true);
                        }
                        Err(error) => ui.warning = Some(error.to_string()),
                    }
                }
            } else if ui.state == RecordingState::Paused {
                if !captures_session::capture_session_available() {
                    ui.warning =
                        Some("Desktop session unavailable; recording remains paused".into());
                    return;
                }
                let path = session
                    .directory
                    .join(format!("segment-{:03}.mp4", session.completed.len()));
                match XcapRecordingSegment::start(&session.options, &path, &session.display) {
                    Ok(active) => {
                        session.segment_paths.push(path);
                        session.active = Some(active);
                        let _ = ui.transition(RecordingState::Recording, true);
                    }
                    Err(error) => ui.warning = Some(error.to_string()),
                }
            }
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
    unsafe fn restart_recording(&mut self) {
        unsafe {
            let Some(mut session) = self.recording.take() else {
                return;
            };
            if let Some(active) = session.active.take() {
                let _ = active.discard();
            }
            let _ = fs::remove_dir_all(&session.directory);
            self.state.recording = None;
            self.start_recording(session.options.target, session.display);
        }
    }
    unsafe fn stop_recording(&mut self, discard: bool) {
        unsafe {
            let Some(mut session) = self.recording.take() else {
                return;
            };
            if let Some(active) = session.active.take() {
                if discard {
                    let _ = active.discard();
                } else if let Ok(info) = active.stop() {
                    session.completed.push(info)
                }
            }
            if discard {
                let _ = fs::remove_dir_all(session.directory);
                self.state.recording = None;
                self.show_surface(Surface::Menu, 420, 430, false);
                return;
            }
            let output = unique_path(&self.settings.output_directory, "Recording", "mp4");
            let result = publish_segments(&session.completed, &output);
            match result {
                Ok(()) => {
                    let info = session.completed.first();
                    let artifact = Artifact::from_path(
                        output,
                        info.map_or(0, |i| i.width),
                        info.map_or(0, |i| i.height),
                    );
                    let _ = self.history.add(artifact.clone());
                    if self.settings.recording.open_editor_after_recording {
                        self.open_recording_editor(artifact.path);
                        self.state.surface = Surface::RecordingEditor;
                        self.show_surface(Surface::RecordingEditor, 1000, 680, false)
                    } else {
                        self.show_surface(Surface::Menu, 420, 430, false)
                    }
                }
                Err(error) => {
                    self.set_error(error);
                    self.show_surface(Surface::Menu, 420, 430, false)
                }
            }
            self.state.recording = None;
        }
    }

    unsafe fn confirm_delete(&mut self) {
        unsafe {
            if let Some(artifact) = self.state.pending_delete.take() {
                let old = artifact.path.clone();
                let result = if artifact.is_trashed() {
                    safe_delete(&old, &data_dir().join("trash")).map(|()| None)
                } else {
                    move_to_trash(
                        &artifact,
                        &self.settings.output_directory,
                        &data_dir().join("trash"),
                    )
                    .map(Some)
                };
                match result {
                    Ok(Some(trashed)) => {
                        retire_capture_session(&mut self.editor_draft, &old);
                        let _ = self
                            .drafts
                            .discard_and_wait(DraftIdentity::capture(old.clone()));
                        let _ = self.history.replace_entry(&old, trashed);
                        let mut enabled = BOOL(1);
                        let _ = SystemParametersInfoW(
                            SPI_GETCLIENTAREAANIMATION,
                            0,
                            Some((&mut enabled as *mut BOOL).cast()),
                            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
                        );
                        if let Some(preview) = self
                            .state
                            .previews
                            .iter_mut()
                            .find(|preview| preview.artifact.path == old)
                        {
                            preview.start_deletion(
                                Instant::now(),
                                !self.fixture_mode && !enabled.as_bool(),
                            );
                        }
                        self.show_delete_return()
                    }
                    Ok(None) => {
                        retire_capture_session(&mut self.editor_draft, &old);
                        let _ = self
                            .drafts
                            .discard_and_wait(DraftIdentity::capture(old.clone()));
                        let _ = self.history.remove_entry(&old);
                        self.show_delete_return()
                    }
                    Err(error) => {
                        self.set_error(error);
                        self.show_delete_return()
                    }
                }
            }
        }
    }

    unsafe fn show_delete_return(&mut self) {
        unsafe {
            match self.delete_return {
                Surface::Preview if !self.state.previews.is_empty() => self.show_preview(),
                Surface::History => self.show_surface(Surface::History, 720, 430, false),
                _ => self.show_surface(Surface::Menu, 420, 430, false),
            }
        }
    }
    fn set_error(&mut self, error: impl Into<String>) {
        self.state.status = Some((error.into(), Instant::now()));
    }
    unsafe fn add_tray(&mut self) -> Result<(), String> {
        unsafe {
            let mut tray = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
                uCallbackMessage: WM_TRAY,
                hIcon: LoadIconW(None, IDI_APPLICATION).map_err(win_error)?,
                ..Default::default()
            };
            let tip = "Captures\0".encode_utf16().collect::<Vec<_>>();
            tray.szTip[..tip.len()].copy_from_slice(&tip);
            if !Shell_NotifyIconW(NIM_ADD, &tray).as_bool() {
                return Err(last_error("add tray icon"));
            }
            self.tray = tray;
            Ok(())
        }
    }
    unsafe fn remove_tray(&mut self) {
        unsafe {
            if self.tray.cbSize != 0 {
                let _ = Shell_NotifyIconW(NIM_DELETE, &self.tray);
                self.tray.cbSize = 0;
            }
        }
    }
}

fn publish_segments(segments: &[RecordingSegmentInfo], output: &Path) -> Result<(), String> {
    if segments.is_empty() {
        return Err("Recording produced no completed segment".into());
    }
    if segments.len() == 1 {
        fs::hard_link(&segments[0].path, output)
            .or_else(|_| fs::copy(&segments[0].path, output).map(|_| ()))
            .map_err(|e| e.to_string())
    } else {
        let inputs = segments
            .iter()
            .map(|s| captures_media::RecordingSegmentInput {
                video_path: s.path.clone(),
                system_audio_path: s.system_audio_path.clone(),
                system_audio_offset_ms: s.system_audio_offset_ms,
                microphone_path: s.microphone_path.clone(),
                microphone_offset_ms: s.microphone_offset_ms,
                duration_ms: s.duration_ms,
            })
            .collect::<Vec<_>>();
        captures_media::MediaToolchain::from_command_names()
            .assemble_recording_segments(
                &inputs,
                output,
                captures_media::RecordingAudioLayout {
                    system_audio: segments.iter().any(|s| s.system_audio_path.is_some()),
                    microphone_audio: segments.iter().any(|s| s.microphone_path.is_some()),
                },
                &captures_media::CancelToken::default(),
            )
            .map_err(|e| e.to_string())
    }
}

fn copy_image(hwnd: HWND, image: &RgbaImage) -> Result<(), String> {
    unsafe {
        let header = 124usize;
        let stride = (image.width() as usize * 4 + 3) & !3;
        let size = header + stride * image.height() as usize;
        let memory = GlobalAlloc(GMEM_MOVEABLE, size).map_err(win_error)?;
        let pointer = GlobalLock(memory);
        if pointer.is_null() {
            return Err(last_error("lock clipboard image"));
        }
        let bytes = std::slice::from_raw_parts_mut(pointer.cast::<u8>(), size);
        bytes.fill(0);
        bytes[0..4].copy_from_slice(&(124u32).to_le_bytes());
        bytes[4..8].copy_from_slice(&(image.width() as i32).to_le_bytes());
        bytes[8..12].copy_from_slice(&(-(image.height() as i32)).to_le_bytes());
        bytes[12..14].copy_from_slice(&(1u16).to_le_bytes());
        bytes[14..16].copy_from_slice(&(32u16).to_le_bytes());
        bytes[16..20].copy_from_slice(&(3u32).to_le_bytes());
        bytes[20..24].copy_from_slice(&((stride * image.height() as usize) as u32).to_le_bytes());
        bytes[40..44].copy_from_slice(&0x00ff0000u32.to_le_bytes());
        bytes[44..48].copy_from_slice(&0x0000ff00u32.to_le_bytes());
        bytes[48..52].copy_from_slice(&0x000000ffu32.to_le_bytes());
        bytes[52..56].copy_from_slice(&0xff000000u32.to_le_bytes());
        for (y, row) in image.rows().enumerate() {
            for (x, pixel) in row.enumerate() {
                let at = header + y * stride + x * 4;
                bytes[at..at + 4].copy_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        let _ = GlobalUnlock(memory);
        OpenClipboard(Some(hwnd)).map_err(win_error)?;
        EmptyClipboard().map_err(win_error)?;
        let result = SetClipboardData(17, Some(HANDLE(memory.0))).map_err(win_error);
        CloseClipboard().map_err(win_error)?;
        result.map(|_| ())
    }
}
fn drag_file(path: &Path) -> Result<(), String> {
    unsafe {
        let absolute = path.canonicalize().map_err(|error| error.to_string())?;
        let wide = absolute
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let absolute_pidl = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
        if absolute_pidl.is_null() {
            return Err(last_error("create file drag item"));
        }
        let parent_pidl = ILClone(absolute_pidl);
        if parent_pidl.is_null() {
            CoTaskMemFree(Some(absolute_pidl.cast()));
            return Err(last_error("clone file drag parent"));
        }
        let child = ILFindLastID(absolute_pidl);
        if child.is_null() || !ILRemoveLastID(Some(parent_pidl)).as_bool() {
            CoTaskMemFree(Some(parent_pidl.cast()));
            CoTaskMemFree(Some(absolute_pidl.cast()));
            return Err("cannot split file drag shell path".into());
        }
        let children = [child as *const ITEMIDLIST];
        let data: windows::core::Result<IDataObject> =
            SHCreateDataObject(Some(parent_pidl), Some(&children), None::<&IDataObject>);
        CoTaskMemFree(Some(parent_pidl.cast()));
        CoTaskMemFree(Some(absolute_pidl.cast()));
        let data = data.map_err(win_error)?;
        let source: IDropSource = FileDropSource.into();
        let mut effect = DROPEFFECT::default();
        let result = DoDragDrop(&data, &source, DROPEFFECT_COPY, &mut effect);
        if result == DRAGDROP_S_DROP || result == DRAGDROP_S_CANCEL {
            Ok(())
        } else {
            result.ok().map_err(win_error)
        }
    }
}
fn unique_path(root: &Path, prefix: &str, extension: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    for suffix in 0..1000 {
        let name = if suffix == 0 {
            format!("{prefix}-{stamp}.{extension}")
        } else {
            format!("{prefix}-{stamp}-{suffix}.{extension}")
        };
        let path = root.join(name);
        if !path.exists() {
            return path;
        }
    }
    root.join(format!("{prefix}-{}.{}", uuid::Uuid::new_v4(), extension))
}
fn set_autostart(enabled: bool) -> Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        );
        if status != ERROR_SUCCESS {
            return Err(format!(
                "cannot open autostart registry key: OS error {}",
                status.0
            ));
        }
        let status = if enabled {
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            let command = format!("\"{}\" --background\0", executable.display());
            let wide = command.encode_utf16().collect::<Vec<_>>();
            let bytes = std::slice::from_raw_parts(wide.as_ptr().cast::<u8>(), wide.len() * 2);
            RegSetValueExW(key, w!("CapturesWindowsNative"), None, REG_SZ, Some(bytes))
        } else {
            RegDeleteValueW(key, w!("CapturesWindowsNative"))
        };
        let _ = RegCloseKey(key);
        if status == ERROR_SUCCESS || (!enabled && status == ERROR_FILE_NOT_FOUND) {
            Ok(())
        } else {
            Err(format!("cannot update autostart: OS error {}", status.0))
        }
    }
}

fn replace_file_safely(replacement: &Path, original: &Path) -> Result<(), String> {
    let backup = original.with_file_name(format!(
        ".{}.captures-backup-{}",
        original
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("recording"),
        uuid::Uuid::new_v4()
    ));
    fs::rename(original, &backup).map_err(|error| error.to_string())?;
    if let Err(error) = fs::rename(replacement, original) {
        let rollback = fs::rename(&backup, original);
        return Err(match rollback {
            Ok(()) => format!("could not replace recording: {error}"),
            Err(rollback) => format!(
                "could not replace recording ({error}) or restore backup {} ({rollback})",
                backup.display()
            ),
        });
    }
    let _ = fs::remove_file(backup);
    Ok(())
}

fn polygon_points(bounds: Rect, sides: usize, rotation_degrees: f32) -> Vec<Point> {
    let center = Point {
        x: bounds.x + bounds.width / 2.0,
        y: bounds.y + bounds.height / 2.0,
    };
    (0..sides)
        .map(|index| {
            let angle = (rotation_degrees + index as f32 * 360.0 / sides as f32).to_radians();
            Point {
                x: center.x + angle.cos() * bounds.width / 2.0,
                y: center.y + angle.sin() * bounds.height / 2.0,
            }
        })
        .collect()
}

fn next_blend_mode(mode: BlendMode) -> BlendMode {
    match mode {
        BlendMode::Normal => BlendMode::Multiply,
        BlendMode::Multiply => BlendMode::Screen,
        BlendMode::Screen => BlendMode::Overlay,
        BlendMode::Overlay => BlendMode::Darken,
        BlendMode::Darken => BlendMode::Lighten,
        BlendMode::Lighten => BlendMode::Normal,
    }
}

fn parse_hex_color(value: &str) -> Option<[u8; 4]> {
    if value.len() != 7 || !value.starts_with('#') {
        return None;
    }
    Some([
        u8::from_str_radix(&value[1..3], 16).ok()?,
        u8::from_str_radix(&value[3..5], 16).ok()?,
        u8::from_str_radix(&value[5..7], 16).ok()?,
        255,
    ])
}

fn format_color(color: [u8; 4]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}
fn work_area() -> RECT {
    unsafe {
        let mut rect = RECT::default();
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some((&mut rect as *mut RECT).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok();
        rect
    }
}
fn point(value: isize) -> Point {
    Point {
        x: i16::from_le_bytes((value as u16).to_le_bytes()) as f32,
        y: i16::from_le_bytes(((value >> 16) as u16).to_le_bytes()) as f32,
    }
}

fn prepare_fixture(
    state: &mut AppState,
    view: &str,
    fixture_image: Option<RgbaImage>,
    fixture_video: Option<PathBuf>,
) {
    let image = fixture_image.unwrap_or_else(|| {
        RgbaImage::from_fn(1280, 720, |x, y| {
            let block = ((x / 80) + (y / 80)) % 2;
            if block == 0 {
                image::Rgba([38, 38, 45, 255])
            } else {
                image::Rgba([49, 52, 65, 255])
            }
        })
    });
    match view {
        "editor" => state.edit_image(image),
        "editor-image" => {
            let imported = RgbaImage::from_fn(420, 260, |x, y| {
                image::Rgba([
                    (32 + x / 2).min(255) as u8,
                    (48 + y / 2).min(255) as u8,
                    if x > y { 210 } else { 82 },
                    224,
                ])
            });
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                let id = document.add_image(imported, 0, "Imported gradient.png".into());
                document.set_layer_rotation(id, -8.0);
                state.selected_layer = Some(id);
                state.editor_tool = captures_windows_native::editor::Tool::Select;
            }
        }
        "editor-shapes" => {
            state.edit_image(image);
            state.editor_tool = captures_windows_native::editor::Tool::Rectangle;
            state.editor_shapes_open = true;
        }
        "editor-export" => {
            state.edit_image(image);
            state.editor_export_settings_open = true;
        }
        "editor-properties" => {
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                let id = document.add(
                    captures_windows_native::editor::Shape::Rectangle(Rect {
                        x: 170.0,
                        y: 120.0,
                        width: 390.0,
                        height: 220.0,
                    }),
                    [37, 99, 235, 255],
                    7.0,
                );
                document.set_layer_fill(id, Some([37, 99, 235, 72]));
                document.set_layer_rotation(id, 13.0);
                state.selected_layer = Some(id);
                state.editor_tool = captures_windows_native::editor::Tool::Select;
            }
        }
        "editor-line" => {
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                let id = document.add(
                    captures_windows_native::editor::Shape::Line(
                        Point { x: 180.0, y: 270.0 },
                        Point { x: 780.0, y: 270.0 },
                    ),
                    [239, 70, 80, 255],
                    8.0,
                );
                document.set_layer_rotation(id, 24.0);
                state.selected_layer = Some(id);
                state.editor_tool = captures_windows_native::editor::Tool::Select;
            }
        }
        "editor-merge" => {
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                document.add(
                    captures_windows_native::editor::Shape::Rectangle(Rect {
                        x: 180.0,
                        y: 160.0,
                        width: 360.0,
                        height: 240.0,
                    }),
                    [37, 99, 235, 255],
                    10.0,
                );
                let id = document.add(
                    captures_windows_native::editor::Shape::Ellipse(Rect {
                        x: 390.0,
                        y: 240.0,
                        width: 310.0,
                        height: 190.0,
                    }),
                    [239, 70, 80, 255],
                    12.0,
                );
                state.selected_layer = Some(id);
                state.editor_tool = captures_windows_native::editor::Tool::Select;
                state.editor_layer_menu_open = true;
            }
        }
        "editor-merge-visible" => {
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                document.add(
                    captures_windows_native::editor::Shape::Rectangle(Rect {
                        x: 240.0,
                        y: 170.0,
                        width: 390.0,
                        height: 250.0,
                    }),
                    [37, 99, 235, 255],
                    10.0,
                );
                let id = document
                    .merge_visible_layers()
                    .expect("fixture merge visible renders")
                    .expect("fixture source and shape are visible");
                state.selected_layer = Some(id);
                state.editor_tool = captures_windows_native::editor::Tool::Select;
            }
        }
        "editor-flatten-source" => {
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                document.set_background(Some([247, 247, 245, 255]));
            }
        }
        "editor-eraser" => {
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                let _ = document.remove_background_stroke(
                    captures_windows_native::editor::ImageTarget::Source,
                    &[
                        Point { x: 470.0, y: 220.0 },
                        Point { x: 570.0, y: 300.0 },
                        Point { x: 670.0, y: 380.0 },
                    ],
                    84.0,
                    18.0,
                    false,
                );
            }
            state.editor_tool = captures_windows_native::editor::Tool::Eraser;
            state.editor_remove_mode = RemoveBackgroundMode::Restore;
        }
        "editor-trim" => {
            let imported = RgbaImage::from_fn(420, 260, |x, y| {
                image::Rgba([
                    (24 + x / 2).min(255) as u8,
                    (36 + y / 2).min(255) as u8,
                    if x > y { 220 } else { 76 },
                    255,
                ])
            });
            state.edit_image(image);
            if let Some(document) = state.editor.as_mut() {
                let id = document.add_image(imported, 0, "Trim subject.png".into());
                if let Some(layer) = document.layers.iter_mut().find(|layer| layer.id == id)
                    && let captures_windows_native::editor::Shape::Image {
                        origin,
                        width,
                        height,
                        ..
                    } = &mut layer.shape
                {
                    *origin = Point { x: 190.2, y: 130.4 };
                    *width = 420.0;
                    *height = 260.0;
                }
                state.selected_layer = Some(id);
                state.editor_tool = captures_windows_native::editor::Tool::Select;
            }
        }
        "recording-selector" => state.surface = Surface::RecordingSelector,
        "recording-hud" => {
            state.surface = Surface::RecordingHud;
            state.recording = Some(RecordingUi {
                state: RecordingState::Recording,
                kind: RecordingKind::Video,
                started: Instant::now(),
                elapsed_before_pause: Duration::from_secs(42),
                muted: false,
                hidden: false,
                warning: None,
                source: None,
            });
        }
        "recording-editor" => {
            state.surface = Surface::RecordingEditor;
            state.recording_preview = Some(image);
            state.recording = Some(RecordingUi {
                state: RecordingState::Editor,
                kind: RecordingKind::Video,
                started: Instant::now(),
                elapsed_before_pause: Duration::ZERO,
                muted: false,
                hidden: false,
                warning: None,
                source: fixture_video,
            });
        }
        "preview"
        | "preview-delete-input"
        | "preview-dust-start"
        | "preview-dust-wave"
        | "preview-dust-end" => {
            let path = data_dir().join("fixture-preview.png");
            state.add_preview(
                Artifact::from_path(path, image.width(), image.height()),
                image,
                Instant::now(),
            );
            if view.starts_with("preview-dust-") {
                let preview = &mut state.previews[0];
                preview.start_deletion(Instant::now(), false);
                preview.deletion.as_mut().unwrap().fixture_elapsed_ms = Some(match view {
                    "preview-dust-start" => 0.0,
                    "preview-dust-wave" => 650.0,
                    _ => 1200.0,
                });
            }
            state.surface = Surface::Preview;
        }
        "history" => state.surface = Surface::History,
        "preferences" => state.surface = Surface::Preferences,
        "preferences-capture" => {
            state.surface = Surface::Preferences;
            state.preferences_page = PreferencesPage::Capture;
        }
        "preferences-recording" => {
            state.surface = Surface::Preferences;
            state.preferences_page = PreferencesPage::Recording;
        }
        "preferences-appearance" => {
            state.surface = Surface::Preferences;
            state.preferences_page = PreferencesPage::Appearance;
        }
        "feedback" => {
            state.surface = Surface::Feedback;
            state.feedback_message = "The export controls overlap at 150% scaling.".into();
            state.feedback_contact = "optional@example.com".into();
        }
        "delete-confirmation" => {
            state.pending_delete = Some(Artifact::from_path(
                data_dir().join("Capture-example.png"),
                1280,
                720,
            ));
            state.surface = Surface::DeleteConfirmation;
        }
        _ => {}
    }
}
fn argument_value(name: &str) -> Option<String> {
    let mut arguments = std::env::args();
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments.next();
        }
    }
    None
}
fn loword(value: isize) -> u16 {
    value as u16
}
fn hiword(value: isize) -> u16 {
    (value >> 16) as u16
}
fn scale(value: i32, dpi: f32) -> i32 {
    (value as f32 * dpi / 96.0).round() as i32
}
fn win_error(error: windows::core::Error) -> String {
    error.to_string()
}
fn last_error(context: &str) -> String {
    format!("{context}: {}", std::io::Error::last_os_error())
}
