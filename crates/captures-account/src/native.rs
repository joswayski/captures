//! Native host worker: explicit account operations and sharing, independent of
//! any window/preview lifetime. Hosts submit one operation at a time and render
//! replies; this worker alone owns the bearer, OTP challenge and selected bytes.
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{AccountClient, Error as AccountError, OsVault, User, Vault, sharing};

#[derive(Clone)]
pub struct Selection {
    pub artifact_id: String,
    pub path: PathBuf,
    pub name: String,
    pub content_type: String,
}

pub enum Command {
    Open(Selection),
    Refresh,
    RequestCode(String),
    Verify(String),
    RetrySave,
    Logout,
    Upload(sharing::SharePatch),
    Configure {
        enabled: bool,
        patch: sharing::SharePatch,
    },
    Trash,
    Restore,
    Shutdown,
}

#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    SignedOut,
    CodeSent,
    /// The OTP was accepted. Retry only vault persistence, never verification.
    SaveRequired,
    SignedIn(User),
    /// Vault/network failure is not proof of a missing or expired session.
    Unavailable,
}

pub struct State {
    pub auth: Auth,
    pub opened: sharing::Opened,
    pub error: Option<sharing::Error>,
}

pub enum Event {
    Finished(State),
    Progress { read: u64, total: u64 },
}

pub struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Spawn a worker without touching the vault or network until a command.
    /// Call only in response to the user's Share action; do not use on startup.
    pub fn production(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self::spawn(root, AccountClient::<OsVault>::production, wake)
    }

    /// Inject an already origin-bound client for disposable host tests. The
    /// production OS vault still cannot be bound to a noncanonical origin.
    pub fn with_client<V: Vault + Send + 'static>(
        root: PathBuf,
        account: AccountClient<V>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self::spawn(root, move || Ok(account), wake)
    }

    pub fn send(&self, command: Command) -> bool {
        self.cancelled.store(false, Ordering::Release);
        self.tx.send(command).is_ok()
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Cancel cooperatively, then join bounded HTTP/vault work before exit.
    /// Preview/popup closure must not call this method.
    pub fn shutdown(&mut self) {
        self.cancel();
        let _ = self.tx.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }

    fn spawn<V: Vault + Send + 'static>(
        root: PathBuf,
        factory: impl FnOnce() -> Result<AccountClient<V>, AccountError> + Send + 'static,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let thread = thread::spawn(move || {
            let mut account = factory();
            let mut selected: Option<Selection> = None;
            let mut snapshot: Option<tempfile::NamedTempFile> = None;
            let mut auth = Auth::Unavailable;
            let mut challenge = None;
            let mut opened = sharing::Opened::Unassociated;
            while let Ok(command) = jobs.recv() {
                if matches!(command, Command::Shutdown) {
                    break;
                }
                let result = (|| {
                    let account = account.as_mut().map_err(|e| sharing::Error::Account(*e))?;
                    match command {
                        Command::Open(selection) => {
                            // Retain the same snapshot/settings on close/reopen.
                            if selected
                                .as_ref()
                                .is_none_or(|s| s.artifact_id != selection.artifact_id)
                            {
                                snapshot = None;
                                selected = Some(selection);
                                challenge = None;
                                opened = sharing::Opened::Unassociated;
                                if auth == Auth::CodeSent {
                                    auth = Auth::SignedOut;
                                }
                            }
                            // Pin before vault prompts or network work can outlive
                            // the local preview/file. Defer a missing-file error
                            // until we know whether a Ready asset can be managed.
                            let snapshot_error = if snapshot.is_none() {
                                match copy_original(
                                    &selected.as_ref().ok_or(sharing::Error::InvalidInput)?.path,
                                    &cancel,
                                ) {
                                    Ok(copy) => {
                                        snapshot = Some(copy);
                                        None
                                    }
                                    Err(error) => Some(error),
                                }
                            } else {
                                None
                            };
                            if auth != Auth::CodeSent {
                                authenticate(account, &mut auth)?;
                            }
                            if matches!(auth, Auth::SignedIn(_)) {
                                opened = coordinator(account, &root)?.open(id(&selected)?)?;
                            }
                            if !matches!(opened, sharing::Opened::Asset(_))
                                && let Some(error) = snapshot_error
                            {
                                return Err(error);
                            }
                        }
                        Command::Refresh => {
                            authenticate(account, &mut auth)?;
                            if matches!(auth, Auth::SignedIn(_)) {
                                opened = coordinator(account, &root)?.open(id(&selected)?)?;
                            }
                        }
                        Command::RequestCode(email) => {
                            challenge = Some(account.request_code(&email)?);
                            auth = Auth::CodeSent;
                        }
                        Command::Verify(code) => {
                            match account.verify(
                                challenge.as_deref().ok_or(sharing::Error::InvalidInput)?,
                                &code,
                            ) {
                                Ok(user) => auth = Auth::SignedIn(user),
                                Err(error @ AccountError::Vault(_)) => {
                                    auth = Auth::SaveRequired;
                                    return Err(error.into());
                                }
                                Err(error) => return Err(error.into()),
                            }
                            challenge = None;
                            opened = coordinator(account, &root)?.open(id(&selected)?)?;
                        }
                        Command::RetrySave => {
                            let user = account.retry_save()?.ok_or(sharing::Error::InvalidInput)?;
                            auth = Auth::SignedIn(user);
                            challenge = None;
                            opened = coordinator(account, &root)?.open(id(&selected)?)?;
                        }
                        Command::Logout => {
                            account.logout()?;
                            auth = Auth::SignedOut;
                            challenge = None;
                            opened = sharing::Opened::Unassociated;
                        }
                        Command::Upload(patch) => {
                            let selection =
                                selected.as_ref().ok_or(sharing::Error::InvalidInput)?;
                            let mut sharing = coordinator(account, &root)?;
                            opened = sharing.open(&selection.artifact_id)?;
                            if !matches!(opened, sharing::Opened::Asset(_)) {
                                if snapshot.is_none() {
                                    snapshot = Some(copy_original(&selection.path, &cancel)?);
                                }
                                let sender = out.clone();
                                let wake = wake.clone();
                                let last =
                                    std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
                                let progress = Arc::new(move |read, total| {
                                    let mut last = last.lock().unwrap();
                                    if last.elapsed() >= Duration::from_millis(100) || read == total
                                    {
                                        *last = Instant::now();
                                        let _ = sender.send(Event::Progress { read, total });
                                        wake();
                                    }
                                });
                                let asset = sharing.upload(
                                    &selection.artifact_id,
                                    snapshot.as_ref().unwrap().path(),
                                    &selection.name,
                                    &selection.content_type,
                                    cancel.clone(),
                                    progress,
                                )?;
                                opened = sharing::Opened::Asset(Box::new(asset));
                            }
                            if cancel.load(Ordering::Acquire) {
                                return Err(sharing::Error::Cancelled);
                            }
                            // Publish a link only after this succeeds. A failed
                            // configuration keeps the Ready asset for retry.
                            let share =
                                sharing.configure_share(&selection.artifact_id, true, patch)?;
                            if let sharing::Opened::Asset(asset) = &mut opened {
                                asset.share = share;
                            }
                        }
                        Command::Configure { enabled, patch } => {
                            let mut sharing = coordinator(account, &root)?;
                            opened = sharing.open(id(&selected)?)?;
                            let share = sharing.configure_share(id(&selected)?, enabled, patch)?;
                            if let sharing::Opened::Asset(asset) = &mut opened {
                                asset.share = share;
                            }
                        }
                        Command::Trash => {
                            coordinator(account, &root)?.trash(id(&selected)?)?;
                            opened = coordinator(account, &root)?.open(id(&selected)?)?;
                        }
                        Command::Restore => {
                            coordinator(account, &root)?.restore(id(&selected)?)?;
                            opened = coordinator(account, &root)?.open(id(&selected)?)?;
                        }
                        Command::Shutdown => unreachable!(),
                    }
                    Ok(())
                })();
                if result == Err(sharing::Error::Account(AccountError::InvalidSession)) {
                    auth = Auth::SignedOut;
                    challenge = None;
                    opened = sharing::Opened::Unassociated;
                } else if account
                    .as_ref()
                    .is_ok_and(|a| matches!(a.session, crate::Session::Invalid))
                {
                    // Revocation succeeded but deleting the vault item failed.
                    // Refresh retries deletion before offering a new sign-in.
                    auth = Auth::Unavailable;
                    opened = sharing::Opened::Unassociated;
                }
                let _ = out.send(Event::Finished(State {
                    auth: auth.clone(),
                    opened: opened.clone(),
                    error: result.err(),
                }));
                wake();
            }
        });
        Self {
            tx,
            rx,
            cancelled,
            thread: Some(thread),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn authenticate<V: Vault>(
    account: &mut AccountClient<V>,
    auth: &mut Auth,
) -> Result<(), sharing::Error> {
    // Never consume an OTP again after a failed vault write.
    if *auth == Auth::SaveRequired {
        return Ok(());
    }
    account.clear_invalid()?;
    *auth = if account.load()? {
        Auth::SignedIn(account.me()?)
    } else {
        Auth::SignedOut
    };
    Ok(())
}

fn coordinator<'a, V: Vault>(
    account: &'a mut AccountClient<V>,
    root: &std::path::Path,
) -> Result<sharing::SharingCoordinator<'a, V>, sharing::Error> {
    sharing::SharingCoordinator::new(account, sharing::AssociationStore::new(root))
}

