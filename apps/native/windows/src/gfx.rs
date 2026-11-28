//! Devices and drawing primitives: D3D11 → DXGI → Direct2D device context, the
//! DirectComposition desktop device, and DirectWrite on the system font
//! collection (Captures' `--font-sans` resolves to Segoe UI Variable Text on
//! Windows 11 and Segoe UI before it).
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Threading::INFINITE;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::tokens::{Shadow, Tokens};

/// Text roles in the Preferences, History and editor windows, each a shipping
/// CSS rule.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// `.preferences-nav-brand strong`
    Brand,
    /// `.preferences-nav nav button`
    Nav,
    /// `.preferences-header h1`
    Title,
    /// `.preferences-header > div > p`
    Subtitle,
    /// `.preferences-history-button`
    Button,
    /// `.settings-card-header h2`
    CardTitle,
    /// `.settings-card-header p`
    CardDescription,
    /// `.check-row.switch-row > span`
    RowTitle,
    /// `.check-row small`
    RowDescription,
    /// `.ui-segmented button`
    Segment,
    /// `.theme-option-copy strong`
    Chip,
    /// `.theme-option-check`
    Check,
    /// `.directory-input input`: `--font-mono`
    Mono,
    /// `.directory-input button` (`.ui-btn`)
    Control,
    /// `.custom-select-trigger > span`, `.custom-select-option-copy > span`
    /// and `.shortcut-prompt`.
    Option,
    /// `.shortcut-error`
    Small,
    /// `kbd`
    Kbd,
    /// `.preferences-find-input` (`--text-md`, `--leading-snug`)
    Input,
    /// `.preferences-save-status`
    Status,
    /// `.custom-theme-color-control input[type="text"]`: mono `--text-xs`.
    Hex,
    /// `.custom-theme-field > small` and `.history-card-error`
    Note,
    /// `.history-heading h1`
    Heading,
    /// `.history-heading > p:last-child`
    Lede,
    /// `.history-card-body time`
    Date,
    /// `.history-error`
    Error,
    /// `.history-empty p`
    Body,
    /// `.history-edit` (`.ui-btn-primary`: semibold)
    Strong,
    /// `.screenshot-export-folder` copy and buttons (`--text-2xs`, medium).
    Tiny,
    /// `.screenshot-export-folder code` (mono `--text-2xs`).
    MonoTiny,
    /// Canvas W/H values and the zoom preset (mono `--text-xs`).
    MonoSmall,
    /// `.screenshot-text-style-preview`: 17 px bold, tighter, `line-height: 1`.
    Preview,
    /// `.screenshot-text-style-preview.style-mono`: mono 15 px semibold.
    PreviewMono,
    /// `.screenshot-format-buttons em`: the Italic button's "I".
    Italic,
    /// `.recording-editor-header h1`: `--text-2xl`, semibold, tighter.
    RecordingTitle,
    /// `.recording-start`: `--text-md`, semibold.
    Action,
    /// `.capture-selector-note strong`: `--text-xs`, bold.
    NoteStrong,
    /// `.onboarding-lede`: `--text-md`, `--leading-normal`, wrapping.
    SetupLede,
    /// `.feedback-review-summary`: wrapping mono `--text-sm`.
    MonoBlock,
}

const STYLES: [Style; 38] = [
    Style::MonoBlock,
    Style::SetupLede,
    Style::NoteStrong,
    Style::Action,
    Style::RecordingTitle,
    Style::Brand,
    Style::Nav,
    Style::Title,
    Style::Subtitle,
    Style::Button,
    Style::CardTitle,
    Style::CardDescription,
    Style::RowTitle,
    Style::RowDescription,
    Style::Segment,
    Style::Chip,
    Style::Check,
    Style::Mono,
    Style::Control,
    Style::Option,
    Style::Small,
    Style::Kbd,
    Style::Input,
    Style::Status,
    Style::Hex,
    Style::Note,
    Style::Heading,
    Style::Lede,
    Style::Date,
    Style::Error,
    Style::Body,
    Style::Strong,
    Style::Tiny,
    Style::MonoTiny,
    Style::MonoSmall,
    Style::Preview,
    Style::PreviewMono,
    Style::Italic,
];

struct Spec {
    size: &'static str,
    weight: &'static str,
    tracking: Option<&'static str>,
    leading: Option<&'static str>,
    /// A literal CSS `line-height` multiplier when there is no token.
    line: Option<f32>,
    /// A literal CSS `letter-spacing` in em when there is no token.
    letter: Option<f32>,
    wrap: bool,
    mono: bool,
    /// A literal CSS `font-size` in px when there is no token.
    px: Option<f32>,
    italic: bool,
}

impl Spec {
    fn size(&self, t: &Tokens) -> f32 {
        self.px.unwrap_or_else(|| t.n(self.size))
    }
}

fn spec(style: Style) -> Spec {
    let spec = |size, weight, tracking, leading, wrap| Spec {
        size,
        weight,
        tracking,
        leading,
        line: None,
        letter: None,
        wrap,
        mono: false,
        px: None,
        italic: false,
    };
    match style {
        Style::Brand => spec(
            "text-md",
            "weight-semibold",
            Some("tracking-tight"),
            None,
            false,
        ),
        Style::Nav => spec("text-md", "weight-medium", None, None, false),
        Style::Title => spec(
            "text-xl",
            "weight-semibold",
            Some("tracking-tighter"),
            Some("leading-tight"),
            false,
        ),
        Style::Subtitle => spec("text-sm", "weight-regular", None, None, false),
        Style::Button => spec("text-sm", "weight-medium", None, None, false),
        Style::CardTitle => spec(
            "text-lg",
            "weight-semibold",
            Some("tracking-tight"),
            Some("leading-tight"),
            true,
        ),
        Style::CardDescription | Style::RowDescription => spec(
            "text-sm",
            "weight-regular",
            None,
            Some("leading-snug"),
            true,
        ),
        Style::RowTitle => spec("text-md", "weight-medium", None, None, true),
        Style::Segment | Style::Chip => spec(
            "text-sm",
            "weight-medium",
            None,
            Some("leading-snug"),
            false,
        ),
        Style::Check => spec("text-2xs", "weight-bold", None, None, false),
        Style::Mono => Spec {
            mono: true,
            ..spec(
                "text-sm",
                "weight-regular",
                None,
                Some("leading-snug"),
                false,
            )
        },
        Style::Control => spec(
            "text-sm",
            "weight-medium",
            Some("tracking-tight"),
            Some("leading-snug"),
            false,
        ),
        Style::Option => spec(
            "text-sm",
            "weight-regular",
            None,
            Some("leading-snug"),
            false,
        ),
        Style::Small => spec("text-xs", "weight-regular", None, None, true),
        Style::Hex => Spec {
            mono: true,
            letter: Some(0.02),
            ..spec(
                "text-xs",
                "weight-regular",
                None,
                Some("leading-snug"),
                false,
            )
        },
        Style::Note => spec(
            "text-xs",
            "weight-regular",
            None,
            Some("leading-snug"),
            true,
        ),
        Style::Status => spec(
            "text-xs",
            "weight-medium",
            None,
            Some("leading-snug"),
            false,
        ),
        Style::Input => spec(
            "text-md",
            "weight-regular",
            None,
            Some("leading-snug"),
            false,
        ),
        Style::Heading => spec(
            "text-2xl",
            "weight-semibold",
            Some("tracking-tighter"),
            Some("leading-tight"),
            false,
        ),
        Style::Lede => spec(
            "text-md",
            "weight-regular",
            None,
            Some("leading-snug"),
            true,
        ),
        Style::Date => spec(
            "text-md",
            "weight-medium",
            None,
            Some("leading-snug"),
            false,
        ),
        Style::Error => spec("text-sm", "weight-regular", None, None, true),
        Style::Body => spec("text-md", "weight-regular", None, None, false),
        Style::SetupLede => spec(
            "text-md",
            "weight-regular",
            None,
            Some("leading-normal"),
            true,
        ),
        Style::Strong => spec(
            "text-sm",
            "weight-semibold",
            Some("tracking-tight"),
            Some("leading-snug"),
            false,
        ),
        Style::Tiny => spec(
            "text-2xs",
            "weight-medium",
            None,
            Some("leading-snug"),
            false,
        ),
        Style::MonoTiny => Spec {
            mono: true,
            ..spec(
                "text-2xs",
                "weight-regular",
                None,
                Some("leading-snug"),
                false,
            )
        },
        Style::MonoBlock => Spec {
            mono: true,
            ..spec(
                "text-sm",
                "weight-regular",
                None,
                Some("leading-snug"),
                true,
            )
        },
        Style::MonoSmall => Spec {
            mono: true,
            ..spec(
                "text-xs",
                "weight-regular",
                None,
                Some("leading-snug"),
                false,
            )
        },
        Style::Preview => Spec {
            px: Some(17.),
            line: Some(1.),
            ..spec(
                "text-md",
                "weight-bold",
                Some("tracking-tighter"),
                None,
                false,
            )
        },
        Style::PreviewMono => Spec {
            px: Some(15.),
            line: Some(1.),
            mono: true,
            ..spec("text-md", "weight-semibold", None, None, false)
        },
        Style::RecordingTitle => spec(
            "text-xl",
            "weight-semibold",
            Some("tracking-tighter"),
            Some("leading-tight"),
            false,
        ),
        Style::Action => spec("text-md", "weight-semibold", None, None, false),
        Style::NoteStrong => spec("text-xs", "weight-bold", None, Some("leading-snug"), false),
        Style::Italic => Spec {
            italic: true,
            ..spec("text-sm", "weight-semibold", None, None, false)
        },
        Style::Kbd => Spec {
            line: Some(1.3),
            ..spec("text-2xs", "weight-semibold", None, None, false)
        },
    }
}

