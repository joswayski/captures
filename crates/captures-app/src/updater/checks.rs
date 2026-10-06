//! Explicit, check-only native development updates. No automatic requests,
//! downloads, installation, profile access or channel activation.
use std::{
    io::Read,
    path::Path,
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
};

use captures_media::CancelToken;
use serde::Serialize;

use super::{Error, ReleaseInfo, Renderer, Target, UpdateClient};

/// Both native hosts pin an explicitly supplied, bounded Minisign public key.
/// This reads only the key file; construction performs no network request.
pub fn client_from_key_file(
    endpoint: &str,
    key_file: &Path,
    renderer: Renderer,
    current_version: &str,
) -> Result<UpdateClient, Error> {
    const LIMIT: u64 = 8 * 1024;
    let invalid_key = || {
        Error::Configuration("public key must be a readable regular UTF-8 file of at most 8 KiB")
    };
    if !key_file.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(invalid_key());
    }
    let file = std::fs::File::open(key_file).map_err(|_| invalid_key())?;
    let mut key = String::new();
    file.take(LIMIT + 1)
        .read_to_string(&mut key)
        .map_err(|_| invalid_key())?;
    if key.len() as u64 > LIMIT {
        return Err(invalid_key());
    }
    UpdateClient::new(
        endpoint,
        &key,
        renderer,
        Target::current_host().ok_or(Error::Configuration("unsupported native update host"))?,
        current_version,
    )
}

/// Read-only metadata states cannot express downloading or installation.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CheckStatus {
    Idle,
    Checking,
    UpToDate,
    Available { release: ReleaseInfo },
    Error { message: String },
}

/// UI copy shared by AppKit and wgpu. A new worker stays idle until Check Now.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Presentation {
    pub version: String,
    pub channel: &'static str,
    pub status: String,
    pub action: &'static str,
    pub enabled: bool,
    pub failed: bool,
    pub detail: &'static str,
}

pub struct CheckWorker {
    current_version: String,
    status: CheckStatus,
    jobs: mpsc::Sender<()>,
    results: mpsc::Receiver<CheckStatus>,
    cancel: CancelToken,
    thread: Option<JoinHandle<()>>,
}

impl CheckWorker {
    /// Construction performs no request. The worker only reads authenticated
    /// metadata; it does not retain an installable or downloaded package.
    pub fn new(client: UpdateClient, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let current_version = client.current_version.to_string();
        let (jobs, requests) = mpsc::channel();
        let (finished, results) = mpsc::channel();
        let cancel = CancelToken::default();
        let cancelled = cancel.clone();
        let thread = thread::spawn(move || {
            while requests.recv().is_ok() {
                if cancelled.is_cancelled() {
                    break;
                }
                let checked = client.check(&cancelled);
                if cancelled.is_cancelled() {
                    break;
                }
                let status = match checked {
                    Ok(Some(update)) => CheckStatus::Available {
                        release: update.info().clone(),
                    },
                    Ok(None) => CheckStatus::UpToDate,
                    Err(error) => CheckStatus::Error {
                        message: error.to_string(),
                    },
                };
                if finished.send(status).is_err() {
                    break;
                }
                wake();
            }
        });
        Self {
            current_version,
            status: CheckStatus::Idle,
            jobs,
            results,
            cancel,
            thread: Some(thread),
        }
    }

    /// Repeated input during a check cannot enqueue duplicate HTTP requests.
    pub fn check(&mut self) -> bool {
        self.poll();
        if self.checking() || self.thread.is_none() {
            return false;
        }
        self.status = CheckStatus::Checking;
        self.jobs.send(()).is_ok()
    }

    pub fn checking(&self) -> bool {
        matches!(self.status, CheckStatus::Checking)
    }

