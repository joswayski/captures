use super::{Action, tray::NativeTray};
use core_graphics::event::CGEvent;
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplication, NSPasteboard, NSPasteboardWriting, NSScreen, NSWindowSharingType, NSWorkspace,
};
use objc2_foundation::{NSArray, NSPoint, NSString, NSURL};
use std::{
    path::Path,
    sync::{Mutex, Once, OnceLock, mpsc},
    time::Duration,
};

pub use super::unix::{Instance, private_directory, private_file};

pub struct Integration(NativeTray);

impl Integration {
    pub fn new(sender: mpsc::Sender<Action>) -> anyhow::Result<Self> {
        Ok(Self(NativeTray::new(sender)?))
    }

    pub fn poll(&self) {
        self.0.poll();
    }
}

pub fn ensure_supported_session() -> anyhow::Result<()> {
    Ok(())
}

pub fn pointer() -> (i32, i32) {
    CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .ok()
        .and_then(|source| CGEvent::new(source).ok())
        .map(|event| {
            let point = event.location();
            (point.x.round() as i32, point.y.round() as i32)
        })
        .unwrap_or((0, 0))
}

pub fn login(enabled: bool) -> Result<(), String> {
    let home = std::env::var_os("HOME").ok_or("HOME is unavailable")?;
    let path = Path::new(&home).join("Library/LaunchAgents/io.captures.gpui.plist");
    if !enabled {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        };
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let executable = xml_escape(&executable.to_string_lossy());
    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>io.captures.gpui</string><key>ProgramArguments</key><array><string>{executable}</string><string>--background</string></array><key>RunAtLoad</key><true/></dict></plist>\n"
    );
    std::fs::create_dir_all(path.parent().expect("LaunchAgent path has a parent"))
        .map_err(|error| error.to_string())?;
    std::fs::write(&path, plist).map_err(|error| error.to_string())?;
    private_file(&path).map_err(|error| error.to_string())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn position_guide(title: &str, x: i32, y: i32) -> anyhow::Result<()> {
    let title = title.to_owned();
    run_on_main(move || position_guide_inner(&title, x, y))
        .ok_or_else(|| anyhow::anyhow!("timed out positioning recording guide"))?
}

fn position_guide_inner(title: &str, x: i32, y: i32) -> anyhow::Result<()> {
    let mtm = MainThreadMarker::new().expect("recording guides run on the AppKit main thread");
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app
        .windows()
        .iter()
        .find(|window| window.title().to_string() == title)
    else {
        return Ok(());
    };
    let frame = window.frame();
    let top = NSScreen::screens(mtm)
        .firstObject()
        .map(|screen| screen.frame().origin.y + screen.frame().size.height)
        .unwrap_or(frame.origin.y + frame.size.height);
    window.setFrameOrigin(NSPoint::new(
        f64::from(x),
        top - f64::from(y) - frame.size.height,
    ));
    window.setIgnoresMouseEvents(true);
    Ok(())
}

pub fn copy_file(path: &Path) -> Result<(), String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    run_on_main(move || copy_file_inner(&path))
        .ok_or_else(|| "timed out copying file on the AppKit main thread".to_owned())?
}

fn copy_file_inner(path: &Path) -> Result<(), String> {
    let string = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath(&string);
    let object = ProtocolObject::<dyn NSPasteboardWriting>::from_retained(url);
    let objects = NSArray::from_retained_slice(&[object]);
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    if pasteboard.writeObjects(&objects) {
        Ok(())
    } else {
        Err(format!("copy {} to the pasteboard", path.display()))
    }
}

pub fn reveal(path: &Path) -> Result<(), String> {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    run_on_main(move || reveal_inner(&path))
        .ok_or_else(|| "timed out revealing file on the AppKit main thread".to_owned())?
}

fn reveal_inner(path: &Path) -> Result<(), String> {
    let string = NSString::from_str(&path.to_string_lossy());
    let urls = NSArray::from_retained_slice(&[NSURL::fileURLWithPath(&string)]);
    NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&urls);
    Ok(())
}

pub fn exclude_from_capture(title: &str, excluded: bool) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("capture exclusion requires the AppKit main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app
        .windows()
        .iter()
        .find(|window| window.title().to_string() == title)
    else {
        return Ok(());
    };
    window.setSharingType(if excluded {
        NSWindowSharingType::None
    } else {
        NSWindowSharingType::ReadOnly
    });
    Ok(())
}

type PreviewRect = (f32, f32, f32, f32);
static PREVIEW_REGION: OnceLock<Mutex<Option<Vec<PreviewRect>>>> = OnceLock::new();
static PREVIEW_WATCHER: Once = Once::new();

pub fn set_preview_input_region(
    rectangles: &[PreviewRect],
    _: f32,
    _: (f32, f32),
) -> Result<(), String> {
    *PREVIEW_REGION
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "preview input region is poisoned".to_owned())? = Some(rectangles.to_vec());
    PREVIEW_WATCHER.call_once(|| {
        std::thread::Builder::new()
            .name("captures-preview-hit-test".into())
            .spawn(|| {
                loop {
                    let point = pointer();
                    DispatchQueue::main().exec_async(move || update_preview_hit_test(point));
                    std::thread::sleep(Duration::from_millis(25));
                }
            })
            .expect("spawn preview hit-test worker");
    });
    update_preview_hit_test(pointer());
    Ok(())
}

pub fn clear_preview_input_region() {
    if let Ok(mut region) = PREVIEW_REGION.get_or_init(Default::default).lock() {
        *region = None;
    }
}

fn update_preview_hit_test(pointer: (i32, i32)) {
    let mtm = MainThreadMarker::new().expect("preview hit tests run on the AppKit main thread");
    let rectangles = PREVIEW_REGION
        .get_or_init(Default::default)
        .lock()
        .ok()
        .and_then(|region| region.clone());
    let Some(rectangles) = rectangles else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let Some(window) = app
        .windows()
        .iter()
        .find(|window| window.title().to_string() == "Captures GPUI Previews")
    else {
        return;
    };
    let frame = window.frame();
    let local_x = f64::from(pointer.0) - frame.origin.x;
    let primary_top = NSScreen::screens(mtm)
        .firstObject()
        .map(|screen| screen.frame().origin.y + screen.frame().size.height)
        .unwrap_or(0.0);
    let pointer_y = primary_top - f64::from(pointer.1);
    let local_y = frame.size.height - (pointer_y - frame.origin.y);
    let interactive = rectangles.iter().any(|&(x, y, width, height)| {
        local_x >= f64::from(x)
            && local_x < f64::from(x + width)
            && local_y >= f64::from(y)
            && local_y < f64::from(y + height)
    });
    window.setIgnoresMouseEvents(!interactive);
}

fn run_on_main<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    if MainThreadMarker::new().is_some() {
        return Some(work());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    DispatchQueue::main().exec_async(move || {
        let _ = sender.send(work());
    });
    receiver.recv_timeout(Duration::from_secs(2)).ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn launch_agent_xml_escapes_executable_paths() {
        assert_eq!(super::xml_escape("/A&B/'C'"), "/A&amp;B/&apos;C&apos;");
    }
}
