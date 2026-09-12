mod renderer;

use captures_capture::{CaptureMode, DisplayDescriptor, XcapBackend};
use captures_recording::{
    AudioOptions, GifOptions, RecordingKind, RecordingOptions, RecordingSegmentInfo,
    RecordingState, RecordingTarget,
};
use captures_recording_xcap::XcapRecordingSegment;
use captures_windows_native::{
    geometry::{Point, Rect, SelectionDrag, update_selection},
    history::{Artifact, History, safe_delete},
    settings::{Settings, data_dir},
    state::{AppState, RecordingUi, Surface},
    theme::{palette, theme_colors},
};
use image::RgbaImage;
use renderer::Renderer;
use std::{
    fs::{self, OpenOptions},
    io::BufWriter,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{HANDLE, HMODULE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION, D3D11CreateDevice,
                ID3D11Device,
            },
            DirectComposition::{
                DCompositionCreateDevice, IDCompositionDevice, IDCompositionTarget,
                IDCompositionVisual,
            },
            Dxgi::{IDXGIAdapter, IDXGIDevice},
            Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT},
        },
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            LibraryLoader::GetModuleHandleW,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
            Ole::{OleInitialize, OleUninitialize},
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                HOT_KEY_MODIFIERS, MOD_CONTROL, MOD_SHIFT, RegisterHotKey, ReleaseCapture,
                SetCapture, UnregisterHotKey, VK_ESCAPE, VK_RETURN,
            },
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                Shell_NotifyIconW,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{Interface, PCWSTR, w},
};

const CLASS: PCWSTR = w!("CapturesWindowsNativeWindow");
const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ANIMATION: usize = 1;
const HOTKEY_CAPTURE: i32 = 100;
const HOTKEY_DISPLAY: i32 = 101;

struct Composition {
    _device: IDCompositionDevice,
    _target: IDCompositionTarget,
    _root: IDCompositionVisual,
}

struct RecordingSession {
    options: RecordingOptions,
    display: DisplayDescriptor,
    active: Option<XcapRecordingSegment>,
    segment_paths: Vec<PathBuf>,
    completed: Vec<RecordingSegmentInfo>,
    directory: PathBuf,
}

struct App {
    hwnd: HWND,
    renderer: Option<Renderer>,
    composition: Option<Composition>,
    state: AppState,
    settings: Settings,
    history: History,
    recording: Option<RecordingSession>,
    editor_drag: Option<Point>,
    next_session_check: Instant,
    width: u32,
    height: u32,
    dpi: f32,
    tray: NOTIFYICONDATAW,
}

