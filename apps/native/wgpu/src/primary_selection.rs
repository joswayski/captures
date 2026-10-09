//! Linux PRIMARY reads are isolated from the GUI and shared capture workers.
//! arboard has neither a bounded transfer nor a cancellable connection API.
use std::{
    cell::Cell,
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
    },
    time::{Duration, Instant},
};

use eframe::egui;

pub const ID: &str = "linux-primary-selection";
const HELPER: &str = "--native-primary-selection";
const MAX_BYTES: usize = 64 * 1024;
const DEADLINE: Duration = Duration::from_secs(2);
const UNAVAILABLE: &str = "Primary selection is unavailable.";

#[derive(Clone, Copy)]
pub enum Backend {
    X11,
    Wayland,
}

/// Called before package/profile election, logging, renderer or window setup.
/// Even malformed private arguments are consumed, never normal app launches.
pub fn helper(mut args: impl Iterator<Item = String>) -> Option<Result<(), String>> {
    if args.next().as_deref() != Some(HELPER) {
        return None;
    }
    Some((|| {
        let backend = match args.next().as_deref() {
            Some("x11") => Backend::X11,
            Some("wayland") => Backend::Wayland,
            _ => return Err(UNAVAILABLE.into()),
        };
        let parent: u32 = args
            .next()
            .ok_or(UNAVAILABLE)?
            .parse()
            .map_err(|_| UNAVAILABLE)?;
        if parent == 0 || args.next().is_some() {
            return Err(UNAVAILABLE.into());
        }
        use rustix::process::{Resource, Rlimit, Signal, setrlimit};
        rustix::process::set_parent_process_death_signal(Some(Signal::KILL))
            .map_err(|_| UNAVAILABLE)?;
        // Close the race where the spawning supervisor died before prctl.
        if rustix::process::getppid().map(|pid| pid.as_raw_pid() as u32) != Some(parent) {
            return Err(UNAVAILABLE.into());
        }
        // Bounds arboard's unbounded internal buffers; this is a virtual-memory
        // ceiling, not an exact RSS bound or pre-exec loader confinement.
        for (resource, limit) in [
            (Resource::As, 512 * 1024 * 1024),
            (Resource::Core, 0),
            (Resource::Cpu, 2),
        ] {
            setrlimit(
                resource,
                Rlimit {
                    current: Some(limit),
                    maximum: Some(limit),
                },
            )
            .map_err(|_| UNAVAILABLE)?;
        }
        match backend {
            Backend::Wayland
                if std::env::var_os("DISPLAY").is_some()
                    || std::env::var_os("WAYLAND_DISPLAY").is_none() =>
            {
                return Err(UNAVAILABLE.into());
            }
            Backend::X11
                if std::env::var_os("WAYLAND_DISPLAY").is_some()
                    || std::env::var_os("WAYLAND_SOCKET").is_some()
                    || std::env::var_os("DISPLAY").is_none() =>
            {
                return Err(UNAVAILABLE.into());
            }
            _ => {}
        }
        use arboard::GetExtLinux;
        let text = arboard::Clipboard::new()
            .map_err(|_| UNAVAILABLE)?
            .get()
            .clipboard(arboard::LinuxClipboardKind::Primary)
            .text()
            .map_err(|_| UNAVAILABLE)?;
        if text.len() > MAX_BYTES {
            return Err("Primary selection exceeds 64 KiB.".into());
        }
        std::io::stdout()
            .write_all(text.as_bytes())
            .map_err(|_| UNAVAILABLE)?;
        Ok(())
    })())
}

fn command(backend: Option<Backend>) -> Result<Command, String> {
    let backend = backend.ok_or(UNAVAILABLE)?;
    let mut command = Command::new(std::env::current_exe().map_err(|_| UNAVAILABLE)?);
    command
        .arg(HELPER)
        .arg(match backend {
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
        })
        .arg(std::process::id().to_string());
    match backend {
        Backend::Wayland => {
            command.env_remove("DISPLAY");
        }
        Backend::X11 => {
            command
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("WAYLAND_SOCKET");
        }
    }
    Ok(command)
}

/// One supervisor owns at most one child and one replaceable queued request.
/// Context is held by a bounded request, never the service itself.
#[derive(Clone)]
pub struct Reader(Arc<Owner>);

struct Owner(
    Arc<Shared>,
    Duration,
    Mutex<Option<std::thread::JoinHandle<()>>>,
);

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    stopped: bool,
    latest: Option<Request>,
    active: Option<Arc<AtomicBool>>,
    current: Option<Arc<AtomicBool>>,
}

