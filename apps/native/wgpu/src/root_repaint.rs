//! Event-driven fallback for Windows' visible ROOT paint starvation in winit
//! 0.30.13 / eframe 0.36.2. Never invent work or advance a requested deadline.
use std::time::Instant;

#[derive(Default)]
pub struct Pending {
    requests: Vec<(u64, Instant)>,
}

impl Pending {
    pub fn clear(&mut self) {
        self.requests.clear();
    }

    pub fn prune(&mut self, current_pass: u64) {
        // Match eframe's per-viewport acceptance, without overflow. At most
        // two generations survive; each keeps its own earliest deadline.
        self.requests
            .retain(|(pass, _)| matches!(current_pass.checked_sub(*pass), Some(0 | 1)));
    }

    pub fn request(&mut self, current_pass: u64, requested_pass: u64, when: Instant) {
        self.prune(current_pass);
        if !matches!(current_pass.checked_sub(requested_pass), Some(0 | 1)) {
            return;
        }
        if let Some((_, previous)) = self
            .requests
            .iter_mut()
            .find(|(pass, _)| *pass == requested_pass)
        {
            *previous = (*previous).min(when);
        } else {
            self.requests.push((requested_pass, when));
        }
    }

    pub fn take_due(&mut self, current_pass: u64, now: Instant) -> bool {
        self.prune(current_pass);
        let before = self.requests.len();
        // Consume before dispatch. A failed/nonadvancing frame must not spin.
        self.requests.retain(|(_, when)| *when > now);
        before != self.requests.len()
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.requests.iter().map(|(_, when)| *when).min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn deadlines_are_never_early_and_are_consumed_once() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(3);
        let mut pending = Pending::default();
        assert!(
            !pending.take_due(7, now),
            "child-only activity creates no ROOT work"
        );
        pending.request(7, 7, deadline);
        pending.request(7, 7, deadline + Duration::from_secs(2));
        assert_eq!(pending.next_deadline(), Some(deadline));
        assert!(!pending.take_due(8, deadline - Duration::from_nanos(1)));
        assert!(pending.take_due(8, deadline));
        assert!(!pending.take_due(8, deadline));
        assert_eq!(pending.next_deadline(), None);
    }

    #[test]
    fn stale_earlier_generation_cannot_discard_or_accelerate_later_work() {
        let now = Instant::now();
        let later = now + Duration::from_secs(5);
        let mut pending = Pending::default();
        pending.request(10, 10, now);
        pending.request(11, 11, later);
        assert!(!pending.take_due(12, now));
        assert_eq!(pending.next_deadline(), Some(later));
        assert!(pending.take_due(12, later));
        pending.request(12, 10, now); // stale
        pending.request(12, 13, now); // not yet a real pass
        assert_eq!(pending.next_deadline(), None);
        pending.request(u64::MAX, u64::MAX, later);
        assert!(pending.take_due(u64::MAX, later));
    }

    #[test]
    fn due_generations_coalesce_and_ineligible_windows_clear_work() {
        let now = Instant::now();
        let mut pending = Pending::default();
        pending.request(4, 3, now);
        pending.request(4, 4, now);
        assert!(pending.take_due(4, now));
        assert_eq!(pending.next_deadline(), None);
        pending.request(4, 4, now);
        pending.clear();
        assert!(!pending.take_due(4, now));
    }
}
