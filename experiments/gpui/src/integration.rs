//! Process-lifetime native shell integration for the GPUI application.
//!
//! Native libraries deliver events outside GPUI. This module only queues those
//! events there; a foreground GPUI task drains and routes them on the app thread.

use crate::{Launch, preferences::settings::Settings};
use anyhow::{Context as _, Result, anyhow, bail};
use auto_launch::{AutoLaunch, AutoLaunchBuilder};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use gpui::{
    App, Bounds, Context, Global, Render, SharedString, Subscription, Timer, TitlebarOptions,
    Window, WindowBounds, WindowOptions, div, prelude::*, px, size,
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};
#[cfg(target_os = "windows")]
use tray_icon::{MouseButton, MouseButtonState};

const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(16);

gpui::actions!(
    integration,
    [OpenPreferences, OpenHistory, OpenFeedback, QuitCaptures]
);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CaptureAction {
    Screenshot,
    Video,
    Gif,
}

impl CaptureAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Screenshot => "screenshot",
            Self::Video => "video",
            Self::Gif => "gif",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CaptureTarget {
    Region,
    Window,
    Display,
}

impl CaptureTarget {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::Window => "window",
            Self::Display => "display",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShortcutAction {
    NewCapture,
    Capture(CaptureAction, CaptureTarget),
    Preferences,
    History,
    Feedback,
    Quit,
}

#[derive(Clone, Debug)]
struct RegisteredShortcut {
    hotkey: HotKey,
    action: ShortcutAction,
}

pub struct Integration {
    launch: Launch,
    shortcuts: Option<GlobalHotKeyManager>,
    registered: Vec<RegisteredShortcut>,
    actions_by_id: HashMap<u32, ShortcutAction>,
    armed: HashSet<u32>,
    win_shift_s_action: Option<ShortcutAction>,
    win_shift_s_armed: bool,
    shortcut_capture_active: bool,
    captured_shortcut: Option<String>,
    autolaunch: AutoLaunch,
    launch_at_login: bool,
    onboarding_completed: bool,
    _tray: TrayIcon,
    _keepalive_window: gpui::WindowHandle<KeepaliveWindow>,
    alive: Arc<AtomicBool>,
    _quit_subscription: Option<Subscription>,
}

/// Tray bounds in GPUI logical desktop coordinates. Linux AppIndicator does
/// not expose icon geometry, so callers must use an unanchored fallback there.
#[derive(Clone, Copy, Debug)]
pub struct TrayAnchor {
    pub bounds: Bounds<gpui::Pixels>,
}

impl Global for Integration {}

impl Drop for Integration {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Installs process-lifetime tray, shortcut, startup, and event routing state.
///
/// The integration is stored as GPUI global state so it remains alive even when
/// every document window is closed.
pub fn install(launch: Launch, cx: &mut App) -> Result<()> {
    if cx.has_global::<Integration>() {
        bail!("native integration is already installed");
    }

    let settings = crate::preferences::settings::load(&launch.profile)
        .context("load native integration settings")?;
    let autolaunch = build_autolaunch(&launch.profile)?;
    let shortcuts = if global_shortcuts_supported() {
        Some(GlobalHotKeyManager::new().context("create global shortcut manager")?)
    } else {
        eprintln!(
            "Global shortcuts are disabled: GPUI's current Wayland session has no supported global-shortcut backend"
        );
        None
    };
    let tray = build_tray().context("create system tray icon")?;
    let keepalive_window = open_keepalive_window(cx)?;
    let alive = Arc::new(AtomicBool::new(true));

    let mut integration = Integration {
        launch,
        shortcuts,
        registered: Vec::new(),
        actions_by_id: HashMap::new(),
        armed: HashSet::new(),
        win_shift_s_action: None,
        win_shift_s_armed: false,
        shortcut_capture_active: false,
        captured_shortcut: None,
        autolaunch,
        launch_at_login: false,
        onboarding_completed: settings.onboarding_completed,
        _tray: tray,
        _keepalive_window: keepalive_window,
        alive: alive.clone(),
        _quit_subscription: None,
    };
    // Keep the native shell alive on registration conflicts so Preferences can
    // repair them; a conflicting shortcut must not strand every settings page.
    let registration_result = integration.reconcile(&settings);

    cx.set_global(integration);
    install_app_key_routes(cx);
    let quit = cx.on_app_quit(|cx| {
        cx.global_mut::<Integration>().shutdown();
        async {}
    });
    cx.global_mut::<Integration>()._quit_subscription = Some(quit);

    cx.spawn(async move |cx| {
        while alive.load(Ordering::Acquire) {
            Timer::after(EVENT_POLL_INTERVAL).await;
            if cx.update(drain_native_events).is_err() {
                break;
            }
        }
    })
    .detach();
    registration_result
}

pub fn tray_anchor(cx: &mut App) -> Option<TrayAnchor> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let (rect, keepalive_window) = {
            let integration = cx.try_global::<Integration>()?;
            (integration._tray.rect()?, integration._keepalive_window)
        };
        if rect.size.width == 0 || rect.size.height == 0 {
            return None;
        }
        // tray-icon reports physical coordinates; GPUI window bounds are logical.
        // The hidden native owner is created on the primary display where status
        // items normally live and gives us GPUI's platform scale conversion.
        let scale = keepalive_window
            .update(cx, |_, window, _| window.scale_factor())
            .ok()?
            .max(1.);
        return Some(TrayAnchor {
            bounds: Bounds::new(
                gpui::point(
                    gpui::px(rect.position.x as f32 / scale),
                    gpui::px(rect.position.y as f32 / scale),
                ),
                size(
                    gpui::px(rect.size.width as f32 / scale),
                    gpui::px(rect.size.height as f32 / scale),
                ),
            ),
        });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = cx;
        None
    }
}