#[derive(Clone)]
pub struct Text {
    pub layout: IDWriteTextLayout,
    pub width: f32,
    pub height: f32,
    /// The laid-out string, for selection and copy.
    pub units: Vec<u16>,
}

#[derive(Clone)]
pub struct Gfx {
    /// The device under `dc` and `dcomp`, for [`Gfx::lost`].
    d3d: ID3D11Device,
    /// The development hook marked this device lost (`lose_shared`).
    simulated_loss: Rc<Cell<bool>>,
    pub dc: ID2D1DeviceContext,
    pub factory: ID2D1Factory1,
    pub dcomp: Option<IDCompositionDesktopDevice>,
    dwrite: IDWriteFactory,
    /// Bundled cross-platform UI faces (`Figtree` + `JetBrains Mono`).
    ui_fonts: Option<IDWriteFontCollection>,
    formats: Vec<(Style, IDWriteTextFormat)>,
    /// The UI family, for sizes no fixed [`Style`] covers.
    family: HSTRING,
    axes: Option<IDWriteFactory6>,
    /// Ascent and descent per em of the UI face, for CSS `line-height`.
    ascent: f32,
    descent: f32,
}

const FAMILIES: [&str; 2] = ["Segoe UI Variable Text", "Segoe UI"];
/// The Windows entries of shipping `--font-mono`, then a last resort.
const MONO_FAMILIES: [&str; 4] = ["Cascadia Mono", "Segoe UI Mono", "Consolas", "Courier New"];

thread_local! {
    static UI_FONTS: RefCell<Option<Option<IDWriteFontCollection>>> = const { RefCell::new(None) };
    /// This thread's composition renderer, created once (see [`Gfx::shared`]).
    static SHARED: RefCell<Option<Gfx>> = const { RefCell::new(None) };
    /// A cold capture's WARP renderer (see [`Gfx::overlay`]), dropped once
    /// its overlay lets go of it.
    static WARP: RefCell<Option<Gfx>> = const { RefCell::new(None) };
    /// [`Gfx::shared`] replaced a lost device that open windows may still
    /// draw with, until [`renderer_recovered`].
    static REPLACED_LOST: Cell<bool> = const { Cell::new(false) };
}

/// The shared device was removed or reset (driver update, GPU reset, a
/// remote session), so windows created on it no longer draw.
pub fn renderer_lost() -> bool {
    REPLACED_LOST.get() || SHARED.with(|s| s.borrow().as_ref().is_some_and(Gfx::lost))
}

/// Open windows were closed after [`renderer_lost`]: drop a lost device so
/// the next [`Gfx::shared`] starts a new one.
pub fn renderer_recovered() {
    REPLACED_LOST.set(false);
    SHARED.with(|s| {
        let mut s = s.borrow_mut();
        if s.as_ref().is_some_and(Gfx::lost) {
            *s = None;
            set_alive(None);
        }
    });
}

/// How long the resident app keeps the shared device once no window uses it.
/// While any D3D11 device exists, one NVIDIA driver thread wakes ~66 times a
/// second (15/s without a device, 83/s with a bare one, measured; device
/// flags, a held DXGI adapter, D3D12 and leaving out composition change
/// nothing), so an idle tray releases it. Recreating it costs ~115 ms, more
/// than the capture's ~60 ms screen grab, so a capture's first overlay draws
/// on WARP ([`prewarm_overlay`]) and moves to it once it is ready.
const IDLE_RELEASE: Duration = Duration::from_secs(60);

/// A D3D11 device is free-threaded (it is never created single-threaded
/// here), so a worker can create it for the UI thread.
struct SendDevice(ID3D11Device);
// SAFETY: see above; the worker never touches the immediate context.
unsafe impl Send for SendDevice {}

/// A device a worker is creating for the next [`Gfx::shared`], and when it
/// started. Any thread may start one: the tray's handlers run on whichever
/// thread delivers the event.
static PREWARM: Mutex<Option<(Receiver<Result<SendDevice>>, Instant)>> = Mutex::new(None);
/// The UI thread holds a shared renderer, for [`prewarm`] on other threads.
static RENDERER_ALIVE: AtomicBool = AtomicBool::new(false);

/// The UI thread's shared renderer changed. Window grabs on the capture
/// worker borrow its device (`captures_capture::share_device`) rather than
/// create a second one, which took 18-33 ms next to it; they copy and map
/// on its immediate context, so that context takes Direct3D's lock.
fn set_alive(g: Option<&Gfx>) {
    let lent = g.and_then(|g| unsafe {
        let multithread = g.d3d.cast::<ID3D11Multithread>().ok()?;
        let _ = multithread.SetMultithreadProtected(true);
        Some(g.d3d.clone())
    });
    captures_capture::share_device(lent);
    RENDERER_ALIVE.store(g.is_some(), Ordering::Release);
}

/// A capture or a window is about to need the shared device: start creating
/// it on a worker so its ~115 ms overlaps the work before the first frame.
pub fn prewarm() {
    if RENDERER_ALIVE.load(Ordering::Acquire) {
        return;
    }
    let mut pending = PREWARM.lock().unwrap_or_else(PoisonError::into_inner);
    if pending.is_some() {
        return;
    }
    let (sender, receiver) = channel();
    let spawned = std::thread::Builder::new()
        .name("captures-gfx-prewarm".into())
        .spawn(move || {
            let _ = sender.send(unsafe { create_d3d(HARDWARE_FIRST) }.map(SendDevice));
        });
    if spawned.is_ok() {
        *pending = Some((receiver, Instant::now()));
    }
}

/// A WARP device a worker is creating for a cold capture's overlay, and when
/// it started (see [`prewarm_overlay`]).
static WARP_PREWARM: Mutex<Option<(Receiver<Result<SendDevice>>, Instant)>> = Mutex::new(None);

