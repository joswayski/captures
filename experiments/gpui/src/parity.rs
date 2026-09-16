//! Isolated component benchmark; no capture profile, tray, or full-app services.
//! Protocol: experiments/macos-parity/README.md, version 1.
use anyhow::{Context as _, Result, ensure};
use gpui::*;
use image::RgbaImage;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Instant};

pub mod effects;
pub mod motion;
mod parity_measurement;
mod transient_images;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Scenario {
    DustBottomLeft,
    DustTopRight,
    SettleBottom,
    SettleTop,
}

impl Scenario {
    fn dust(self) -> bool {
        matches!(self, Self::DustBottomLeft | Self::DustTopRight)
    }

    fn name(self) -> &'static str {
        match self {
            Self::DustBottomLeft => "dust-bottom-left",
            Self::DustTopRight => "dust-top-right",
            Self::SettleBottom => "settle-bottom",
            Self::SettleTop => "settle-top",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Checkpoint,
    Run,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    id: usize,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    card_width: f32,
    card_height: f32,
    source_left: f32,
    source_top: f32,
    surface_width: f32,
    surface_height: f32,
    surface_offset_x: f32,
    surface_offset_y: f32,
    dx: f32,
    dy: f32,
    rotate: f32,
    delay_ms: f32,
    duration_ms: f32,
}

impl Descriptor {
    fn particle(&self) -> effects::Particle {
        effects::Particle {
            x: self.source_left,
            y: self.source_top,
            width: self.width,
            height: self.height,
            dx: self.dx,
            dy: self.dy,
            rotate: self.rotate,
            delay: self.delay_ms,
            duration: self.duration_ms,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    schema: u32,
    scenario: Scenario,
    mode: Mode,
    checkpoint_ms: f64,
    duration_ms: f64,
    cycle_ms: f64,
    width: f32,
    height: f32,
    scale: f32,
    fixture_path: PathBuf,
    seed: u32,
    particles: Vec<Descriptor>,
    #[serde(default)]
    start_gate_path: Option<PathBuf>,
}

impl Config {
    fn validate(&self, source: &RgbaImage) -> Result<()> {
        ensure!(self.schema == 1, "expected schema 1");
        ensure!(
            self.start_gate_path
                .as_ref()
                .is_none_or(|path| path.is_absolute()),
            "startGatePath must be absolute"
        );
        ensure!(
            self.width == 640. && self.height == 720. && self.scale == 2.,
            "expected 640×720 at 2×"
        );
        ensure!(
            self.cycle_ms == 3200. && self.duration_ms > 0. && self.checkpoint_ms >= 0.,
            "invalid animation times"
        );
        ensure!(
            self.fixture_path.is_absolute(),
            "fixturePath must be absolute"
        );
        ensure!(
            source.dimensions() == (397, 251),
            "expected shared 397×251 fixture"
        );
        ensure!(
            self.seed == 739 && self.particles.len() == 198,
            "expected seed 739 and 198 shipping descriptors"
        );
        let surface_width = 284.;
        let surface_height = 251. * 284. / 397.;
        for (index, p) in self.particles.iter().enumerate() {
            ensure!(
                p.id == index && p.card_width == 284. && p.card_height == 160.,
                "invalid particle identity or card size"
            );
            ensure!(
                [
                    p.left - p.source_left - 120.,
                    p.top - p.source_top - 120.,
                    p.surface_width - surface_width,
                    p.surface_height - surface_height,
                    p.surface_offset_x,
                    p.surface_offset_y - (160. - surface_height) / 2.,
                ]
                .into_iter()
                .all(|difference| difference.abs() < 0.002),
                "particle {index} disagrees with the shared cover layout"
            );
            ensure!(
                p.width > 0.
                    && p.width <= 285.
                    && p.height > 0.
                    && p.height <= 161.
                    && p.source_left >= 0.
                    && p.source_left < 284.
                    && p.source_top >= 0.
                    && p.source_top < 160.
                    && p.delay_ms >= 0.
                    && p.duration_ms > 0.
                    && [p.dx, p.dy, p.rotate, p.delay_ms, p.duration_ms]
                        .into_iter()
                        .all(f32::is_finite),
                "invalid particle {index}"
            );
        }
        Ok(())
    }
}

fn survivor_y(scenario: Scenario, elapsed: f32) -> f32 {
    let delay = if scenario.dust() { 1800. } else { 0. };
    let progress = motion::stack_settle_progress(elapsed, delay);
    if matches!(scenario, Scenario::DustBottomLeft | Scenario::SettleBottom) {
        136. + 184. * progress
    } else {
        504. - 184. * progress
    }
}

fn texture(mut image: RgbaImage) -> Arc<RenderImage> {
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(image)]))
}

