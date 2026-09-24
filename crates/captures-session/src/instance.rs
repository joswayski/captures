//! Native development-host election and bounded, current-user local IPC.
//!
//! The canonical History root is the profile identity, independent of settings
//! overrides. Acquire before opening any host windows, workers or global keys.
//! Acknowledgement means queued, not that media decoded or was persisted. Never
//! retry an exchange after sending: a lost acknowledgement has an unknown outcome.
//! No TCP, idle polling, durable delivery, or installed-app data is involved.

mod platform;

use interprocess::local_socket::{
    Name,
    tokio::{Listener, Stream, prelude::*},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    runtime::{Builder, Runtime},
    sync::oneshot,
    time::timeout,
};

const MAX_PATHS: usize = 64;
const MAX_BYTES: usize = 256 * 1024;
const MAX_QUEUED: usize = 32;
const IO_DEADLINE: Duration = Duration::from_secs(2);
const STARTUP_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    pub paths: Vec<PathBuf>,
}

impl OpenRequest {
    /// Resolve against the sender's CWD without requiring files to exist. Media
    /// validation and per-file errors remain the existing host queue's job.
    pub fn from_paths(paths: Vec<PathBuf>) -> io::Result<Self> {
        let cwd = std::env::current_dir()?;
        let request = Self {
            paths: paths
                .into_iter()
                .map(|path| {
                    if path.as_os_str().is_empty() {
                        Err(invalid("Empty media path"))
                    } else {
                        std::path::absolute(cwd.join(path))
                    }
                })
                .collect::<io::Result<_>>()?,
        };
        request.validate()?;
        Ok(request)
    }