/// A capture shortcut or tray row: its overlay follows a ~60 ms screen grab.
/// When an idle tray released the device, a hardware one takes ~110 ms and
/// its creation is serialized with any other device's (a WARP device made
/// alongside it waited for it), so create only a WARP device now, in a few
/// milliseconds, for the overlay to draw on; the overlay starts the hardware
/// one with [`prewarm`] once it is on screen. A tray row's hardware device,
/// started when the icon was pressed, is used only once it is ready; until
/// then the WARP one at worst finishes with it.
pub fn prewarm_overlay() {
    if RENDERER_ALIVE.load(Ordering::Acquire)
        || prewarmed_ready() == Some(true)
        || WARP.with(|w| w.borrow().is_some())
    {
        return;
    }
    let mut pending = WARP_PREWARM.lock().unwrap_or_else(PoisonError::into_inner);
    if pending.is_some() {
        return;
    }
    let (sender, receiver) = channel();
    let spawned = std::thread::Builder::new()
        .name("captures-gfx-prewarm".into())
        .spawn(move || {
            let _ = sender.send(unsafe { create_d3d(&[D3D_DRIVER_TYPE_WARP]) }.map(SendDevice));
        });
    if spawned.is_ok() {
        *pending = Some((receiver, Instant::now()));
    }
}

/// Whether the device [`prewarm`] is creating is ready; `None` when none is.
fn prewarmed_ready() -> Option<bool> {
    let mut pending = PREWARM.lock().unwrap_or_else(PoisonError::into_inner);
    let (receiver, _) = pending.as_mut()?;
    Some(match receiver.try_recv() {
        Err(TryRecvError::Empty) => false,
        Err(TryRecvError::Disconnected) => true,
        // Put it back for `take_prewarmed`.
        Ok(device) => {
            let (sender, again) = channel();
            let _ = sender.send(device);
            *receiver = again;
            true
        }
    })
}

fn take_prewarmed() -> Option<ID3D11Device> {
    let (receiver, _) = PREWARM
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()?;
    receiver.recv().ok()?.ok().map(|device| device.0)
}

/// Once no window uses the shared device, free what closed windows left on
/// it at once, and release it (or an unused prewarmed one) after
/// [`IDLE_RELEASE`]. Returns how long the host loop may wait before calling
/// again.
pub fn release_idle() -> u32 {
    thread_local! {
        static UNUSED_SINCE: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    WARP.with(|w| {
        let mut w = w.borrow_mut();
        if w.as_ref()
            .is_some_and(|g| Rc::strong_count(&g.simulated_loss) == 1)
            && let Some(g) = w.take()
        {
            g.flush();
        }
    });
    let now = Instant::now();
    let prewarmed = PREWARM
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, started)| *started)
        .or_else(|| {
            WARP_PREWARM
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .map(|(_, started)| *started)
        });
    let since = SHARED.with(|s| match s.borrow().as_ref() {
        // Windows hold clones; only this cache holding it means unused.
        Some(g) if Rc::strong_count(&g.simulated_loss) > 1 => {
            UNUSED_SINCE.set(None);
            None
        }
        Some(g) => Some(UNUSED_SINCE.get().unwrap_or_else(|| {
            g.flush();
            UNUSED_SINCE.set(Some(now));
            now
        })),
        None => {
            UNUSED_SINCE.set(None);
            prewarmed
        }
    });
    let Some(since) = since else {
        return INFINITE;
    };
    let idle = now.saturating_duration_since(since);
    if idle < IDLE_RELEASE {
        return u32::try_from((IDLE_RELEASE - idle).as_millis()).map_or(INFINITE - 1, |ms| ms + 1);
    }
    PREWARM
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    WARP_PREWARM
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(g) = SHARED.with(|s| s.borrow_mut().take()) {
        g.flush();
    }
    set_alive(None);
    UNUSED_SINCE.set(None);
    INFINITE
}

/// Development test hook: treat the shared device as lost, as a GPU reset
/// would, without disturbing the real device.
pub fn lose_shared() {
    SHARED.with(|s| {
        if let Some(g) = s.borrow().as_ref() {
            g.simulated_loss.set(true);
        }
    });
}

fn ui_font_collection(dwrite: &IDWriteFactory) -> Option<IDWriteFontCollection> {
    UI_FONTS.with(|cell| {
        cell.borrow_mut()
            .get_or_insert_with(|| load_ui_fonts(dwrite).ok())
            .clone()
    })
}

fn load_ui_fonts(dwrite: &IDWriteFactory) -> Result<IDWriteFontCollection> {
    unsafe {
        let factory: IDWriteFactory5 = dwrite.cast()?;
        let loader = factory.CreateInMemoryFontFileLoader()?;
        factory.RegisterFontFileLoader(&loader)?;
        let builder: IDWriteFontSetBuilder1 = factory.CreateFontSetBuilder()?.cast()?;
        for (_, bytes) in captures_app::ui_fonts::files() {
            let file = loader.CreateInMemoryFontFileReference(
                dwrite,
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                None::<&IUnknown>,
            )?;
            builder.AddFontFile(&file)?;
        }
        let set = builder.CreateFontSet()?;
        factory.CreateFontCollectionFromFontSet(&set)?.cast()
    }
}

impl Gfx {
    /// The UI thread's one composition renderer, shared by every window.
    /// A device carries about 54 NVIDIA driver threads once a surface is
    /// committed, and a closed window does not release its device: its
    /// composition objects stay referenced until that device's next commit
    /// and its Direct2D resources stay cached on it. A device per window
    /// therefore grew the resident app by ~70 MB private and ~170 MB GPU
    /// memory per capture. The driver itself frees all of it once every
    /// reference is gone.
    /// Text formats depend only on token numbers, which every theme and
    /// appearance shares, so the first window's `tokens` serve them all.
    /// A lost device is replaced here; [`renderer_lost`] stays true so the
    /// resident app reopens the windows still on the old one.
    pub fn shared(tokens: &Tokens) -> Result<Self> {
        SHARED.with(|shared| {
            if let Some(g) = shared.borrow().as_ref() {
                if !g.lost() {
                    return Ok(g.clone());
                }
                REPLACED_LOST.set(true);
            }
            let g = Self::create(tokens, true, take_prewarmed())?;
            *shared.borrow_mut() = Some(g.clone());
            set_alive(Some(&g));
            Ok(g)
        })
    }

    /// The capture selector's renderer: the shared one, or after an idle
    /// release the WARP device [`prewarm_overlay`] made alongside the grab,
    /// until [`Gfx::hardware_ready`] has the hardware one.
    pub fn overlay(tokens: &Tokens) -> Result<Self> {
        let shared = SHARED.with(|s| s.borrow().as_ref().filter(|g| !g.lost()).cloned());
        if let Some(g) = shared.or_else(|| WARP.with(|w| w.borrow().clone())) {
            return Ok(g);
        }
        let warp = WARP_PREWARM
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if prewarmed_ready() == Some(true) {
            return Self::shared(tokens);
        }
        let warp = warp.and_then(|(receiver, _)| receiver.recv().ok()?.ok());
        if let Some(d3d) = warp
            && let Ok(g) = Self::create(tokens, true, Some(d3d.0))
        {
            WARP.with(|w| *w.borrow_mut() = Some(g.clone()));
            return Ok(g);
        }
        Self::shared(tokens)
    }

    /// This is a cold capture's WARP renderer ([`Gfx::overlay`]).
    pub fn on_warp(&self) -> bool {
        WARP.with(|w| {
            w.borrow()
                .as_ref()
                .is_some_and(|w| Rc::ptr_eq(&w.simulated_loss, &self.simulated_loss))
        })
    }