    /// Nonblocking publication, including while Preferences is closed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Ok(status) = self.results.try_recv() {
            self.status = status;
            changed = true;
        }
        changed
    }

    pub fn status(&self) -> &CheckStatus {
        &self.status
    }

    pub fn presentation(&self) -> Presentation {
        let status = match &self.status {
            CheckStatus::Idle => "Not checked".into(),
            CheckStatus::Checking => "Checking signed metadata…".into(),
            CheckStatus::UpToDate => "No development updates available".into(),
            CheckStatus::Available { release } => {
                format!("Development update {} available", release.version)
            }
            CheckStatus::Error { message } => message.clone(),
        };
        Presentation {
            version: format!("Native development {}", self.current_version),
            channel: "Explicit development endpoint · Check only",
            status,
            action: if self.checking() {
                "Checking…"
            } else {
                "Check Now"
            },
            enabled: !self.checking() && self.thread.is_some(),
            failed: matches!(self.status, CheckStatus::Error { .. }),
            detail: "Verifies signed native development metadata. No download, installation or update channel is enabled.",
        }
    }

    /// Cancel and join bounded HTTP work before destroying the native host.
    /// A currently blocked request can take up to its 60-second timeout.
    pub fn shutdown(&mut self) {
        self.cancel.cancel();
        let _ = self.jobs.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for CheckWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updater::{
        Renderer, Target,
        tests::{manifest, serve, signed},
    };
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        time::{Duration, Instant},
    };

    fn worker(endpoint: &str, key: &str, current: &str) -> (CheckWorker, mpsc::Receiver<()>) {
        let client =
            UpdateClient::new(endpoint, key, Renderer::Wgpu, Target::LinuxX64, current).unwrap();
        let (wake, observed) = mpsc::channel();
        (
            CheckWorker::new(
                client,
                Arc::new(move || {
                    let _ = wake.send(());
                }),
            ),
            observed,
        )
    }

    #[test]
    fn explicit_key_configuration_is_bounded_and_does_not_reveal_the_endpoint() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("public.key");
        let (key, _, _) = signed(&manifest("https://example.invalid/artifact"));
        std::fs::write(&path, &key).unwrap();
        let client = client_from_key_file(
            "http://127.0.0.1:9/private-endpoint",
            &path,
            Renderer::Wgpu,
            "2026.9.99",
        )
        .unwrap();
        let debug = format!("{client:?}");
        assert!(!debug.contains("private-endpoint") && !debug.contains(&key));
        for bytes in [vec![b'x'; 8193], vec![0xff], b"not a public key".to_vec()] {
            std::fs::write(&path, bytes).unwrap();
            assert!(
                client_from_key_file(
                    "http://127.0.0.1:9/native.json",
                    &path,
                    Renderer::Wgpu,
                    "2026.9.99"
                )
                .is_err()
            );
        }
        assert!(
            client_from_key_file(
                "http://127.0.0.1:9/native.json",
                root.path(),
                Renderer::Wgpu,
                "2026.9.99"
            )
            .is_err()
        );
    }

    #[test]
    fn only_an_explicit_check_publishes_signed_metadata_and_never_downloads() {
        for current in ["2026.9.99", "2026.10.50", "2026.10.51"] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (key, bytes, signature) = signed(&manifest(&format!("{base}/artifact")));
            let server = serve(
                listener,
                vec![
                    (200, bytes.clone()),
                    (200, signature.clone()),
                    (200, bytes),
                    (200, signature),
                ],
            );
            let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, current);
            assert!(matches!(worker.status(), CheckStatus::Idle));
            assert_eq!(worker.presentation().status, "Not checked");
            assert!(observed.try_recv().is_err());
            for _ in 0..2 {
                assert!(worker.check());
                assert!(worker.checking());
                assert!(!worker.presentation().enabled);
                observed.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(worker.poll());
                assert!(!worker.checking() && worker.presentation().enabled);
                if current == "2026.9.99" {
                    let CheckStatus::Available { release } = worker.status() else {
                        panic!("newer release not published")
                    };
                    assert_eq!(release.version, "2026.10.50");
                    assert_eq!(
                        release.notes.as_deref(),
                        Some("Native development fixture only.")
                    );
                    assert_eq!(release.size, 23);
                    assert_eq!(
                        worker.presentation().status,
                        "Development update 2026.10.50 available"
                    );
                } else {
                    assert!(matches!(worker.status(), CheckStatus::UpToDate));
                    assert_eq!(
                        worker.presentation().status,
                        "No development updates available"
                    );
                }
                assert_eq!(
                    worker.presentation().version,
                    format!("Native development {current}")
                );
            }
            worker.shutdown();
            assert!(!worker.check());
            assert_eq!(
                server.join().unwrap(),
                [
                    "/native.json",
                    "/native.json.minisig",
                    "/native.json",
                    "/native.json.minisig"
                ]
            );
        }
    }

    #[test]
    fn failed_checks_are_errors_not_up_to_date_and_can_be_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, mut tampered, signature) = signed(&manifest(&format!("{base}/artifact")));
        let original = tampered.clone();
        tampered.push(b' '); // Still valid JSON; no longer the signed bytes.
        let server = serve(
            listener,
            vec![
                (200, tampered),
                (200, signature.clone()),
                (503, vec![]),
                (200, original),
                (200, signature),
            ],
        );
        let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, "2026.9.99");
        for message in [
            "Native update signature verification failed.",
            "Native update service returned HTTP 503.",
        ] {
            assert!(worker.check());
            observed.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(worker.poll());
            assert!(matches!(worker.status(), CheckStatus::Error { .. }));
            assert_eq!(worker.presentation().status, message);
            assert_eq!(worker.presentation().action, "Check Now");
            assert!(worker.presentation().failed);
        }
        assert!(worker.check());
        assert!(!worker.presentation().failed);
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(worker.poll());
        assert!(matches!(worker.status(), CheckStatus::Available { .. }));
        assert_eq!(
            server.join().unwrap(),
            [
                "/native.json",
                "/native.json.minisig",
                "/native.json",
                "/native.json",
                "/native.json.minisig"
            ]
        );
    }

    // Hold the first response until the caller has exercised input or shutdown.
    // This tests the boundary deterministically instead of racing a fast server.
    fn delayed_manifest(
        listener: TcpListener,
        bytes: Vec<u8>,
        signature: Option<Vec<u8>>,
    ) -> (
        mpsc::Receiver<()>,
        mpsc::Sender<()>,
        JoinHandle<Vec<String>>,
    ) {
        let (started, observed) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.split_whitespace().nth(1), Some("/native.json"));
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
            }
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
            drop(stream);
            let mut paths = vec!["/native.json".into()];
            if let Some(signature) = signature {
                paths.extend(serve(listener, vec![(200, signature)]).join().unwrap());
            }
            paths
        });
        (observed, release, server)
    }

    #[test]
    fn a_second_check_during_the_response_cannot_enqueue_work() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, signature) = signed(&manifest(&format!("{base}/artifact")));
        let (started, release, server) = delayed_manifest(listener, bytes, Some(signature));
        let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, "2026.9.99");
        assert!(worker.check());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!worker.check());
        release.send(()).unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(worker.poll());
        assert!(matches!(worker.status(), CheckStatus::Available { .. }));
        worker.shutdown();
        assert!(observed.try_recv().is_err());
        assert_eq!(
            server.join().unwrap(),
            ["/native.json", "/native.json.minisig"]
        );
    }

    #[test]
    fn shutdown_joins_pending_http_without_publishing_or_waking_a_closed_host() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (key, bytes, _) = signed(&manifest(&format!("{base}/artifact")));
        let (started, release, server) = delayed_manifest(listener, bytes, None);
        let (mut worker, observed) = worker(&format!("{base}/native.json"), &key, "2026.9.99");
        assert!(worker.check());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let cancel = worker.cancel.clone();
        let joined = thread::spawn(move || {
            worker.shutdown();
            worker
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cancel.is_cancelled() {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        release.send(()).unwrap();
        let mut worker = joined.join().unwrap();
        assert!(!worker.poll() && !worker.check());
        assert!(observed.try_recv().is_err());
        assert_eq!(server.join().unwrap(), ["/native.json"]);
    }
}