fn write_json(path: &std::path::Path, value: &Value) -> Result<()> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().context("output has no parent")?)?;
    serde_json::to_writer(temporary.as_file_mut(), value)?;
    temporary.persist(path)?;
    Ok(())
}

struct Surface {
    config: Config,
    output: PathBuf,
    source: RgbaImage,
    media: Arc<RenderImage>,
    effect: Option<effects::Dissolve>,
    frame: Option<Arc<RenderImage>>,
    frame_images: transient_images::TransientImages,
    scene_ms: f32,
    cycle: u64,
    started: Option<Instant>,
    started_host_time_ns: Option<u64>,
    gate_open: bool,
    last_callback: Option<Instant>,
    intervals: Vec<f64>,
    setup: Vec<f64>,
    metadata: Option<Value>,
    complete: bool,
    lab_wait: bool,
}

impl Surface {
    fn new(config: Config, source: RgbaImage, output: PathBuf) -> Self {
        let scene_ms = if config.mode == Mode::Checkpoint {
            config.checkpoint_ms as f32
        } else {
            0.
        };
        // Published GPUI 0.2.2 has no source-rectangle cropping in paint_image.
        // Pre-crop instead of relying on its broken rounded ObjectFit::Cover.
        let media = texture(effects::cover_media(&source, 568, 320));
        let gate_open = config.mode == Mode::Checkpoint || config.start_gate_path.is_none();
        let mut surface = Self {
            config,
            output,
            media,
            source,
            effect: None,
            frame: None,
            frame_images: Default::default(),
            scene_ms,
            cycle: 0,
            started: None,
            started_host_time_ns: None,
            gate_open,
            last_callback: None,
            intervals: Vec::new(),
            setup: Vec::new(),
            metadata: None,
            complete: false,
            lab_wait: cfg!(target_os = "linux")
                && std::env::var_os("CAPTURES_GPUI_X11_RESIZE_WORKAROUND").is_some(),
        };
        surface.prepare_cycle();
        surface.sample_frame();
        surface
    }

    fn prepare_cycle(&mut self) {
        let start = Instant::now();
        self.frame = None;
        self.effect = None;
        if self.config.scenario.dust() {
            self.effect = Some(effects::Dissolve::from_particles(
                &self.source,
                self.config
                    .particles
                    .iter()
                    .map(Descriptor::particle)
                    .collect(),
                self.config.scale,
            ));
        }
        self.setup.push(start.elapsed().as_secs_f64() * 1000.);
    }

    fn sample_frame(&mut self) {
        self.frame = self
            .effect
            .as_ref()
            .map(|effect| texture(effect.frame(self.scene_ms)));
    }

    fn advance(&mut self, window: &Window) -> Result<()> {
        if self.lab_wait || self.complete {
            return Ok(());
        }
        if self.metadata.is_none() {
            ensure!(
                (window.scale_factor() - self.config.scale).abs() < 0.001,
                "window backing scale must be 2×"
            );
            ensure!(
                (f32::from(window.viewport_size().width) - self.config.width).abs() < 0.5
                    && (f32::from(window.viewport_size().height) - self.config.height).abs() < 0.5,
                "window viewport {:?} differs from requested 640×720",
                window.viewport_size()
            );
            let metadata = json!({
                "schema": 1, "pid": std::process::id(), "window_id": native_window_id(window)?,
                "measurementProtocol": 2, "hostClock": parity_measurement::HOST_CLOCK,
                "scale": window.scale_factor(), "scenario": self.config.scenario.name(),
                "mode": if self.config.mode == Mode::Run { "run" } else { "checkpoint" },
                "renderer": "gpui-0.2.2-subpixel-cpu-raster-texture-upload", "checkpointMs": self.config.checkpoint_ms,
            });
            write_json(
                &PathBuf::from(format!("{}.ready.json", self.output.display())),
                &metadata,
            )?;
            self.metadata = Some(metadata);
        }
        if self.config.mode == Mode::Checkpoint {
            return Ok(());
        }
        self.tick(Instant::now())
    }