    /// The shared hardware renderer for an overlay on WARP to move to, once
    /// the device [`prewarm`] is creating is ready; `None` until then. WARP
    /// drew a 4K selection drag on the CPU at ~30 times the hardware cost.
    pub fn hardware_ready(tokens: &Tokens) -> Option<Self> {
        if let Some(g) = SHARED.with(|s| s.borrow().as_ref().filter(|g| !g.lost()).cloned()) {
            return Some(g);
        }
        let mut pending = PREWARM.lock().unwrap_or_else(PoisonError::into_inner);
        let device = match pending.as_ref().map(|(receiver, _)| receiver.try_recv()) {
            None => {
                drop(pending);
                prewarm();
                return None;
            }
            Some(Err(TryRecvError::Empty)) => return None,
            Some(Err(TryRecvError::Disconnected)) => None,
            Some(Ok(device)) => device.ok().map(|device| device.0),
        };
        pending.take();
        drop(pending);
        let g = Self::create(tokens, true, device).ok()?;
        SHARED.with(|s| *s.borrow_mut() = Some(g.clone()));
        set_alive(Some(&g));
        Some(g)
    }

    /// Free what closed windows left on the device: their composition
    /// objects are released only by the device's next commit, and Direct3D
    /// destroys released resources lazily until a flush. Dropping the last
    /// renderer after this lets the driver free the device itself (without
    /// it, the device outlived the cache and kept waking). Direct2D's own
    /// caches stay until then: clearing them (`ClearResources`, `Trim`)
    /// freed ~95 MB private and ~64 MB GPU a minute early but made each
    /// following capture's overlay ~10 ms slower.
    fn flush(&self) {
        unsafe {
            if let Some(dcomp) = &self.dcomp {
                let _ = dcomp.Commit();
                let _ = dcomp.WaitForCommitCompletion();
            }
            self.dc.SetTarget(None);
            if let Ok(context) = self.d3d.GetImmediateContext() {
                context.ClearState();
                context.Flush();
            }
        }
    }

    /// The device was removed or reset and must be recreated.
    pub fn lost(&self) -> bool {
        self.simulated_loss.get() || unsafe { self.d3d.GetDeviceRemovedReason() }.is_err()
    }

    /// `composition` is false for the offscreen `--screenshot` renderer.
    pub fn new(tokens: &Tokens, composition: bool) -> Result<Self> {
        Self::create(tokens, composition, None)
    }

    fn create(tokens: &Tokens, composition: bool, d3d: Option<ID3D11Device>) -> Result<Self> {
        unsafe {
            let d3d = match d3d {
                Some(d3d) => d3d,
                None => create_d3d(HARDWARE_FIRST)?,
            };
            let dxgi: IDXGIDevice = d3d.cast()?;
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d = factory.CreateDevice(&dxgi)?;
            let dc = d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let dcomp = if composition {
                Some(DCompositionCreateDevice2(&d2d)?)
            } else {
                None
            };
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let ui_fonts = ui_font_collection(&dwrite);
            let mut system_collection = None;
            dwrite.GetSystemFontCollection(&mut system_collection, false)?;
            let system_collection = system_collection.ok_or_else(Error::empty)?;
            let find_in = |collection: &IDWriteFontCollection, families: &[&'static str]| {
                families.iter().find_map(|family| {
                    let (mut index, mut exists) = (0, BOOL(0));
                    collection
                        .FindFamilyName(&HSTRING::from(*family), &mut index, &mut exists)
                        .ok()?;
                    exists.as_bool().then_some((*family, index))
                })
            };
            let (collection, family, index, mono, using_bundled) = if let Some(bundled) =
                ui_fonts.as_ref()
                && let Some((sans, idx)) =
                    find_in(bundled, &[captures_app::ui_fonts::UI_SANS_FAMILY])
                && let Some((mono, _)) = find_in(bundled, &[captures_app::ui_fonts::UI_MONO_FAMILY])
            {
                (bundled, sans, idx, mono, true)
            } else {
                let (family, index) =
                    find_in(&system_collection, &FAMILIES).unwrap_or((FAMILIES[1], 0));
                let mono = find_in(&system_collection, &MONO_FAMILIES)
                    .map_or(MONO_FAMILIES[2], |(family, _)| family);
                (&system_collection, family, index, mono, false)
            };
            let mut metrics = DWRITE_FONT_METRICS::default();
            if let Ok(font) = collection.GetFontFamily(index).and_then(|f| {
                f.GetFirstMatchingFont(
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                )
            }) {
                font.GetMetrics(&mut metrics);
            }
            let em = f32::from(metrics.designUnitsPerEm.max(1));
            let (ascent, descent) = if metrics.designUnitsPerEm == 0 {
                (1.07, 0.26)
            } else {
                (
                    f32::from(metrics.ascent) / em,
                    f32::from(metrics.descent) / em,
                )
            };
            // Windows 10 1809+: weights as `wght` axis values for variable
            // system faces; bundled Figtree/JetBrains Mono ship static weights.
            let axes: Option<IDWriteFactory6> = if using_bundled {
                None
            } else {
                dwrite.cast().ok()
            };
            let ui_collection = using_bundled.then(|| ui_fonts.clone()).flatten();
            let mut formats = Vec::new();
            for style in STYLES {
                let spec = spec(style);
                let (size, weight) = (spec.size(tokens), tokens.n(spec.weight));
                let family = &HSTRING::from(if spec.mono { mono } else { family });
                // Segoe UI Variable has no italic axis: oblique like Chromium.
                let axes = axes.as_ref().filter(|_| !spec.italic);
                let format: IDWriteTextFormat = match axes.and_then(|f| {
                    f.CreateTextFormat(
                        family,
                        None,
                        &[DWRITE_FONT_AXIS_VALUE {
                            axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
                            value: weight,
                        }],
                        size,
                        w!("en-us"),
                    )
                    .ok()
                }) {
                    Some(format) => format.cast()?,
                    None => dwrite.CreateTextFormat(
                        family,
                        ui_collection.as_ref(),
                        DWRITE_FONT_WEIGHT(weight as i32),
                        if spec.italic {
                            DWRITE_FONT_STYLE_ITALIC
                        } else {
                            DWRITE_FONT_STYLE_NORMAL
                        },
                        DWRITE_FONT_STRETCH_NORMAL,
                        size,
                        w!("en-us"),
                    )?,
                };
                format.SetWordWrapping(if spec.wrap {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })?;
                formats.push((style, format));
            }
            Ok(Self {
                d3d,
                simulated_loss: Rc::default(),
                dc,
                factory,
                dcomp,
                dwrite,
                ui_fonts: ui_collection,
                formats,
                family: HSTRING::from(family),
                axes,
                ascent,
                descent,
            })
        }
    }

