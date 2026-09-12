pub fn accepts(current_epoch: u64, current_request: u64, epoch: u64, request: u64) -> bool {
    current_epoch == epoch && current_request == request
}

#[cfg(test)]
mod tests {
    use super::accepts;

    #[test]
    fn rejects_old_editor_and_superseded_request_results() {
        assert!(accepts(7, 13, 7, 13));
        assert!(!accepts(7, 13, 6, 13));
        assert!(!accepts(7, 13, 7, 12));
    }
}
