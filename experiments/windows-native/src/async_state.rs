use std::{
    collections::HashMap,
    path::Path,
    sync::{Condvar, Mutex},
};

pub fn accepts(current_epoch: u64, current_request: u64, epoch: u64, request: u64) -> bool {
    current_epoch == epoch && current_request == request
}

pub fn same_document(current_render_key: Option<(u64, u64)>, origin_document: u64) -> bool {
    current_render_key.is_some_and(|(document, _)| document == origin_document)
}

pub fn accepts_document_request(
    current_render_key: Option<(u64, u64)>,
    current_request: u64,
    origin_document: u64,
    request: u64,
) -> bool {
    current_request == request && same_document(current_render_key, origin_document)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveCompletion {
    Stale,
    ExportedRevision,
    NewerRevision,
}

pub fn classify_save_completion(
    current_render_key: Option<(u64, u64)>,
    current_request: u64,
    exported_render_key: (u64, u64),
    request: u64,
) -> SaveCompletion {
    if current_request != request
        || current_render_key.is_none_or(|current| current.0 != exported_render_key.0)
    {
        SaveCompletion::Stale
    } else if current_render_key == Some(exported_render_key) {
        SaveCompletion::ExportedRevision
    } else {
        SaveCompletion::NewerRevision
    }
}

#[derive(Default)]
pub struct SaveTracker {
    active: HashMap<u64, SaveIdentity>,
}

struct SaveIdentity {
    document_id: u64,
    destination: String,
}

impl SaveTracker {
    pub fn try_start(&mut self, request: u64, document_id: u64, destination: &Path) -> bool {
        let destination = destination_key(destination);
        if self.active.contains_key(&request)
            || self.active.values().any(|active| {
                active.document_id == document_id || active.destination == destination
            })
        {
            return false;
        }
        self.active.insert(
            request,
            SaveIdentity {
                document_id,
                destination,
            },
        );
        true
    }

    pub fn finish(&mut self, request: u64) -> bool {
        self.active.remove(&request).is_some()
    }
}

fn destination_key(path: &Path) -> String {
    let stable_path = match (path.parent(), path.file_name()) {
        (Some(parent), Some(file_name)) => std::fs::canonicalize(parent)
            .unwrap_or_else(|_| parent.to_path_buf())
            .join(file_name),
        _ => path.to_path_buf(),
    };
    normalize_windows_destination(&stable_path.to_string_lossy())
}

fn normalize_windows_destination(path: &str) -> String {
    let normalized = path.replace('/', "\\").to_lowercase();
    if let Some(path) = normalized.strip_prefix("\\\\?\\unc\\") {
        format!("\\\\{path}")
    } else if let Some(path) = normalized.strip_prefix("\\\\?\\") {
        path.to_owned()
    } else {
        normalized
    }
}

pub struct LatestQueue<T> {
    state: Mutex<LatestQueueState<T>>,
    ready: Condvar,
}

struct LatestQueueState<T> {
    latest_request: u64,
    pending: Option<(u64, T)>,
    closed: bool,
}

impl<T> LatestQueue<T> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(LatestQueueState {
                latest_request: 0,
                pending: None,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    pub fn submit(&self, request: u64, value: T) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.latest_request = request;
        state.pending = Some((request, value));
        self.ready.notify_one();
    }

    pub fn take(&self) -> Option<(u64, T)> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        while state.pending.is_none() && !state.closed {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
        state.pending.take()
    }

    pub fn is_latest(&self, request: u64) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .latest_request
            == request
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.closed = true;
        state.pending = None;
        self.ready.notify_one();
    }
}

impl<T> Default for LatestQueue<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn cleanup_undelivered_staged_file(path: &std::path::Path, delivered: bool) {
    if !delivered {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LatestQueue, SaveCompletion, SaveTracker, accepts, accepts_document_request,
        classify_save_completion, cleanup_undelivered_staged_file, destination_key,
        normalize_windows_destination, same_document,
    };

    #[test]
    fn rejects_old_editor_and_superseded_request_results() {
        assert!(accepts(7, 13, 7, 13));
        assert!(!accepts(7, 13, 6, 13));
        assert!(!accepts(7, 13, 7, 12));
    }

    #[test]
    fn distinct_document_completions_cannot_retarget_the_current_editor() {
        let document_a = crate::editor::Document::new(image::RgbaImage::new(13, 7));
        let document_b = crate::editor::Document::new(image::RgbaImage::new(29, 11));
        let key_a = document_a.render_key();
        let key_b = document_b.render_key();
        assert_ne!(key_a.0, key_b.0);

        let mut current_source = "B-source";
        let mut history = Vec::new();
        // B completes first, then stale A completes out of order. Both successful
        // saves belong in history, but only B may update B's editor state.
        for (origin, request, destination) in [(key_b.0, 42, "B-save"), (key_a.0, 41, "A-save")] {
            history.push(destination);
            if accepts_document_request(Some(key_b), 42, origin, request) {
                current_source = destination;
            }
        }
        assert_eq!(history, ["B-save", "A-save"]);
        assert_eq!(current_source, "B-save");
        assert!(!accepts_document_request(Some(key_b), 42, key_b.0, 41));
        assert!(!same_document(Some(key_b), key_a.0));
        assert!(!same_document(None, key_b.0));
    }