struct KeepaliveWindow;

impl Render for KeepaliveWindow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// GPUI 0.2.2 tears down its Linux display connection when its final platform
/// window closes. Retain an unmapped GPUI window so tray and shortcut actions
/// can create document windows after every visible window has closed.
fn open_keepalive_window(cx: &mut App) -> Result<gpui::WindowHandle<KeepaliveWindow>> {
    let handle = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(16.), px(16.)),
                    cx,
                ))),
                titlebar: None,
                kind: gpui::WindowKind::PopUp,
                app_id: Some("captures-gpui-keepalive".into()),
                focus: false,
                show: false,
                is_movable: false,
                is_resizable: false,
                is_minimizable: false,
                ..Default::default()
            },
            |_, cx| cx.new(|_| KeepaliveWindow),
        )
        .context("create native integration keepalive window")?;
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        // Window::new unconditionally maps X11 windows in GPUI 0.2.2, even
        // with show:false. Unmap the real native surface, retaining its owner.
        set_x11_window_visible("captures-gpui-keepalive", false)?;
    }
    Ok(handle)
}

#[cfg(target_os = "linux")]
pub fn set_x11_window_visible(class: &str, visible: bool) -> Result<()> {
    use x11rb::protocol::xproto::ConnectionExt;
    with_x11_window(class, |connection, id| {
        if visible {
            connection.map_window(id)?.check()?;
        } else {
            connection.unmap_window(id)?.check()?;
        }
        Ok(())
    })
}

#[cfg(target_os = "linux")]
pub fn configure_x11_floating(class: &str, bounds: Bounds<gpui::Pixels>) -> Result<()> {
    use x11rb::{
        protocol::xproto::{ConfigureWindowAux, ConnectionExt, PropMode},
        wrapper::ConnectionExt as _,
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return Ok(());
    }
    with_x11_window(class, |connection, id| {
        let motif = connection
            .intern_atom(false, b"_MOTIF_WM_HINTS")?
            .reply()?
            .atom;
        connection
            .change_property32(PropMode::REPLACE, id, motif, motif, &[2, 0, 0, 0, 0])?
            .check()?;
        connection
            .configure_window(
                id,
                &ConfigureWindowAux::new()
                    .x(f32::from(bounds.origin.x).round() as i32)
                    .y(f32::from(bounds.origin.y).round() as i32)
                    .width(f32::from(bounds.size.width).round() as u32)
                    .height(f32::from(bounds.size.height).round() as u32),
            )?
            .check()?;
        Ok(())
    })
}

#[cfg(target_os = "linux")]
pub fn keep_x11_window_above(class: &str) -> Result<()> {
    use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};
    with_x11_window(class, |connection, id| {
        let root = connection.query_tree(id)?.reply()?.root;
        let state = connection
            .intern_atom(false, b"_NET_WM_STATE")?
            .reply()?
            .atom;
        let above = connection
            .intern_atom(false, b"_NET_WM_STATE_ABOVE")?
            .reply()?
            .atom;
        connection
            .send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                ClientMessageEvent::new(32, id, state, [1, above, 0, 1, 0]),
            )?
            .check()?;
        Ok(())
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn with_x11_window(
    class: &str,
    operation: impl FnOnce(&x11rb::rust_connection::RustConnection, u32) -> Result<()>,
) -> Result<()> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{AtomEnum, ConnectionExt},
    };
    let (connection, screen) = x11rb::connect(None)?;
    let pid_atom = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let mut nodes = VecDeque::from([connection.setup().roots[screen].root]);
    // GPUI 0.2.2's X11 HasWindowHandle panics. Resolve only our process's
    // explicitly named native surface instead of invoking that incomplete API.
    while let Some(id) = nodes.pop_front() {
        let class_matches = connection
            .get_property(false, id, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
            .reply()
            .ok()
            .is_some_and(|p| {
                p.value
                    .split(|b| *b == 0)
                    .any(|value| value == class.as_bytes())
            });
        let our_pid = connection
            .get_property(false, id, pid_atom, AtomEnum::CARDINAL, 0, 1)?
            .reply()
            .ok()
            .and_then(|p| p.value32().and_then(|mut v| v.next()))
            == Some(std::process::id());
        if class_matches && our_pid {
            operation(&connection, id)?;
            connection.flush()?;
            return Ok(());
        }
        if let Ok(tree) = connection.query_tree(id)?.reply() {
            nodes.extend(tree.children);
        }
    }
    bail!("native Captures surface {class:?} was not found")
}

