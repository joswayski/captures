# Matched macOS preview effects

This experiment compares **the same preview media effects** in a minimal Tauri
window and a native AppKit/Core Animation window. It does not replace Captures or
restore the retired native apps. The reference imports the shipping dust renderer
and preview CSS directly; the candidate has no WebView, JavaScript runtime, or
pre-recorded animation frames.

The selected behavior is radial dust deletion with the surviving card settling
into the vacated slot, mirrored across left/right and top/bottom placements.
The comparison intentionally isolates **media surfaces**: buttons, labels,
outline/shadow, desktop transparency, editor, recording, pile fan, and capture
backend are not part of these measurements. Neither candidate includes tray or
full-app services. Do not use these memory numbers as full-app memory numbers.

## Run on your Mac

Requirements: macOS 13+, a Retina display at 2× backing scale with at least
640×720 points of usable desktop, Xcode command-line tools (`xcode-select --install`),
Rust matching `rust-toolchain.toml`, Node 24+, and Python 3.9+. Use AC power, turn
off Low Power Mode, keep the same display/refresh rate, and close heavy background
work. Grant the terminal **Screen & System Audio Recording** permission when
macOS requests it, then restart the terminal if required. No accessibility or
capture permission is needed by the candidate applications themselves.

From the repository root:

```sh
npm ci
bash experiments/macos-parity/build.sh
bash experiments/macos-parity/run.sh
```

The build uses `swiftc -O` and `cargo build --release`, checks the Swift particle
poses against shipping TypeScript, and creates a private Python venv. It does not
install or alter Captures. Tauri uses a separate experimental bundle identifier;
any WebKit cache belongs to that identifier, not the shipping app. Close test
windows with Ctrl+C in the terminal. The runner terminates only processes it
launched. The benchmark needs a logged-in graphical session, not SSH alone.

Default run: 24 checkpoint pairs, then one excluded warmup and three measured
9.6-second trials per scenario/candidate, in alternating order (about six minutes
plus startup and screenshot overhead). **Do not switch applications, minimize,
cover, move the windows, or change display settings during trials.**

```sh
# First collect only paired screenshots and visual diffs:
bash experiments/macos-parity/run.sh --visual-only

# Longer live trials:
bash experiments/macos-parity/run.sh --trials 5 --duration 19.2

# Once the other thread's optimized adapter is available:
bash experiments/macos-parity/run.sh --gpui /absolute/path/captures-gpui-parity
```

### Full app/helper accounting and observed animation frames

Use the profiling mode for framework comparisons:

```sh
bash experiments/macos-parity/run.sh --profile --capture-hz 120 \
  --gpui /absolute/path/captures-gpui-parity --output /absolute/path/new-results
```

This retains the same 24 visual checkpoints and thresholds, then runs **separate
resource and frame passes** with one excluded warmup and three measured 12.8-second
trials per scenario/app/pass. With three candidates, allow roughly 25–35 minutes
including launches and checkpoints. `--duration 32` extends each trial for a longer
memory-growth check. The observer may request Screen Recording permission; grant
it in System Settings and restart the command with a new output directory if needed.

- **Resources:** LaunchServices starts a unique temporary `.app` for each trial.
  The sampler enumerates that app's macOS resource coalition, including WebKit
  WebContent, GPU, and Networking processes that are not ordinary child processes.
  Benchmark-only WebKit private APIs provide their PIDs for independent membership
  checks. Missing ownership, unsupported private APIs, PID reuse, or changing
  process membership fail the run instead of returning partial totals.
  `physicalFootprint*MiB` is summed process physical footprint; `summedRSS*MiB` is
  also retained but can double-count shared pages. Peaks are concurrent **sampled**
  peaks, not sums of each process's unrelated historical maximum. CPU is the total
  user+system CPU-time delta divided by actual sampling duration: 100% is one core.
  Raw samples, first/last footprint, and second-half growth are retained. Sampling
  starts immediately before releasing the workload and ends after its result;
  its small boundary overhead is included in `sampledSeconds`.
- **Frames:** A separate ScreenCaptureKit process observes only the synthetic
  benchmark window. It records WindowServer display timestamps, frame statuses,
  and hashes of visible BGRA pixels, not screenshots of the rest of your desktop.
  The workload waits until observation is ready. Reports count changed frames and
  unchanged spans within dust/settle motion phases, excluding intentional idle
  gaps and identical redraws. Observer latency and hashing time flag backlog.
  `--capture-hz` requests a capture ceiling; it does **not** set the display refresh
  rate or certify that every presented frame was observed. These are observed
  window changes, not physical-panel FPS, GPU time, or automatic dropped-frame counts.
- Frame recording overhead is excluded from the resource pass. Neither pass
  attributes shared WindowServer/kernel costs or all GPU residency to an app.
  These remain component windows, not the full Captures application. An app-coalition
  total is substantially more complete than the old root-process number, not
  whole-system accounting.