struct Request {
    cancel: Arc<AtomicBool>,
    deadline: Instant,
    reply: SyncSender<Result<String, String>>,
    ctx: egui::Context,
    viewport: egui::ViewportId,
}

pub struct Pending {
    cancel: Arc<AtomicBool>,
    reply: Receiver<Result<String, String>>,
    completed: Cell<bool>,
}

impl Reader {
    pub fn new(backend: Option<Backend>) -> Self {
        Self::start(move || command(backend), DEADLINE)
    }

    fn start(
        factory: impl Fn() -> Result<Command, String> + Send + 'static,
        timeout: Duration,
    ) -> Self {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let started = std::thread::Builder::new()
            .name("primary-selection".into())
            .spawn(move || {
                loop {
                    let request = {
                        let mut state = worker.state.lock().unwrap();
                        loop {
                            if state.stopped {
                                return;
                            }
                            if let Some(request) = state.latest.take() {
                                state.active = Some(request.cancel.clone());
                                break request;
                            }
                            state = worker.wake.wait(state).unwrap();
                        }
                    };
                    let result = factory()
                        .and_then(|command| read(command, &request.cancel, request.deadline));
                    if !request.cancel.load(Ordering::Acquire) {
                        let _ = request.reply.try_send(result);
                        request.ctx.request_repaint_of(request.viewport);
                    }
                    worker.state.lock().unwrap().active = None;
                }
            });
        if started.is_err() {
            shared.stop();
        }
        // The deadline starts on the UI request, including queue/old-child drain.
        Self(Arc::new(Owner(shared, timeout, Mutex::new(started.ok()))))
    }

    pub fn request(&self, ctx: egui::Context, viewport: egui::ViewportId) -> Pending {
        let cancel = Arc::new(AtomicBool::new(false));
        let (reply, receive) = mpsc::sync_channel(1);
        let pending = Pending {
            cancel: cancel.clone(),
            reply: receive,
            completed: Cell::new(false),
        };
        let mut state = self.0.0.state.lock().unwrap();
        if state.stopped {
            let _ = reply.try_send(Err(UNAVAILABLE.into()));
            return pending;
        }
        // Supersession also invalidates a completed but unconsumed reply.
        if let Some(current) = state.current.replace(cancel.clone()) {
            current.store(true, Ordering::Release);
        }
        if let Some(active) = &state.active {
            active.store(true, Ordering::Release);
        }
        let old = state.latest.replace(Request {
            cancel,
            deadline: Instant::now() + self.0.1,
            reply,
            ctx,
            viewport,
        });
        if let Some(old) = &old {
            old.cancel.store(true, Ordering::Release);
        }
        self.0.0.wake.notify_one();
        drop(state);
        drop(old);
        pending
    }

    /// Cancel before a Quit snapshot; a failed Quit may accept a new gesture,
    /// but none of the old results can revive.
    pub fn cancel(&self) {
        self.0.0.cancel(false);
    }

    pub fn shutdown(&self) {
        self.0.0.stop();
        // Never hold state while joining: the supervisor clears active state
        // after its Reading guard kills and reaps the packaged helper process.
        let mut owned = self.0.2.lock().unwrap();
        if let Some(worker) = owned.take() {
            let _ = worker.join();
        }
    }
}

impl Shared {
    fn stop(&self) {
        self.cancel(true);
    }

