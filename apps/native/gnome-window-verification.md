# Stock GNOME window capture verification

This opt-in diagnostic uses unmodified GNOME Shell/Mutter and
`xdg-desktop-portal-gnome` on a disposable, private, software-rendered Wayland
desktop. It is not a passing native parity gate. No production capture policy,
installed profile, renderer selection, signing, release channel or deployment
changes are part of this slice.

## Reproduce in a disposable Debian 12 environment

The repository's Linux dependencies must already be installed. Add the stock
GNOME backend and its CLI runtime only in the disposable environment:

```sh
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
  gnome-shell mutter xdg-desktop-portal-gnome gjs gir1.2-atspi-2.0
node apps/native/prepare.mjs --output apps/native/wgpu/resources
cargo +1.95.0 build --manifest-path apps/native/wgpu/Cargo.toml --locked --bins -j 2
mkdir -p .amp/in/artifacts
/usr/bin/python3 apps/native/gnome_window_smoke.py \
  --video-binary apps/native/wgpu/target/debug/wayland_video_probe \
  --screenshot-binary apps/native/wgpu/target/debug/wayland_screenshot_probe \
  --recording-binary apps/native/wgpu/target/debug/wayland_recording_probe \
  --binary apps/native/wgpu/target/debug/captures-wgpu-workbench \
  --output .amp/in/artifacts/gnome-window-current
```

The output directory must not exist. This command currently **exits 1** while
retaining `result.json`, logs, unmodified capture files, chooser screenshots,
recording History metadata and recursive `SHA256SUMS`. Do not mask that exit as
a passing gate. The optional recording/native binaries add their respective
cases; omitting them does not verify native screenshot/History behavior.

The harness launches the stock compositor with:

```sh
gnome-shell --wayland --headless --virtual-monitor 1280x900 \
  --no-x11 --wayland-display captures-gnome
```

It supplies fresh HOME/XDG directories, a private session bus (also the private
system-bus address), private PipeWire/WirePlumber, `LIBGL_ALWAYS_SOFTWARE=1`,
`LP_NUM_THREADS=2` and the stock GNOME portal descriptor. `DISPLAY` is absent
from every capture process. GSettings uses a disposable keyfile backend and
native profiles disable system-shortcut takeover and login registration.
Owned process groups, D-Bus-activated helpers and the disposable document mount
are stopped/detached; the private profile is removed even on failed verdicts.

AT-SPI identifies real chooser rows. Mutter's private RemoteDesktop API injects
pointer/keyboard input to click the actual chooser; it does not replace consent.
An associated monitor stream supplies pointer-coordinate mapping only and is
never consumed as capture output. The pinned chooser's centered placement is an
input fixture, not a source/window geometry oracle. Shell Screenshot supplies
review images only. The invisible SNI host used to invoke the native tray menu
is simulated; the capture portal and its chooser are not.

## Observed on unchanged capture code