    fn tick(&mut self, now: Instant) -> Result<()> {
        if self.complete || !self.gate_open {
            return Ok(());
        }
        if self.started.is_none() {
            let host_time = parity_measurement::host_time_ns()?;
            write_json(
                &PathBuf::from(format!("{}.started.json", self.output.display())),
                &json!({
                    "schema": 1, "pid": std::process::id(), "startHostTimeNs": host_time,
                    "hostClock": parity_measurement::HOST_CLOCK,
                }),
            )?;
            self.started_host_time_ns = Some(host_time);
        }
        let elapsed = now
            .duration_since(*self.started.get_or_insert(now))
            .as_secs_f64()
            * 1000.;
        if let Some(previous) = self.last_callback.replace(now) {
            self.intervals
                .push(now.duration_since(previous).as_secs_f64() * 1000.);
        }
        // Finish before preparing a new cycle. Preserve the last sampled pose.
        if elapsed >= self.config.duration_ms {
            let mut result = self.metadata.clone().context("missing ready metadata")?;
            result.as_object_mut().unwrap().extend(json!({
                "elapsedMs": elapsed, "callbackIntervalsMs": self.intervals, "setupMs": self.setup,
                "cycles": self.setup.len(), "complete": true,
                "startedHostTimeNs": self.started_host_time_ns,
                "metric": "Actual main-thread render callback intervals; NOT presented frames or GPU timings. CPU rasterization and GPUI texture upload path.",
            }).as_object().unwrap().clone());
            write_json(&self.output, &result)?;
            self.complete = true;
            return Ok(());
        }
        let cycle = (elapsed / self.config.cycle_ms).floor() as u64;
        if cycle != self.cycle {
            self.prepare_cycle();
            self.cycle = cycle;
        }
        self.scene_ms = (elapsed % self.config.cycle_ms) as f32;
        self.sample_frame();
        Ok(())
    }
}

impl Render for Surface {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.frame_images.begin_scene(window);
        if let Err(error) = self.advance(window) {
            eprintln!("GPUI parity adapter: {error:#}");
            std::process::exit(1);
        }
        if !self.lab_wait && self.gate_open && self.config.mode == Mode::Run && !self.complete {
            window.request_animation_frame();
        }
        div()
            .size_full()
            .bg(rgb(0x20242b))
            .overflow_hidden()
            .child(motion::translated(
                178.,
                survivor_y(self.config.scenario, self.scene_ms),
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(284.))
                    .h(px(160.))
                    .rounded(px(12.))
                    .overflow_hidden()
                    .child(
                        img(self.media.clone())
                            .w(px(284.))
                            .h(px(160.))
                            .rounded(px(12.))
                            .object_fit(ObjectFit::Fill),
                    ),
            ))
            .children(self.frame.clone().map(|frame| {
                let frame = self.frame_images.retain(frame);
                div()
                    .absolute()
                    .left(px(58.))
                    .top(px(200.))
                    .w(px(524.))
                    .h(px(400.))
                    .child(img(frame).size_full())
            }))
    }
}

#[cfg(target_os = "macos")]
fn native_window_id(window: &Window) -> Result<u64> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| {
            anyhow::anyhow!("could not obtain the GPUI AppKit window handle: {error}")
        })?
        .as_raw();
    let RawWindowHandle::AppKit(handle) = handle else {
        anyhow::bail!("expected AppKit window");
    };
    // Borrowed from a live GPUI window, accessed only on the AppKit main thread.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
    let native = view.window().context("NSView has no NSWindow")?;
    native.setHasShadow(false);
    let id = native.windowNumber();
    ensure!(id > 0, "NSWindow has no WindowServer number");
    Ok(id as u64)
}

