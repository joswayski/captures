//! Ephemeral recording-timeline geometry. Hosts own staged trim values and
//! discard drag state when pointer capture ends; no edit or media is published.

pub const TRIM_DRAG_THRESHOLD_PX: f64 = 3.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TimelineTrimEdge {
    Start = 0,
    End = 1,
}

impl TryFrom<u8> for TimelineTrimEdge {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Start),
            1 => Ok(Self::End),
            _ => Err(()),
        }
    }
}

/// Copyable state captured on pointer down. Coordinates are host logical points;
/// times are source-relative milliseconds and may remain fractional while staged.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct TimelineTrimDrag {
    pub start_time_ms: f64,
    pub start_x: f64,
    pub last_x: f64,
    pub min_time_ms: f64,
    pub max_time_ms: f64,
    pub duration_ms: f64,
    pub dragging: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct TimelineTrimUpdate {
    pub drag: TimelineTrimDrag,
    pub time_ms: f64,
}

impl TimelineTrimDrag {
    pub fn begin(
        edge: TimelineTrimEdge,
        pointer_x: f64,
        trim_start_ms: f64,
        trim_end_ms: f64,
        duration_ms: f64,
    ) -> Option<Self> {
        if ![pointer_x, trim_start_ms, trim_end_ms, duration_ms]
            .into_iter()
            .all(f64::is_finite)
        {
            return None;
        }
        let duration_ms = duration_ms.max(1.);
        if trim_start_ms < 0. || trim_end_ms > duration_ms || trim_end_ms - trim_start_ms < 1. {
            return None;
        }
        let (start_time_ms, min_time_ms, max_time_ms) = match edge {
            TimelineTrimEdge::Start => (trim_start_ms, 0., trim_end_ms - 1.),
            TimelineTrimEdge::End => (trim_end_ms, trim_start_ms + 1., duration_ms),
        };
        Some(Self {
            start_time_ms,
            start_x: pointer_x,
            last_x: pointer_x,
            min_time_ms,
            max_time_ms,
            duration_ms,
            dragging: false,
        })
    }

    /// Apply one pointer sample. Samples farther than one track width outside
    /// either edge are ignored without advancing `last_x`; later valid samples
    /// therefore recover from the original pointer-down origin.
    pub fn update(
        self,
        client_x: f64,
        track_left: f64,
        track_width: f64,
    ) -> Option<TimelineTrimUpdate> {
        if ![
            self.start_time_ms,
            self.start_x,
            self.last_x,
            self.min_time_ms,
            self.max_time_ms,
            self.duration_ms,
            client_x,
            track_left,
            track_width,
        ]
        .into_iter()
        .all(f64::is_finite)
            || self.min_time_ms > self.max_time_ms
            || self.duration_ms < 1.
        {
            return None;
        }
        let width = track_width.max(1.);
        let current = self.time_at(self.last_x, width)?;
        let mut drag = self;
        let distance = (client_x - self.start_x).abs();
        if !distance.is_finite() {
            return None;
        }
        if !drag.dragging {
            if distance < TRIM_DRAG_THRESHOLD_PX {
                return Some(TimelineTrimUpdate {
                    drag,
                    time_ms: current,
                });
            }
            drag.dragging = true;
        }

        let min_x = track_left - width;
        let max_x = track_left + width + width;
        if !min_x.is_finite() || !max_x.is_finite() {
            return None;
        }
        if client_x < min_x || client_x > max_x {
            return Some(TimelineTrimUpdate {
                drag,
                time_ms: current,
            });
        }
        let time_ms = drag.time_at(client_x, width)?;
        drag.last_x = client_x;
        Some(TimelineTrimUpdate { drag, time_ms })
    }

    fn time_at(self, x: f64, width: f64) -> Option<f64> {
        let delta = (x - self.start_x) / width * self.duration_ms;
        let time_ms = self.start_time_ms + delta;
        time_ms
            .is_finite()
            .then(|| time_ms.clamp(self.min_time_ms, self.max_time_ms))
    }
}

pub fn timeline_ratio(time_ms: f64, duration_ms: f64) -> Option<f64> {
    if !time_ms.is_finite() || !duration_ms.is_finite() {
        return None;
    }
    Some((time_ms / duration_ms.max(1.)).clamp(0., 1.))
}

