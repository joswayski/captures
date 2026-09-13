use captures_windows_native::{
    draft::{DraftIdentity, DraftStore},
    editor::Document,
};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
};

pub struct Event {
    pub identity: DraftIdentity,
    pub document_key: (u64, u64),
    pub result: Result<(), String>,
}

enum Job {
    Save {
        identity: DraftIdentity,
        source_path: Option<PathBuf>,
        document: Document,
        completion: Option<SyncSender<Result<(), String>>>,
    },
    Discard {
        identity: DraftIdentity,
        completion: SyncSender<Result<(), String>>,
    },
}

pub struct DraftWorker {
    jobs: SyncSender<Job>,
    events: Receiver<Event>,
}

impl DraftWorker {
    pub fn new(profile_root: PathBuf) -> Self {
        let (job_tx, job_rx) = mpsc::sync_channel::<Job>(1);
        let (event_tx, event_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let store = DraftStore::new(&profile_root);
            while let Ok(job) = job_rx.recv() {
                match job {
                    Job::Save {
                        identity,
                        source_path,
                        document,
                        completion,
                    } => {
                        let document_key = document.render_key();
                        let result = store.save(&identity, source_path.as_deref(), &document);
                        if let Some(completion) = completion {
                            let _ = completion.send(result.clone());
                        }
                        if event_tx
                            .send(Event {
                                identity,
                                document_key,
                                result,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Job::Discard {
                        identity,
                        completion,
                    } => {
                        let _ = completion.send(store.discard(&identity));
                    }
                }
            }
        });
        Self {
            jobs: job_tx,
            events: event_rx,
        }
    }

    pub fn save(
        &self,
        identity: DraftIdentity,
        source_path: Option<PathBuf>,
        document: Document,
    ) -> Result<(), String> {
        match self.jobs.try_send(Job::Save {
            identity,
            source_path,
            document,
            completion: None,
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("screenshot draft worker is busy".into()),
            Err(TrySendError::Disconnected(_)) => Err("screenshot draft worker stopped".into()),
        }
    }

    pub fn save_and_wait(
        &self,
        identity: DraftIdentity,
        source_path: Option<PathBuf>,
        document: Document,
    ) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs
            .send(Job::Save {
                identity,
                source_path,
                document,
                completion: Some(tx),
            })
            .map_err(|_| "screenshot draft worker stopped".to_owned())?;
        rx.recv()
            .map_err(|_| "screenshot draft worker stopped".to_owned())?
    }

    pub fn discard_and_wait(&self, identity: DraftIdentity) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs
            .send(Job::Discard {
                identity,
                completion: tx,
            })
            .map_err(|_| "screenshot draft worker stopped".to_owned())?;
        rx.recv()
            .map_err(|_| "screenshot draft worker stopped".to_owned())?
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn ordered_discard_cannot_be_undone_by_a_queued_save() {
        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("capture.png");
        let identity = DraftIdentity::capture(path.clone());
        let worker = DraftWorker::new(profile.path().to_path_buf());
        worker
            .save(
                identity.clone(),
                Some(path.clone()),
                Document::new(RgbaImage::from_pixel(2, 1, Rgba([11, 23, 37, 255]))),
            )
            .unwrap();
        worker.discard_and_wait(identity.clone()).unwrap();

        assert!(
            DraftStore::new(profile.path())
                .load(&identity, Some(&path))
                .unwrap()
                .is_none()
        );
    }
}