The GPUI adapter must implement measurement protocol2 and contain the transient
texture-lifetime fix (PR #527). Its renderer is **CPU raster + texture upload**;
resource reclamation is tested separately from process RSS. Build it in a sibling
worktree using full Xcode, including its Metal tools; CLT alone is insufficient.
The harness and GPUI branches do not need merging. `report.json` contains
`profileMedians`, `profileTrials`, completeness, and backpressure flags. Any failure
preserves raw files and exits nonzero. A pixel failure or observer backlog prevents
a comparable-performance verdict; neither is silently removed from group medians.

Results are written to a new directory under `experiments/macos-parity/results/`.
Send that directory as a zip: `report.json`, raw `measured.json` files, and the
`*.comparison.png` / `*.diff.png` images are the useful evidence. It contains only
synthetic capture imagery, but includes OS/display/power information, local paths,
process names, and process resource samples. Review it before sharing.

## Visual agreement comes before a performance conclusion

Each checkpoint launches real windows and captures the selected WindowServer
window using `screencapture -l`. No NSView bitmap cache or off-screen native render
is used as visual proof. Source and candidate must both be 1280×1440 physical
pixels. The comparison normalizes embedded color profiles to sRGB and measures
error over the union of non-background content, not the mostly empty window.
Blank captures fail. Limits: mean absolute RGB error ≤2/255 and at most 1% of
active pixels with a channel error >16/255. These are **diagnostic thresholds**,
not a claim that the human-visible animation is identical. Inspect the full-size
PNGs as well as the half-size contact sheets and watch the live windows.

If any checkpoint fails, screenshots and the report are preserved, the process
exits nonzero, and performance trials do not run. To collect explicitly
non-comparable timing diagnostics while investigating a mismatch:

```sh
bash experiments/macos-parity/run.sh --diagnostic-performance
```

That option never changes thresholds or turns a failure into a pass. There is no
automatic framework recommendation. Fix visible differences before choosing one.

## What the legacy callback-only numbers mean

Without `--profile`, the original lightweight mode remains available. Its partial
process-tree figures are not the complete app/helper accounting above.

- Main-thread animation callback intervals: p50, p95, max, and count over 25ms.
  These expose application stalls; **they are not presented FPS, GPU execution
  time, or a count of dropped display frames**. Web reference live trials leave
  CSS/WAAPI on their real compositor path and Canvas on its production rAF path.
  Native uses Core Image textures with Core Animation layer composition and a
  display-linked main-thread pose update; it is not a CPU image raster per frame.
  Its Core Image context explicitly selects a Metal device, records the GPU name,
  and fails if no Metal device exists rather than silently benchmarking software.
- Per-cycle setup time includes effect resource creation; images are decoded
  before readiness. Native currently creates filtered chip textures separately;
  shipping Canvas filters an atlas in one pass. That is an implementation cost
  under test, not work silently excluded from the native timing.
- Process-tree RSS and CPU include all discoverable descendants. RSS can
  double-count shared pages. WebKit's XPC services can belong to `launchd`, so
  unmatched WebKit processes are recorded separately, including whether they
  appeared after launch. They are **not silently counted as zero or attributed
  to this app**. WindowServer/GPU residency is also outside these metrics.
- One warmup per scenario/app is retained but excluded from group medians.
  Every measured group must have the requested successful trial count. A failed
  trial stops the run rather than being discarded from the median.
- Executable, source, fixture, config, and bundled-web hashes prevent comparing
  stale builds. `build.sh` must run again after source changes. The report records
  OS, hardware, displays, power state, raw callback intervals, and process samples.

For actual presented-frame/GPU/energy evidence, use Xcode Instruments' Animation
Hitches/Core Animation or Metal System Trace tools on the same live workload.
Attach to the candidate PID recorded in its `renderer.json.ready.json`; use a
long `--duration` for time to attach. Run instrumented trials separately because
instrumentation itself changes overhead. Do not combine Instruments and
uninstrumented process samples into one ranking.

## Version 1 adapter contract

Invocation: `EXECUTABLE CONFIG_JSON OUTPUT_JSON`. Paths are absolute. The generated
configs are `.build/public/<scenario>.json`; particle descriptors come directly
from `buildThumbnailDustParticles` in the current checkout. The schema fields are:

```json
{
  "schema": 1,
  "scenario": "dust-bottom-left",
  "mode": "checkpoint",
  "checkpointMs": 420,
  "durationMs": 9600,
  "cycleMs": 3200,
  "width": 640,
  "height": 720,
  "scale": 2,
  "fixturePath": "/absolute/path/fixture.png",
  "seed": 739,
  "particles": []
}
```

The real `particles` array contains 198 `ThumbnailDustParticle` objects with all
shipping fields. It is **model input, not sampled poses or pre-rendered frames**.
LCG: unsigned32 `state = 1664525*state + 1013904223`, output `state / 2^32`, seed739.
Fixture: the generated 397×251 sRGB PNG. Card: 284×160, radius12, object-fit cover.
Opaque diagnostic background: sRGB `#20242b`. Coordinates use top-left origin.
No window decoration or shadow; no surface shadow, ring, text, or buttons.

| Scenario | Exiting card | Survivor | Delete origin |
| --- | --- | --- | --- |
| `dust-bottom-left` | (178,320) | (178,136), moves +184px | (22.5,22.5), unsaved left |
| `dust-top-right` | (178,320) | (178,504), moves −184px | (226.5,22.5), saved right |
| `settle-bottom` | absent | (178,136), moves +184px | none |
| `settle-top` | absent | (178,504), moves −184px | none |

Dust elapsed0 is immediately after delete, not after chrome has disappeared.
The exiting source image stays above the chips, with blur2/brightness.5/scale1.015,
clipped by its rounded media shell. Source opacity is1 through110ms, then fades
to0 at550ms with cubic(.22,.1,.25,1) applied **to that 110..550 segment**.
Dust lives in a padded524×400 layer whose origin is (58,200). Clip inset120/r12
holds through204ms, opens to inset0/r0 at1785ms with cubic(.33,0,.2,1) on that
segment; layer opacity holds1 through1785ms and fades to0 at2295ms with the same
segment easing. Remain at0 through2550ms. Particle motion uses
`thumbnailDustVisualAt`: cubic(.28,0,.12,1) applies to **the whole local particle
duration**, then linear mixes explicit keyframes. This distinction is intentional.

Each chip first slices the rounded cover surface (including .55px overlap), then
applies blur2/brightness.5 with transparent8px padding. Native and web independently
decode/crop/filter the fixture. The reference chooses Canvas or DOM/WAAPI using
the real shipping capability probe and records the path it used.

Media image elements (the survivor and fading source) snap both fitted cover edges
to device pixels before sampling, matching WebKit `RenderImage::paintReplaced`.
Dust retains floating cover geometry. For the 397×251 fixture at 2×, the media
destination is (0,−20,568,360) physical pixels, clipped to the 568×320 card;
the dust destination height remains approximately 359.113 pixels. Negative half
ties round toward positive infinity; rounding size separately is not equivalent.

Survivor settle: delay1800ms for dust, delay0 for settle-only; translate by the
signed184px over580ms with cubic(.4,0,.2,1). No survivor fading or blur. In run mode
repeat the same initial state every3200ms and include resource preparation at each
restart. Checkpoint mode freezes exact elapsed time; it never guesses the frame
from a sleep. The runner's small post-ready wait only lets the fixed pose present.

Write `OUTPUT_JSON.ready.json` atomically once prepared and visible:
`{schema,pid,window_id,scale,scenario,mode,renderer,checkpointMs}`. This is an
**application-state marker**, not a presentation timestamp. In `run` mode also
write `OUTPUT_JSON` atomically with those fields plus `elapsedMs`,
`callbackIntervalsMs`, `setupMs`, `cycles`, `complete:true`, and a description in
`metric`. Callback intervals measure actual main-thread arrival, not scheduled
display timestamps. Remain open until the parent terminates the process. Exit
nonzero on decode/config/scale errors. Never operate on a user's capture profile.

### Additive measurement protocol 2

Schema1 configs may additionally contain an absolute `startGatePath`. After ready,
the renderer waits asynchronously for that file, with a 90-second deadline. Ready
includes `measurementProtocol:2`. Before starting the live clock/resource setup,
write `OUTPUT_JSON.started.json` atomically:
`{schema:1,pid,startHostTimeNs}`. `startHostTimeNs` is `mach_absolute_time` converted
with `mach_timebase_info` to nanoseconds; it is not process-relative time or a
presentation claim. Checkpoints and ungated runs retain their existing behavior.
The Tauri adapter's `profileResources:true` option adds explicit
`webkitProcesses:[{role,pid}]` for `webContent`, `gpu`, and `network` to readiness.

## Local checks

```sh
node experiments/macos-parity/generate.mjs
swiftc -O -swift-version 5 experiments/macos-parity/native/Math.swift experiments/macos-parity/test-cover.swift -o /tmp/parity-cover
/tmp/parity-cover
swiftc -O -swift-version 5 experiments/macos-parity/native/*.swift -o /tmp/parity-poses
/tmp/parity-poses --poses experiments/macos-parity/.build/poses-input.json /tmp/poses.json
node experiments/macos-parity/verify-poses.mjs /tmp/poses.json
python3 -m unittest discover -s experiments/macos-parity -p 'test_*.py'
# Built helpers, logged-in Mac; verifies launch, gate, clocks, and helper ownership:
CAPTURES_NATIVE_PROFILE_TEST=1 experiments/macos-parity/.build/python/bin/python -m unittest discover -s experiments/macos-parity -p 'test_native_profile.py'
npm run check
```

The Foundation-only sampler also compiles on Linux. Its differential checks cover
both origins, every particle's delay boundary, and asymmetric midflight times.
They test math, not native rasterization or presentation. macOS CI builds both
graphical candidates; acceptance and performance results must come from the Mac
running the on-screen suite. A Chromium screenshot is only a reference-harness
check, never evidence of AppKit or WKWebView rendering.
CI also compiles the observer and executes the resource smoke test. It does not
grant Screen Recording access or establish 2× frame-capture acceptance.