    fn validate(&self) -> io::Result<()> {
        if self.paths.len() > MAX_PATHS
            || self.paths.iter().any(|path| {
                !path.is_absolute()
                    || path.as_os_str().as_encoded_bytes().contains(&0)
                    || path.as_os_str().len() > 32 * 1024
            })
        {
            return Err(invalid("Invalid or oversized native open request"));
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    request: OpenRequest,
}

pub enum Launch {
    Primary(Instance),
    Forwarded,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Inbox {
    requests: VecDeque<OpenRequest>,
    wake: Option<Wake>,
    failure: Option<String>,
}

/// Drop on host shutdown, after normal accepted UI work drains. Cancels partial
/// exchanges, joins the worker and destroys its listener BEFORE releasing the
/// stable-inode election lock. Explicit quit/crash does not promise durable opens.
pub struct Instance {
    inbox: Arc<Mutex<Inbox>>,
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    _lock: File,
}

impl Instance {
    /// A secondary blocks only for bounded startup/connect/exchange deadlines.
    /// Only the lock holder may reclaim a stale Unix socket. A failed connection
    /// never makes a secondary primary while another process still holds the lock.
    pub fn start(history_root: &Path, request: OpenRequest) -> io::Result<Launch> {
        let payload = encode(&request)?;
        fs::create_dir_all(history_root)?;
        let root = history_root.canonicalize()?;
        let directory = platform::directory()?;
        let mut hash = Sha256::new();
        hash.update(directory.as_os_str().as_encoded_bytes());
        hash.update([0]);
        hash.update(root.as_os_str().as_encoded_bytes());
        let key = format!("{:x}", hash.finalize());
        let endpoint = directory.join(format!("{}.sock", &key[..32]));
        let name = endpoint_name(&endpoint, &key)?;
        let lock_path = directory.join(format!("{}.lock", &key[..32]));
        if let Ok(metadata) = fs::symlink_metadata(&lock_path)
            && !metadata.file_type().is_file()
        {
            return Err(invalid("Native instance lock is not a regular file"));
        }
        // Never unlink this file: concurrent owners must lock the same inode.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        let runtime = runtime()?;
        let started = Instant::now();
        loop {
            match lock.try_lock() {
                Ok(()) => return Self::listen(lock, endpoint, name).map(Launch::Primary),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(error)) => return Err(error),
            }
            // Retries are only before ANY request bytes are sent; no duplicate
            // opens on a lost acknowledgement or late primary shutdown.
            if let Ok(Ok(mut stream)) = runtime.block_on(timeout_connect(name.clone())) {
                runtime.block_on(async {
                    timeout(IO_DEADLINE, async {
                        stream.write_all(&payload).await?;
                        match stream.read_u8().await? {
                            1 => Ok(()),
                            2 => Err(io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "Native open queue is full; try again",
                            )),
                            _ => Err(invalid("Native instance rejected the open request")),
                        }
                    })
                    .await
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Native open acknowledgement timed out; delivery is unknown",
                        )
                    })?
                })?;
                return Ok(Launch::Forwarded);
            }
            if started.elapsed() >= STARTUP_DEADLINE {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The running native instance did not become ready; no files were sent",
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn listen(lock: File, endpoint: PathBuf, name: Name<'static>) -> io::Result<Self> {
        // Directory is private and election is held. Never replace arbitrary
        // filesystem content or an endpoint while a live owner holds its lock.
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            match fs::symlink_metadata(&endpoint) {
                Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(&endpoint)?,
                Ok(_) => return Err(invalid("Native IPC endpoint is not a socket")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        #[cfg(windows)]
        let _ = endpoint;
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let shared = inbox.clone();
        let (stop, mut stopped) = oneshot::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("native-instance".into())
            .spawn(move || {
                let result = runtime().and_then(|runtime| {
                    runtime.block_on(async {
                        let listener: Listener =
                            platform::listener_options(name)?.create_tokio()?;
                        let _ = ready_tx.send(Ok(()));
                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut stopped => break,
                                result = listener.accept() => {
                                    let stream = result?;
                                    // Serial exchanges preserve acceptance order; each slow
                                    // peer holds the server for at most one total deadline.
                                    tokio::select! {
                                        biased;
                                        _ = &mut stopped => break,
                                        _ = timeout(IO_DEADLINE, receive(stream, &shared)) => {}
                                    }
                                }
                            }
                        }
                        Ok(())
                    })
                });
                if let Err(error) = result {
                    let _ = ready_tx.try_send(Err(io::Error::new(error.kind(), error.to_string())));
                    let wake = {
                        let mut inbox = shared.lock().unwrap();
                        inbox.failure = Some(error.to_string());
                        inbox.wake.clone()
                    };
                    if let Some(wake) = wake {
                        wake();
                    }
                }
            })?;
        match ready_rx.recv().map_err(io::Error::other)? {
            Ok(()) => Ok(Self {
                inbox,
                stop: Some(stop),
                worker: Some(worker),
                _lock: lock,
            }),
            Err(error) => {
                let _ = worker.join();
                Err(error)
            }
        }
    }

    /// Install after the UI event proxy exists. Scheduling only; no synchronous
    /// reentry. Installing also wakes for requests received during renderer startup.
    pub fn set_wake(&self, wake: impl Fn() + Send + Sync + 'static) {
        let wake: Wake = Arc::new(wake);
        let pending = {
            let mut inbox = self.inbox.lock().unwrap();
            inbox.wake = Some(wake.clone());
            !inbox.requests.is_empty() || inbox.failure.is_some()
        };
        if pending {
            wake();
        }
    }

    pub fn next_request(&self) -> io::Result<Option<OpenRequest>> {
        let mut inbox = self.inbox.lock().unwrap();
        if let Some(error) = inbox.failure.take() {
            return Err(io::Error::other(error));
        }
        Ok(inbox.requests.pop_front())
    }

    /// Stop new deliveries after the user accepts quit, but retain election while
    /// the host drains its capture/export workers. A cancelled quit must not call this.
    pub fn stop_accepting(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.stop_accepting();
        // _lock drops only after the listener (and all connected streams) above.
    }
}

fn runtime() -> io::Result<Runtime> {
    Builder::new_current_thread().enable_all().build()
}

async fn timeout_connect(
    name: Name<'static>,
) -> Result<io::Result<Stream>, tokio::time::error::Elapsed> {
    timeout(Duration::from_millis(100), Stream::connect(name)).await
}

fn encode(request: &OpenRequest) -> io::Result<Vec<u8>> {
    request.validate()?;
    let bytes = serde_json::to_vec(&Envelope {
        version: 1,
        request: request.clone(),
    })?;
    if bytes.len() > MAX_BYTES {
        return Err(invalid("Native open request exceeds 256 KiB"));
    }
    let mut framed = (bytes.len() as u32).to_be_bytes().to_vec();
    framed.extend(bytes);
    Ok(framed)
}

async fn receive(mut stream: Stream, shared: &Arc<Mutex<Inbox>>) -> io::Result<()> {
    let size = stream.read_u32().await? as usize;
    if size == 0 || size > MAX_BYTES {
        return Err(invalid("Invalid native request size"));
    }
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    let envelope: Envelope = serde_json::from_slice(&bytes)?;
    if envelope.version != 1 {
        return Err(invalid("Unsupported native request version"));
    }
    envelope.request.validate()?;
    let (reply, wake) = {
        let mut inbox = shared.lock().unwrap();
        if inbox.requests.len() == MAX_QUEUED {
            (2, None)
        } else {
            inbox.requests.push_back(envelope.request);
            (1, inbox.wake.clone())
        }
    };
    if let Some(wake) = wake {
        wake();
    }
    stream.write_all(&[reply]).await
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(unix)]
fn endpoint_name(path: &Path, _: &str) -> io::Result<Name<'static>> {
    use interprocess::local_socket::{GenericFilePath, ToFsName};
    path.to_owned().to_fs_name::<GenericFilePath>()
}

