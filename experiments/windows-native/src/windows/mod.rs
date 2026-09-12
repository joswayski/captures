mod renderer;

use captures_capture::{CaptureMode, DisplayDescriptor, XcapBackend};
use captures_media::{
    AudioEdit, CancelToken, EditSpec, ExportFormat, ExportSpec, MediaToolchain, QualityPreset,
    extrapolate_sampled_size,
};
use captures_recording::{
    AudioOptions, GifOptions, RecordingKind, RecordingOptions, RecordingSegmentInfo,
    RecordingState, RecordingTarget,
};
use captures_recording_xcap::XcapRecordingSegment;
use captures_windows_native::{
    geometry::{Point, Rect, SelectionDrag, rounded_contains, update_selection},
    history::{Artifact, History, move_to_trash, restore_from_trash, safe_delete},
    settings::{Settings, data_dir, profile_id},
    state::{AppState, RecordingEditorState, RecordingUi, Surface},
    theme::{palette, theme_colors},
};
use image::RgbaImage;
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
            BeginPaint, CombineRgn, CreateRoundRectRgn, DeleteObject, EndPaint, HGDIOBJ,
            InvalidateRect, PAINTSTRUCT, RGN_OR, ScreenToClient, SetWindowRgn,
        },
        System::{
            Com::{
                COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
                IDataObject,
            },
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{
                DROPEFFECT, DROPEFFECT_COPY, DoDragDrop, IDropSource, IDropSource_Impl,
                OleInitialize, OleUninitialize,
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
                ReleaseCapture, SetCapture, UnregisterHotKey, VK_CONTROL, VK_DELETE, VK_ESCAPE,
                VK_RETURN,
            },
            Shell::{
                Common::ITEMIDLIST, ILClone, ILCreateFromPathW, ILFindLastID, ILRemoveLastID,
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                SHCreateDataObject, Shell_NotifyIconW,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{BOOL, HRESULT, HSTRING, PCWSTR, implement, w},
};

const CLASS: PCWSTR = w!("CapturesWindowsNativeWindow");
const WM_TRAY: u32 = WM_APP + 1;
const WM_ACTIVATE_INSTANCE: u32 = WM_APP + 2;
const TIMER_ANIMATION: usize = 1;
const HOTKEY_CAPTURE: i32 = 100;
const HOTKEY_DISPLAY: i32 = 101;

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
    last_video_frame_ms: u64,
    fixture_mode: bool,
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
        let history = History::load(&data_dir())?;
        let mut state = AppState::default();
        let requested_view = argument_value("--view");
        if let Some(view) = requested_view.as_deref() {
            let fixture_image = argument_value("--fixture-image")
                .map(image::open)
                .transpose()
                .map_err(|error| format!("cannot open fixture image: {error}"))?
                .map(|image| image.to_rgba8());
            prepare_fixture(
                &mut state,
                view,
                fixture_image,
                argument_value("--fixture-video").map(PathBuf::from),
            );
        }
        let app = Box::new(App {
            hwnd: HWND::default(),
            renderer: None,
            state,
            settings,
            history,
            recording: None,
            recording_mode: CaptureMode::Region,
            editor_drag: None,
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
            last_video_frame_ms: u64::MAX,
            fixture_mode: requested_view.is_some(),
        });
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
                app.character(char::from_u32(wparam.0 as u32));
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
                app.state.tick(now);
                app.tick_recording_editor(now);
                if had_previews
                    && app.state.previews.is_empty()
                    && app.state.surface == Surface::Preview
                {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                if app.state.surface == Surface::RecordingHud || !app.state.previews.is_empty() {
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
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_DESTROY => {
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
                self.open_recording_editor(source)?;
            }
            let (surface, width, height) = match self.state.surface {
                Surface::ScreenshotEditor => (Surface::ScreenshotEditor, 1100, 720),
                Surface::RecordingSelector => (Surface::RecordingSelector, 460, 450),
                Surface::RecordingHud => (Surface::RecordingHud, 500, 82),
                Surface::RecordingEditor => (Surface::RecordingEditor, 1000, 680),
                Surface::Preview => (Surface::Preview, 340, 200),
                Surface::History => (Surface::History, 720, 430),
                Surface::Preferences => (Surface::Preferences, 760, 520),
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
            let _ = renderer.draw(Frame {
                state: &self.state,
                settings: &self.settings,
                history: self.history.entries(),
                recording_mode: self.recording_mode,
                palette: palette(light, accent, signal),
                width: self.width as f32 * 96.0 / self.dpi,
                height: self.height as f32 * 96.0 / self.dpi,
            });
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
                    if p.y > self.height as f32 * 96.0 / self.dpi - 42.0 {
                        if p.x < 85.0 {
                            if let Some(preview) = self.state.previews.first() {
                                self.state.edit_image(preview.image.clone());
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
                Surface::History => self.history_click(p),
                Surface::ScreenshotEditor => self.editor_pointer_down(p),
                Surface::RecordingEditor => self.recording_editor_click(p),
            }
        }
    }

    unsafe fn pointer_move(&mut self, p: Point) {
        unsafe {
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
            if let Some(overlay) = &mut self.state.overlay {
                overlay.drag = None;
                let _ = ReleaseCapture();
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            } else if self.state.surface == Surface::ScreenshotEditor {
                self.editor_pointer_up(_p);
            }
        }
    }

    unsafe fn preferences_click(&mut self, p: Point) {
        if p.x < 200.0 || p.y < 78.0 {
            return;
        }
        match ((p.y - 84.0) / 64.0).floor() as i32 {
            0 => {
                let enabled = !self.settings.launch_at_login;
                if let Err(error) = set_autostart(enabled) {
                    self.set_error(error);
                    return;
                }
                self.settings.launch_at_login = enabled;
            }
            1 => self.settings.auto_copy_to_clipboard = !self.settings.auto_copy_to_clipboard,
            2 => self.settings.show_mini_previews = !self.settings.show_mini_previews,
            3 => self.settings.freeze_screen = !self.settings.freeze_screen,
            4 => {
                self.settings.appearance = match self.settings.appearance.as_str() {
                    "system" => "light",
                    "light" => "dark",
                    _ => "system",
                }
                .into()
            }
            5 => {
                self.settings.theme = match self.settings.theme.as_str() {
                    "mustard" => "cobalt",
                    "cobalt" => "mint",
                    _ => "mustard",
                }
                .into()
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
                            self.state.edit_image(image.to_rgba8());
                            unsafe {
                                self.show_surface(Surface::ScreenshotEditor, 1100, 720, false)
                            };
                        }
                        Err(error) => self.set_error(error.to_string()),
                    }
                } else {
                    match self.open_recording_editor(artifact.path) {
                        Ok(()) => unsafe {
                            self.show_surface(Surface::RecordingEditor, 1000, 680, false)
                        },
                        Err(error) => self.set_error(error),
                    }
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
        if p.y <= 56.0 {
            if p.x > width - 84.0 {
                unsafe {
                    self.show_surface(Surface::Menu, 420, 430, false);
                }
                return;
            } else if p.x > width - 160.0 {
                let Some(result) = self.state.editor.as_ref().map(|document| document.render())
                else {
                    return;
                };
                let image = match result {
                    Ok(image) => image,
                    Err(error) => {
                        self.set_error(error);
                        return;
                    }
                };
                match self.save_image(&image) {
                    Ok(artifact) => {
                        let _ = self.history.add(artifact.clone());
                        self.state.add_preview(artifact, image, Instant::now());
                        unsafe { self.show_preview() };
                    }
                    Err(error) => self.set_error(error),
                }
                return;
            } else if p.x > width - 236.0 {
                if let Some(document) = &self.state.editor {
                    match document.render() {
                        Ok(image) => {
                            let _ = copy_image(self.hwnd, &image);
                        }
                        Err(error) => self.set_error(error),
                    }
                }
                return;
            }
            let index = (p.x / 70.0).floor() as usize;
            self.state.editor_tool = [
                captures_windows_native::editor::Tool::Select,
                captures_windows_native::editor::Tool::Crop,
                captures_windows_native::editor::Tool::Text,
                captures_windows_native::editor::Tool::Pen,
                captures_windows_native::editor::Tool::Arrow,
                captures_windows_native::editor::Tool::Line,
                captures_windows_native::editor::Tool::Rectangle,
                captures_windows_native::editor::Tool::Ellipse,
                captures_windows_native::editor::Tool::Triangle,
                captures_windows_native::editor::Tool::Diamond,
                captures_windows_native::editor::Tool::Star,
            ]
            .get(index)
            .copied()
            .unwrap_or(captures_windows_native::editor::Tool::Select);
        } else if p.x > width - 190.0 && p.x < width - 78.0 && (228.0..262.0).contains(&p.y) {
            self.state.editor_editing_color = true;
            self.state.status = Some((
                "Type a #RRGGBB color and press Enter".into(),
                Instant::now(),
            ));
        } else if let Some(source) = self.editor_source_point(p) {
            if self.state.editor_tool == captures_windows_native::editor::Tool::Select {
                self.state.selected_layer = self
                    .state
                    .editor
                    .as_ref()
                    .and_then(|document| document.hit_test(source, 6.0));
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
            } else {
                self.editor_drag = Some(source);
            }
        }
    }

    unsafe fn editor_pointer_up(&mut self, p: Point) {
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
            captures_windows_native::editor::Tool::Pen => {
                captures_windows_native::editor::Shape::Stroke(vec![start, end])
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
        self.state.selected_layer = Some(document.add(shape, self.state.editor_color, 3.0));
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn editor_source_point(&self, point: Point) -> Option<Point> {
        let document = self.state.editor.as_ref()?;
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let viewport = Rect {
            x: 76.0,
            y: 80.0,
            width: width - 300.0,
            height: height - 130.0,
        };
        if !viewport.contains(point) {
            return None;
        }
        Some(Point {
            x: document.crop.x + (point.x - viewport.x) / viewport.width * document.crop.width,
            y: document.crop.y + (point.y - viewport.y) / viewport.height * document.crop.height,
        })
    }

    unsafe fn recording_editor_click(&mut self, point: Point) {
        let width = self.width as f32 * 96.0 / self.dpi;
        let height = self.height as f32 * 96.0 / self.dpi;
        let Some(editor) = self.state.recording_editor.as_mut() else {
            return;
        };
        let mut rebuild_comparison = false;
        if point.y < 58.0 && point.x > width - 120.0 {
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
        } else if point.x > width - 100.0 && (376.0..422.0).contains(&point.y) {
            editor.save_as_new = !editor.save_as_new;
        } else if (height - 120.0..height - 52.0).contains(&point.y) {
            let track_start = 104.0;
            let track_width = (width - 164.0).max(1.0);
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
            } else if point.x < 88.0 {
                editor.toggle_playback(Instant::now());
            } else {
                editor.seek(at);
                self.last_video_frame_ms = u64::MAX;
            }
        }
        if self.last_video_frame_ms == u64::MAX
            && let Err(error) = self.refresh_recording_frame()
        {
            self.set_error(error);
        }
        if rebuild_comparison && let Err(error) = self.build_compression_comparison() {
            self.set_error(error);
        }
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn open_recording_editor(&mut self, source: PathBuf) -> Result<(), String> {
        let tools = MediaToolchain::from_command_names();
        tools.verify().map_err(|error| error.to_string())?;
        let probe = tools.probe(&source).map_err(|error| error.to_string())?;
        let duration_ms = probe.metadata.duration_ms.unwrap_or(0);
        self.state.recording_editor = Some(
            RecordingEditorState::new(source, duration_ms, probe.has_audio)
                .map_err(str::to_owned)?,
        );
        self.last_video_frame_ms = u64::MAX;
        self.refresh_recording_frame()?;
        self.build_compression_comparison()?;
        Ok(())
    }

    fn tick_recording_editor(&mut self, now: Instant) {
        let should_refresh =
            self.state
                .recording_editor
                .as_mut()
                .is_some_and(|editor| editor.tick(now))
                && self.state.recording_editor.as_ref().is_some_and(|editor| {
                    editor.position_ms.abs_diff(self.last_video_frame_ms) >= 100
                });
        if should_refresh {
            if let Err(error) = self.refresh_recording_frame() {
                self.set_error(error);
                if let Some(editor) = self.state.recording_editor.as_mut() {
                    editor.playing = false;
                }
            }
            unsafe {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    fn refresh_recording_frame(&mut self) -> Result<(), String> {
        let editor = self
            .state
            .recording_editor
            .as_ref()
            .ok_or("recording editor is unavailable")?;
        let at_ms = editor.position_ms.min(editor.duration_ms.saturating_sub(1));
        let output = data_dir().join(format!("playback-frame-{}.png", uuid::Uuid::new_v4()));
        let result = MediaToolchain::from_command_names()
            .extract_frame(&editor.source, at_ms, &output, &CancelToken::default())
            .map_err(|error| error.to_string())
            .and_then(|()| image::open(&output).map_err(|error| error.to_string()));
        let _ = fs::remove_file(&output);
        self.state.recording_preview = Some(result?.to_rgba8());
        self.last_video_frame_ms = at_ms;
        Ok(())
    }

    fn export_recording_editor(&mut self) {
        let Some(editor) = self.state.recording_editor.clone() else {
            return;
        };
        editor_export(self, &editor);
    }

    fn build_compression_comparison(&mut self) -> Result<(), String> {
        let editor = self
            .state
            .recording_editor
            .clone()
            .ok_or("recording editor is unavailable")?;
        let sample_start = editor
            .position_ms
            .saturating_sub(500)
            .max(editor.trim_start_ms);
        let sample_end = (sample_start + 1_000).min(editor.trim_end_ms);
        let sample_duration = sample_end.saturating_sub(sample_start);
        if sample_duration == 0 {
            return Err("compression comparison range is empty".into());
        }
        let extension = if editor
            .source
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("gif"))
        {
            "gif"
        } else {
            "mp4"
        };
        let sample = data_dir().join(format!("compare-{}.{}", uuid::Uuid::new_v4(), extension));
        let after_path = data_dir().join(format!("compare-after-{}.png", uuid::Uuid::new_v4()));
        let tools = MediaToolchain::from_command_names();
        let result: Result<(RgbaImage, u64), String> = (|| {
            let edit = EditSpec {
                trim_start_ms: sample_start,
                trim_end_ms: Some(sample_end),
                audio: AudioEdit {
                    source_has_system_audio: editor.has_audio,
                    ..Default::default()
                },
                ..Default::default()
            };
            let format = if extension == "gif" {
                ExportFormat::Gif
            } else {
                ExportFormat::Mp4
            };
            let outcome = tools
                .export(
                    &editor.source,
                    &sample,
                    &edit,
                    &ExportSpec {
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
                    &CancelToken::default(),
                    |_| {},
                )
                .map_err(|error| error.to_string())?;
            tools
                .extract_frame(
                    &sample,
                    editor.position_ms.saturating_sub(sample_start),
                    &after_path,
                    &CancelToken::default(),
                )
                .map_err(|error| error.to_string())?;
            let before = self
                .state
                .recording_preview
                .as_ref()
                .ok_or("original comparison frame is unavailable")?;
            let mut after = image::open(&after_path)
                .map_err(|error| error.to_string())?
                .to_rgba8();
            if after.dimensions() != before.dimensions() {
                after = image::imageops::resize(
                    &after,
                    before.width(),
                    before.height(),
                    image::imageops::FilterType::Triangle,
                );
            }
            let mut comparison = before.clone();
            let split = comparison.width() / 2;
            for y in 0..comparison.height() {
                for x in split..comparison.width() {
                    comparison.put_pixel(x, y, *after.get_pixel(x, y));
                }
            }
            let estimated = extrapolate_sampled_size(
                outcome.size_bytes,
                sample_duration,
                editor.trim_end_ms.saturating_sub(editor.trim_start_ms),
            );
            Ok((comparison, estimated))
        })();
        let _ = fs::remove_file(sample);
        let _ = fs::remove_file(after_path);
        let (comparison, estimated) = result?;
        self.state.recording_preview = Some(comparison);
        if let Some(editor) = self.state.recording_editor.as_mut() {
            editor.comparison_estimated_bytes = Some(estimated);
        }
        Ok(())
    }
    unsafe fn key(&mut self, key: u32) {
        unsafe {
            let editor = self.state.surface == Surface::ScreenshotEditor;
            let control = GetKeyState(VK_CONTROL.0 as i32) < 0;
            if editor && key == VK_RETURN.0 as u32 && self.editor_text_origin.is_some() {
                self.commit_editor_text();
            } else if editor && key == VK_RETURN.0 as u32 && self.state.editor_editing_color {
                match parse_hex_color(&self.state.editor_color_hex) {
                    Some(color) => {
                        self.state.editor_color = color;
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
            } else if key == VK_ESCAPE.0 as u32 {
                if self.editor_text_origin.take().is_some() || self.state.editor_editing_color {
                    self.editor_text.clear();
                    self.state.editor_editing_color = false;
                    let _ = InvalidateRect(Some(self.hwnd), None, false);
                } else if self.state.surface == Surface::Overlay {
                    self.state.cancel_overlay();
                    self.show_surface(Surface::Menu, 420, 430, false)
                } else if self.state.surface == Surface::Preview {
                    if let Some(preview) = self.state.previews.first_mut() {
                        preview.dismissing = Some(Instant::now());
                    }
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

    fn character(&mut self, character: Option<char>) {
        let Some(character) = character else {
            return;
        };
        if self.state.surface != Surface::ScreenshotEditor {
            return;
        }
        if self.state.editor_editing_color {
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
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        let bytes = match extension.as_str() {
            "jpeg" => captures_windows_native::encoder::encode_jpeg(image, 92),
            "webp" => captures_windows_native::encoder::encode_webp(image, None),
            _ => captures_windows_native::encoder::encode_png(image, None),
        };
        std::io::Write::write_all(&mut BufWriter::new(file), &bytes?).map_err(|e| e.to_string())?;
        Ok(Artifact::from_path(path, image.width(), image.height()))
    }
    unsafe fn show_preview(&mut self) {
        unsafe {
            let work = work_area();
            let logical_height =
                200 + self.state.previews.len().saturating_sub(1).min(4) as i32 * 28;
            let x = work.right - scale(368, self.dpi);
            let y = work.bottom - scale(logical_height + 28, self.dpi);
            self.state.surface = Surface::Preview;
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                scale(340, self.dpi),
                scale(logical_height, self.dpi),
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
            self.update_preview_region();
            let _ = SetWindowDisplayAffinity(
                self.hwnd,
                if self.settings.include_mini_previews_in_captures {
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
            let y = (count - 1 - index) as f32 * 28.0;
            rounded_contains(
                point,
                Rect {
                    x: 0.0,
                    y,
                    width: 340.0,
                    height: 200.0,
                },
                14.0,
            )
        })
    }

    unsafe fn update_preview_region(&self) {
        unsafe {
            let count = self.state.previews.len().clamp(1, 5);
            let diameter = scale(28, self.dpi);
            let combined = CreateRoundRectRgn(
                0,
                0,
                scale(340, self.dpi) + 1,
                scale(200, self.dpi) + 1,
                diameter,
                diameter,
            );
            if combined.is_invalid() {
                return;
            }
            for index in 1..count {
                let y = scale(index as i32 * 28, self.dpi);
                let card = CreateRoundRectRgn(
                    0,
                    y,
                    scale(340, self.dpi) + 1,
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
                        match self.open_recording_editor(artifact.path) {
                            Ok(()) => {
                                self.state.surface = Surface::RecordingEditor;
                                self.show_surface(Surface::RecordingEditor, 1000, 680, false)
                            }
                            Err(error) => {
                                self.set_error(error);
                                self.show_surface(Surface::Menu, 420, 430, false)
                            }
                        }
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
                        let _ = self.history.replace_entry(&old, trashed);
                        self.state
                            .previews
                            .retain(|preview| preview.artifact.path != old);
                        self.show_delete_return()
                    }
                    Ok(None) => {
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

fn editor_export(app: &mut App, editor: &RecordingEditorState) {
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
    let edit = EditSpec {
        trim_start_ms: editor.trim_start_ms,
        trim_end_ms: Some(editor.trim_end_ms),
        audio: AudioEdit {
            source_has_system_audio: editor.has_audio,
            ..Default::default()
        },
        ..Default::default()
    };
    let spec = ExportSpec {
        format,
        quality: editor.quality,
        max_size_bytes: None,
        frames_per_second: Some(if format == ExportFormat::Gif {
            app.settings.recording.gif_fps
        } else {
            app.settings.recording.video_fps
        }),
        gif_max_colors: (format == ExportFormat::Gif)
            .then_some(app.settings.recording.gif_max_colors),
    };
    let destination = unique_path(
        &app.settings.output_directory,
        if editor.save_as_new {
            "Recording edit"
        } else {
            ".Captures replacement"
        },
        &extension,
    );
    let result = MediaToolchain::from_command_names()
        .export(
            &editor.source,
            &destination,
            &edit,
            &spec,
            &CancelToken::default(),
            |_| {},
        )
        .map_err(|error| error.to_string())
        .and_then(|outcome| {
            if editor.save_as_new {
                let probe = MediaToolchain::from_command_names()
                    .probe(&outcome.path)
                    .map_err(|error| error.to_string())?;
                app.history.add(Artifact::from_path(
                    outcome.path,
                    probe.metadata.width,
                    probe.metadata.height,
                ))?;
                Ok("Exported as a new recording".to_owned())
            } else {
                replace_file_safely(&destination, &editor.source)?;
                Ok("Updated the original recording".to_owned())
            }
        });
    match result {
        Ok(message) => app.state.status = Some((message, Instant::now())),
        Err(error) => {
            let _ = fs::remove_file(destination);
            app.set_error(error);
        }
    }
    unsafe {
        let _ = InvalidateRect(Some(app.hwnd), None, false);
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
        "preview" => {
            let path = data_dir().join("fixture-preview.png");
            state.add_preview(
                Artifact::from_path(path, image.width(), image.height()),
                image,
                Instant::now(),
            );
            state.surface = Surface::Preview;
        }
        "history" => state.surface = Surface::History,
        "preferences" => state.surface = Surface::Preferences,
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
