use anyhow::Result;
use captures_media::{CancelToken, MediaToolchain, RecordingAudioLayout, RecordingSegmentInput};
use captures_recording::{DraftStore, RecordingDraftManifest, RecordingKind, RecordingState};
use gpui::{
    App, AppContext, Bounds, Context, IntoElement, PromptLevel, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, rgb, size,
};
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::lifecycle::publish_unique;
use crate::ui::{button, metric, root, theme};

struct RecoveryView {
    store: DraftStore,
    drafts: Vec<RecordingDraftManifest>,
    busy: Option<String>,
    error: Option<String>,
}

impl RecoveryView {
    fn recover(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(id.clone());
        self.error = None;
        let store = self.store.clone();
        let recover_id = id.clone();
        let task = cx.background_spawn(async move { recover_one(&store, &recover_id) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(path) => {
                        this.drafts.retain(|draft| draft.session_id != id);
                        crate::app::saved(path, cx);
                    }
                    Err(error) => this.error = Some(error),
                }
                cx.notify();
            })
        })
        .detach();
        cx.notify();
    }

    fn confirm_discard(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let remove_id = id.clone();
        let answer = window.prompt(
            PromptLevel::Warning,
            "Discard unfinished recording?",
            Some("Its private source segments cannot be recovered afterward."),
            &["Discard", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.unwrap_or(1) == 0 {
                this.update(cx, |this, cx| {
                    match this.store.remove(&remove_id) {
                        Ok(()) => this.drafts.retain(|draft| draft.session_id != remove_id),
                        Err(error) => this.error = Some(error.to_string()),
                    }
                    cx.notify();
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

impl Render for RecoveryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        let mut page = root(colors).id("recording-recovery-page").overflow_y_scroll().p(metric("--s-8")).gap(metric("--s-6"))
            .child(div().text_size(px(24.)).font_weight(gpui::FontWeight::BOLD).child("Recover unfinished recordings"))
            .child(div().text_color(colors.muted()).child("Completed segments survived an interrupted session. Recovery creates a new file and keeps source media until publishing succeeds."));
        if self.drafts.is_empty() {
            page = page.child(
                div()
                    .p(metric("--s-8"))
                    .child("No interrupted recordings need recovery."),
            );
        }
        for (index, draft) in self.drafts.clone().into_iter().enumerate() {
            let id = draft.session_id.clone();
            let recovering = self.busy.as_deref() == Some(id.as_str());
            let completed = draft
                .segments
                .iter()
                .filter(|segment| segment.complete)
                .count();
            page = page.child(
                div()
                    .p(metric("--s-6"))
                    .rounded(metric("--r-lg"))
                    .border_1()
                    .border_color(colors.border())
                    .bg(colors.raised())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(metric("--s-3"))
                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                                "{} · {completed} completed segment{}",
                                if draft.options.kind == RecordingKind::Gif {
                                    "GIF"
                                } else {
                                    "Video"
                                },
                                if completed == 1 { "" } else { "s" }
                            )))
                            .child(
                                div().text_size(px(12.)).text_color(colors.muted()).child(
                                    draft
                                        .last_error
                                        .unwrap_or_else(|| "Ready to recover".into()),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(metric("--s-4"))
                            .child(
                                button(
                                    ("recover", index),
                                    if recovering {
                                        "Recovering…"
                                    } else {
                                        "Recover"
                                    },
                                    colors,
                                )
                                .when(
                                    !recovering && self.busy.is_none(),
                                    |button| {
                                        let id = id.clone();
                                        button.on_click(cx.listener(move |this, _, _, cx| {
                                            this.recover(id.clone(), cx)
                                        }))
                                    },
                                ),
                            )
                            .child(button(("discard", index), "Discard", colors).when(
                                self.busy.is_none(),
                                |button| {
                                    let id = id.clone();
                                    button.on_click(cx.listener(move |this, _, window, cx| {
                                        this.confirm_discard(id.clone(), window, cx)
                                    }))
                                },
                            )),
                    ),
            );
        }
        if let Some(error) = self.error.clone() {
            page = page.child(
                div()
                    .p(metric("--s-5"))
                    .rounded(metric("--r-sm"))
                    .bg(rgb(0x5a1d24))
                    .text_color(rgb(0xffd7da))
                    .child(error),
            );
        }
        page
    }
}

pub(super) fn open(cx: &mut App) -> Result<()> {
    let root = crate::settings::data_dir().join("recording-drafts");
    let store = DraftStore::new(root);
    let drafts = store.list()?;
    if drafts.is_empty() {
        return Ok(());
    }
    let bounds = Bounds::centered(None, size(px(700.), px(520.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("captures-gpui-recording-recovery".into()),
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title("Captures GPUI Recording recovery");
            cx.new(|_| RecoveryView {
                store,
                drafts,
                busy: None,
                error: None,
            })
        },
    )?;
    Ok(())
}

fn recover_one(store: &DraftStore, id: &str) -> std::result::Result<PathBuf, String> {
    let mut manifest = store.load(id).map_err(|error| error.to_string())?;
    let result = (|| {
        if manifest.schema_version != 1 {
            return Err(format!(
                "Recording draft schema {} is unsupported",
                manifest.schema_version
            ));
        }
        let directory = store
            .session_directory(id)
            .map_err(|error| error.to_string())?;
        let destination = fs::read_to_string(directory.join("destination.txt"))
            .map(PathBuf::from)
            .map_err(|_| {
                "The interrupted recording does not include a recovery destination.".to_owned()
            })?;
        let mut complete = manifest
            .segments
            .iter()
            .filter(|segment| segment.complete)
            .collect::<Vec<_>>();
        complete.sort_by_key(|segment| segment.index);
        if complete.is_empty() {
            return Err("The recording draft has no completed media segments.".into());
        }
        if complete
            .iter()
            .enumerate()
            .any(|(index, segment)| segment.index != index as u32)
        {
            return Err("The recording draft segment indexes are not contiguous.".into());
        }
        let inputs = complete
            .into_iter()
            .map(|segment| {
                Ok(RecordingSegmentInput {
                    video_path: checked(&directory, &segment.relative_path)?,
                    system_audio_path: segment
                        .system_audio_relative_path
                        .as_deref()
                        .map(|path| checked(&directory, path))
                        .transpose()?,
                    system_audio_offset_ms: segment.system_audio_offset_ms,
                    microphone_path: segment
                        .microphone_relative_path
                        .as_deref()
                        .map(|path| checked(&directory, path))
                        .transpose()?,
                    microphone_offset_ms: segment.microphone_offset_ms,
                    duration_ms: segment.duration_ms,
                })
            })
            .collect::<std::result::Result<Vec<_>, String>>()?;
        let tools = MediaToolchain::from_command_names();
        tools.verify().map_err(|error| error.to_string())?;
        let cancel = CancelToken::default();
        let extension = if manifest.options.kind == RecordingKind::Gif {
            "gif"
        } else {
            "mp4"
        };
        let staging = directory.join(format!("recovered.{extension}"));
        if manifest.options.kind == RecordingKind::Gif {
            let master = directory.join("recovered-master.mp4");
            tools
                .concatenate_segments(
                    &inputs
                        .iter()
                        .map(|input| input.video_path.clone())
                        .collect::<Vec<_>>(),
                    &master,
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
            tools
                .create_gif(
                    &master,
                    &staging,
                    manifest.options.frames_per_second,
                    manifest.options.gif.max_width,
                    manifest.options.gif.max_colors,
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
        } else {
            tools
                .assemble_recording_segments(
                    &inputs,
                    &staging,
                    RecordingAudioLayout {
                        system_audio: inputs.iter().any(|input| input.system_audio_path.is_some()),
                        microphone_audio: inputs
                            .iter()
                            .any(|input| input.microphone_path.is_some()),
                    },
                    &cancel,
                )
                .map_err(|error| error.to_string())?;
        }
        let path = publish_unique(&staging, &destination, extension)?;
        store.remove(id).map_err(|error| error.to_string())?;
        Ok(path)
    })();
    if let Err(error) = &result {
        manifest.state = RecordingState::Failed;
        manifest.last_error = Some(error.clone());
        let _ = store.save(&manifest);
    }
    result
}

fn checked(root: &Path, relative: &str) -> std::result::Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Recording draft media path escaped its session.".into());
    }
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Recording draft media must be a regular private file.".into());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_rejects_parent_and_absolute_paths() {
        let root = Path::new("/tmp/draft");
        assert!(checked(root, "../secret.mp4").is_err());
        assert!(checked(root, "/tmp/secret.mp4").is_err());
    }
}