/// Makes an X11 overlay passive and limits its visible/native shape to the
/// rectangles outside the capture hole. This avoids both input interception
/// and compositor-dependent pixels over the recording region.
#[cfg(target_os = "linux")]
pub fn configure_x11_region_indicator(
    class: &str,
    bounds: Bounds<gpui::Pixels>,
    outside: &[(i16, i16, u16, u16)],
) -> Result<()> {
    use x11rb::protocol::{
        shape::{ConnectionExt as _, SK, SO},
        xproto::Rectangle,
    };
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        bail!("passive recording-region indicators are not supported by the GPUI Wayland backend");
    }
    configure_x11_floating(class, bounds)?;
    with_x11_window(class, |connection, id| {
        let rectangles = outside
            .iter()
            .map(|&(x, y, width, height)| Rectangle {
                x,
                y,
                width,
                height,
            })
            .collect::<Vec<_>>();
        connection
            .shape_rectangles(SO::SET, SK::BOUNDING, 0.into(), id, 0, 0, &rectangles)?
            .check()?;
        // An empty input shape is click-through for pointer and keyboard input.
        connection
            .shape_rectangles(SO::SET, SK::INPUT, 0.into(), id, 0, 0, &[])?
            .check()?;
        Ok(())
    })
}

fn install_app_key_routes(cx: &mut App) {
    #[cfg(target_os = "macos")]
    cx.bind_keys([
        gpui::KeyBinding::new("cmd-,", OpenPreferences, None),
        gpui::KeyBinding::new("cmd-shift-h", OpenHistory, None),
        gpui::KeyBinding::new("cmd-shift-f", OpenFeedback, None),
        gpui::KeyBinding::new("cmd-q", QuitCaptures, None),
    ]);
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        gpui::KeyBinding::new("ctrl-,", OpenPreferences, None),
        gpui::KeyBinding::new("ctrl-shift-h", OpenHistory, None),
        gpui::KeyBinding::new("ctrl-shift-f", OpenFeedback, None),
        gpui::KeyBinding::new("ctrl-q", QuitCaptures, None),
    ]);
    cx.on_action(|_: &OpenPreferences, cx| dispatch_or_show(ShortcutAction::Preferences, cx));
    cx.on_action(|_: &OpenHistory, cx| dispatch_or_show(ShortcutAction::History, cx));
    cx.on_action(|_: &OpenFeedback, cx| dispatch_or_show(ShortcutAction::Feedback, cx));
    cx.on_action(|_: &QuitCaptures, cx| dispatch_or_show(ShortcutAction::Quit, cx));
}

fn dispatch_or_show(action: ShortcutAction, cx: &mut App) {
    if let Err(error) = dispatch(action, cx) {
        eprintln!("Could not handle app action {action:?}: {error:#}");
        show_native_error(error, cx);
    }
}

/// Reconciles OS registrations transactionally. If any new shortcut or startup
/// registration fails, the previous working native state is restored.
pub fn reconcile_settings(settings: &Settings, cx: &mut App) -> Result<()> {
    if !cx.has_global::<Integration>() {
        bail!("native integration is not installed");
    }
    cx.global_mut::<Integration>().reconcile(settings)?;
    let (completed_setup, launch) = {
        let integration = cx.global_mut::<Integration>();
        let completed_setup = !integration.onboarding_completed && settings.onboarding_completed;
        integration.onboarding_completed = settings.onboarding_completed;
        (completed_setup, integration.launch.clone())
    };
    if completed_setup {
        let shortcut = crate::notices::shortcut_tokens(&settings.new_capture_shortcut);
        cx.defer(move |cx| {
            // Preferences reconciles before saving. Wait until that synchronous
            // save (or rollback) finishes rather than announcing failed setup.
            if crate::preferences::settings::load(&launch.profile)
                .is_ok_and(|s| s.onboarding_completed)
                && let Err(error) = crate::notices::show_launch_after_setup(shortcut, launch, cx)
            {
                eprintln!("Could not show post-setup launch notice: {error:#}");
            }
        });
    }
    Ok(())
}

/// Suppresses registered shortcut dispatch while Preferences records a new key.
///
/// Registrations stay installed so reconciliation can replace them during the
/// gesture. Clearing both pressed state and the Windows takeover queue prevents
/// the captured key's release from launching an action after suppression ends.
pub fn set_shortcut_capture(active: bool, cx: &mut App) {
    if !cx.has_global::<Integration>() {
        return;
    }
    while GlobalHotKeyEvent::receiver().try_recv().is_ok() {}
    clear_win_shift_s_events();
    let integration = cx.global_mut::<Integration>();
    integration.shortcut_capture_active = active;
    integration.captured_shortcut = None;
    integration.armed.clear();
    integration.win_shift_s_armed = false;
}