    /// Lay out `text` as `style`, wrapping at `max_width` when the style wraps.
    pub fn text(&self, t: &Tokens, style: Style, text: &str, max_width: f32) -> Result<Text> {
        let spec = spec(style);
        let size = spec.size(t);
        let wide: Vec<u16> = text.encode_utf16().collect();
        let format = &self
            .formats
            .iter()
            .find(|(s, _)| *s == style)
            .expect("format")
            .1;
        unsafe {
            let layout =
                self.dwrite
                    .CreateTextLayout(&wide, format, max_width.max(1.), 100_000.)?;
            let letter = spec
                .tracking
                .map(|tracking| t.em(tracking, size))
                .or(spec.letter.map(|em| em * size));
            if let Some(letter) = letter {
                // CSS `letter-spacing` adds after every character.
                let layout1: IDWriteTextLayout1 = layout.cast()?;
                layout1.SetCharacterSpacing(
                    0.,
                    letter,
                    0.,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: wide.len() as u32,
                    },
                )?;
            }
            if let Some(multiplier) = spec.leading.map(|leading| t.n(leading)).or(spec.line) {
                // CSS line boxes split the leading evenly above and below.
                let line = multiplier * size;
                let content = (self.ascent + self.descent) * size;
                layout.SetLineSpacing(
                    DWRITE_LINE_SPACING_METHOD_UNIFORM,
                    line,
                    (line - content) / 2. + self.ascent * size,
                )?;
            }
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(Text {
                layout,
                width: metrics.width,
                height: metrics.height,
                units: wide,
            })
        }
    }

    /// One unwrapped line of the UI face at a computed `size` (CSS `clamp()`
    /// sizes such as the countdown), `weight` and optional `tracking` token.
    pub fn text_sized(
        &self,
        t: &Tokens,
        text: &str,
        size: f32,
        weight: &str,
        tracking: Option<&str>,
    ) -> Result<Text> {
        let weight = t.n(weight);
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let format: IDWriteTextFormat = match self.axes.as_ref().and_then(|f| {
                f.CreateTextFormat(
                    &self.family,
                    None,
                    &[DWRITE_FONT_AXIS_VALUE {
                        axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT,
                        value: weight,
                    }],
                    size,
                    w!("en-us"),
                )
                .ok()
            }) {
                Some(format) => format.cast()?,
                None => self.dwrite.CreateTextFormat(
                    &self.family,
                    self.ui_fonts.as_ref(),
                    DWRITE_FONT_WEIGHT(weight as i32),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )?,
            };
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let layout = self
                .dwrite
                .CreateTextLayout(&wide, &format, 100_000., 100_000.)?;
            if let Some(tracking) = tracking {
                let layout1: IDWriteTextLayout1 = layout.cast()?;
                layout1.SetCharacterSpacing(
                    0.,
                    t.em(tracking, size),
                    0.,
                    DWRITE_TEXT_RANGE {
                        startPosition: 0,
                        length: wide.len() as u32,
                    },
                )?;
            }
            let mut metrics = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut metrics)?;
            Ok(Text {
                layout,
                width: metrics.width,
                height: metrics.height,
                units: wide,
            })
        }
    }

    /// Clip one-line `text` at `width` with a trailing ellipsis (CSS
    /// `text-overflow: ellipsis`).
    pub fn ellipsis(&self, text: &Text, width: f32) -> Result<()> {
        unsafe {
            text.layout.SetMaxWidth(width.max(1.))?;
            let sign = self.dwrite.CreateEllipsisTrimmingSign(&text.layout)?;
            text.layout.SetTrimming(
                &DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                },
                &sign,
            )
        }
    }

    /// The DirectWrite factory, for text outside the token styles (the
    /// editor's inline text in the document's faces).
    pub fn dwrite(&self) -> &IDWriteFactory {
        &self.dwrite
    }

    /// The advance of `0` in `style`, for CSS `ch` units.
    pub fn ch(&self, t: &Tokens, style: Style) -> Result<f32> {
        let zeros = self.text(t, style, "0000000000", 100_000.)?;
        Ok(zeros.width / 10.)
    }

    pub fn surface(&self, w: u32, h: u32) -> Result<IDCompositionSurface> {
        let dcomp = self.dcomp.as_ref().ok_or_else(Error::empty)?;
        unsafe {
            dcomp.CreateSurface(
                w.max(1),
                h.max(1),
                DXGI_FORMAT_B8G8R8A8_UNORM,
                DXGI_ALPHA_MODE_PREMULTIPLIED,
            )
        }
    }
}

/// Hardware first; WARP keeps headless CI runners and RDP sessions working.
const HARDWARE_FIRST: &[D3D_DRIVER_TYPE] = &[D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP];

unsafe fn create_d3d(drivers: &[D3D_DRIVER_TYPE]) -> Result<ID3D11Device> {
    let mut last = Error::empty();
    for &driver in drivers {
        let mut device = None;
        let result = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[
                    D3D_FEATURE_LEVEL_11_1,
                    D3D_FEATURE_LEVEL_11_0,
                    D3D_FEATURE_LEVEL_10_1,
                ]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                None,
            )
        };
        match (result, device) {
            (Ok(()), Some(device)) => return Ok(device),
            (Err(error), _) => last = error,
            _ => {}
        }
    }
    Err(last)
}

/// Draw a whole composition surface of `size` pixels. The closure works in
/// DIPs with its origin `bleed_px` pixels inside the surface's top-left corner;
/// `scale` maps DIPs to pixels per axis so the node's rect lands exactly on
/// its device-pixel-snapped box.
pub fn draw_surface(
    surface: &IDCompositionSurface,
    size: (u32, u32),
    scale: (f32, f32),
    bleed_px: f32,
    f: impl FnOnce(&ID2D1DeviceContext) -> Result<()>,
) -> Result<()> {
    unsafe {
        let mut offset = POINT::default();
        let dc: ID2D1DeviceContext = surface.BeginDraw(None, &mut offset)?;
        dc.SetDpi(96., 96.);
        dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        // Atlased surfaces share a texture: clip to ours before clearing.
        dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
        dc.PushAxisAlignedClip(
            &rect(0., 0., size.0 as f32, size.1 as f32),
            D2D1_ANTIALIAS_MODE_ALIASED,
        );
        dc.Clear(Some(&D2D1_COLOR_F::default()));
        dc.SetTransform(&Matrix3x2 {
            M11: scale.0,
            M12: 0.,
            M21: 0.,
            M22: scale.1,
            M31: offset.x as f32 + bleed_px,
            M32: offset.y as f32 + bleed_px,
        });
        let result = f(&dc);
        dc.PopAxisAlignedClip();
        surface.EndDraw()?;
        result
    }
}

pub fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

pub fn brush(dc: &ID2D1DeviceContext, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
    unsafe { dc.CreateSolidColorBrush(&color, None) }
}

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
    }
}

pub fn fill(dc: &ID2D1DeviceContext, r: D2D_RECT_F, color: D2D1_COLOR_F) -> Result<()> {
    unsafe { dc.FillRectangle(&r, &brush(dc, color)?) };
    Ok(())
}

pub fn fill_round(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    unsafe { dc.FillRoundedRectangle(&rounded(r, radius), &brush(dc, color)?) };
    Ok(())
}

/// A CSS `border` of `width` drawn inside the border box.
pub fn border(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    width: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    ring(dc, r, radius, -width, 0., color)
}

