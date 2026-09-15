use crate::{
    Launch,
    effects::{Dissolve, HEIGHT, PAD, WIDTH},
    theme::{Theme, font},
};
use gpui::{prelude::*, *};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

fn save_copy(source: &Path, destination: &Path) -> std::io::Result<()> {
    // Reading into a separate file first also handles choosing the source itself
    // or a hard-link alias without truncating the user's capture.
    let parent = destination.parent().unwrap_or(Path::new("."));
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    std::io::copy(&mut std::fs::File::open(source)?, staged.as_file_mut())?;
    staged.as_file().sync_all()?;
    staged.persist(destination).map_err(|error| error.error)?;
    Ok(())
}

struct Preview {
    launch: Launch,
    paths: Vec<PathBuf>,
    collapsed: bool,
    transition: Option<Instant>,
    deleting: Option<(usize, Instant, Dissolve)>,
    confirm: Option<usize>,
    status: String,
}

pub fn open(launch: Launch, cx: &mut App) -> anyhow::Result<()> {
    let paths = if let Some(path) = &launch.path {
        vec![path.clone(); if launch.mock { 3 } else { 1 }]
    } else {
        let mut paths = std::fs::read_dir(launch.profile.join("captures"))?
            .filter_map(Result::ok)
            .map(|f| f.path())
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| matches!(e, "png" | "jpg" | "jpeg" | "webp"))
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths.reverse();
        paths.truncate(3);
        paths
    };
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(560.), px(760.)),
                cx,
            ))),
            titlebar: launch.mock.then(|| TitlebarOptions {
                title: Some("Captures Preview fixture".into()),
                ..Default::default()
            }),
            window_background: if launch.mock {
                WindowBackgroundAppearance::Opaque
            } else {
                WindowBackgroundAppearance::Transparent
            },
            kind: if launch.mock {
                WindowKind::Normal
            } else {
                WindowKind::PopUp
            },
            ..Default::default()
        },
        |_, cx| {
            cx.new(|_| Preview {
                launch,
                paths,
                collapsed: false,
                transition: None,
                deleting: None,
                confirm: None,
                status: String::new(),
            })
        },
    )?;
    Ok(())
}

fn render_image(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(rgba)]))
}