pub fn timeline_time_at_client_x(
    client_x: f64,
    track_left: f64,
    track_width: f64,
    duration_ms: f64,
) -> Option<f64> {
    if ![client_x, track_left, track_width, duration_ms]
        .into_iter()
        .all(f64::is_finite)
    {
        return None;
    }
    let duration_ms = duration_ms.max(1.);
    let time_ms = (client_x - track_left) / track_width.max(1.) * duration_ms;
    time_ms.is_finite().then(|| time_ms.clamp(0., duration_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle_start() -> TimelineTrimDrag {
        TimelineTrimDrag::begin(
            TimelineTrimEdge::Start,
            228.571_428_571_428_58,
            2_000.,
            6_750.,
            8_750.,
        )
        .unwrap()
    }

    #[test]
    fn ratios_and_track_positions_match_shipping_bounds() {
        assert_eq!(timeline_ratio(0., 8_750.), Some(0.));
        assert_eq!(timeline_ratio(4_375., 8_750.), Some(0.5));
        assert_eq!(timeline_ratio(9_000., 8_750.), Some(1.));
        assert_eq!(
            timeline_time_at_client_x(500., 0., 1_000., 8_750.),
            Some(4_375.)
        );
        assert_eq!(
            timeline_time_at_client_x(-20., 0., 1_000., 8_750.),
            Some(0.)
        );
        assert_eq!(
            timeline_time_at_client_x(2_000., 0., 1_000., 8_750.),
            Some(8_750.)
        );
        assert_eq!(timeline_ratio(f64::NAN, 1.), None);
        assert_eq!(timeline_time_at_client_x(0., 0., f64::INFINITY, 1.), None);
    }

    #[test]
    fn begin_sets_shipping_one_millisecond_bounds_for_each_edge() {
        let start =
            TimelineTrimDrag::begin(TimelineTrimEdge::Start, 40., 1_200., 2_000., 3_000.).unwrap();
        assert_eq!(
            (start.start_time_ms, start.min_time_ms, start.max_time_ms),
            (1_200., 0., 1_999.)
        );
        let end =
            TimelineTrimDrag::begin(TimelineTrimEdge::End, 80., 1_200., 2_000., 3_000.).unwrap();
        assert_eq!(
            (end.start_time_ms, end.min_time_ms, end.max_time_ms),
            (2_000., 1_201., 3_000.)
        );
        assert!(TimelineTrimDrag::begin(TimelineTrimEdge::Start, 0., 5., 5.5, 10.).is_none());
        assert!(TimelineTrimDrag::begin(TimelineTrimEdge::End, 0., 0., 11., 10.).is_none());
    }

    #[test]
    fn threshold_is_inclusive_and_retains_the_pointer_down_origin() {
        let start =
            TimelineTrimDrag::begin(TimelineTrimEdge::Start, 73., 2_000., 6_750., 8_750.).unwrap();
        let pending = start.update(75.999, 10., 1_000.).unwrap();
        assert!(!pending.drag.dragging);
        assert_eq!(pending.time_ms, 2_000.);
        assert_eq!(pending.drag.last_x, 73.);
        let moved = pending.drag.update(76., 10., 1_000.).unwrap();
        assert!(moved.drag.dragging);
        assert!((moved.time_ms - 2_026.25).abs() < 1e-10);
        assert_eq!(
            (moved.drag.start_x, moved.drag.start_time_ms),
            (73., 2_000.)
        );
    }

    #[test]
    fn asymmetric_oracle_accepts_fast_motion_rejects_glitches_and_recovers() {
        let start = oracle_start();
        let small = start.update(start.start_x + 50., 0., 1_000.).unwrap();
        assert!((small.time_ms - 2_437.5).abs() < 1e-10);
        let fast = start.update(start.start_x + 500., 0., 1_000.).unwrap();
        assert!((fast.time_ms - 6_375.).abs() < 1e-10);

        let glitch = start.update(9_000., 0., 1_000.).unwrap();
        assert!(glitch.drag.dragging);
        assert_eq!(glitch.time_ms, 2_000.);
        assert_eq!(glitch.drag.last_x, start.last_x);
        let repeated = glitch.drag.update(9_050., 0., 1_000.).unwrap();
        assert_eq!(repeated.time_ms, 2_000.);
        assert_eq!(repeated.drag.last_x, start.last_x);
        let recovered = repeated
            .drag
            .update(start.start_x + 50., 0., 1_000.)
            .unwrap();
        assert!((recovered.time_ms - 2_437.5).abs() < 1e-10);
        assert_eq!(
            (recovered.drag.start_x, recovered.drag.start_time_ms),
            (start.start_x, 2_000.)
        );
    }

    #[test]
    fn slack_boundaries_are_inclusive_and_times_clamp_to_handle_bounds() {
        let start = oracle_start();
        let left = start.update(-1_000., 0., 1_000.).unwrap();
        assert_eq!(left.time_ms, 0.);
        assert_eq!(left.drag.last_x, -1_000.);
        let right = start.update(2_000., 0., 1_000.).unwrap();
        assert_eq!(right.time_ms, 6_749.);
        assert_eq!(right.drag.last_x, 2_000.);
        let outside = start.update(2_000.001, 0., 1_000.).unwrap();
        assert_eq!(outside.time_ms, 2_000.);
        assert_eq!(outside.drag.last_x, start.last_x);
    }

    #[test]
    fn nonfinite_and_overflowing_samples_are_transactional() {
        let start = oracle_start();
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(start.update(invalid, 0., 1_000.).is_none());
        }
        assert!(start.update(0., f64::NAN, 1_000.).is_none());
        assert!(start.update(0., 0., f64::INFINITY).is_none());
        let extreme = TimelineTrimDrag {
            start_x: -f64::MAX,
            ..start
        };
        assert!(extreme.update(f64::MAX, 0., 1.).is_none());
    }
}