/// Aperture optical anodized finish: whisper-thin ambient top sheen and a
/// crisp 1 DIP top specular hairline (`--control-highlight` / `--primary-highlight`),
/// avoiding heavy 3D plastic gradients or chunky bottom bevel lips.
pub fn tactile_finish(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    (r, radius): (D2D_RECT_F, f32),
    t: &Tokens,
    primary: bool,
    hovered: bool,
    opacity: f32,
) -> Result<()> {
    if r.right - r.left <= 2. || r.bottom - r.top <= 2. || opacity <= 0. {
        return Ok(());
    }
    let dark = t.color("text").r > 0.5;
    let (top_alpha, bottom_alpha) = match (primary, hovered) {
        (true, true) => (0.08, 0.03),
        (true, false) => (0.05, 0.03),
        (false, true) => (
            if dark { 0.06 } else { 0.03 },
            if dark { 0.02 } else { 0.015 },
        ),
        (false, false) => (
            if dark { 0.035 } else { 0.015 },
            if dark { 0.02 } else { 0.015 },
        ),
    };
    let stops = [
        D2D1_GRADIENT_STOP {
            position: 0.,
            color: D2D1_COLOR_F {
                r: 1.,
                g: 1.,
                b: 1.,
                a: top_alpha * opacity,
            },
        },
        D2D1_GRADIENT_STOP {
            position: 1.,
            color: D2D1_COLOR_F {
                r: 0.,
                g: 0.,
                b: 0.,
                a: bottom_alpha * opacity,
            },
        },
    ];
    let top_color = t.color_alpha(
        match (primary, hovered) {
            (true, true) => "primary-highlight-hover",
            (true, false) => "primary-highlight",
            (false, true) => "control-highlight-hover",
            (false, false) => "control-highlight",
        },
        opacity,
    );
    unsafe {
        let clip = factory.CreateRoundedRectangleGeometry(&rounded(r, radius))?;
        let mut layer = D2D1_LAYER_PARAMETERS1 {
            contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
            geometricMask: std::mem::ManuallyDrop::new(Some(clip.cast()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.,
            ..Default::default()
        };
        dc.PushLayer(&layer, None::<&ID2D1Layer>);
        let drawn: Result<()> = (|| {
            let target: &ID2D1RenderTarget = dc;
            let collection = target.CreateGradientStopCollection(
                &stops,
                D2D1_GAMMA_2_2,
                D2D1_EXTEND_MODE_CLAMP,
            )?;
            let sheen = target.CreateLinearGradientBrush(
                &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: Vector2 {
                        X: r.left,
                        Y: r.top,
                    },
                    endPoint: Vector2 {
                        X: r.left,
                        Y: r.bottom,
                    },
                },
                None,
                &collection,
            )?;
            dc.FillRoundedRectangle(&rounded(r, radius), &sheen);
            let top_rect = rect(r.left, r.top + 1., r.right, r.bottom + 1.);
            dc.DrawRoundedRectangle(&rounded(top_rect, radius), &brush(dc, top_color)?, 1., None);
            Ok(())
        })();
        dc.PopLayer();
        std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
        drawn
    }
}

/// A band between `from` and `to` DIPs outside `r` (negative is inside), as a
/// CSS `box-shadow: 0 0 0 Npx` focus ring draws it.
pub fn ring(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    from: f32,
    to: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let mid = (from + to) / 2.;
    let band = rect(r.left - mid, r.top - mid, r.right + mid, r.bottom + mid);
    unsafe {
        dc.DrawRoundedRectangle(
            &rounded(band, (radius + mid).max(0.)),
            &brush(dc, color)?,
            to - from,
            None,
        )
    };
    Ok(())
}

/// A CSS `box-shadow` stack behind a rounded box. The blur is approximated by
/// stacked translucent outsets (no Gaussian blur effect) so it stays cheap and
/// identical between composition surfaces and screenshots.
pub fn shadow(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    layers: &[Shadow],
) -> Result<()> {
    for layer in layers.iter().rev() {
        let steps = (layer.blur.ceil() as usize).clamp(1, 8);
        let [cr, cg, cb, ca] = layer.color;
        for step in 0..steps {
            // Outsets from -blur/2 to +blur/2 accumulate a linear falloff.
            let d = layer.spread + layer.blur * ((step as f32 + 0.5) / steps as f32 - 0.5);
            let band = rect(
                r.left + layer.x - d,
                r.top + layer.y - d,
                r.right + layer.x + d,
                r.bottom + layer.y + d,
            );
            fill_round(
                dc,
                band,
                (radius + d).max(0.),
                D2D1_COLOR_F {
                    r: cr,
                    g: cg,
                    b: cb,
                    a: ca / steps as f32,
                },
            )?;
        }
    }
    Ok(())
}

/// A CSS `linear-gradient(<angle>deg, …)` of `(position, 0xRRGGBB)` stops
/// interpolated in sRGB, filling the rounded rect at `alpha`.
pub fn gradient(
    dc: &ID2D1DeviceContext,
    (r, radius): (D2D_RECT_F, f32),
    angle: f32,
    hues: &[(f32, u32)],
    alpha: f32,
) -> Result<()> {
    gradient_over(dc, (r, radius), r, angle, hues, alpha)
}

/// [`gradient`] whose gradient line spans `line` (a CSS box with an inset)
/// while it fills only `r`.
pub fn gradient_over(
    dc: &ID2D1DeviceContext,
    (r, radius): (D2D_RECT_F, f32),
    line: D2D_RECT_F,
    angle: f32,
    hues: &[(f32, u32)],
    alpha: f32,
) -> Result<()> {
    let (w, h) = (line.right - line.left, line.bottom - line.top);
    let stops: Vec<D2D1_GRADIENT_STOP> = hues
        .iter()
        .map(|&(position, rgb)| D2D1_GRADIENT_STOP {
            position,
            color: D2D1_COLOR_F {
                r: ((rgb >> 16) & 0xff) as f32 / 255.,
                g: ((rgb >> 8) & 0xff) as f32 / 255.,
                b: (rgb & 0xff) as f32 / 255.,
                a: alpha,
            },
        })
        .collect();
    unsafe {
        // The plain render-target overloads (sRGB, 8-bit stops).
        let target: &ID2D1RenderTarget = dc;
        let collection =
            target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
        let angle = angle.to_radians();
        let (dx, dy) = (angle.sin(), -angle.cos());
        let half = (w * dx.abs() + h * dy.abs()) / 2.;
        let (cx, cy) = (line.left + w / 2., line.top + h / 2.);
        let brush = target.CreateLinearGradientBrush(
            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 {
                    X: cx - dx * half,
                    Y: cy - dy * half,
                },
                endPoint: Vector2 {
                    X: cx + dx * half,
                    Y: cy + dy * half,
                },
            },
            None,
            &collection,
        )?;
        dc.FillRoundedRectangle(&rounded(r, radius), &brush);
    }
    Ok(())
}

/// A CSS `box-shadow: 0 0 <blur> <color>` glow drawn only outside the
/// rounded box (CSS clips outer shadows to the outside), as 2 px bands whose
/// alpha follows the Gaussian tail of a `blur / 2` sigma. No layers or
/// geometry masks, so a translucent fill above it shows nothing through.
pub fn glow(
    dc: &ID2D1DeviceContext,
    r: D2D_RECT_F,
    radius: f32,
    blur: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let sigma = (blur / 2.).max(0.5);
    let band = 2.;
    let mut d = 0.;
    while d < blur {
        // 0.5·erfc(z) at the band's middle, with Winitzki's erf.
        let z = (d + band / 2.) / sigma / std::f32::consts::SQRT_2;
        let a = 0.147;
        let z2 = z * z;
        let erf = (1. - (-z2 * (4. / std::f32::consts::PI + a * z2) / (1. + a * z2)).exp()).sqrt();
        let erfc = 1. - erf;
        let alpha = color.a * 0.5 * erfc;
        ring(
            dc,
            r,
            radius,
            d,
            d + band,
            D2D1_COLOR_F { a: alpha, ..color },
        )?;
        d += band;
    }
    Ok(())
}

/// The text position nearest `(x, y)` in `text`'s layout coordinates, and
/// whether the point is over the text itself.
pub fn hit_text(text: &Text, x: f32, y: f32) -> (usize, bool) {
    let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    if unsafe {
        text.layout
            .HitTestPoint(x, y, &mut trailing, &mut inside, &mut metrics)
    }
    .is_err()
    {
        return (0, false);
    }
    let position = metrics.textPosition as usize
        + if trailing.as_bool() {
            metrics.length as usize
        } else {
            0
        };
    (position.min(text.units.len()), inside.as_bool())
}

/// Line boxes covering the range `a..b` of `text`, in its layout coordinates.
pub fn range_boxes(text: &Text, a: usize, b: usize) -> Vec<D2D_RECT_F> {
    let mut buffer = vec![DWRITE_HIT_TEST_METRICS::default(); 8];
    let mut count = 0;
    let (start, length) = (a as u32, b.saturating_sub(a) as u32);
    let hit = |buffer: &mut [DWRITE_HIT_TEST_METRICS], count: &mut u32| unsafe {
        text.layout
            .HitTestTextRange(start, length, 0., 0., Some(buffer), count)
    };
    if hit(&mut buffer, &mut count).is_err() {
        if count as usize <= buffer.len() {
            return Vec::new();
        }
        buffer.resize(count as usize, DWRITE_HIT_TEST_METRICS::default());
        if hit(&mut buffer, &mut count).is_err() {
            return Vec::new();
        }
    }
    buffer[..(count as usize).min(buffer.len())]
        .iter()
        .map(|m| rect(m.left, m.top, m.left + m.width, m.top + m.height))
        .collect()
}