impl Preview {
    fn button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
        let t = Theme::new(false);
        div()
            .id(id)
            .px_2()
            .h(px(29.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .bg(t.glass)
            .text_color(t.glass_text)
            .cursor_pointer()
            .hover(|s| s.bg(t.glass_border))
            .child(label)
    }
    fn dissolve(&mut self, index: usize, cx: &mut Context<Self>) {
        if self.deleting.is_some() {
            return;
        }
        match image::open(&self.paths[index]) {
            Ok(image) => {
                if !self.launch.mock {
                    if self.confirm != Some(index) {
                        self.confirm = Some(index);
                        cx.notify();
                        return;
                    }
                    if let Err(error) = std::fs::remove_file(&self.paths[index]) {
                        self.status = error.to_string();
                        cx.notify();
                        return;
                    }
                }
                self.confirm = None;
                self.deleting = Some((index, Instant::now(), Dissolve::new(&image.to_rgba8(), 83)));
            }
            Err(error) => self.status = error.to_string(),
        }
        cx.notify();
    }
}

impl Render for Preview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::new(false);
        if let Some((index, start, _)) = &self.deleting
            && start.elapsed().as_millis() >= 2550
        {
            self.paths.remove(*index);
            self.deleting = None;
        }
        let transition = self
            .transition
            .map(|start| (start.elapsed().as_secs_f32() / 0.28).min(1.))
            .unwrap_or(1.);
        if transition < 1. || self.deleting.is_some() {
            window.request_animation_frame();
        }
        let amount = if self.collapsed {
            1. - (1. - transition).powi(3)
        } else {
            (1. - transition).powi(3)
        };
        let mut body = div()
            .relative()
            .size_full()
            .font_family(font())
            .text_size(px(12.))
            .text_color(t.glass_text)
            .when(self.launch.mock, |d| d.bg(rgb(0x252b38)));
        for (index, path) in self.paths.iter().enumerate().rev() {
            let top = 150. + index as f32 * (172. * (1. - amount) + 10. * amount);
            let inset = index as f32 * 7. * amount;
            if let Some((deleting, start, effect)) = &self.deleting
                && *deleting == index
            {
                body = body.child(
                    div()
                        .absolute()
                        .left(px(138. - PAD as f32))
                        .top(px(top - PAD as f32))
                        .child(
                            img(render_image(
                                effect.frame(start.elapsed().as_secs_f32() * 1000.),
                            ))
                            .w(px((WIDTH + 2 * PAD) as f32))
                            .h(px((HEIGHT + 2 * PAD) as f32)),
                        ),
                );
                continue;
            }
            let open_path = path.clone();
            let save_path = path.clone();
            body = body.child(
                div()
                    .id(("preview", index))
                    .absolute()
                    .left(px(138. + inset))
                    .top(px(top))
                    .w(px(WIDTH as f32 - 2. * inset))
                    .h(px(HEIGHT as f32))
                    .rounded(px(12.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(t.glass_border)
                    .shadow_lg()
                    .cursor_pointer()
                    .on_click(cx.listener(|s, _, _, cx| {
                        if s.collapsed {
                            s.collapsed = false;
                            s.transition = Some(Instant::now());
                            cx.notify();
                        }
                    }))
                    .child(img(path.clone()).size_full().object_fit(ObjectFit::Cover))
                    .when(!self.collapsed, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(8.))
                                .left(px(8.))
                                .right(px(8.))
                                .flex()
                                .gap(px(6.))
                                .child(Self::button(("close", index), "×").on_click(cx.listener(
                                    move |s, _, _, cx| {
                                        cx.stop_propagation();
                                        if s.deleting.is_none() {
                                            s.paths.remove(index);
                                        }
                                        cx.notify();
                                    },
                                )))
                                .child(
                                    Self::button(
                                        ("trash", index),
                                        if self.confirm == Some(index) {
                                            "Delete?"
                                        } else {
                                            "⌫"
                                        },
                                    )
                                    .on_click(cx.listener(
                                        move |s, _, _, cx| {
                                            cx.stop_propagation();
                                            s.dissolve(index, cx);
                                        },
                                    )),
                                )
                                .child(div().flex_1())
                                .child(Self::button(("edit", index), "Edit").on_click(cx.listener(
                                    move |s, _, _, cx| {
                                        cx.stop_propagation();
                                        let mut launch = s.launch.clone();
                                        launch.path = Some(open_path.clone());
                                        if let Err(error) =
                                            crate::open_view("screenshot-editor", launch, cx)
                                        {
                                            s.status = error.to_string();
                                            cx.notify();
                                        }
                                    },
                                )))
                                .child(Self::button(("save", index), "Save").on_click(
                                    cx.listener(move |_, _, window, cx| {
                                        cx.stop_propagation();
                                        let source = save_path.clone();
                                        let receiver = cx.prompt_for_new_path(
                                            source.parent().unwrap_or(std::path::Path::new("/")),
                                            source.file_name().and_then(|n| n.to_str()),
                                        );
                                        cx.spawn_in(window, async move |this, cx| {
                                            if let Ok(Ok(Some(path))) = receiver.await {
                                                let result = cx
                                                    .background_executor()
                                                    .spawn(async move { save_copy(&source, &path) })
                                                    .await;
                                                let _ = this.update(cx, |s, cx| {
                                                    s.status = match result {
                                                        Ok(_) => "Saved".into(),
                                                        Err(e) => e.to_string(),
                                                    };
                                                    cx.notify();
                                                });
                                            }
                                        })
                                        .detach();
                                    }),
                                )),
                        )
                    }),
            );
        }
        body.child(
            div()
                .absolute()
                .top(px(108.))
                .left(px(138.))
                .w(px(284.))
                .flex()
                .gap_2()
                .child(
                    Self::button(
                        "collapse",
                        if self.collapsed {
                            "Show more"
                        } else {
                            "Show less"
                        },
                    )
                    .on_click(cx.listener(|s, _, _, cx| {
                        if s.deleting.is_none() {
                            s.collapsed = !s.collapsed;
                            s.transition = Some(Instant::now());
                            cx.notify();
                        }
                    })),
                )
                .child(div().flex_1())
                .when(self.paths.len() > 1, |d| {
                    d.child(Self::button("clear", "Clear all").on_click(cx.listener(
                        |s, _, _, cx| {
                            if s.deleting.is_none() {
                                s.paths.clear();
                                cx.notify();
                            }
                        },
                    )))
                }),
        )
        .child(
            div()
                .absolute()
                .bottom(px(20.))
                .left(px(40.))
                .right(px(40.))
                .child(self.status.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn saving_over_source_or_hardlink_preserves_capture() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.png");
        let alias = directory.path().join("alias.png");
        let bytes = b"complete capture bytes";
        std::fs::write(&source, bytes).unwrap();
        std::fs::hard_link(&source, &alias).unwrap();
        save_copy(&source, &source).unwrap();
        save_copy(&source, &alias).unwrap();
        assert_eq!(std::fs::read(source).unwrap(), bytes);
        assert_eq!(std::fs::read(alias).unwrap(), bytes);
    }
}