#[cfg(target_os = "linux")]
fn native_window_id(_: &Window) -> Result<u64> {
    use x11rb::{
        connection::Connection,
        protocol::xproto::{AtomEnum, ConnectionExt},
    };
    // GPUI 0.2.2 does not implement X11 raw-window-handle; do not call it here.
    let (connection, screen) = x11rb::connect(None)?;
    let pid_atom = connection.intern_atom(false, b"_NET_WM_PID")?.reply()?.atom;
    let mut pending = vec![connection.setup().roots[screen].root];
    while let Some(id) = pending.pop() {
        let pid = connection
            .get_property(false, id, pid_atom, AtomEnum::CARDINAL, 0, 1)?
            .reply()?;
        let class = connection
            .get_property(false, id, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256)?
            .reply()?;
        if pid.value32().and_then(|mut values| values.next()) == Some(std::process::id())
            && class
                .value
                .split(|byte| *byte == 0)
                .any(|part| part == b"captures-gpui-parity")
        {
            return Ok(id.into());
        }
        pending.extend(connection.query_tree(id)?.reply()?.children);
    }
    anyhow::bail!("could not find the adapter's X11 window")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn native_window_id(_: &Window) -> Result<u64> {
    anyhow::bail!("this comparison adapter supports macOS and Linux diagnostics only")
}

fn main() -> Result<()> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    ensure!(
        args.len() == 2,
        "usage: captures-gpui-parity CONFIG_JSON OUTPUT_JSON"
    );
    let config_path = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    ensure!(
        config_path.is_absolute() && output.is_absolute(),
        "config/output paths must be absolute"
    );
    let config: Config = serde_json::from_slice(&std::fs::read(config_path)?)?;
    let source = image::open(&config.fixture_path)?.to_rgba8();
    config.validate(&source)?;
    let start_gate = config
        .start_gate_path
        .clone()
        .filter(|_| config.mode == Mode::Run);
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    Application::new().run(move |cx| {
        let dimensions = size(px(config.width), px(config.height));
        let bounds = Bounds::centered(None, dimensions, cx);
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    kind: WindowKind::PopUp,
                    app_id: Some("captures-gpui-parity".into()),
                    is_movable: false,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Opaque,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Surface::new(config, source, output)),
            )
            .expect("open benchmark window");
        if let Some(path) = start_gate {
            cx.spawn(async move |cx| {
                if let Err(error) = parity_measurement::wait_for_gate(&path).await {
                    eprintln!("GPUI parity adapter: {error:#}");
                    std::process::exit(1);
                }
                let _ = handle.update(cx, |surface, _, cx| {
                    surface.gate_open = true;
                    cx.notify();
                });
            })
            .detach();
        }
        #[cfg(target_os = "linux")]
        if std::env::var_os("CAPTURES_GPUI_X11_RESIZE_WORKAROUND").is_some() {
            cx.spawn(async move |cx| {
                Timer::after(std::time::Duration::from_millis(500)).await;
                let _ = handle.update(cx, |_, window, _| {
                    window.resize(dimensions - size(px(20.), px(20.)))
                });
                Timer::after(std::time::Duration::from_millis(100)).await;
                let _ = handle.update(cx, |_, window, _| window.resize(dimensions));
                Timer::after(std::time::Duration::from_millis(100)).await;
                let _ = handle.update(cx, |surface, _, cx| {
                    surface.lab_wait = false;
                    cx.notify();
                });
            })
            .detach();
        }
        #[cfg(not(target_os = "linux"))]
        let _ = handle;
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn settle_uses_segment_delay_signed_distance_and_non_linear_easing() {
        assert_eq!(survivor_y(Scenario::DustBottomLeft, 1799.), 136.);
        assert_eq!(survivor_y(Scenario::DustTopRight, 1800.), 504.);
        // At Bezier parameter .5, x=.35 and y=.5, independently of the solver.
        assert!((survivor_y(Scenario::SettleBottom, 203.) - 228.).abs() < 0.001);
        assert!((survivor_y(Scenario::DustTopRight, 2003.) - 412.).abs() < 0.001);
        assert_eq!(survivor_y(Scenario::DustBottomLeft, 2380.), 320.);
        assert_eq!(survivor_y(Scenario::SettleTop, 580.), 320.);
    }

    #[test]
    fn completion_holds_the_previous_pose_without_preparing_an_extra_cycle() {
        check_run(false);
    }

    #[test]
    fn gated_run_excludes_wait_from_clock_callbacks_and_markers() {
        check_run(true);
    }

    fn check_run(gated: bool) {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("result.json");
        let config = Config {
            schema: 1,
            scenario: Scenario::SettleTop,
            mode: Mode::Run,
            checkpoint_ms: 0.,
            duration_ms: 6400.,
            cycle_ms: 3200.,
            width: 640.,
            height: 720.,
            scale: 2.,
            fixture_path: directory.path().join("fixture.png"),
            seed: 739,
            particles: Vec::new(),
            start_gate_path: gated.then(|| directory.path().join("start")),
        };
        let mut surface = Surface::new(config, RgbaImage::new(397, 251), output.clone());
        surface.metadata = Some(json!({"schema": 1}));
        let ready = Instant::now();
        let marker = PathBuf::from(format!("{}.started.json", output.display()));
        if gated {
            // Far longer than the run duration: waiting must not complete it,
            // consume callbacks, rebuild resources, or emit a start marker.
            surface.tick(ready).unwrap();
            surface
                .tick(ready + std::time::Duration::from_secs(30))
                .unwrap();
            assert!(surface.started.is_none());
            assert!(surface.started_host_time_ns.is_none());
            assert!(surface.last_callback.is_none());
            assert!(surface.intervals.is_empty());
            assert_eq!(surface.setup.len(), 1);
            assert_eq!(surface.scene_ms, 0.);
            assert!(!surface.complete);
            assert!(!marker.exists());
            assert!(!output.exists());
            surface.gate_open = true;
        }
        let start = ready + std::time::Duration::from_secs(30);
        surface.tick(start).unwrap();
        let started: Value = serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        assert_eq!(started["schema"], 1);
        assert_eq!(started["pid"], std::process::id());
        assert_eq!(started["hostClock"], parity_measurement::HOST_CLOCK);
        assert_eq!(
            started["startHostTimeNs"].as_u64(),
            surface.started_host_time_ns
        );
        assert_eq!(surface.started, Some(start));
        surface
            .tick(start + std::time::Duration::from_millis(3200))
            .unwrap();
        surface
            .tick(start + std::time::Duration::from_millis(3403))
            .unwrap();
        assert_eq!(surface.setup.len(), 2);
        assert_eq!(surface.scene_ms, 203.);
        assert!(!surface.complete);
        surface
            .tick(start + std::time::Duration::from_millis(6400))
            .unwrap();
        surface
            .tick(start + std::time::Duration::from_millis(9900))
            .unwrap();
        assert_eq!(surface.setup.len(), 2);
        assert_eq!(surface.scene_ms, 203.);
        let result: Value = serde_json::from_slice(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(result["cycles"], 2);
        assert_eq!(result["elapsedMs"], 6400.);
        assert_eq!(result["callbackIntervalsMs"], json!([3200., 203., 2997.]));
        assert_eq!(result["complete"], true);
        assert_eq!(result["startedHostTimeNs"], started["startHostTimeNs"]);
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(marker).unwrap()).unwrap(),
            started
        );
    }

    #[test]
    fn legacy_config_omits_gate_and_relative_gate_is_rejected() {
        let mut value = json!({
            "schema": 1, "scenario": "settle-top", "mode": "checkpoint",
            "checkpointMs": 145, "durationMs": 9600, "cycleMs": 3200,
            "width": 640, "height": 720, "scale": 2,
            "fixturePath": "/fixture.png", "seed": 739, "particles": [],
        });
        let legacy: Config = serde_json::from_value(value.clone()).unwrap();
        assert!(legacy.start_gate_path.is_none());
        value["startGatePath"] = json!("relative/start");
        let invalid: Config = serde_json::from_value(value).unwrap();
        assert_eq!(
            invalid
                .validate(&RgbaImage::new(397, 251))
                .unwrap_err()
                .to_string(),
            "startGatePath must be absolute"
        );
    }
}