    fn cancel(&self, stop: bool) {
        let mut state = self.state.lock().unwrap();
        state.stopped |= stop;
        if let Some(current) = state.current.take() {
            current.store(true, Ordering::Release);
        }
        if let Some(active) = &state.active {
            active.store(true, Ordering::Release);
        }
        let old = state.latest.take();
        if let Some(old) = &old {
            old.cancel.store(true, Ordering::Release);
        }
        self.wake.notify_one();
        drop(state);
        drop(old);
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.0.stop();
        if let Some(worker) = self.2.get_mut().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Pending {
    pub fn poll(&self) -> Option<Result<String, String>> {
        if self.completed.get() {
            return None;
        }
        if self.cancel.load(Ordering::Acquire) {
            self.completed.set(true);
            return Some(Err(UNAVAILABLE.into()));
        }
        let result = match self.reply.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err(UNAVAILABLE.into()),
        };
        self.completed.set(true);
        Some(result)
    }

    #[cfg(test)]
    pub fn ready(result: Result<String, String>) -> Self {
        let (send, reply) = mpsc::sync_channel(1);
        send.try_send(result).unwrap();
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            reply,
            completed: Cell::new(false),
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Drop also handles spawn/read/validation failures: no detached reader lives
/// beyond a deadline, cancellation or supervisor shutdown.
struct Reading(Child);

impl Drop for Reading {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read(mut command: Command, cancel: &AtomicBool, deadline: Instant) -> Result<String, String> {
    if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
        return Err(UNAVAILABLE.into());
    }
    let mut child = Reading(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| UNAVAILABLE)?,
    );
    let mut stdout = child.0.stdout.take().unwrap();
    let flags = rustix::fs::fcntl_getfl(&stdout).map_err(|_| UNAVAILABLE)?;
    rustix::fs::fcntl_setfl(&stdout, flags | rustix::fs::OFlags::NONBLOCK)
        .map_err(|_| UNAVAILABLE)?;
    let mut bytes = Vec::new();
    let mut eof = false;
    loop {
        if cancel.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err("Primary selection read was cancelled or timed out.".into());
        }
        let mut chunk = [0; 4096];
        if !eof {
            match stdout.read(&mut chunk) {
                Ok(0) => eof = true,
                Ok(count) => {
                    if bytes.len() + count > MAX_BYTES {
                        return Err("Primary selection exceeds 64 KiB.".into());
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    continue;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(UNAVAILABLE.into()),
            }
        }
        if let Some(status) = child.0.try_wait().map_err(|_| UNAVAILABLE)?
            && eof
        {
            if !status.success() {
                return Err(UNAVAILABLE.into());
            }
            return String::from_utf8(bytes)
                .map_err(|_| "Primary selection is not valid UTF-8.".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python(code: &str) -> Command {
        let mut command = Command::new("python3");
        command.arg("-c").arg(code);
        command
    }

    fn transfer(code: &str) -> Result<String, String> {
        read(
            python(code),
            &AtomicBool::new(false),
            Instant::now() + DEADLINE,
        )
    }

    fn wait_for(mut condition: impl FnMut() -> bool) {
        let limit = Instant::now() + Duration::from_secs(3);
        while !condition() {
            assert!(Instant::now() < limit, "supervisor did not settle");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn reply(pending: &Pending) -> Result<String, String> {
        let mut result = None;
        wait_for(|| {
            result = pending.poll();
            result.is_some()
        });
        assert!(pending.poll().is_none(), "a reply is delivered only once");
        result.unwrap()
    }

    #[test]
    fn validates_byte_cap_utf8_and_success_without_truncating_or_accepting_partial_output() {
        assert_eq!(transfer("print('α\\nBeta', end='')").unwrap(), "α\nBeta");
        assert_eq!(transfer("pass").unwrap(), "");
        let boundary = transfer("import sys; sys.stdout.write('é' * 32768)").unwrap();
        assert_eq!(boundary, "é".repeat(32768));
        assert_eq!(boundary.len(), 65536);
        assert!(transfer("import sys; sys.stdout.write('é' * 32769)").is_err());
        assert!(transfer("import os; os.write(1, b'valid prefix\\xff')").is_err());
        assert!(transfer("import sys; print('partial', end=''); sys.exit(1)").is_err());
    }

    #[test]
    fn absolute_deadline_bounds_stalled_initialization_and_trickling_output() {
        for code in [
            "import time; time.sleep(20)",
            "import os,time\nwhile True:\n os.write(1,b'x'); time.sleep(.02)",
        ] {
            let start = Instant::now();
            assert!(
                read(
                    python(code),
                    &AtomicBool::new(false),
                    start + Duration::from_millis(300),
                )
                .is_err()
            );
            assert!(start.elapsed() < Duration::from_secs(2));
        }
        let scratch = tempfile::tempdir().unwrap();
        let marker = scratch.path().join("must-not-spawn");
        for (cancel, deadline) in [
            (true, Instant::now() + DEADLINE),
            (false, Instant::now() - Duration::from_millis(1)),
        ] {
            assert!(
                read(
                    python(&format!("open({:?}, 'w').close()", marker)),
                    &AtomicBool::new(cancel),
                    deadline,
                )
                .is_err()
            );
            assert!(!marker.exists());
        }
    }

    #[test]
    fn cancellation_supersession_and_owner_drop_reap_before_starting_another_child() {
        let scratch = tempfile::tempdir().unwrap();
        let pid_file = scratch.path().join("pid");
        let marker = pid_file.clone();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = requests.clone();
        let reader = Reader::start(
            move || {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(python(&format!(
                        "import os,time; open({marker:?},'w').write(str(os.getpid())); time.sleep(20)"
                    )))
                } else {
                    let pid: i32 = std::fs::read_to_string(&marker).unwrap().parse().unwrap();
                    assert!(
                        !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                        "old child must be reaped first"
                    );
                    Ok(python("print('new PRIMARY', end='')"))
                }
            },
            DEADLINE,
        );
        let old = reader.request(egui::Context::default(), egui::ViewportId::ROOT);
        wait_for(|| {
            pid_file.exists() && std::fs::read_to_string(&pid_file).is_ok_and(|s| !s.is_empty())
        });
        let new = reader.request(egui::Context::default(), egui::ViewportId::ROOT);
        assert!(reply(&old).is_err());
        assert_eq!(reply(&new).unwrap(), "new PRIMARY");
        assert_eq!(requests.load(Ordering::SeqCst), 2);

        let marker = pid_file.clone();
        let reader = Reader::start(
            move || {
                Ok(python(&format!(
                    "import os,time; open({marker:?},'w').write(str(os.getpid())); time.sleep(20)"
                )))
            },
            DEADLINE,
        );
        std::fs::remove_file(&pid_file).unwrap();
        let pending = reader.request(egui::Context::default(), egui::ViewportId::ROOT);
        wait_for(|| {
            pid_file.exists() && std::fs::read_to_string(&pid_file).is_ok_and(|s| !s.is_empty())
        });
        let pid: i32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        reader.shutdown();
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "shutdown must return only after the blocked packaged child is reaped"
        );
        drop(reader);
        assert!(reply(&pending).is_err());
        wait_for(|| !std::path::Path::new(&format!("/proc/{pid}")).exists());
    }

    #[test]
    fn cancel_invalidates_ready_results_but_shutdown_alone_is_permanent() {
        let reader = Reader::start(|| Ok(python("print('old', end='')")), DEADLINE);
        let old = reader.request(egui::Context::default(), egui::ViewportId::ROOT);
        wait_for(|| {
            let state = reader.0.0.state.lock().unwrap();
            state.latest.is_none() && state.active.is_none()
        });
        reader.cancel();
        assert!(reply(&old).is_err());
        assert_eq!(
            reply(&reader.request(egui::Context::default(), egui::ViewportId::ROOT)).unwrap(),
            "old"
        );
        reader.shutdown();
        assert!(reply(&reader.request(egui::Context::default(), egui::ViewportId::ROOT)).is_err());
    }

    #[test]
    fn latest_slot_is_bounded_and_queue_time_is_not_a_fresh_deadline() {
        let scratch = tempfile::tempdir().unwrap();
        let marker = scratch.path().join("must-not-spawn");
        let gate = Arc::new(std::sync::Barrier::new(2));
        let worker_gate = gate.clone();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = calls.clone();
        let file = marker.clone();
        let reader = Reader::start(
            move || {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    worker_gate.wait();
                    worker_gate.wait();
                }
                Ok(python(&format!("open({file:?}, 'w').close()")))
            },
            Duration::from_millis(100),
        );
        let ctx = egui::Context::default();
        let first = reader.request(ctx.clone(), egui::ViewportId::ROOT);
        gate.wait();
        let mut last = reader.request(ctx.clone(), egui::ViewportId::ROOT);
        for _ in 0..100 {
            last = reader.request(ctx.clone(), egui::ViewportId::ROOT);
        }
        std::thread::sleep(Duration::from_millis(120));
        gate.wait();
        assert!(reply(&first).is_err());
        assert!(reply(&last).is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "only active and latest survive"
        );
        assert!(!marker.exists(), "expired requests cannot spawn");
    }

    #[test]
    fn helper_arguments_fail_closed_and_commands_select_one_backend() {
        assert!(helper(["--ordinary".into()].into_iter()).is_none());
        for args in [
            vec![HELPER],
            vec![HELPER, "bad", "1"],
            vec![HELPER, "x11", "0"],
            vec![HELPER, "wayland", "no"],
            vec![HELPER, "x11", "1", "extra"],
        ] {
            assert!(
                helper(args.into_iter().map(str::to_owned))
                    .unwrap()
                    .is_err()
            );
        }
        assert!(command(None).is_err());
        for (backend, name, removed) in [
            (Backend::Wayland, "wayland", vec!["DISPLAY"]),
            (
                Backend::X11,
                "x11",
                vec!["WAYLAND_DISPLAY", "WAYLAND_SOCKET"],
            ),
        ] {
            let command = command(Some(backend)).unwrap();
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                [HELPER, name, &std::process::id().to_string()].map(std::ffi::OsStr::new)
            );
            assert_eq!(
                command.get_envs().collect::<Vec<_>>(),
                removed
                    .into_iter()
                    .map(|key| (std::ffi::OsStr::new(key), None))
                    .collect::<Vec<_>>()
            );
        }
    }
}
