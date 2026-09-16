//! The shipping permissions screen, backed by the shared native permission APIs.
use super::*;
use std::time::Duration;

#[derive(Default, PartialEq)]
pub(super) struct Permissions {
    screen_ready: bool,
    can_request: bool,
    requested: bool,
    microphone_ready: bool,
    microphone_asked: bool,
    microphone_can_request: bool,
}

impl Permissions {
    fn refresh(&mut self, _settings: &Settings) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.can_request =
                _settings.last_screen_permission_request_id.as_deref() != Some(&request_id()?);
            self.screen_ready = captures_capture::XcapBackend
                .ensure_permission(!self.can_request || self.requested)
                .is_ok();
            self.microphone_ready = captures_recording_macos::microphone_authorized();
            self.microphone_can_request = captures_recording_macos::microphone_can_request();
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.screen_ready = true;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn request_id() -> Result<String> {
    let executable = std::env::current_exe()?;
    let metadata = executable.metadata()?;
    Ok(format!(
        "{}:{}:{}",
        executable.display(),
        metadata.len(),
        metadata
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ))
}

#[cfg(target_os = "macos")]
fn open_settings(section: &str) -> Result<()> {
    anyhow::ensure!(
        std::process::Command::new("open")
            .arg(format!(
                "x-apple.systempreferences:com.apple.preference.security?{section}"
            ))
            .status()?
            .success(),
        "Could not open macOS Privacy settings"
    );
    Ok(())
}

fn icon(kind: &'static str, color: Rgba) -> Img {
    thread_local! {
        static ICONS: std::cell::RefCell<std::collections::HashMap<String, std::sync::Arc<RenderImage>>> = Default::default();
    }
    let color = format!(
        "#{:02x}{:02x}{:02x}",
        (color.r * 255.) as u8,
        (color.g * 255.) as u8,
        (color.b * 255.) as u8
    );
    let key = format!("{kind}{color}");
    let image = ICONS.with(|cache| cache.borrow_mut().entry(key).or_insert_with(|| {
        let path = match kind {
            "mark" => r#"<path d="M9 4H7a3 3 0 0 0-3 3v2M15 4h2a3 3 0 0 1 3 3v2M20 15v2a3 3 0 0 1-3 3h-2M9 20H7a3 3 0 0 1-3-3v-2M12 8.5c.4 1.8 1.7 3.1 3.5 3.5-1.8.4-3.1 1.7-3.5 3.5-.4-1.8-1.7-3.1-3.5-3.5 1.8-.4 3.1-1.7 3.5-3.5Z"/>"#,
            "mic" => r#"<rect x="9" y="3" width="6" height="11" rx="3"/><path d="M6 11a6 6 0 0 0 12 0M12 17v4M9 21h6"/>"#,
            "check" => r#"<path d="m4.8 12.3 4.8 4.8L19.2 6.9"/>"#,
            _ => r#"<rect x="3" y="4" width="18" height="13" rx="2.5"/><path d="M8 21h8M12 17v4"/>"#,
        };
        crate::editor::svg_render_image(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="{color}" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">{path}</svg>"#))
    }).clone());
    img(image).size(px(20.))
}

fn granted(label: &'static str, t: Theme) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.positive)
        .child(label)
        .child(icon("check", t.positive).size(px(14.)))
}

impl Surface {
    fn refresh_permissions(&mut self) {
        if let Err(error) = self.permissions.refresh(&self.settings) {
            self.status = format!("Couldn’t check screen access: {error}");
        }
    }

    fn request_screen(&mut self, cx: &mut Context<Self>) {
        #[cfg(target_os = "macos")]
        {
            let result = (|| -> Result<()> {
                let id = request_id()?;
                let request = self.settings.last_screen_permission_request_id.as_ref() != Some(&id);
                self.settings.last_screen_permission_request_id = Some(id);
                self.persist(cx);
                anyhow::ensure!(self.status == "Changes saved", "{}", self.status);
                match captures_capture::XcapBackend.ensure_permission(request) {
                    Ok(()) => {}
                    Err(captures_capture::CaptureError::PermissionRequestStarted) => {
                        self.permissions.requested = true
                    }
                    Err(captures_capture::CaptureError::PermissionDenied) => {
                        self.permissions.requested = true;
                        open_settings("Privacy_ScreenCapture")?;
                    }
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            })();
            self.status = result
                .err()
                .map(|e| format!("Couldn’t open screen access: {e}"))
                .unwrap_or_default();
        }
        self.refresh_permissions();
        cx.notify();
    }

    pub(super) fn onboarding(&mut self, window: &Window, cx: &mut Context<Self>, t: Theme) -> Div {
        if !self.permission_polling {
            self.permission_polling = true;
            self.refresh_permissions();
            cx.spawn(async |this, cx| {
                loop {
                    Timer::after(Duration::from_millis(1500)).await;
                    if !this
                        .update(cx, |s, cx| {
                            if s.page != Page::Onboarding {
                                s.permission_polling = false;
                                return false;
                            }
                            let previous = s.permissions.screen_ready;
                            s.refresh_permissions();
                            if cfg!(target_os = "macos") || previous != s.permissions.screen_ready {
                                cx.notify();
                            }
                            true
                        })
                        .unwrap_or(false)
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        let short = window.viewport_size().height <= px(600.);
        let ch = window
            .text_system()
            .shape_line(
                "0".into(),
                px(13.),
                &[TextRun {
                    len: 1,
                    font: gpui::font(font()),
                    color: t.text.into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
            .width;
        let restart = !self.permissions.screen_ready && self.permissions.requested;
        let still_off = !self.permissions.screen_ready && !self.permissions.can_request && !restart;
        let description = if cfg!(target_os = "macos") {
            if still_off {
                "The switch for this copy of Captures is still off. A local build is a different row from a downloaded app. Turn it on, then restart."
            } else if restart {
                "Turn the switch on next to this copy of Captures, then restart. A local build is a different row from a downloaded app. macOS does not apply the permission until Captures relaunches."
            } else {
                "This allows Captures to read the pixels you choose to capture. macOS keeps everything else hidden."
            }
        } else if cfg!(target_os = "windows") {
            "Windows provides screen capture access without a separate permission prompt. Secure and protected windows remain private."
        } else {
            "Your desktop may show its own screen-sharing picker when a capture starts. There is nothing to approve ahead of time."
        };
        let screen_action = if self.permissions.screen_ready {
            granted(
                if cfg!(target_os = "macos") {
                    "Granted"
                } else {
                    "Ready"
                },
                t,
            )
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(6.))
                .min_w(px(118.))
                .when(restart || still_off, |d| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.subtle)
                            .child(if restart {
                                "Restart required"
                            } else {
                                "Still off"
                            }),
                    )
                })
                .child(
                    self.button(
                        "screen-permission",
                        if self.permissions.can_request {
                            "Allow access"
                        } else {
                            "Open Settings"
                        },
                        false,
                        t,
                    )
                    .on_click(cx.listener(|s, _, _, cx| s.request_screen(cx))),
                )
                .into_any_element()
        };
        let permissions = div()
            .rounded(px(14.))
            .border_1()
            .border_color(t.border)
            .bg(t.raised)
            .child(
                div()
                    .p_4()
                    .flex()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .w(px(22.))
                            .flex_shrink_0()
                            .mt(px(1.))
                            .child(icon("screen", t.subtle)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Screen capture"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .line_height(px(16.2))
                                    .text_color(t.subtle)
                                    .child(description),
                            ),
                    )
                    .child(screen_action),
            );
        #[cfg(target_os = "macos")]
        let permissions = permissions.child(div().p_4().border_t_1().border_color(t.border).flex().items_start().gap_3()
            .child(div().w(px(22.)).flex_shrink_0().child(icon("mic", t.subtle)))
            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                .child(div().flex().items_baseline().gap_2().child("Microphone").child(div().text_size(px(11.)).text_color(t.subtle).child("Optional")))
                .child(div().text_size(px(12.)).line_height(px(16.2)).text_color(t.subtle).child(if self.permissions.microphone_ready {
                    "macOS will not ask again. Turn the microphone on when you start a recording."
                } else if self.permissions.microphone_asked { "Turn Captures on in Microphone settings. macOS only lists apps after they ask." }
                else { "Allow it now so a recording does not pause to ask, or wait until you pick a mic." })))
            .child(if self.permissions.microphone_ready { granted("Granted", t).into_any_element() } else {
                self.button("mic-permission", if self.permissions.microphone_asked && !self.permissions.microphone_can_request { "Open Settings" } else { "Allow microphone" }, false, t)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.permissions.microphone_asked = true;
                        if !captures_recording_macos::request_microphone_access() && !captures_recording_macos::microphone_can_request() {
                            s.status = open_settings("Privacy_Microphone").err().map(|e| e.to_string()).unwrap_or_default();
                        }
                        s.refresh_permissions(); cx.notify();
                    })).into_any_element()
            }));
        let ready = self.permissions.screen_ready;
        let button = self
            .button(
                "finish",
                if restart {
                    "Restart Captures"
                } else {
                    "Start capturing"
                },
                true,
                t,
            )
            .min_w(px(148.))
            .h(px(40.))
            .rounded_none()
            .border_0()
            .bg(t.text)
            .text_color(t.raised)
            .font_weight(FontWeight::SEMIBOLD)
            .flex()
            .items_center()
            .justify_center()
            .opacity(if ready || restart { 1. } else { 0.4 })
            .on_click(cx.listener(move |s, _, window, cx| {
                if restart {
                    cx.restart();
                    return;
                }
                s.refresh_permissions();
                if !s.permissions.screen_ready {
                    return;
                }
                s.settings.onboarding_completed = true;
                s.persist(cx);
                if s.status != "Changes saved" {
                    return;
                }
                let launch = s.launch.clone();
                let shortcut = crate::notices::shortcut_tokens(&s.settings.new_capture_shortcut);
                window.remove_window();
                cx.spawn(async move |_, cx| {
                    Timer::after(Duration::from_millis(16)).await;
                    let _ = cx
                        .update(|cx| crate::notices::show_launch_after_setup(shortcut, launch, cx));
                })
                .detach();
            }));
        let button = if ready && !crate::theme::reduced_motion() {
            button
                .with_animation(
                    "onboarding-cta-pulse",
                    Animation::new(Duration::from_millis(2600)).repeat(),
                    move |button, progress| {
                        let halo = (1. - (progress * std::f32::consts::TAU).cos()) * 2.5;
                        button.shadow(vec![BoxShadow {
                            color: t.hover.into(),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(0.),
                            spread_radius: px(halo),
                        }])
                    },
                )
                .into_any_element()
        } else {
            button.into_any_element()
        };
        div().size_full().bg(t.canvas).child(div().w_full().max_w(px(620.)).h_full().mx_auto()
            .px(px(if short { 20. } else { 24. })).py(px(if short { 16. } else { 32. }))
            .flex().flex_col().justify_center().gap(px(if short { 12. } else { 20. }))
            .child(div().flex().flex_col().items_start().gap(px(6.))
                .child(div().size(px(40.)).mb_1().rounded(px(10.)).bg(t.accent).flex().items_center().justify_center().child(icon("mark", rgb(0x131318)).size(px(24.))))
                .child(div().text_size(px(10.)).font_weight(FontWeight::SEMIBOLD).text_color(t.subtle).child("WELCOME TO CAPTURES"))
                .child(div().text_size(px(22.)).line_height(px(26.4)).font_weight(FontWeight::SEMIBOLD).child(if cfg!(target_os="macos") { "Required permissions" } else { "You’re ready to capture" }))
                .when(!short, |d| d.child(div().max_w(ch * 54.).text_color(t.subtle).line_height(px(19.5)).child("Captures only reads the pixels you choose to capture. Nothing is uploaded, and nothing leaves this computer unless you send it somewhere."))))
            .child(div().flex().flex_col().gap_3().child(permissions)
                .when(!self.status.is_empty(), |d| d.child(div().px_3().py_2().text_size(px(12.)).text_color(t.signal).child(self.status.clone())))
                .child(div().flex().justify_end().child(button))))
    }
}
