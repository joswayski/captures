//! Isolated GPUI Capture History.
//!
//! Every indexed capture gets a private recovery copy. Expiration and confirmed
//! deletion remove that copy and private unsaved sources; a user's permanently
//! saved source is never deleted.
use crate::{
    settings::{atomic_write, data_dir},
    ui::{Theme, button, metric, root, theme},
};
use gpui::{
    App, AppContext, Bounds, Context, Entity, IntoElement, ObjectFit, Render, RetainAllImageCache,
    SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, div, image_cache, img,
    prelude::*, px, size,
};
use serde::{Deserialize, Serialize};
use std::{
    borrow::BorrowMut,
    path::{Path, PathBuf},
    process::Command,
    sync::{LazyLock, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

const RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const PAGE_SIZE: usize = 12;
static INDEX_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Screenshot,
    Video,
    Gif,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    id: String,
    source_path: PathBuf,
    library_path: PathBuf,
    poster_path: Option<PathBuf>,
    created_ms: u64,
    kind: Kind,
    width: u32,
    height: u32,
    size_bytes: u64,
    duration_ms: Option<u64>,
    has_audio: bool,
    #[serde(default)]
    dropped_frames: u64,
    #[serde(default)]
    source_private: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Filter {
    All,
    Screenshot,
    Video,
    Gif,
}

impl Filter {
    fn accepts(self, kind: Kind) -> bool {
        matches!(self, Self::All)
            || matches!(
                (self, kind),
                (Self::Screenshot, Kind::Screenshot)
                    | (Self::Video, Kind::Video)
                    | (Self::Gif, Kind::Gif)
            )
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn index_path(root: &Path) -> PathBuf {
    root.join("history.json")
}
fn library_dir(root: &Path) -> PathBuf {
    root.join("history")
}

fn read_index(root: &Path) -> Result<Vec<Entry>, String> {
    match std::fs::read(index_path(root)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("Could not parse GPUI capture history: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("Could not read GPUI capture history: {error}")),
    }
}

fn write_index(root: &Path, entries: &[Entry]) -> Result<(), String> {
    atomic_write(
        &index_path(root),
        &serde_json::to_vec_pretty(entries).map_err(|error| error.to_string())?,
    )
}

fn prune(entries: &mut Vec<Entry>, current_ms: u64) -> Vec<Entry> {
    let mut expired = Vec::new();
    entries.retain(|entry| {
        let keep = current_ms.saturating_sub(entry.created_ms) <= RETENTION_MS;
        if !keep {
            expired.push(entry.clone());
        }
        keep
    });
    expired
}

fn load_and_cleanup() -> Result<Vec<Entry>, String> {
    load_and_cleanup_at(&data_dir())
}

fn load_and_cleanup_at(root: &Path) -> Result<Vec<Entry>, String> {
    let _guard = INDEX_LOCK.lock().map_err(|error| error.to_string())?;
    let mut entries = read_index(root)?;
    let before = entries.len();
    let expired = prune(&mut entries, now_ms());
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_ms));
    if entries.len() != before {
        write_index(root, &entries)?;
        for entry in &expired {
            let _ = remove_private(root, entry);
        }
    }
    Ok(entries)
}

fn remove_private(root: &Path, entry: &Entry) -> Result<(), String> {
    let directory = entry
        .library_path
        .parent()
        .ok_or("History entry has no private directory.")?;
    if !directory.starts_with(library_dir(root)) {
        return Err("Refusing to delete a file outside GPUI Capture History.".into());
    }
    match std::fs::remove_dir_all(directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Could not delete private recovery copy: {error}")),
    }
    if entry.source_private && source_is_private(root, &entry.source_path) {
        match std::fs::remove_file(&entry.source_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Could not delete private unsaved capture: {error}")),
        }
    }
    Ok(())
}

fn source_is_private(root: &Path, source: &Path) -> bool {
    root.join("unsaved")
        .canonicalize()
        .is_ok_and(|unsaved| unsaved.is_dir() && source.starts_with(unsaved))
}

fn set_private_permissions(path: &Path, mode: u32) -> Result<(), String> {
    let result = if mode == 0o700 {
        crate::desktop::private_directory(path)
    } else {
        crate::desktop::private_file(path)
    };
    result.map_err(|error| format!("Could not protect GPUI Capture History data: {error}"))
}

/// Add a saved capture to the isolated GPUI history and create an independent
/// recovery copy. Call this off the UI thread for large recordings.
pub fn add(path: &Path) -> Result<(), String> {
    add_at(&data_dir(), path)
}

fn add_at(root: &Path, path: &Path) -> Result<(), String> {
    let source_path = path
        .canonicalize()
        .map_err(|error| format!("Could not index capture: {error}"))?;
    if !source_path.is_file() {
        return Err("Capture history accepts files only.".into());
    }
    let source_private = source_is_private(root, &source_path);
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = match extension.as_str() {
        "png" | "jpg" | "jpeg" | "webp" => Kind::Screenshot,
        "gif" => Kind::Gif,
        "mp4" | "webm" => Kind::Video,
        _ => {
            return Err(
                "Capture history supports PNG, JPEG, WebP, GIF, MP4, and WebM files.".into(),
            );
        }
    };
    let library_root = library_dir(root);
    std::fs::create_dir_all(&library_root).map_err(|error| error.to_string())?;
    set_private_permissions(&library_root, 0o700)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let (id, directory) = (0..=u32::MAX)
        .find_map(|attempt| {
            let id = format!("{stamp}-{}-{attempt}", std::process::id());
            let directory = library_root.join(&id);
            match std::fs::create_dir(&directory) {
                Ok(()) => Some(Ok((id, directory))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error.to_string())),
            }
        })
        .ok_or("Could not allocate a unique GPUI Capture History entry.")??;
    if let Err(error) = set_private_permissions(&directory, 0o700) {
        let _ = std::fs::remove_dir(&directory);
        return Err(error);
    }
    let library_path = directory.join(format!("media.{extension}"));
    if let Err(error) = std::fs::copy(&source_path, &library_path) {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(format!("Could not create history recovery copy: {error}"));
    }
    if let Err(error) = set_private_permissions(&library_path, 0o600) {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error);
    }

    let result = (|| {
        let metadata = std::fs::metadata(&library_path).map_err(|error| error.to_string())?;
        let (width, height, duration_ms, has_audio) = match kind {
            Kind::Screenshot => {
                let (width, height) =
                    image::image_dimensions(&library_path).map_err(|error| error.to_string())?;
                (width, height, None, false)
            }
            Kind::Gif | Kind::Video => {
                let probe = crate::media::toolchain()
                    .probe(&library_path)
                    .map_err(|error| error.to_string())?;
                (
                    probe.metadata.width,
                    probe.metadata.height,
                    probe.metadata.duration_ms,
                    probe.has_audio,
                )
            }
        };
        let poster_path = match kind {
            Kind::Screenshot | Kind::Gif => Some(library_path.clone()),
            Kind::Video => {
                let poster = directory.join("poster.jpg");
                let output = Command::new(crate::media::tool("ffmpeg"))
                    .args(["-v", "error", "-y", "-i"])
                    .arg(&library_path)
                    .args([
                        "-frames:v",
                        "1",
                        "-vf",
                        "scale=640:400:force_original_aspect_ratio=decrease",
                    ])
                    .arg(&poster)
                    .output();
                let poster = output
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|_| poster);
                if let Some(path) = &poster {
                    set_private_permissions(path, 0o600)?;
                }
                poster
            }
        };
        let new_entry = Entry {
            id,
            source_path: source_path.clone(),
            library_path: library_path.clone(),
            poster_path,
            created_ms: now_ms(),
            kind,
            width,
            height,
            size_bytes: metadata.len(),
            duration_ms,
            has_audio,
            dropped_frames: 0,
            source_private,
        };
        let _guard = INDEX_LOCK.lock().map_err(|error| error.to_string())?;
        let mut entries = read_index(root)?;
        let mut obsolete = prune(&mut entries, now_ms());
        obsolete.extend(
            entries
                .iter()
                .filter(|entry| entry.source_path == source_path)
                .cloned(),
        );
        entries.retain(|entry| entry.source_path != source_path);
        entries.push(new_entry);
        write_index(root, &entries)?;
        for entry in &obsolete {
            let _ = remove_private(root, entry);
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(directory);
    }
    result
}

fn permanently_delete(id: &str) -> Result<Vec<Entry>, String> {
    permanently_delete_at(&data_dir(), id)
}

fn permanently_delete_at(root: &Path, id: &str) -> Result<Vec<Entry>, String> {
    let _guard = INDEX_LOCK.lock().map_err(|error| error.to_string())?;
    let mut entries = read_index(root)?;
    let removed = entries.iter().find(|entry| entry.id == id).cloned();
    entries.retain(|entry| entry.id != id);
    write_index(root, &entries)?;
    if let Some(entry) = removed {
        remove_private(root, &entry)?;
    }
    Ok(entries)
}

fn permanently_clear() -> Result<(), String> {
    permanently_clear_at(&data_dir())
}

fn permanently_clear_at(root: &Path) -> Result<(), String> {
    let _guard = INDEX_LOCK.lock().map_err(|error| error.to_string())?;
    let entries = read_index(root)?;
    write_index(root, &[])?;
    for entry in &entries {
        remove_private(root, entry)?;
    }
    Ok(())
}

struct History {
    entries: Vec<Entry>,
    loading: bool,
    error: Option<String>,
    filter: Filter,
    page: usize,
    confirming: Option<String>,
    confirming_all: bool,
    status: Option<String>,
    image_cache: Entity<RetainAllImageCache>,
}

impl History {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            entries: Vec::new(),
            loading: true,
            error: None,
            filter: Filter::All,
            page: 0,
            confirming: None,
            confirming_all: false,
            status: None,
            image_cache: RetainAllImageCache::new(cx.borrow_mut()),
        }
    }
    fn set_filter(&mut self, filter: Filter, window: &mut Window, cx: &mut Context<Self>) {
        self.filter = filter;
        self.page = 0;
        self.confirming = None;
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        cx.notify();
    }
    fn set_page(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.confirming = None;
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        cx.notify();
    }
    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        if self.confirming.as_deref() != Some(&id) {
            let source_private = self
                .entries
                .iter()
                .find(|entry| entry.id == id)
                .is_some_and(|entry| entry.source_private);
            self.confirming = Some(id);
            self.status = Some(if source_private {
                "Select delete again to permanently remove this private unsaved capture. Any separately saved export is preserved."
            } else {
                "Select delete again to permanently remove the private history copy. Your saved file is preserved."
            }.into());
            cx.notify();
            return;
        }
        match permanently_delete(&id) {
            Ok(entries) => {
                self.entries = entries;
                self.confirming = None;
                self.status = Some("Removed from Capture History. Files outside Captures’ private unsaved directory were preserved.".into());
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn clear(&mut self, cx: &mut Context<Self>) {
        if !self.confirming_all {
            self.confirming_all = true;
            self.status = Some("Select “Delete all forever” to remove private history and unsaved captures. Files you explicitly saved stay untouched.".into());
            cx.notify();
            return;
        }
        match permanently_clear() {
            Ok(()) => {
                self.entries.clear();
                self.confirming_all = false;
                self.status =
                    Some("Capture History deleted. Explicitly saved files were preserved.".into());
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
    fn count(&self, filter: Filter) -> usize {
        self.entries
            .iter()
            .filter(|entry| filter.accepts(entry.kind))
            .count()
    }
    fn filter_button(
        &self,
        id: &'static str,
        label: &'static str,
        filter: Filter,
        colors: Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let selected = self.filter == filter;
        let count = self.count(filter);
        div()
            .id(id)
            .h(metric("--h-md"))
            .px(metric("--s-5"))
            .flex()
            .items_center()
            .gap(metric("--s-3"))
            .rounded(metric("--r-sm"))
            .border_1()
            .border_color(if selected {
                colors.accent
            } else {
                colors.border()
            })
            .bg(if selected {
                gpui::Hsla {
                    a: if colors.dark { 0.14 } else { 0.13 },
                    ..colors.accent
                }
            } else {
                colors.raised()
            })
            .cursor_pointer()
            .child(label)
            .child(div().text_color(colors.muted()).child(count.to_string()))
            .on_click(cx.listener(move |this, _, window, cx| this.set_filter(filter, window, cx)))
    }
    fn card(&self, entry: &Entry, colors: Theme, cx: &mut Context<Self>) -> gpui::Div {
        let id = entry.id.clone();
        let action_path = if entry.library_path.is_file() {
            entry.library_path.clone()
        } else {
            entry.source_path.clone()
        };
        let missing = !action_path.is_file();
        let preview = entry
            .poster_path
            .as_ref()
            .filter(|path| path.is_file())
            .cloned();
        let confirming = self.confirming.as_deref() == Some(&entry.id);
        let kind = match entry.kind {
            Kind::Screenshot => "Screenshot",
            Kind::Video => "Video",
            Kind::Gif => "GIF",
        };
        let meta = format!(
            "{} × {} · {}{}{}",
            entry.width,
            entry.height,
            format_size(entry.size_bytes),
            entry
                .duration_ms
                .map(|value| format!(" · {}", format_duration(value)))
                .unwrap_or_default(),
            if entry.has_audio { " · Audio" } else { "" }
        );
        let open_path = action_path.clone();
        let restore_path = action_path.clone();
        let reveal_path = entry.source_path.parent().map(Path::to_path_buf);
        let preview_panel = div()
            .relative()
            .h(px(150.))
            .bg(colors.color("--surface-sunken"))
            .flex()
            .items_center()
            .justify_center()
            .when_some(preview, |container, path| {
                container.child(img(path).size_full().object_fit(ObjectFit::Contain))
            })
            .when(missing, |container| {
                container.child(
                    div()
                        .absolute()
                        .px(metric("--s-4"))
                        .py(metric("--s-2"))
                        .rounded(metric("--r-sm"))
                        .bg(colors.glass())
                        .text_color(colors.glass_text())
                        .child("File missing"),
                )
            })
            .child(
                div()
                    .id(SharedString::from(format!("delete-{id}")))
                    .absolute()
                    .top(metric("--s-3"))
                    .right(metric("--s-3"))
                    .size(px(30.))
                    .rounded(px(15.))
                    .bg(if confirming {
                        parse_hex("#ef4650")
                    } else {
                        colors.glass()
                    })
                    .text_color(colors.glass_text())
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| this.remove(id.clone(), cx))),
            );
        let actions = div()
            .flex()
            .gap(metric("--s-3"))
            .child(
                button(
                    SharedString::from(format!("open-{}", entry.id)),
                    if entry.kind == Kind::Screenshot {
                        "Edit"
                    } else {
                        "Open"
                    },
                    colors,
                )
                .on_click(move |_, _, cx| {
                    if !missing {
                        crate::app::open(open_path.clone(), cx);
                    }
                }),
            )
            .when(entry.kind == Kind::Screenshot, |container| {
                container.child(
                    button(
                        SharedString::from(format!("restore-{}", entry.id)),
                        "Restore",
                        colors,
                    )
                    .on_click(move |_, _, cx| {
                        if restore_path.is_file() {
                            crate::app::restore_preview(restore_path.clone(), cx);
                        }
                    }),
                )
            })
            .when(
                entry.kind != Kind::Screenshot && reveal_path.is_some(),
                |container| {
                    container.child(
                        button(
                            SharedString::from(format!("reveal-{}", entry.id)),
                            "Show in Folder",
                            colors,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(path) = &reveal_path
                                && let Err(error) = open::that_detached(path)
                            {
                                this.error =
                                    Some(format!("Could not show capture folder: {error}"));
                                cx.notify();
                            }
                        })),
                    )
                },
            );
        let body = div()
            .p(metric("--s-5"))
            .flex()
            .flex_col()
            .gap(metric("--s-3"))
            .child(
                div()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(format!("{kind} · {}", format_date(entry.created_ms))),
            )
            .child(
                div()
                    .text_size(metric("--text-sm"))
                    .text_color(colors.muted())
                    .child(meta),
            )
            .when(entry.dropped_frames > 0, |container| {
                container.child(
                    div()
                        .text_size(metric("--text-sm"))
                        .text_color(parse_hex("#ef4650"))
                        .child(format!("{} frames dropped", entry.dropped_frames)),
                )
            })
            .child(actions);
        div()
            .min_w(px(210.))
            .min_h(px(280.))
            .flex()
            .flex_col()
            .rounded(metric("--r-md"))
            .overflow_hidden()
            .border_1()
            .border_color(colors.border())
            .bg(colors.raised())
            .child(preview_panel)
            .child(body)
    }
}

impl Render for History {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        let filtered: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| self.filter.accepts(entry.kind))
            .collect();
        let pages = filtered.len().div_ceil(PAGE_SIZE).max(1);
        let page = self.page.min(pages - 1);
        let visible = filtered.into_iter().skip(page * PAGE_SIZE).take(PAGE_SIZE);
        let header = div()
            .flex()
            .items_start()
            .justify_between()
            .gap(metric("--s-6"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(metric("--s-2"))
                    .child(div().text_size(metric("--text-xs")).font_weight(gpui::FontWeight::BOLD).text_color(colors.accent).child("ON THIS DEVICE"))
                    .child(div().text_size(px(28.)).font_weight(gpui::FontWeight::BOLD).child("Capture History"))
                    .child(div().max_w(px(650.)).text_color(colors.muted()).child("Screenshots, videos, and GIFs are kept as private recovery copies for 30 days.")),
            )
            .when(!self.entries.is_empty(), |container| {
                container.child(
                    button("clear-history", if self.confirming_all { "Delete all forever" } else { "Delete all" }, colors)
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                )
            })
            .child(button("new-capture-history", "New Capture", colors).on_click(|_, _, cx| crate::app::capture(cx)));
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .gap(metric("--s-3"))
                    .child(self.filter_button("filter-all", "All", Filter::All, colors, cx))
                    .child(self.filter_button(
                        "filter-shots",
                        "Screenshots",
                        Filter::Screenshot,
                        colors,
                        cx,
                    ))
                    .child(self.filter_button("filter-video", "Video", Filter::Video, colors, cx))
                    .child(self.filter_button("filter-gif", "GIF", Filter::Gif, colors, cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(metric("--s-3"))
                    .child(
                        button("previous-page", "Previous", colors).on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.set_page(page.saturating_sub(1), window, cx)
                            },
                        )),
                    )
                    .child(format!("{} / {}", page + 1, pages))
                    .child(button("next-page", "Next", colors).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.set_page((page + 1).min(pages - 1), window, cx)
                        },
                    ))),
            );
        let content =
            div()
                .max_w(px(1080.))
                .mx_auto()
                .flex()
                .flex_col()
                .gap(metric("--s-6"))
                .child(header)
                .when_some(self.status.clone(), |container, status| {
                    container.child(
                        div()
                            .p(metric("--s-4"))
                            .rounded(metric("--r-sm"))
                            .bg(colors.color("--surface-sunken"))
                            .child(status),
                    )
                })
                .when_some(self.error.clone(), |container, error| {
                    container.child(
                        div()
                            .p(metric("--s-4"))
                            .rounded(metric("--r-sm"))
                            .border_1()
                            .border_color(parse_hex("#ef4650"))
                            .child(error),
                    )
                })
                .when(!self.loading && !self.entries.is_empty(), |container| {
                    container.child(toolbar)
                })
                .when(self.loading, |container| {
                    container.child(
                        div()
                            .h(px(320.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(colors.muted())
                            .child("Loading Capture History…"),
                    )
                })
                .when(!self.loading && self.entries.is_empty(), |container| {
                    container.child(
                        div()
                            .h(px(320.))
                            .flex()
                            .flex_col()
                            .gap(metric("--s-4"))
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(22.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child("No captures yet"),
                            )
                            .child(div().text_color(colors.muted()).child(
                                "New screenshots, videos, and GIFs appear here automatically.",
                            )),
                    )
                })
                .when(!self.loading && !self.entries.is_empty(), |container| {
                    container.child(
                        div()
                            .grid()
                            .grid_cols(3)
                            .gap(metric("--s-5"))
                            .children(visible.map(|entry| self.card(entry, colors, cx))),
                    )
                });
        root(colors).child(
            image_cache(self.image_cache.clone()).child(
                div()
                    .id("history-scroll")
                    .size_full()
                    .overflow_x_hidden()
                    .overflow_y_scroll()
                    .p(metric("--s-6"))
                    .child(content),
            ),
        )
    }
}

