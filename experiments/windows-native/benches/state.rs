use captures_recording::{RecordingKind, RecordingState};
use captures_windows_native::{
    geometry::{Point, Rect, SelectionDrag, update_selection},
    state::RecordingUi,
};
use std::time::{Duration, Instant};

fn main() {
    let iterations = 2_000_000_u32;
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        width: 3840.0,
        height: 2160.0,
    };
    let start = Instant::now();
    let mut checksum = 0.0_f32;
    for i in 0..iterations {
        let rect = update_selection(
            SelectionDrag::Create {
                anchor: Point {
                    x: 1700.0,
                    y: 930.0,
                },
            },
            Point {
                x: (i % 4000) as f32,
                y: (i % 2300) as f32,
            },
            bounds,
        );
        checksum += rect.width.fract() + rect.height.fract();
    }
    let geometry = start.elapsed();
    let mut recording = RecordingUi {
        state: RecordingState::Paused,
        kind: RecordingKind::Video,
        started: Instant::now(),
        elapsed_before_pause: Duration::ZERO,
        muted: false,
        hidden: false,
        warning: None,
        source: None,
    };
    let start = Instant::now();
    for _ in 0..100_000 {
        recording
            .transition(RecordingState::Recording, true)
            .unwrap();
        recording.transition(RecordingState::Paused, true).unwrap();
    }
    println!(
        "selection_updates={iterations} elapsed_us={} ns_per_update={:.1} checksum={checksum}",
        geometry.as_micros(),
        geometry.as_nanos() as f64 / f64::from(iterations)
    );
    println!(
        "recording_transition_pairs=100000 elapsed_us={}",
        start.elapsed().as_micros()
    );
}
