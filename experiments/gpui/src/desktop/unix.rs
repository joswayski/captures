use super::Action;
use std::{
    io::{Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

pub fn private_directory(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private directory is not an owner-controlled directory",
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[allow(dead_code)]
pub fn private_file(path: &std::path::Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "private file is not an owner-controlled regular file",
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

pub struct Instance {
    commands: mpsc::Receiver<Action>,
    running: Arc<AtomicBool>,
    path: PathBuf,
    _lock: std::fs::File,
}

impl Instance {
    pub fn acquire(args: &[String]) -> anyhow::Result<Option<Self>> {
        Self::acquire_at(crate::settings::data_dir(), args)
    }

    fn acquire_at(directory: PathBuf, args: &[String]) -> anyhow::Result<Option<Self>> {
        private_directory(&directory)?;
        let path = directory.join("instance.sock");
        let lock_path = directory.join("instance.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&lock_path)?;
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o600))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                for _ in 0..100 {
                    if let Ok(mut stream) = UnixStream::connect(&path) {
                        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
                        stream.write_all(&serde_json::to_vec(args)?)?;
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                anyhow::bail!("Captures is already running but its command channel is unavailable");
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (send, commands) = mpsc::sync_channel(64);
        let running = Arc::new(AtomicBool::new(true));
        let worker = running.clone();
        std::thread::spawn(move || {
            while worker.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                        let mut bytes = vec![];
                        if Read::by_ref(&mut stream)
                            .take(64 * 1024 + 1)
                            .read_to_end(&mut bytes)
                            .is_ok()
                            && bytes.len() <= 64 * 1024
                            && let Ok(args) = serde_json::from_slice(&bytes)
                            && send.send(Action::Arguments(args)).is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Some(Self {
            commands,
            running,
            path,
            _lock: lock,
        }))
    }

    pub fn commands(&self) -> Vec<Action> {
        let mut commands: Vec<_> = self.commands.try_iter().collect();
        commands.extend(super::take_native_events());
        commands
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_directory_creates_missing_path_and_private_file_secures_existing_file() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("missing/nested");
        private_directory(&directory).unwrap();
        assert_eq!(std::fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
        let file = directory.join("state.json");
        std::fs::write(&file, b"{}").unwrap();
        private_file(&file).unwrap();
        assert_eq!(std::fs::metadata(file).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn second_instance_forwards_exact_arguments_and_uses_private_files() {
        let directory = tempfile::tempdir().unwrap();
        let first = Instance::acquire_at(directory.path().into(), &[])
            .unwrap()
            .unwrap();
        let args = vec!["--open".into(), "/tmp/a file with spaces.png".into()];
        assert!(
            Instance::acquire_at(directory.path().into(), &args)
                .unwrap()
                .is_none()
        );
        let received = first.commands.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(received, Action::Arguments(values) if values == args));
        assert_eq!(
            std::fs::metadata(directory.path().join("instance.sock"))
                .unwrap()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn partial_ipc_message_is_reassembled_off_the_ui_thread() {
        let directory = tempfile::tempdir().unwrap();
        let instance = Instance::acquire_at(directory.path().into(), &[])
            .unwrap()
            .unwrap();
        let mut stream = UnixStream::connect(directory.path().join("instance.sock")).unwrap();
        stream.write_all(b"[\"--open\",\"/tmp/").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(instance.commands().is_empty());
        stream.write_all(b"asymmetric.png\"]").unwrap();
        drop(stream);
        let received = instance
            .commands
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(
            matches!(received, Action::Arguments(values) if values == ["--open", "/tmp/asymmetric.png"])
        );
    }
}