/// Registered keys can be consumed by the OS before GPUI receives key events.
/// Deliver their release to the shortcut recorder without giving up registration.
pub fn take_captured_shortcut(cx: &mut App) -> Option<String> {
    cx.has_global::<Integration>()
        .then(|| cx.global_mut::<Integration>().captured_shortcut.take())
        .flatten()
}

/// Returns true only when native window protection was actually applied.
/// Linux capture does not support exclusion; macOS recordings use the shared
/// ScreenCaptureKit process filter instead of this Windows window property.
pub fn set_window_capture_excluded(window: &Window, excluded: bool) -> Result<bool> {
    #[cfg(target_os = "windows")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
        };
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|error| {
                anyhow!("could not obtain the GPUI capture-exclusion window handle: {error}")
            })?
            .as_raw();
        let RawWindowHandle::Win32(handle) = handle else {
            bail!("the GPUI window has no Win32 handle");
        };
        // The borrowed handle belongs to this live GPUI window and this process.
        if unsafe {
            SetWindowDisplayAffinity(
                handle.hwnd.get() as _,
                if excluded {
                    WDA_EXCLUDEFROMCAPTURE
                } else {
                    WDA_NONE
                },
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(excluded)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (window, excluded);
        Ok(false)
    }
}

/// Make a passive native surface transparent to desktop input. X11 uses its
/// Shape input region instead, because GPUI's X11 raw-handle accessor panics.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn set_window_mouse_passthrough(window: &Window) -> Result<()> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| {
            anyhow!("could not obtain the GPUI mouse-passthrough window handle: {error}")
        })?
        .as_raw();
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::{
            Foundation::{GetLastError, SetLastError},
            UI::WindowsAndMessaging::{
                GWL_EXSTYLE, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
                WS_EX_TRANSPARENT,
            },
        };
        let RawWindowHandle::Win32(handle) = handle else {
            bail!("the GPUI window has no Win32 handle");
        };
        // Match Tao's IGNORE_CURSOR_EVENT flags, preserving GPUI's other styles.
        // All calls run on the foreground thread with a borrowed, live HWND.
        unsafe {
            let hwnd = handle.hwnd.get() as _;
            SetLastError(0);
            let previous = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            if previous == 0 && GetLastError() != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            SetLastError(0);
            if SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                previous | (WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE) as isize,
            ) == 0
                && GetLastError() != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let RawWindowHandle::AppKit(handle) = handle else {
            bail!("the GPUI window has no AppKit handle");
        };
        // The borrowed NSView is owned by this live GPUI window. AppKit calls
        // stay on GPUI's foreground (main) thread.
        let view = unsafe { &*handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
        let window = view.window().context("the GPUI NSView has no NSWindow")?;
        window.setIgnoresMouseEvents(true);
    }
    Ok(())
}

/// Routes an action synchronously on GPUI's main thread.
pub fn dispatch(action: ShortcutAction, cx: &mut App) -> Result<()> {
    let launch = cx.global::<Integration>().launch.clone();
    dispatch_with_launch(action, launch, cx)
}

/// Returns whether this process can register system-wide shortcuts.
///
/// The upstream backend is X11-only on Linux. A Wayland session therefore
/// keeps tray/startup integration but deliberately does not claim shortcuts.
pub fn global_shortcuts_supported() -> bool {
    #[cfg(target_os = "linux")]
    return std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_some();

    #[cfg(not(target_os = "linux"))]
    true
}

impl Integration {
    fn reconcile(&mut self, settings: &Settings) -> Result<()> {
        let desired = desired_shortcuts(settings)?;
        let desired_takeover = windows_takeover(&desired);
        let ordinary = desired
            .iter()
            .filter(|registered| {
                desired_takeover
                    .as_ref()
                    .is_none_or(|takeover| takeover.hotkey.id() != registered.hotkey.id())
            })
            .cloned()
            .collect::<Vec<_>>();

        let previous = self.registered.clone();
        let previous_takeover = self.win_shift_s_action;
        let previous_launch_at_login = self.launch_at_login;
        self.replace_shortcuts(&ordinary).map_err(|error| {
            anyhow!("could not register updated shortcuts; previous shortcuts restored: {error}")
        })?;

        if let Err(error) =
            self.configure_windows_takeover(desired_takeover.map(|entry| entry.action))
        {
            let rollback = self.replace_shortcuts(&previous).err();
            let takeover_rollback = self.configure_windows_takeover(previous_takeover).err();
            return Err(match rollback {
                Some(rollback) => anyhow!(
                    "could not install Win+Shift+S takeover: {error}; shortcut rollback also failed: {rollback}"
                ),
                None if takeover_rollback.is_some() => anyhow!(
                    "could not install Win+Shift+S takeover: {error}; takeover rollback also failed: {}",
                    takeover_rollback.unwrap()
                ),
                None => anyhow!(
                    "could not install Win+Shift+S takeover; native state restored: {error}"
                ),
            });
        }
        if let Err(error) = self.set_launch_at_login(settings.launch_at_login) {
            let shortcut_rollback = self.replace_shortcuts(&previous).err();
            let takeover_rollback = self.configure_windows_takeover(previous_takeover).err();
            let startup_rollback = self.set_launch_at_login(previous_launch_at_login).err();
            let rollback_errors = [shortcut_rollback, takeover_rollback, startup_rollback]
                .into_iter()
                .flatten()
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
            if rollback_errors.is_empty() {
                return Err(anyhow!(
                    "could not update launch-at-login; native state restored: {error}"
                ));
            }
            return Err(anyhow!(
                "could not update launch-at-login: {error}; rollback also failed: {}",
                rollback_errors.join("; ")
            ));
        }
        Ok(())
    }

