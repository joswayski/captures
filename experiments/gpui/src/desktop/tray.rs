use super::Action;
use captures_capture::CaptureMode;
use std::sync::mpsc;
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

pub struct NativeTray {
    sender: mpsc::Sender<Action>,
    _tray: TrayIcon,
}

impl NativeTray {
    pub fn new(sender: mpsc::Sender<Action>) -> anyhow::Result<Self> {
        let menu = Menu::new();
        for (index, (label, _)) in actions().iter().enumerate() {
            menu.append(&MenuItem::with_id(
                format!("captures-{index}"),
                label,
                true,
                None,
            ))?;
        }
        let tray = TrayIconBuilder::new()
            .with_id("captures-gpui")
            .with_tooltip("Captures")
            .with_icon(icon()?)
            .with_icon_as_template(cfg!(target_os = "macos"))
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .build()?;
        Ok(Self {
            sender,
            _tray: tray,
        })
    }

    pub fn poll(&self) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            let Some(index) = event.id.0.strip_prefix("captures-") else {
                continue;
            };
            let Some((_, action)) = index
                .parse::<usize>()
                .ok()
                .and_then(|index| actions().into_iter().nth(index))
            else {
                continue;
            };
            let _ = self.sender.send(action);
        }
    }
}

fn actions() -> [(&'static str, Action); 12] {
    [
        ("New Capture", Action::Capture(CaptureMode::Region, 0)),
        ("Region screenshot", Action::Capture(CaptureMode::Region, 0)),
        ("Window screenshot", Action::Capture(CaptureMode::Window, 0)),
        (
            "Full screen screenshot",
            Action::Capture(CaptureMode::Display, 0),
        ),
        ("Record video", Action::Capture(CaptureMode::Region, 1)),
        ("Record GIF", Action::Capture(CaptureMode::Region, 2)),
        ("Show recording controls", Action::RestoreControls),
        ("Show mini previews", Action::Previews),
        ("Capture History", Action::History),
        ("Open image or recording…", Action::Open),
        ("Preferences", Action::Preferences),
        ("Quit Captures", Action::Quit),
    ]
}

fn icon() -> Result<Icon, tray_icon::BadIcon> {
    const SIDE: u32 = 32;
    let mut rgba = vec![0_u8; (SIDE * SIDE * 4) as usize];
    for y in 5..27 {
        for x in 4..28 {
            let border = !(7..25).contains(&x) || !(8..24).contains(&y);
            let lens = {
                let dx = x as i32 - 16;
                let dy = y as i32 - 16;
                (36..=64).contains(&(dx * dx + dy * dy))
            };
            if border || lens {
                let pixel = ((y * SIDE + x) * 4) as usize;
                rgba[pixel..pixel + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, SIDE, SIDE)
}