fn format_size(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.)
    } else if bytes >= 1_000 {
        format!("{} KB", bytes / 1_000)
    } else {
        format!("{bytes} B")
    }
}
fn format_duration(ms: u64) -> String {
    let seconds = ms / 1_000;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
fn format_date(ms: u64) -> String {
    match now_ms().saturating_sub(ms) / (24 * 60 * 60 * 1_000) {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        days => format!("{days} days ago"),
    }
}
fn parse_hex(value: &str) -> gpui::Hsla {
    let rgb = u32::from_str_radix(value.trim_start_matches('#'), 16).unwrap_or(0xef4650);
    gpui::rgba((rgb << 8) | 0xff).into()
}

pub fn open(cx: &mut App) -> anyhow::Result<()> {
    let load = cx.background_spawn(async { load_and_cleanup() });
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1120.), px(720.)),
                cx,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some("Captures GPUI Capture History".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(760.), px(540.))),
            ..Default::default()
        },
        |_, cx| cx.new(History::new),
    )?;
    cx.spawn(async move |cx| {
        let result = load.await;
        let _ = window.update(cx, |view, _, cx| {
            view.loading = false;
            match result {
                Ok(entries) => view.entries = entries,
                Err(error) => view.error = Some(error),
            }
            cx.notify();
        });
    })
    .detach();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scratch(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "captures-gpui-history-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn entry(source: PathBuf, library: PathBuf, created_ms: u64) -> Entry {
        Entry {
            id: created_ms.to_string(),
            source_path: source,
            library_path: library.clone(),
            poster_path: Some(library),
            created_ms,
            kind: Kind::Screenshot,
            width: 3,
            height: 2,
            size_bytes: 10,
            duration_ms: None,
            has_audio: false,
            dropped_frames: 0,
            source_private: false,
        }
    }
    #[test]
    fn retention_boundary_deletes_only_expired_private_copy() {
        let dir = scratch("retention");
        let profile = dir.join("profile");
        let saved = dir.join("saved.png");
        let old_dir = profile.join("history/old");
        let current_dir = profile.join("history/current");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::create_dir_all(&current_dir).unwrap();
        std::fs::write(&saved, b"saved").unwrap();
        std::fs::write(old_dir.join("media.png"), b"old").unwrap();
        std::fs::write(current_dir.join("media.png"), b"current").unwrap();
        let current_ms = now_ms();
        write_index(
            &profile,
            &[
                entry(
                    saved.clone(),
                    old_dir.join("media.png"),
                    current_ms - RETENTION_MS - 1,
                ),
                entry(saved.clone(), current_dir.join("media.png"), current_ms),
            ],
        )
        .unwrap();

        let entries = load_and_cleanup_at(&profile).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].created_ms, current_ms);
        assert!(!old_dir.exists());
        assert!(current_dir.exists());
        assert_eq!(std::fs::read(saved).unwrap(), b"saved");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn add_records_an_independent_copy() {
        let dir = scratch("add-copy");
        let profile = dir.join("profile");
        let saved = dir.join("saved.png");
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(7, 3, image::Rgba([12, 34, 56, 255]))
            .save(&saved)
            .unwrap();

        add_at(&profile, &saved).unwrap();
        let entries = read_index(&profile).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].width, entries[0].height), (7, 3));
        assert_eq!(
            entries[0].size_bytes,
            std::fs::metadata(&saved).unwrap().len()
        );
        assert_ne!(entries[0].library_path, saved);
        assert_eq!(
            std::fs::read(&entries[0].library_path).unwrap(),
            std::fs::read(&saved).unwrap()
        );
        let private = entries[0].library_path.clone();

        let original = std::fs::read(&saved).unwrap();
        std::fs::write(&saved, b"changed external source").unwrap();
        assert_eq!(std::fs::read(private).unwrap(), original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_adds_do_not_lose_index_entries() {
        let dir = scratch("concurrent");
        let profile = dir.join("profile");
        let first = dir.join("first.png");
        let second = dir.join("second.png");
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(2, 3, image::Rgba([1, 2, 3, 255]))
            .save(&first)
            .unwrap();
        image::RgbaImage::from_pixel(5, 7, image::Rgba([4, 5, 6, 255]))
            .save(&second)
            .unwrap();

        std::thread::scope(|scope| {
            scope.spawn(|| add_at(&profile, &first).unwrap());
            scope.spawn(|| add_at(&profile, &second).unwrap());
        });
        let mut dimensions = read_index(&profile)
            .unwrap()
            .into_iter()
            .map(|entry| (entry.width, entry.height))
            .collect::<Vec<_>>();
        dimensions.sort_unstable();
        assert_eq!(dimensions, [(2, 3), (5, 7)]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_metadata_probe_does_not_leak_a_private_copy() {
        let dir = scratch("invalid");
        let profile = dir.join("profile");
        let invalid = dir.join("invalid.png");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&invalid, b"not an image").unwrap();
        assert!(add_at(&profile, &invalid).is_err());
        assert_eq!(
            std::fs::read_dir(profile.join("history")).unwrap().count(),
            0
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn confirmed_delete_removes_private_copy_but_preserves_saved_source() {
        let dir = scratch("delete");
        let profile = dir.join("profile");
        let saved = dir.join("saved.png");
        std::fs::create_dir_all(&dir).unwrap();
        image::RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]))
            .save(&saved)
            .unwrap();
        add_at(&profile, &saved).unwrap();
        let indexed = read_index(&profile).unwrap().pop().unwrap();
        let private_directory = indexed.library_path.parent().unwrap().to_path_buf();

        let entries = permanently_delete_at(&profile, &indexed.id).unwrap();
        assert!(entries.is_empty());
        assert!(!private_directory.exists());
        assert!(saved.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn confirmed_delete_removes_only_sources_inside_private_unsaved_directory() {
        let dir = scratch("delete-unsaved");
        let profile = dir.join("profile");
        let unsaved = profile.join("unsaved");
        let source = unsaved.join("private.png");
        std::fs::create_dir_all(&unsaved).unwrap();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([1, 2, 3, 255]))
            .save(&source)
            .unwrap();
        add_at(&profile, &source).unwrap();
        let indexed = read_index(&profile).unwrap().pop().unwrap();
        assert!(indexed.source_private);

        permanently_delete_at(&profile, &indexed.id).unwrap();
        assert!(!source.exists());
        assert!(!indexed.library_path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pure_retention_boundary_is_inclusive() {
        let mut entries = vec![
            entry(PathBuf::from("a"), PathBuf::from("b"), 5),
            entry(PathBuf::from("c"), PathBuf::from("d"), 6),
        ];
        let expired = prune(&mut entries, RETENTION_MS + 6);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].created_ms, 6);
        assert_eq!(expired.len(), 1);
    }
    #[test]
    fn formats_and_metadata_labels_cover_asymmetric_values() {
        assert_eq!(format_size(5_250_000), "5.2 MB");
        assert_eq!(format_duration(62_400), "1:02");
        assert!(Filter::Screenshot.accepts(Kind::Screenshot));
        assert!(!Filter::Screenshot.accepts(Kind::Video));
    }
}
