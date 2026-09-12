pub fn accepts(current_epoch: u64, current_request: u64, epoch: u64, request: u64) -> bool {
    current_epoch == epoch && current_request == request
}

pub fn cleanup_undelivered_staged_file(path: &std::path::Path, delivered: bool) {
    if !delivered {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::{accepts, cleanup_undelivered_staged_file};

    #[test]
    fn rejects_old_editor_and_superseded_request_results() {
        assert!(accepts(7, 13, 7, 13));
        assert!(!accepts(7, 13, 6, 13));
        assert!(!accepts(7, 13, 7, 12));
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