The complete matrix ran on both the #1056 base and current
[#1053](https://github.com/joswayski/captures/commit/e539a51129524eb50cd2690a9e3e4d763debed68).
The latter contains #1054, #1055, #1056 and #1057. Rebuilt native binaries on that
base reproduce the same findings. The harness records binary hashes and the
checkout commit in `result.json`.

Stock versions: Shell `43.9-0+deb12u2`, Mutter `43.8-0+deb12u1`, GNOME portal
`43.1-2`, public portal `1.16.0-2`, PipeWire `0.3.65-3+deb12u1`, WirePlumber
`0.4.13-1`, gjs `1.74.2-1+deb12u1`. No `/dev/dri` was available. Mutter logs
`Created surfaceless renderer without GPU`, disables DMA-BUF sharing and adds
virtual monitor `Meta-0`.

Public ScreenCast properties are **version 4, AvailableSourceTypes=3,
AvailableCursorModes=7**. This establishes real window-sharing capability, unlike
the existing private wlr backend's `AvailableSourceTypes=1` control. Window
requests use only `types=2`, `multiple=false`, `cursor_mode=1`; real grants return
one stream with `source_type=2`, `id="0"`, `size=[1280,900]`. The ID is not treated
as a native window ID. Only the explicit display control requests `types=1`.

The chosen undecorated GTK window is independently specified as 320×200 with
asymmetric splits at x=117/y=73. Four colors alternate every 120 ms between
`(35,69,103)/(211,37,81)/(51,173,29)/(73,41,197)` and
`(181,23,57)/(31,211,63)/(93,17,191)/(207,127,41)`. A separate teal window is a
negative source-selection control.

| Case | Actual result |
| --- | --- |
| Window video source | 12 frames; both corner colors observed; all 64,000 selected-window pixels match an independently specified phase exactly |
| Window still and fresh repeat | Exact selected-window pixels, but published PNGs are 1280×900; everything outside the top-left 320×200 is opaque black, not the decoy or desktop; `window_contract=false` |
| MP4/History diagnostic | Both phases decoded with original frame timing, four interior RGB samples within 6 levels for lossy H.264; `target={"type":"portal_window"}`, `kind="video"`, `mode=null`, dimensions 1280×900; no window/display ID or bounds invented |
| Display control | Real `source_type=1`, 1280×900, contains the teal decoy and GNOME desktop; visibly different from window-only output |
| Chooser Cancel | `event="cancelled"`, exit 0, no image, client Session.Close, capture nodes removed |
| In-stream cancellation | Exit 1, `Video portal diagnostic cancelled`, no image, Session.Close and capture nodes removed |
| Selected-window loss | Exit 1 after 32 ms in the current run; stream disconnected, no image, capture nodes removed; a recreated fixture acquires successfully afterward (still padded) |
| Native dark/light Window menu | No OpenPipeWireRemote, no new History files; successful native screenshot/History unverified; Session.Close not observed within 12 seconds before app shutdown |
| Pending Request.Close | Actual chooser left pending; client reports cancellation, no image, Request.Close/Session.Close; stock GNOME portal exits -11 (SIGSEGV), with a `gtk_window_destroy` critical and public-portal NoReply |
| Request after backend crash | Exit 1, CreateSession Response 2, no image or display substitution; public portal logs missing backend ScreenCast interface |
| Disposal | `owned_processes_stopped=true`, `private_profile_removed=true`; no remaining private compositor, portal, PipeWire or profile after the run |

The current MP4 History entry records 919 ms, 9,160 bytes and 11 dropped frames.
This is a no-window recording diagnostic, **not native screenshot History**.
Software-rendered frame delivery is not a performance or hardware acceptance gate.

## Limits and source interpretation

Mutter intentionally negotiates a monitor-sized window stream because its stream
cannot resize with the window, and supplies window bounds through VideoCrop:
[window stream sizing](https://github.com/GNOME/mutter/blob/43.8/src/backends/meta-screen-cast-window-stream.c#L248-L255),
[VideoCrop calculation](https://github.com/GNOME/mutter/blob/43.8/src/backends/meta-screen-cast-window-stream-src.c#L313-L330).
The [current adapter](../../crates/captures-recording-xcap/src/portal.rs) requests
and reads VideoTransform, not VideoCrop, and decodes the full negotiated extent.
This explains a likely integration defect, not proof of crop metadata delivered
to this client: this slice did not negotiate or inspect the actual VideoCrop.
It never crops the published evidence to make the contract pass. A separate
implementation slice should first inspect real buffer metadata.

The native route additionally requires a known active/unlocked session. GNOME
43 creates ScreenShield only when
[LoginManager.canLock succeeds](https://github.com/GNOME/gnome-shell/blob/43.9/js/ui/main.js#L239-L240),
which requires
[real GDM and systemd seats](https://github.com/GNOME/gnome-shell/blob/43.9/js/misc/loginManager.js#L35-L50).
Neither exists on this private bus. Public no-window acquisition works, but the
native capture guard must not assume unlock. No fake login1/ScreenSaver or
production bypass was added. Native grant, successful screenshot History fields,
failure/cancellation restoration and repeated native capture remain unverified.
The post-action review images show Preferences, not a successful History result.

The stock headless/virtual-monitor switches and software-renderer path are
documented in authoritative Mutter sources:
[context options](https://github.com/GNOME/mutter/blob/43.8/src/core/meta-context-main.c),
[native renderer](https://github.com/GNOME/mutter/blob/43.8/src/backends/native/meta-renderer-native.c).
The actual portal's
[window selection](https://github.com/GNOME/xdg-desktop-portal-gnome/blob/43.1/src/screencast.c)
remains unchanged. No mock/dependency evidence closes a gate. Physical GNOME,
KDE, GPU/DMA-BUF, cursor/audio, resizing, rotation, mixed-DPI and physical
macOS/Windows/X11 acceptance remain open; those other platform implementations
are unchanged and were not exercised by this slice.

## Supporting checks and artifact identity

`npm run check` passed. All native binaries built with the command above. Existing
private-bus window protocol controls passed: 14 source, 15 screenshot and 14
recording cases. They test spoof/admission/cancellation/cleanup contracts, not
real window capture. Invoke the unchanged `protocols` function under
`dbus-run-session` with `target="window"`, the respective probe binary and
`screenshot=True` or `recording=True`, with DISPLAY removed and a diagnostic-only
WAYLAND_DISPLAY. The full Sway rotation harness was not rerun: its independent
grim/flipped reference failure is owned by a separate investigation.

Pixel-checker mutation controls accept an independently drawn exact 320×200
fixture, fail the size contract for its 1280×900 padded version, and reject a
single altered pixel. During iteration, extracting only the first eight frames
with FFmpeg's default duplication falsely suggested a frozen MP4; the final
harness uses `-fps_mode passthrough` and checks all decoded frames (bounded at
100). Earlier native attempts timed out on cancellation/repeat; this result
records that limit instead of claiming cleanup or simulating a session.

Current-run SHA-256 identities (full paths/hashes, including binaries, are in
`result.json` and `SHA256SUMS`; evidence is not checked into Git):

```text
f1b47caeb502d6926a8dc28d6aea8a5766d3d42bdffc261229c2188547432503  result.json
9649083d1f9849ab64c5787afbe60dad429162abe428d258103036ad2b941dad  still.png
2051ff4d71143dc42b046613bf9767871ec2db29432efd4fd3b90dbe2c95662e  public-portal.jsonl
00f91f7d423c18094874190b9894c3925801ab32a33b5d83d74ea703ecd9a800  recording/history/ff3281c5-34ad-42ec-afb0-7120e2e37493/media.mp4
21d74970a8cb32fbfd9755f19af64de4edd0431023868a3e35dcfaa0aa762c4d  recording/history/ff3281c5-34ad-42ec-afb0-7120e2e37493/metadata.json
```
