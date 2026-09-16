//! Physical atlas reclamation follows GPU completion, not logical key eviction.
use std::collections::BTreeSet;

pub(crate) struct AtlasRetirement<T> {
    submitted: u64,
    in_flight: BTreeSet<u64>,
    retired: Vec<(u64, T)>,
}

impl<T> Default for AtlasRetirement<T> {
    fn default() -> Self {
        Self {
            submitted: 0,
            in_flight: BTreeSet::new(),
            retired: Vec::new(),
        }
    }
}

impl<T> AtlasRetirement<T> {
    pub fn submit(&mut self) -> u64 {
        self.submitted += 1;
        self.in_flight.insert(self.submitted);
        self.submitted
    }

    pub fn retire(&mut self, tile: T) {
        self.retired.push((self.submitted, tile));
    }

    pub fn complete(&mut self, serial: u64) {
        self.in_flight.remove(&serial);
    }

    pub fn len(&self) -> usize {
        self.retired.len()
    }

    pub fn drain_ready(&mut self) -> impl Iterator<Item = T> + '_ {
        let oldest = self
            .in_flight
            .first()
            .copied()
            .unwrap_or(self.submitted + 1);
        self.retired
            .extract_if(.., move |(serial, _)| *serial < oldest)
            .map(|(_, tile)| tile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reclamation_waits_for_all_prior_users_not_just_any_completion() {
        let mut queue = AtlasRetirement::default();
        let first = queue.submit();
        queue.retire("first image");
        let second = queue.submit();
        queue.retire("second image");
        queue.complete(second);
        assert_eq!(queue.drain_ready().count(), 0);
        queue.complete(first);
        assert_eq!(
            queue.drain_ready().collect::<Vec<_>>(),
            ["first image", "second image"]
        );
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn an_old_completion_cannot_release_a_newer_frame() {
        let mut queue = AtlasRetirement::default();
        let first = queue.submit();
        let second = queue.submit();
        queue.retire(42);
        queue.complete(first);
        assert_eq!(queue.drain_ready().count(), 0);
        queue.complete(second);
        assert_eq!(queue.drain_ready().collect::<Vec<_>>(), [42]);
        queue.complete(second);
        assert_eq!(queue.drain_ready().count(), 0);
    }

    #[test]
    fn completed_or_unsubmitted_allocations_release_without_another_frame() {
        let mut queue = AtlasRetirement::default();
        queue.retire(7);
        assert_eq!(queue.drain_ready().collect::<Vec<_>>(), [7]);
        let serial = queue.submit();
        queue.complete(serial);
        queue.retire(13);
        assert_eq!(queue.drain_ready().collect::<Vec<_>>(), [13]);
    }
}