/// `::selection` over `r`: the accent fill, with `text` (drawn at `(x, y)`)
/// in accent ink inside it.
pub fn selected(
    dc: &ID2D1DeviceContext,
    text: &Text,
    r: D2D_RECT_F,
    (x, y): (f32, f32),
    fill: D2D1_COLOR_F,
    ink: D2D1_COLOR_F,
) -> Result<()> {
    self::fill(dc, r, fill)?;
    unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) };
    let drawn = self::text(dc, text, x, y, ink);
    unsafe { dc.PopAxisAlignedClip() };
    drawn
}

pub fn text(
    dc: &ID2D1DeviceContext,
    text: &Text,
    x: f32,
    y: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    unsafe {
        dc.DrawTextLayout(
            Vector2 { X: x, Y: y },
            &text.layout,
            &brush(dc, color)?,
            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
        )
    };
    Ok(())
}

/// A CSS black `text-shadow` at `(x, y)` (offset included): the blur is
/// approximated by translucent copies on two rings, like [`shadow`].
pub fn text_shadow(
    dc: &ID2D1DeviceContext,
    layout: &Text,
    x: f32,
    y: f32,
    blur: f32,
    alpha: f32,
) -> Result<()> {
    let color = D2D1_COLOR_F {
        r: 0.,
        g: 0.,
        b: 0.,
        a: alpha / 8.,
    };
    for radius in [blur / 6., blur / 3.] {
        for step in 0..8 {
            let angle = step as f32 * std::f32::consts::FRAC_PI_4;
            let (dx, dy) = (radius * angle.cos(), radius * angle.sin());
            text(dc, layout, x + dx, y + dy, color)?;
        }
    }
    Ok(())
}

/// A CSS black `text-shadow` with a true Gaussian blur (`CLSID_D2D1Shadow`,
/// standard deviation blur ÷ 2) at `(x, y)`, offset included. Large text
/// such as the countdown needs it: [`text_shadow`]'s rings read as an
/// outline there. Falls back to the rings where the effect is unavailable.
pub fn text_blur_shadow(
    dc: &ID2D1DeviceContext,
    layout: &Text,
    (x, y): (f32, f32),
    blur: f32,
    alpha: f32,
) -> Result<()> {
    let blurred = || -> Result<()> {
        unsafe {
            let list = dc.CreateCommandList()?;
            let target = dc.GetTarget()?;
            let mut transform = Matrix3x2::identity();
            dc.GetTransform(&mut transform);
            dc.SetTarget(&list);
            dc.SetTransform(&Matrix3x2::identity());
            let black = brush(
                dc,
                D2D1_COLOR_F {
                    r: 0.,
                    g: 0.,
                    b: 0.,
                    a: 1.,
                },
            );
            if let Ok(black) = &black {
                dc.DrawTextLayout(
                    Vector2 { X: 0., Y: 0. },
                    &layout.layout,
                    black,
                    D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                );
            }
            dc.SetTarget(&target);
            dc.SetTransform(&transform);
            black?;
            list.Close()?;
            let effect = dc.CreateEffect(&CLSID_D2D1Shadow)?;
            effect.SetInput(0, &list, true);
            effect.SetValue(
                D2D1_SHADOW_PROP_BLUR_STANDARD_DEVIATION.0 as u32,
                D2D1_PROPERTY_TYPE_FLOAT,
                &(blur / 2.).to_le_bytes(),
            )?;
            let color: Vec<u8> = [0f32, 0., 0., alpha]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            effect.SetValue(
                D2D1_SHADOW_PROP_COLOR.0 as u32,
                D2D1_PROPERTY_TYPE_VECTOR4,
                &color,
            )?;
            dc.DrawImage(
                &effect.GetOutput()?,
                Some(&Vector2 { X: x, Y: y }),
                None,
                D2D1_INTERPOLATION_MODE_LINEAR,
                D2D1_COMPOSITE_MODE_SOURCE_OVER,
            );
            Ok(())
        }
    };
    blurred().or_else(|_| text_shadow(dc, layout, x, y, blur, alpha))
}

/// A shipping 24-unit icon at `side` DIPs: every path is stroked except those
/// listed in `filled`, which are filled (`.capture-icon-spark`).
pub fn icon(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    name: &str,
    filled: &[usize],
    (x, y, side): (f32, f32, f32),
    stroke: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let Some(paths) = captures_app::icons::paths(name) else {
        return Ok(());
    };
    let scale = side / 24.;
    for (index, d) in paths.iter().enumerate() {
        let fill = filled.contains(&index);
        path(dc, factory, d, fill, (x, y, scale), stroke * scale, color)?;
    }
    Ok(())
}

/// A button's trailing shipping icon (`.ui-btn svg`: 15 DIPs, 1.7 units in
/// the shared 24-unit space) with its left edge at `x`, centred on `mid`.
/// Icons on shipping's 16-unit grid are scaled up like
/// `captures_app::icons::polylines` does.
pub const TRAILING_ICON: f32 = 15.;

pub fn trailing_icon(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    name: &str,
    (x, mid): (f32, f32),
    color: D2D1_COLOR_F,
) -> Result<()> {
    let grow = 24. / captures_app::icons::view_box(name);
    let top = mid - TRAILING_ICON / 2.;
    icon(
        dc,
        factory,
        name,
        &[],
        (x, top, TRAILING_ICON * grow),
        1.7 / grow,
        color,
    )
}

/// An SVG path `d` at `(x, y)` scaled by `scale`, stroked `width` DIPs wide
/// with round caps and joins, or filled.
pub fn path(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    d: &str,
    fill: bool,
    (x, y, scale): (f32, f32, f32),
    width: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let brush = brush(dc, color)?;
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        for line in captures_app::icons::flatten(d) {
            let points: Vec<Vector2> = line
                .iter()
                .map(|p| Vector2 {
                    X: x + p[0] * scale,
                    Y: y + p[1] * scale,
                })
                .collect();
            let Some((first, rest)) = points.split_first() else {
                continue;
            };
            sink.BeginFigure(
                *first,
                if fill {
                    D2D1_FIGURE_BEGIN_FILLED
                } else {
                    D2D1_FIGURE_BEGIN_HOLLOW
                },
            );
            sink.AddLines(rest);
            sink.EndFigure(if fill {
                D2D1_FIGURE_END_CLOSED
            } else {
                D2D1_FIGURE_END_OPEN
            });
        }
        sink.Close()?;
        if fill {
            dc.FillGeometry(&geometry, &brush, None);
        } else {
            let style = factory.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES1 {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.,
                    ..Default::default()
                },
                None,
            )?;
            dc.DrawGeometry(&geometry, &brush, width, &style);
        }
    }
    Ok(())
}