    fn replace_shortcuts(&mut self, desired: &[RegisteredShortcut]) -> Result<()> {
        let Some(manager) = self.shortcuts.as_ref() else {
            self.registered.clear();
            self.actions_by_id.clear();
            return Ok(());
        };
        let previous = self.registered.clone();
        let previous_keys = previous
            .iter()
            .map(|entry| entry.hotkey)
            .collect::<Vec<_>>();
        let desired_keys = desired.iter().map(|entry| entry.hotkey).collect::<Vec<_>>();

        manager
            .unregister_all(&previous_keys)
            .context("unregister previous global shortcuts")?;
        if let Err(error) = manager.register_all(&desired_keys) {
            let _ = manager.unregister_all(&desired_keys);
            manager
                .register_all(&previous_keys)
                .context("restore previous global shortcuts after conflict")?;
            return Err(anyhow!(error).context("register global shortcuts"));
        }
        self.registered = desired.to_vec();
        self.actions_by_id = desired
            .iter()
            .map(|entry| (entry.hotkey.id(), entry.action))
            .collect();
        self.armed.clear();
        Ok(())
    }

    fn set_launch_at_login(&mut self, enabled: bool) -> Result<()> {
        if enabled == self.launch_at_login
            && self.autolaunch.is_enabled().unwrap_or(false) == enabled
        {
            return Ok(());
        }
        if enabled {
            prepare_autolaunch_parent()?;
            self.autolaunch.enable().context("enable launch-at-login")?;
        } else {
            self.autolaunch
                .disable()
                .context("disable launch-at-login")?;
        }
        self.launch_at_login = enabled;
        Ok(())
    }

    fn configure_windows_takeover(&mut self, action: Option<ShortcutAction>) -> Result<()> {
        self.win_shift_s_action = action;
        self.win_shift_s_armed = false;
        set_win_shift_s_queue_enabled(action.is_some())?;
        Ok(())
    }

    fn shutdown(&mut self) {
        if !self.alive.swap(false, Ordering::AcqRel) {
            return;
        }
        if let Some(manager) = &self.shortcuts {
            let keys = self
                .registered
                .iter()
                .map(|entry| entry.hotkey)
                .collect::<Vec<_>>();
            let _ = manager.unregister_all(&keys);
        }
        self.registered.clear();
        self.actions_by_id.clear();
        self.armed.clear();
        let _ = set_win_shift_s_queue_enabled(false);
        clear_win_shift_s_events();
    }
}

fn desired_shortcuts(settings: &Settings) -> Result<Vec<RegisteredShortcut>> {
    let values = [
        (&settings.new_capture_shortcut, ShortcutAction::NewCapture),
        (
            &settings.region_shortcut,
            ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Region),
        ),
        (
            &settings.window_shortcut,
            ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Window),
        ),
        (
            &settings.display_shortcut,
            ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Display),
        ),
        (
            &settings.recording.video_shortcut,
            ShortcutAction::Capture(CaptureAction::Video, CaptureTarget::Region),
        ),
        (
            &settings.recording.window_shortcut,
            ShortcutAction::Capture(CaptureAction::Video, CaptureTarget::Window),
        ),
        (
            &settings.recording.display_shortcut,
            ShortcutAction::Capture(CaptureAction::Video, CaptureTarget::Display),
        ),
        (
            &settings.recording.gif_shortcut,
            ShortcutAction::Capture(CaptureAction::Gif, CaptureTarget::Region),
        ),
    ];
    let mut ids = HashSet::new();
    values
        .into_iter()
        .filter(|(shortcut, _)| !shortcut.trim().is_empty())
        .map(|(shortcut, action)| {
            let hotkey = shortcut
                .parse::<HotKey>()
                .with_context(|| format!("invalid shortcut {shortcut:?}"))?;
            if !ids.insert(hotkey.id()) {
                bail!("shortcut {shortcut:?} is assigned to more than one action");
            }
            Ok(RegisteredShortcut { hotkey, action })
        })
        .collect()
}

fn windows_takeover(desired: &[RegisteredShortcut]) -> Option<RegisteredShortcut> {
    #[cfg(target_os = "windows")]
    return desired
        .iter()
        .find(|entry| shortcut_is_super_shift_s(entry.hotkey))
        .cloned();

    #[cfg(not(target_os = "windows"))]
    {
        let _ = desired;
        None
    }
}

#[cfg(target_os = "windows")]
fn shortcut_is_super_shift_s(hotkey: HotKey) -> bool {
    use global_hotkey::hotkey::{Code, Modifiers};
    hotkey.key == Code::KeyS && hotkey.mods == Some(Modifiers::SUPER | Modifiers::SHIFT)
}