fn id(selected: &Option<Selection>) -> Result<&str, sharing::Error> {
    selected
        .as_ref()
        .map(|s| s.artifact_id.as_str())
        .ok_or(sharing::Error::InvalidInput)
}

fn copy_original(
    path: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<tempfile::NamedTempFile, sharing::Error> {
    let mut source = File::open(path).map_err(|_| sharing::Error::MissingFile)?;
    let before = source.metadata().map_err(|_| sharing::Error::MissingFile)?;
    if !before.is_file() || before.len() == 0 {
        return Err(sharing::Error::MissingFile);
    }
    // Private temporary bytes, not the thumbnail or a lossy re-encode. They are
    // retained by this worker until selection changes/shutdown, not by a card.
    let mut snapshot = tempfile::NamedTempFile::new().map_err(|_| sharing::Error::Storage)?;
    let mut buffer = [0; 64 * 1024];
    let mut copied = 0;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(sharing::Error::Cancelled);
        }
        let read = source
            .read(&mut buffer)
            .map_err(|_| sharing::Error::MissingFile)?;
        if read == 0 {
            break;
        }
        snapshot
            .write_all(&buffer[..read])
            .map_err(|_| sharing::Error::Storage)?;
        copied += read as u64;
    }
    let after = source.metadata().map_err(|_| sharing::Error::MissingFile)?;
    if copied != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err(sharing::Error::ChangedFile);
    }
    snapshot.flush().map_err(|_| sharing::Error::Storage)?;
    Ok(snapshot)
}