#[cfg(windows)]
fn endpoint_name(_: &Path, key: &str) -> io::Result<Name<'static>> {
    use interprocess::local_socket::{GenericNamespaced, ToNsName};
    format!("captures-native-{key}").to_ns_name::<GenericNamespaced>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        process::{Command, Stdio},
        sync::Barrier,
    };

    fn request(paths: &[&str]) -> OpenRequest {
        OpenRequest::from_paths(paths.iter().map(PathBuf::from).collect()).unwrap()
    }

    fn primary(root: &Path) -> Instance {
        match Instance::start(root, request(&[])).unwrap() {
            Launch::Primary(instance) => instance,
            Launch::Forwarded => panic!("expected primary"),
        }
    }

    fn isolated_listener() -> (tempfile::TempDir, Instance, Name<'static>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        let key = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
        let name = endpoint_name(&path, &key).unwrap();
        let lock = File::create(dir.path().join("lock")).unwrap();
        lock.try_lock().unwrap();
        let server = Instance::listen(lock, path, name.clone()).unwrap();
        (dir, server, name)
    }

    #[test]
    fn election_fifo_profile_isolation_late_wake_and_reclaim() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("history");
        let server = primary(&root);
        let paths = request(&["second image.png", "first.webm"]);
        assert!(matches!(
            Instance::start(&root, paths.clone()).unwrap(),
            Launch::Forwarded
        ));
        assert!(matches!(
            Instance::start(&root, request(&[])).unwrap(),
            Launch::Forwarded
        ));
        let (tx, rx) = mpsc::channel();
        server.set_wake(move || {
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(1)).unwrap(); // late UI installation
        assert_eq!(server.next_request().unwrap(), Some(paths));
        assert_eq!(server.next_request().unwrap(), Some(request(&[])));
        assert_eq!(server.next_request().unwrap(), None);
        assert!(
            rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "no idle wakes"
        );
        let other = primary(&dir.path().join("other-history"));
        drop(other);
        drop(server);
        drop(primary(&root));
    }

    #[test]
    fn simultaneous_launches_elect_exactly_one_owner() {
        let root = tempfile::tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let root = root.path().to_owned();
                thread::spawn(move || {
                    barrier.wait();
                    Instance::start(&root, request(&[])).unwrap()
                })
            })
            .collect();
        let launches: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        let owners: Vec<_> = launches
            .iter()
            .filter_map(|launch| match launch {
                Launch::Primary(owner) => Some(owner),
                _ => None,
            })
            .collect();
        assert_eq!(owners.len(), 1);
        for _ in 0..7 {
            assert_eq!(owners[0].next_request().unwrap(), Some(request(&[])));
        }
        assert_eq!(owners[0].next_request().unwrap(), None);
    }

    #[test]
    fn queue_bound_rejects_without_dropping_older_requests() {
        let root = tempfile::tempdir().unwrap();
        let server = primary(root.path());
        for i in 0..MAX_QUEUED {
            assert!(matches!(
                Instance::start(root.path(), request(&[&format!("{i}.png")])).unwrap(),
                Launch::Forwarded
            ));
        }
        assert_eq!(
            Instance::start(root.path(), request(&["overflow.png"]))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        for i in 0..MAX_QUEUED {
            assert_eq!(
                server.next_request().unwrap(),
                Some(request(&[&format!("{i}.png")]))
            );
        }
        assert_eq!(server.next_request().unwrap(), None);
        assert!(matches!(
            Instance::start(root.path(), request(&["retry.png"])).unwrap(),
            Launch::Forwarded
        ));
        assert_eq!(
            server.next_request().unwrap(),
            Some(request(&["retry.png"]))
        );
    }

    #[test]
    fn stopped_listener_keeps_election_until_host_teardown() {
        let root = tempfile::tempdir().unwrap();
        let mut server = primary(root.path());
        server.stop_accepting();
        let started = Instant::now();
        let result = Instance::start(root.path(), request(&["during-quit.png"]));
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < STARTUP_DEADLINE + Duration::from_secs(2));
        assert_eq!(server.next_request().unwrap(), None);
        drop(server);
        drop(primary(root.path()));
    }

    #[test]
    fn rejects_invalid_paths_and_bounded_frames() {
        assert!(OpenRequest::from_paths(vec![PathBuf::new()]).is_err());
        assert!(
            encode(&OpenRequest {
                paths: vec!["relative.png".into()]
            })
            .is_err()
        );
        let path = std::env::current_dir().unwrap().join("valid.png");
        assert!(
            encode(&OpenRequest {
                paths: vec![path.clone(); MAX_PATHS]
            })
            .is_ok()
        );
        assert!(
            encode(&OpenRequest {
                paths: vec![path; MAX_PATHS + 1]
            })
            .is_err()
        );
        let huge = std::env::current_dir().unwrap().join("x".repeat(30 * 1024));
        assert!(
            encode(&OpenRequest {
                paths: vec![huge; 10]
            })
            .is_err()
        );
        let (_dir, server, name) = isolated_listener();
        runtime().unwrap().block_on(async {
            for frame in [vec![255; 4], vec![0; 4], b"\0\0\0\x02{}".to_vec()] {
                let mut stream = Stream::connect(name.clone()).await.unwrap();
                stream.write_all(&frame).await.unwrap();
                assert!(
                    timeout(IO_DEADLINE, stream.read_u8())
                        .await
                        .unwrap()
                        .is_err()
                );
            }
        });
        assert_eq!(server.next_request().unwrap(), None);
    }

    #[test]
    fn partial_peer_deadline_and_shutdown_are_bounded() {
        let (_dir, server, name) = isolated_listener();
        let rt = runtime().unwrap();
        rt.block_on(async {
            let mut peer = Stream::connect(name.clone()).await.unwrap();
            peer.write_all(&[0, 0]).await.unwrap(); // incomplete length header
            let started = Instant::now();
            assert!(
                timeout(IO_DEADLINE + Duration::from_secs(2), peer.read_u8())
                    .await
                    .unwrap()
                    .is_err()
            );
            assert!(started.elapsed() >= IO_DEADLINE / 2);
            let mut valid = Stream::connect(name.clone()).await.unwrap();
            valid
                .write_all(&encode(&request(&["after-stall.png"])).unwrap())
                .await
                .unwrap();
            assert_eq!(valid.read_u8().await.unwrap(), 1);
        });
        assert_eq!(
            server.next_request().unwrap(),
            Some(request(&["after-stall.png"]))
        );
        let _stalled = rt.block_on(Stream::connect(name)).unwrap();
        let started = Instant::now();
        drop(server);
        assert!(
            started.elapsed() < IO_DEADLINE / 2,
            "shutdown must cancel stalled I/O"
        );
    }

    #[test]
    fn process_child() {
        let Some(root) = std::env::var_os("CAPTURES_INSTANCE_TEST_ROOT") else {
            return;
        };
        if let Some(ready) = std::env::var_os("CAPTURES_INSTANCE_TEST_READY") {
            let _owner = primary(Path::new(&root));
            fs::write(ready, b"primary").unwrap();
            let _ = std::io::stdin().read_exact(&mut [0]);
        } else {
            assert!(matches!(
                Instance::start(
                    Path::new(&root),
                    request(&["relative space.png", "later.webm"])
                )
                .unwrap(),
                Launch::Forwarded
            ));
        }
    }

    fn child(root: &Path) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "instance::tests::process_child"])
            .env("CAPTURES_INSTANCE_TEST_ROOT", root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        command
    }

    #[test]
    fn cross_process_sender_cwd_and_crashed_owner_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("history");
        let server = primary(&root);
        let sender = dir.path().join("sender");
        fs::create_dir(&sender).unwrap();
        // Unix getcwd resolves directory aliases (notably /var -> /private/var
        // on macOS). The nonexistent leaf paths must still remain acceptable.
        #[cfg(unix)]
        let expected_sender = fs::canonicalize(&sender).unwrap();
        #[cfg(not(unix))]
        let expected_sender = sender.clone();
        assert!(
            child(&root)
                .current_dir(&sender)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            server.next_request().unwrap().unwrap().paths,
            vec![
                expected_sender.join("relative space.png"),
                expected_sender.join("later.webm")
            ]
        );
        drop(server);

        let ready = dir.path().join("ready");
        let mut owner = child(&root)
            .env("CAPTURES_INSTANCE_TEST_READY", &ready)
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !ready.exists() && started.elapsed() < Duration::from_secs(10) {
            assert!(owner.try_wait().unwrap().is_none());
            thread::sleep(Duration::from_millis(10));
        }
        let ready = ready.exists();
        owner.kill().unwrap();
        owner.wait().unwrap();
        assert!(ready, "child never became primary");
        // Kernel releases the lock, but Unix socket path survives forced exit.
        drop(primary(&root));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_history_uses_same_owner() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("history");
        let owner = primary(&root);
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        assert!(matches!(
            Instance::start(&alias, request(&[])).unwrap(),
            Launch::Forwarded
        ));
        assert_eq!(owner.next_request().unwrap(), Some(request(&[])));
    }
}