fn drain_native_events(cx: &mut App) {
    pump_linux_tray_events();
    let mut actions = Vec::new();
    {
        let integration = cx.global_mut::<Integration>();
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            if integration.shortcut_capture_active {
                if event.state == HotKeyState::Released {
                    integration.captured_shortcut = integration
                        .registered
                        .iter()
                        .find(|entry| entry.hotkey.id() == event.id)
                        .map(|entry| entry.hotkey.to_string());
                }
                continue;
            }
            match event.state {
                HotKeyState::Pressed => {
                    if integration.actions_by_id.contains_key(&event.id) {
                        integration.armed.insert(event.id);
                    }
                }
                HotKeyState::Released if integration.armed.remove(&event.id) => {
                    if let Some(action) = integration.actions_by_id.get(&event.id) {
                        actions.push(*action);
                    }
                }
                HotKeyState::Released => {}
            }
        }
        while let Some(phase) = pop_win_shift_s_event() {
            if integration.shortcut_capture_active {
                if matches!(phase, captures_session::WinShiftSPhase::Released) {
                    integration.captured_shortcut = Some("Super+Shift+S".into());
                }
                continue;
            }
            match phase {
                captures_session::WinShiftSPhase::Pressed => integration.win_shift_s_armed = true,
                captures_session::WinShiftSPhase::Released => {
                    if std::mem::take(&mut integration.win_shift_s_armed)
                        && let Some(action) = integration.win_shift_s_action
                    {
                        actions.push(action);
                    }
                }
            }
        }
    }
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        if let Some(action) = menu_action(event.id().as_ref()) {
            actions.push(action);
        }
    }
    while let Ok(event) = TrayIconEvent::receiver().try_recv() {
        #[cfg(target_os = "windows")]
        if matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
        ) {
            actions.push(ShortcutAction::Preferences);
        }
        #[cfg(not(target_os = "windows"))]
        let _ = event;
    }
    for action in actions {
        if let Err(error) = dispatch(action, cx) {
            eprintln!("Could not handle native action {action:?}: {error:#}");
            show_native_error(error, cx);
        }
    }
}

struct NativeError {
    message: SharedString,
    light: bool,
}

impl Render for NativeError {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::Theme::new(self.light);
        div()
            .size_full()
            .p_6()
            .flex()
            .flex_col()
            .gap_3()
            .bg(theme.raised)
            .text_color(theme.text)
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Capture could not start"),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.muted)
                    .child(self.message.clone()),
            )
    }
}

pub fn show_native_error(error: anyhow::Error, cx: &mut App) {
    let light = cx
        .try_global::<Integration>()
        .is_some_and(|integration| integration.launch.light);
    let bounds = Bounds::centered(None, size(px(460.), px(180.)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Captures error".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        move |_, cx| {
            cx.new(|_| NativeError {
                message: format!("{error:#}").into(),
                light,
            })
        },
    );
    match opened {
        Ok(handle) => {
            let _ = crate::present_window(handle.into(), cx);
        }
        Err(open_error) => eprintln!("Could not show native integration error: {open_error:#}"),
    }
}

fn dispatch_with_launch(action: ShortcutAction, launch: Launch, cx: &mut App) -> Result<()> {
    match action {
        ShortcutAction::NewCapture => {
            if crate::recording::restore_hud(cx) {
                return Ok(());
            }
            dispatch_with_launch(
                ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Region),
                launch,
                cx,
            )
        }
        ShortcutAction::Capture(action, target) => {
            if !captures_session::capture_session_available() {
                bail!("capture is unavailable while the desktop session is locked or inactive");
            }
            captures_session::dismiss_transient_shell_ui_before_capture();
            crate::recording::open_target(launch, action.as_str(), target.as_str(), cx)
        }
        ShortcutAction::Preferences => crate::open_view("preferences", launch, cx),
        ShortcutAction::History => crate::open_view("history", launch, cx),
        ShortcutAction::Feedback => crate::open_view("feedback", launch, cx),
        ShortcutAction::Quit => {
            cx.quit();
            Ok(())
        }
    }
}

fn build_tray() -> Result<TrayIcon> {
    prepare_tray_backend()?;
    let menu = Menu::new();
    let entries = [
        ("new-capture", "New Capture"),
        ("capture-region", "Screenshot Region"),
        ("capture-window", "Screenshot Window"),
        ("capture-display", "Screenshot Display"),
        ("video-region", "Record Region"),
        ("video-window", "Record Window"),
        ("video-display", "Record Display"),
        ("gif-region", "Record GIF"),
        ("history", "Capture History…"),
        ("preferences", "Preferences"),
        ("feedback", "Send Feedback…"),
    ];
    let items = entries
        .into_iter()
        .map(|(id, label)| MenuItem::with_id(id, label, true, None))
        .collect::<Vec<_>>();
    for (index, item) in items.iter().enumerate() {
        if matches!(index, 1 | 4 | 8) {
            menu.append(&PredefinedMenuItem::separator())?;
        }
        menu.append(item)?;
    }
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&MenuItem::with_id("quit", "Quit Captures", true, None))?;

    #[allow(unused_mut)]
    let mut builder = TrayIconBuilder::new()
        .with_id("captures")
        .with_tooltip("Captures")
        .with_menu(Box::new(menu))
        .with_icon(tray_icon()?);
    #[cfg(target_os = "macos")]
    {
        builder = builder.with_icon_as_template(true);
    }
    #[cfg(target_os = "windows")]
    {
        builder = builder.with_menu_on_left_click(false);
    }
    builder.build().map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn prepare_tray_backend() -> Result<()> {
    gtk::init().context("initialize GTK for the Linux AppIndicator tray")
}