/// Shipping `.theme-option-preview`: an accent tile with the signal hue as a
/// wedge in its lower-right half (or the Custom rainbow), clipped to `radius`,
/// with a 1 px inset hairline.
pub fn swatch(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    r: D2D_RECT_F,
    radius: f32,
    colors: Option<(D2D1_COLOR_F, D2D1_COLOR_F)>,
) -> Result<()> {
    let w = r.right - r.left;
    unsafe {
        match colors {
            Some((accent, signal)) => {
                fill_round(dc, r, radius, accent)?;
                // `clip-path: polygon(100% 0, 100% 100%, 0 100%)` on the right column.
                let tile = factory.CreateRoundedRectangleGeometry(&rounded(r, radius))?;
                let wedge = factory.CreatePathGeometry()?;
                let sink = wedge.Open()?;
                sink.BeginFigure(
                    Vector2 {
                        X: r.right,
                        Y: r.top,
                    },
                    D2D1_FIGURE_BEGIN_FILLED,
                );
                sink.AddLines(&[
                    Vector2 {
                        X: r.right,
                        Y: r.bottom,
                    },
                    Vector2 {
                        X: r.left + w / 2.,
                        Y: r.bottom,
                    },
                ]);
                sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                sink.Close()?;
                let mut layer = D2D1_LAYER_PARAMETERS1 {
                    contentBounds: rect(-1e6, -1e6, 1e6, 1e6),
                    geometricMask: std::mem::ManuallyDrop::new(Some(tile.cast()?)),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: 1.,
                    ..Default::default()
                };
                dc.PushLayer(&layer, None::<&ID2D1Layer>);
                let filled = brush(dc, signal).map(|b| dc.FillGeometry(&wedge, &b, None));
                dc.PopLayer();
                // Release the mask reference the parameters own.
                std::mem::ManuallyDrop::drop(&mut layer.geometricMask);
                filled?;
            }
            None => {
                // `.theme-option-custom`'s `linear-gradient(120deg, hsl(…))`.
                let hues = [
                    (0., 0xff475a),
                    (0.18, 0xffab3d),
                    (0.34, 0xf7e845),
                    (0.5, 0x3ee081),
                    (0.66, 0x3cd4ec),
                    (0.84, 0xc357f4),
                    (1., 0xff52ae),
                ];
                gradient(dc, (r, radius), 120., &hues, 1.)?;
            }
        }
    }
    border(
        dc,
        r,
        radius,
        1.,
        D2D1_COLOR_F {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 0.16,
        },
    )
}

/// Draw `layout` as glyph outlines only, `width` DIPs wide and centred on
/// each outline: CSS `color: transparent; -webkit-text-stroke`, for outlined
/// labels and the Outlined style preview.
pub fn outline_text(
    dc: &ID2D1DeviceContext,
    factory: &ID2D1Factory1,
    layout: &IDWriteTextLayout,
    (x, y): (f32, f32),
    width: f32,
    color: D2D1_COLOR_F,
) -> Result<()> {
    let stroke = unsafe {
        factory.CreateStrokeStyle(
            &D2D1_STROKE_STYLE_PROPERTIES1 {
                // Shipping's canvas (`lineJoin = "round"`) and the shared
                // renderer stroke labels with round joins, not miters.
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.,
                ..Default::default()
            },
            None,
        )?
    };
    let renderer: IDWriteTextRenderer = Outliner {
        dc: Some(dc.clone()),
        factory: factory.clone(),
        paint: Some((brush(dc, color)?, stroke.cast()?)),
        width,
        ink: RefCell::new(Vec::new()),
    }
    .into();
    unsafe { layout.Draw(None, &renderer, x, y) }
}

/// The ink of each glyph run in `layout` as `(baseline, top, bottom)`,
/// layout-relative; `outline` widens it by half that stroke.
pub fn ink_runs(
    factory: &ID2D1Factory1,
    layout: &IDWriteTextLayout,
    outline: f32,
) -> Result<Vec<(f32, f32, f32)>> {
    let meter = Outliner {
        dc: None,
        factory: factory.clone(),
        paint: None,
        width: outline,
        ink: RefCell::new(Vec::new()),
    };
    let renderer: IDWriteTextRenderer = meter.into();
    unsafe { layout.Draw(None, &renderer, 0., 0.)? };
    // SAFETY: `renderer` was made from an `Outliner` just above.
    let meter = unsafe { renderer.as_impl() };
    Ok(meter.ink.take())
}

/// Strokes glyph outlines, or with no device context only measures them.
#[implement(IDWriteTextRenderer)]
struct Outliner {
    dc: Option<ID2D1DeviceContext>,
    factory: ID2D1Factory1,
    paint: Option<(ID2D1SolidColorBrush, ID2D1StrokeStyle)>,
    width: f32,
    ink: RefCell<Vec<(f32, f32, f32)>>,
}

impl IDWritePixelSnapping_Impl for Outliner_Impl {
    fn IsPixelSnappingDisabled(&self, _: *const core::ffi::c_void) -> Result<BOOL> {
        // Outlines are geometry: no baseline snapping.
        Ok(true.into())
    }

    fn GetCurrentTransform(
        &self,
        _: *const core::ffi::c_void,
        t: *mut DWRITE_MATRIX,
    ) -> Result<()> {
        unsafe {
            let mut m = Matrix3x2::identity();
            if let Some(dc) = &self.dc {
                dc.GetTransform(&mut m);
            }
            *t = DWRITE_MATRIX {
                m11: m.M11,
                m12: m.M12,
                m21: m.M21,
                m22: m.M22,
                dx: m.M31,
                dy: m.M32,
            };
        }
        Ok(())
    }

    fn GetPixelsPerDip(&self, _: *const core::ffi::c_void) -> Result<f32> {
        Ok(1.)
    }
}

impl IDWriteTextRenderer_Impl for Outliner_Impl {
    fn DrawGlyphRun(
        &self,
        _: *const core::ffi::c_void,
        x: f32,
        y: f32,
        _: DWRITE_MEASURING_MODE,
        run: *const DWRITE_GLYPH_RUN,
        _: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        unsafe {
            let run = &*run;
            let Some(face) = run.fontFace.as_ref() else {
                return Ok(());
            };
            if run.glyphCount == 0 {
                return Ok(());
            }
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            face.GetGlyphRunOutline(
                run.fontEmSize,
                run.glyphIndices,
                Some(run.glyphAdvances),
                (!run.glyphOffsets.is_null()).then_some(run.glyphOffsets),
                run.glyphCount,
                run.isSideways.as_bool(),
                run.bidiLevel % 2 == 1,
                &sink.cast::<ID2D1SimplifiedGeometrySink>()?,
            )?;
            sink.Close()?;
            let (Some(dc), Some((brush, stroke))) = (&self.dc, &self.paint) else {
                let b = geometry.GetBounds(None)?;
                if b.bottom > b.top {
                    let pad = self.width / 2.;
                    self.ink
                        .borrow_mut()
                        .push((y, y + b.top - pad, y + b.bottom + pad));
                }
                return Ok(());
            };
            let mut base = Matrix3x2::default();
            dc.GetTransform(&mut base);
            dc.SetTransform(&(Matrix3x2::translation(x, y) * base));
            dc.DrawGeometry(&geometry, brush, self.width, stroke);
            dc.SetTransform(&base);
        }
        Ok(())
    }

    fn DrawUnderline(
        &self,
        _: *const core::ffi::c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_UNDERLINE,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }

    fn DrawStrikethrough(
        &self,
        _: *const core::ffi::c_void,
        _: f32,
        _: f32,
        _: *const DWRITE_STRIKETHROUGH,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }

    fn DrawInlineObject(
        &self,
        _: *const core::ffi::c_void,
        _: f32,
        _: f32,
        _: Ref<IDWriteInlineObject>,
        _: BOOL,
        _: BOOL,
        _: Ref<IUnknown>,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_share_one_device_until_it_is_lost() {
        let tokens = crate::appearance::tokens(Some("light"), &Default::default());
        let first = Gfx::shared(&tokens).unwrap();
        let dark = crate::appearance::tokens(Some("dark"), &Default::default());
        let again = Gfx::shared(&dark).unwrap();
        assert_eq!(first.d3d, again.d3d, "every window draws on one device");
        assert!(!renderer_lost());

        lose_shared();
        assert!(renderer_lost());
        let replaced = Gfx::shared(&tokens).unwrap();
        assert_ne!(first.d3d, replaced.d3d, "a lost device is replaced");
        // Windows still on the old device must reopen before this clears.
        assert!(renderer_lost());
        renderer_recovered();
        assert!(!renderer_lost());
        assert_eq!(Gfx::shared(&tokens).unwrap().d3d, replaced.d3d);
        // Release the device before the test thread exits: a composition
        // device released by a thread-local destructor blocks the exit.
        drop((first, again, replaced));
        SHARED.with(|s| s.borrow_mut().take());
    }
}