pub fn run() -> Result<(), String> {
    unsafe {
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
        let settings = Settings::load()?;
        fs::create_dir_all(&settings.output_directory).map_err(|e| e.to_string())?;
        let history = History::load(&data_dir())?;
        let mut state = AppState::default();
        let requested_view = std::env::args().skip_while(|arg| arg != "--view").nth(1);
        if let Some(view) = requested_view.as_deref() {
            prepare_fixture(&mut state, view);
        }
        let app = Box::new(App {
            hwnd: HWND::default(),
            renderer: None,
            composition: None,
            state,
            settings,
            history,
            recording: None,
            editor_drag: None,
            next_session_check: Instant::now(),
            width: 420,
            height: 430,
            dpi: 96.0,
            tray: NOTIFYICONDATAW::default(),
        });
        let raw = Box::into_raw(app);
        let _hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            CLASS,
            w!("Captures"),
            WS_POPUP | WS_VISIBLE,
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
            WM_SIZE => {
                app.width = loword(lparam.0) as u32;
                app.height = hiword(lparam.0) as u32;
                if let Some(renderer) = &app.renderer {
                    renderer.resize(app.width, app.height)
                };
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
            WM_CLOSE => {
                let _ = ShowWindow(hwnd, SW_HIDE);
                LRESULT(0)
            }
            WM_DESTROY => {
                app.remove_tray();
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_CAPTURE);
                let _ = UnregisterHotKey(Some(hwnd), HOTKEY_DISPLAY);
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
            self.composition = Some(composition(self.hwnd)?);
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
            self.show_surface(surface, width, height, surface == Surface::Preview);
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
            let _ = renderer.draw(
                &self.state,
                palette(light, accent, signal),
                self.width as f32 * 96.0 / self.dpi,
                self.height as f32 * 96.0 / self.dpi,
            );
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
            let excluded = match surface {
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
                    if let Some(overlay) = &mut self.state.overlay {
                        if overlay.mode == CaptureMode::Region {
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
                }
                Surface::RecordingSelector => {
                    if p.y > 375.0 {
                        self.start_capture(CaptureMode::Region, true)
                    }
                }
                Surface::Preview => {
                    if p.y > self.height as f32 * 96.0 / self.dpi - 42.0 {
                        if p.x < 85.0 {
                            if let Some(preview) = self.state.previews.first() {
                                self.state.edit_image(preview.image.clone());
                                self.show_surface(Surface::ScreenshotEditor, 1100, 720, false)
                            }
                        } else if p.x > 250.0 {
                            if let Some(preview) = self.state.previews.first() {
                                self.state.pending_delete = Some(preview.artifact.clone());
                                self.show_surface(Surface::DeleteConfirmation, 480, 300, false)
                            }
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
                        self.show_surface(Surface::Preview, 340, 200, true)
                    }
                }
                Surface::RecordingHud => self.hud_click(p),
                Surface::Preferences => self.preferences_click(p),
                Surface::History => self.history_click(p),
                Surface::ScreenshotEditor => self.editor_pointer_down(p),
                _ => {}
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
            0 => self.settings.launch_at_login = !self.settings.launch_at_login,
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
        if artifact.kind != captures_windows_native::history::ArtifactKind::Image {
            return;
        }
        match image::open(&artifact.path) {
            Ok(image) => {
                self.state
                    .add_preview(artifact, image.to_rgba8(), Instant::now());
                unsafe {
                    self.show_preview();
                }
            }
            Err(error) => self.set_error(error.to_string()),
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
            }
            if p.x > width - 236.0 {
                if let Some(document) = &self.state.editor {
                    let _ = copy_image(self.hwnd, &document.render());
                }
                return;
            }
            let index = (p.x / 76.0).floor() as usize;
            self.state.editor_tool = [
                captures_windows_native::editor::Tool::Select,
                captures_windows_native::editor::Tool::Crop,
                captures_windows_native::editor::Tool::Text,
                captures_windows_native::editor::Tool::Pen,
                captures_windows_native::editor::Tool::Arrow,
                captures_windows_native::editor::Tool::Line,
                captures_windows_native::editor::Tool::Rectangle,
                captures_windows_native::editor::Tool::Ellipse,
            ]
            .get(index)
            .copied()
            .unwrap_or(captures_windows_native::editor::Tool::Select);
        } else if p.x > 76.0 && p.x < width - 208.0 && p.y > 80.0 {
            self.editor_drag = Some(p);
        }
    }

    unsafe fn editor_pointer_up(&mut self, p: Point) {
        let Some(start) = self.editor_drag.take() else {
            return;
        };
        let Some(document) = &mut self.state.editor else {
            return;
        };
        let shape = match self.state.editor_tool {
            captures_windows_native::editor::Tool::Arrow => {
                captures_windows_native::editor::Shape::Arrow(start, p)
            }
            captures_windows_native::editor::Tool::Line => {
                captures_windows_native::editor::Shape::Line(start, p)
            }
            captures_windows_native::editor::Tool::Ellipse => {
                captures_windows_native::editor::Shape::Ellipse(Rect::from_points(start, p))
            }
            captures_windows_native::editor::Tool::Pen => {
                captures_windows_native::editor::Shape::Stroke(vec![start, p])
            }
            _ => captures_windows_native::editor::Shape::Rectangle(Rect::from_points(start, p)),
        };
        document.add(shape, [239, 70, 80, 255], 3.0);
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }
    unsafe fn key(&mut self, key: u32) {
        unsafe {
            if key == VK_ESCAPE.0 as u32 {
                if self.state.surface == Surface::Overlay {
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
        let mut writer = BufWriter::new(file);
        let format = match extension.as_str() {
            "jpeg" => image::ImageFormat::Jpeg,
            "webp" => image::ImageFormat::WebP,
            _ => image::ImageFormat::Png,
        };
        image::DynamicImage::ImageRgba8(image.clone())
            .write_to(&mut writer, format)
            .map_err(|e| e.to_string())?;
        Ok(Artifact::from_path(path, image.width(), image.height()))
    }
    unsafe fn show_preview(&mut self) {
        unsafe {
            let work = work_area();
            let x = work.right - scale(368, self.dpi);
            let y = work.bottom - scale(228, self.dpi);
            self.state.surface = Surface::Preview;
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                scale(340, self.dpi),
                scale(200, self.dpi),
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
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
                    let _ = self.history.add(artifact);
                    if self.settings.recording.open_editor_after_recording {
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
                match safe_delete(&artifact.path, &self.settings.output_directory) {
                    Ok(()) => {
                        let _ = self.history.remove_entry(&artifact.path);
                        self.state
                            .previews
                            .retain(|p| p.artifact.path != artifact.path);
                        self.show_surface(Surface::Menu, 420, 430, false)
                    }
                    Err(error) => {
                        self.set_error(error);
                        self.show_surface(Surface::Preview, 340, 200, true)
                    }
                }
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

unsafe fn composition(hwnd: HWND) -> Result<Composition, String> {
    unsafe {
        let mut d3d = None;
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        )
        .map_err(win_error)?;
        let d3d: ID3D11Device = d3d.ok_or("D3D11 returned no device")?;
        let dxgi: IDXGIDevice = d3d.cast().map_err(win_error)?;
        let device: IDCompositionDevice = DCompositionCreateDevice(&dxgi).map_err(win_error)?;
        let target = device.CreateTargetForHwnd(hwnd, true).map_err(win_error)?;
        let root = device.CreateVisual().map_err(win_error)?;
        target.SetRoot(&root).map_err(win_error)?;
        device.Commit().map_err(win_error)?;
        Ok(Composition {
            _device: device,
            _target: target,
            _root: root,
        })
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

fn prepare_fixture(state: &mut AppState, view: &str) {
    let image = RgbaImage::from_fn(1280, 720, |x, y| {
        let block = ((x / 80) + (y / 80)) % 2;
        if block == 0 {
            image::Rgba([38, 38, 45, 255])
        } else {
            image::Rgba([49, 52, 65, 255])
        }
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
        "recording-editor" => state.surface = Surface::RecordingEditor,
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