#[cfg(not(target_os = "linux"))]
const fn prepare_tray_backend() -> Result<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn pump_linux_tray_events() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

#[cfg(not(target_os = "linux"))]
const fn pump_linux_tray_events() {}

fn tray_icon() -> Result<Icon> {
    const SIDE: u32 = 22;
    let source = image::load_from_memory(include_bytes!(
        "../../../apps/desktop/src-tauri/icons/icon.png"
    ))?
    .to_rgba8();
    #[allow(unused_mut)]
    let mut icon =
        image::imageops::resize(&source, SIDE, SIDE, image::imageops::FilterType::Lanczos3);
    #[cfg(target_os = "macos")]
    for pixel in icon.pixels_mut() {
        let [red, green, blue, alpha] = pixel.0;
        let minimum = red.min(green).min(blue);
        let maximum = red.max(green).max(blue);
        pixel.0 = if minimum >= 180 && maximum - minimum <= 55 {
            [255, 255, 255, alpha]
        } else {
            [0, 0, 0, 0]
        }
    }
    Icon::from_rgba(icon.into_raw(), SIDE, SIDE).map_err(Into::into)
}

fn menu_action(id: &str) -> Option<ShortcutAction> {
    Some(match id {
        "new-capture" => ShortcutAction::NewCapture,
        "capture-region" => {
            ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Region)
        }
        "capture-window" => {
            ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Window)
        }
        "capture-display" => {
            ShortcutAction::Capture(CaptureAction::Screenshot, CaptureTarget::Display)
        }
        "video-region" => ShortcutAction::Capture(CaptureAction::Video, CaptureTarget::Region),
        "video-window" => ShortcutAction::Capture(CaptureAction::Video, CaptureTarget::Window),
        "video-display" => ShortcutAction::Capture(CaptureAction::Video, CaptureTarget::Display),
        "gif-region" => ShortcutAction::Capture(CaptureAction::Gif, CaptureTarget::Region),
        "history" => ShortcutAction::History,
        "preferences" => ShortcutAction::Preferences,
        "feedback" => ShortcutAction::Feedback,
        "quit" => ShortcutAction::Quit,
        _ => return None,
    })
}

fn build_autolaunch(profile: &Path) -> Result<AutoLaunch> {
    let executable =
        std::env::current_exe().context("find current executable for launch-at-login")?;
    let mut builder = AutoLaunchBuilder::new();
    let app_name = profile_autolaunch_name(profile);
    builder
        .set_app_name(&app_name)
        .set_app_path(&executable.to_string_lossy())
        .set_use_launch_agent(true)
        .set_args(&[
            "--view",
            "background",
            "--profile",
            &profile.to_string_lossy(),
        ]);
    builder
        .build()
        .context("build launch-at-login registration")
}

fn profile_autolaunch_name(profile: &Path) -> String {
    let absolute = if profile.is_absolute() {
        profile.to_owned()
    } else {
        std::env::current_dir()
            .map(|directory| directory.join(profile))
            .unwrap_or_else(|_| profile.to_owned())
    };
    let normalized = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let mut hash = 0xcbf29ce484222325_u64;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        for byte in normalized.as_os_str().as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        for word in normalized.as_os_str().encode_wide() {
            for byte in word.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
    }
    format!("Captures GPUI {hash:016x}")
}

#[cfg(target_os = "linux")]
fn prepare_autolaunch_parent() -> Result<()> {
    let home = std::env::var_os("HOME").context("HOME is unavailable for launch-at-login")?;
    std::fs::create_dir_all(Path::new(&home).join(".config"))
        .context("create Linux configuration directory for launch-at-login")
}

#[cfg(not(target_os = "linux"))]
const fn prepare_autolaunch_parent() -> Result<()> {
    Ok(())
}

static WIN_SHIFT_S_EVENTS: OnceLock<Mutex<VecDeque<captures_session::WinShiftSPhase>>> =
    OnceLock::new();

fn on_win_shift_s(phase: captures_session::WinShiftSPhase) {
    if let Ok(mut events) = WIN_SHIFT_S_EVENTS
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
    {
        events.push_back(phase);
    }
}

fn pop_win_shift_s_event() -> Option<captures_session::WinShiftSPhase> {
    WIN_SHIFT_S_EVENTS
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
        .ok()?
        .pop_front()
}