    #[test]
    fn delayed_save_completion_distinguishes_the_exported_and_newer_revision() {
        let document = crate::editor::Document::new(image::RgbaImage::new(13, 7));
        let exported = document.render_key();
        assert_eq!(
            classify_save_completion(Some(exported), 9, exported, 9),
            SaveCompletion::ExportedRevision
        );
        assert_eq!(
            classify_save_completion(Some((exported.0, exported.1 + 1)), 9, exported, 9),
            SaveCompletion::NewerRevision
        );
        assert_eq!(
            classify_save_completion(Some((exported.0 + 1, exported.1)), 9, exported, 9),
            SaveCompletion::Stale
        );
        assert_eq!(
            classify_save_completion(Some(exported), 10, exported, 9),
            SaveCompletion::Stale
        );
    }

    #[test]
    fn latest_queue_bounds_work_to_one_taken_and_one_newest_pending_job() {
        let queue = LatestQueue::new();
        queue.submit(1, "in flight");
        assert_eq!(queue.take(), Some((1, "in flight")));

        queue.submit(2, "superseded pending");
        queue.submit(3, "newest pending");
        assert!(!queue.is_latest(1));
        assert!(!queue.is_latest(2));
        assert!(queue.is_latest(3));
        assert_eq!(queue.take(), Some((3, "newest pending")));

        queue.close();
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn save_tracker_blocks_document_and_destination_races_without_cross_release() {
        let mut saves = SaveTracker::default();
        let source_a = std::path::Path::new("C:/Captures/A.PNG");
        let source_b = std::path::Path::new("C:/Captures/B.png");

        assert!(saves.try_start(10, 101, source_a));
        assert!(!saves.try_start(11, 101, std::path::Path::new("C:/Captures/A-copy.png")));
        assert!(saves.try_start(12, 202, source_b));
        assert!(!saves.try_start(13, 303, std::path::Path::new("c:\\captures\\a.png")));

        assert!(saves.finish(10));
        assert!(saves.try_start(13, 303, source_a));
        assert!(
            !saves.finish(10),
            "stale completion must not release a new job"
        );
        assert!(!saves.try_start(14, 303, source_a));
        assert!(!saves.try_start(15, 202, source_b));

        assert!(saves.finish(13));
        assert!(saves.try_start(14, 303, source_a));
        assert!(!saves.try_start(16, 404, source_b));
        assert!(saves.finish(12));
        assert!(saves.try_start(16, 404, source_b));
    }

    #[test]
    fn destination_key_is_stable_when_missing_leaf_is_created() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("New Capture.PNG");
        let missing = destination_key(&destination);
        std::fs::write(&destination, b"created").unwrap();
        let existing = destination_key(&destination);
        assert_eq!(missing, existing);
    }

    #[test]
    fn windows_destination_normalization_unifies_verbatim_drive_and_unc_paths() {
        assert_eq!(
            normalize_windows_destination(r"C:\Captures\Folder\Image.PNG"),
            normalize_windows_destination(r"\\?\C:\Captures\Folder\Image.PNG")
        );
        assert_eq!(
            normalize_windows_destination(r"\\Server\Share\Captures\Image.PNG"),
            normalize_windows_destination(r"\\?\UNC\Server\Share\Captures\Image.PNG")
        );
        assert_eq!(
            normalize_windows_destination(r"C:/Captures/Folder/Image.PNG"),
            normalize_windows_destination(r"c:\captures\folder\image.png")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_existing_file_matches_ordinary_and_verbatim_keys() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("Existing Capture.PNG");
        std::fs::write(&destination, b"created").unwrap();
        let ordinary = destination_key(&destination);
        let verbatim = std::path::PathBuf::from(format!(r"\\?\{}", destination.display()));
        assert_eq!(ordinary, destination_key(&verbatim));
    }

    #[test]
    fn disconnected_delivery_removes_only_owned_stage() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp4");
        let staged = directory.path().join("staged.mp4");
        std::fs::write(&source, b"source").unwrap();
        std::fs::write(&staged, b"stage").unwrap();
        let (sender, receiver) = std::sync::mpsc::sync_channel::<()>(1);
        drop(receiver);

        cleanup_undelivered_staged_file(&staged, sender.send(()).is_ok());

        assert_eq!(std::fs::read(source).unwrap(), b"source");
        assert!(!staged.exists());
    }
}