fn clear_win_shift_s_events() {
    if let Ok(mut events) = WIN_SHIFT_S_EVENTS
        .get_or_init(|| Mutex::new(VecDeque::new()))
        .lock()
    {
        events.clear();
    }
}

fn set_win_shift_s_queue_enabled(enabled: bool) -> Result<()> {
    captures_session::set_win_shift_s_takeover_enabled(enabled);
    captures_session::set_win_shift_s_handler(enabled.then_some(on_win_shift_s));
    if enabled {
        captures_session::ensure_win_shift_s_takeover()
            .map_err(|error| anyhow!(error).context("install Win+Shift+S takeover"))?;
    }
    Ok(())
}

/// Samples the native pointer for monitor selection and screenshot cursor render.
/// Windows and Linux intentionally leave the image empty so the shared renderer
/// supplies its fallback arrow.
pub fn pointer_cursor() -> Option<captures_capture::PointerCursor> {
    pointer_position().map(|position| captures_capture::PointerCursor {
        position,
        image: native_cursor_image(),
    })
}

#[cfg(target_os = "linux")]
fn pointer_position() -> Option<(i32, i32)> {
    use x11rb::{connection::Connection, protocol::xproto::ConnectionExt};
    if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
        return None;
    }
    let (connection, screen) = x11rb::connect(None).ok()?;
    let root = connection.setup().roots.get(screen)?.root;
    let reply = connection.query_pointer(root).ok()?.reply().ok()?;
    Some((i32::from(reply.root_x), i32::from(reply.root_y)))
}

#[cfg(target_os = "windows")]
fn pointer_position() -> Option<(i32, i32)> {
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
    let mut point = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut point) } != 0).then_some((point.x, point.y))
}

#[cfg(target_os = "macos")]
fn pointer_position() -> Option<(i32, i32)> {
    use core_graphics::{
        event::CGEvent,
        event_source::{CGEventSource, CGEventSourceStateID},
    };
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?;
    let location = CGEvent::new(source).ok()?.location();
    Some((location.x.round() as i32, location.y.round() as i32))
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn native_cursor_image() -> Option<captures_capture::CursorImage> {
    use objc2_app_kit::NSCursor;
    let cursor = NSCursor::currentSystemCursor()?;
    let image = cursor.image();
    let size = image.size();
    let hot_spot = cursor.hotSpot();
    let tiff = image.TIFFRepresentation()?.to_vec();
    let pixels = image::load_from_memory(&tiff).ok()?.to_rgba8();
    (size.width > 0.0 && size.height > 0.0).then_some(captures_capture::CursorImage {
        pixels,
        logical_width: size.width,
        logical_height: size.height,
        hot_spot_x: hot_spot.x,
        hot_spot_y: hot_spot.y,
    })
}

#[cfg(not(target_os = "macos"))]
const fn native_cursor_image() -> Option<captures_capture::CursorImage> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_every_native_menu_action() {
        for id in [
            "capture-region",
            "capture-window",
            "capture-display",
            "video-region",
            "video-window",
            "video-display",
            "gif-region",
            "history",
            "preferences",
            "feedback",
            "quit",
        ] {
            assert!(menu_action(id).is_some(), "missing route for {id}");
        }
        assert_eq!(menu_action("unknown"), None);
    }

    #[test]
    fn duplicate_shortcuts_are_rejected_before_os_registration() {
        let mut settings = Settings::default();
        settings.window_shortcut = settings.region_shortcut.clone();
        let error = desired_shortcuts(&settings).unwrap_err().to_string();
        assert!(error.contains("more than one action"), "{error}");
    }

    #[test]
    fn empty_shortcuts_are_disabled() {
        let mut settings = Settings::default();
        settings.recording.gif_shortcut.clear();
        assert_eq!(desired_shortcuts(&settings).unwrap().len(), 7);
    }

    #[test]
    fn capture_routes_keep_action_and_target_distinct() {
        assert_eq!(CaptureAction::Screenshot.as_str(), "screenshot");
        assert_eq!(CaptureAction::Video.as_str(), "video");
        assert_eq!(CaptureAction::Gif.as_str(), "gif");
        assert_eq!(CaptureTarget::Region.as_str(), "region");
        assert_eq!(CaptureTarget::Window.as_str(), "window");
        assert_eq!(CaptureTarget::Display.as_str(), "display");
    }

    #[test]
    fn autolaunch_key_is_stable_profile_scoped_and_distinct_from_shipping() {
        let first = profile_autolaunch_name(Path::new("/tmp/captures-profile-a"));
        assert_eq!(
            first,
            profile_autolaunch_name(Path::new("/tmp/captures-profile-a"))
        );
        assert_ne!(
            first,
            profile_autolaunch_name(Path::new("/tmp/captures-profile-b"))
        );
        assert!(first.starts_with("Captures GPUI "));
        assert_ne!(first, "Captures");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_pointer_sample_requires_and_uses_valid_x11() {
        if std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_none() {
            assert!(pointer_cursor().is_none());
        } else {
            let cursor = pointer_cursor().expect("valid X11 display should provide a pointer");
            assert!(cursor.image.is_none());
        }
    }
}
