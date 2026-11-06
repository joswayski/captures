//! Capture History on GTK4, following shipping `CaptureHistory`
//! (`App.tsx`, `styles/windows.css`) and the shared card model in
//! `captures_app::history_view`.
//!
//! The page is one virtualized `GtkListView`: the header and filters are its
//! first rows, then one row per line of cards, so the whole page scrolls like
//! shipping's `.capture-history` while only the rows on screen exist. Rows
//! are recycled and rebound, never rebuilt per scroll. GtkListView keeps up to
//! 200 rows alive however few are on screen, so only rows within a screen of
//! the viewport hold cards (from a shared pool) and thumbnails. Thumbnails are
//! decoded off the UI thread at their displayed size from the History
//! preview, never the full-resolution capture, with a small cache for
//! scrolling back.
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

use captures_app::history_view::{self, Card, CardAction};
use captures_history::{ArtifactKind, HistoryEntry};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::tokens::Tokens;
use crate::widgets::{Icon, MaxWidth, Thumbnail, icon_button};

/// Shipping `.history-shell { max-width: 1180px }`.
const SHELL_MAX_WIDTH: i32 = 1180;
/// Decoded thumbnails kept for scrolling back; about two screens of cards.
const THUMBNAIL_CACHE: usize = 36;

/// What History asks the rest of the app to do.
pub struct Actions {
    pub open_preferences: Box<dyn Fn()>,
    pub edit: Box<dyn Fn(&HistoryEntry)>,
    pub restore: Box<dyn Fn(&HistoryEntry)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Screenshots,
    Video,
    Gif,
}

impl Filter {
    const ALL: [Self; 4] = [Self::All, Self::Screenshots, Self::Video, Self::Gif];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Screenshots => "Screenshots",
            Self::Video => "Video",
            Self::Gif => "GIF",
        }
    }

    fn matches(self, kind: ArtifactKind) -> bool {
        matches!(
            (self, kind),
            (Self::All, _)
                | (Self::Screenshots, ArtifactKind::Screenshot)
                | (Self::Video, ArtifactKind::Video)
                | (Self::Gif, ArtifactKind::Gif)
        )
    }
}

struct Item {
    entry: HistoryEntry,
    card: Card,
}

pub struct History {
    pub window: gtk::ApplicationWindow,
    state: Rc<State>,
}

struct State {
    root: PathBuf,
    tokens: Rc<Tokens>,
    actions: Actions,
    items: RefCell<Vec<Item>>,
    /// Indices into `items` that pass the filter, in display order.
    shown: RefCell<Vec<usize>>,
    loading: Cell<bool>,
    filter: Cell<Filter>,
    columns: Cell<usize>,
    compact: Cell<bool>,
    /// Width of one card column, for thumbnail decode size.
    column_width: Cell<f64>,
    error: RefCell<Option<String>>,
    /// The card awaiting its second Delete click, with its timeout.
    confirming: RefCell<Option<(String, glib::SourceId)>>,
    confirming_all: RefCell<Option<glib::SourceId>>,
    deleting_all: Cell<bool>,
    card_errors: RefCell<HashMap<String, String>>,
    model: gtk::StringList,
    header: Header,
    toolbar: Toolbar,
    thumbnails: Rc<Thumbnails>,
    /// Bound card views, so state changes restyle cards in place.
    cards: RefCell<Vec<Weak<CardView>>>,
    /// Every list row and the idle cards they share.
    slots: RefCell<Vec<Weak<Slot>>>,
    pool: RefCell<Vec<Rc<CardView>>>,
    list: RefCell<Option<gtk::ListView>>,
    visibility_queued: Cell<bool>,
    /// Frames spent waiting for rows the list has not laid out yet.
    layout_waits: Cell<u32>,
}

pub fn build(
    app: &gtk::Application,
    tokens: Rc<Tokens>,
    root: PathBuf,
    actions: Actions,
) -> History {
    let spec = captures_app::app_windows::HISTORY;
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(spec.title)
        .default_width(spec.width as i32)
        .default_height(spec.height as i32)
        .build();
    window.add_css_class("captures");
    window.set_size_request(spec.min_width as i32, spec.min_height as i32);

    let model = gtk::StringList::new(&[]);
    let state = Rc::new(State {
        root,
        tokens: tokens.clone(),
        actions,
        items: RefCell::default(),
        shown: RefCell::default(),
        loading: Cell::new(true),
        filter: Cell::default(),
        columns: Cell::new(1),
        compact: Cell::new(false),
        column_width: Cell::new(400.),
        error: RefCell::default(),
        confirming: RefCell::default(),
        confirming_all: RefCell::default(),
        deleting_all: Cell::new(false),
        card_errors: RefCell::default(),
        model: model.clone(),
        header: Header::new(),
        toolbar: Toolbar::new(),
        thumbnails: Rc::new(Thumbnails::default()),
        cards: RefCell::default(),
        slots: RefCell::default(),
        pool: RefCell::default(),
        list: RefCell::default(),
        visibility_queued: Cell::new(false),
        layout_waits: Cell::new(0),
    });
    state.connect_chrome();

    let factory = gtk::SignalListItemFactory::new();
    let weak = Rc::downgrade(&state);
    factory.connect_setup(move |_, item| {
        let Some(state) = weak.upgrade() else { return };
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        item.set_activatable(false);
        item.set_selectable(false);
        let slot = Rc::new(Slot::new());
        item.set_child(Some(&slot.outer));
        state.slots.borrow_mut().push(Rc::downgrade(&slot));
        let weak_state = Rc::downgrade(&state);
        item.connect_notify_local(Some("item"), move |item, _| {
            let Some(state) = weak_state.upgrade() else {
                return;
            };
            let key = item
                .item()
                .and_downcast::<gtk::StringObject>()
                .map(|object| object.string().to_string());
            slot.bind(&state, key.as_deref());
        });
    });
    let selection = gtk::NoSelection::new(Some(model));
    let list = gtk::ListView::new(Some(selection), Some(factory));
    list.add_css_class("history-list");
    list.update_property(&[gtk::accessible::Property::Label(
        history_view::copy().grid_label,
    )]);
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&list)
        .build();
    scroller.add_css_class("history-page");
    *state.list.borrow_mut() = Some(list.clone());
    let weak = Rc::downgrade(&state);
    scroller.vadjustment().connect_value_changed(move |_| {
        if let Some(state) = weak.upgrade() {
            state.rows_moved();
        }
    });
    // Columns follow the shell width, like `repeat(auto-fill, minmax(252px, 1fr))`.
    let weak = Rc::downgrade(&state);
    let watch = crate::widgets::WidthWatch::new(&scroller, move |width| {
        let Some(state) = weak.upgrade() else { return };
        let padding = 2. * f64::from(state.tokens.number("s-8"));
        let shell = (f64::from(width) - padding).min(f64::from(SHELL_MAX_WIDTH));
        let compact = width <= 720;
        let grid = history_view::grid_in_window(shell, compact);
        let columns = grid.columns;
        state.column_width.set(grid.card_width);
        if state.compact.replace(compact) != compact {
            let state = state.clone();
            glib::idle_add_local_once(move || {
                let gap = state.tokens.number("s-5") as i32;
                state.header.set_compact(compact, gap);
            });
        }
        if columns != state.columns.get() {
            state.columns.set(columns);
            // Never change the model during allocation.
            let state = state.clone();
            glib::idle_add_local_once(move || state.rebuild());
        }
    });
    window.set_child(Some(&watch));

    state.rebuild();
    state.reload();
    History { window, state }
}

impl History {
    /// Load History again from disk (after a capture or an edit).
    pub fn reload(&self) {
        self.state.reload();
    }

    /// Scroll to the top or the end, for the profile exercise.
    pub fn scroll_to_end(&self, end: bool) {
        let Some(scroller) = self
            .window
            .child()
            .and_then(|watch| watch.first_child())
            .and_downcast::<gtk::ScrolledWindow>()
        else {
            return;
        };
        let adjustment = scroller.vadjustment();
        adjustment.set_value(if end { adjustment.upper() } else { 0. });
    }
}

impl State {
    fn reload(self: &Rc<Self>) {
        let root = self.root.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let loaded = gio::spawn_blocking(move || {
                captures_history::load(&root, chrono::Utc::now()).map(|entries| {
                    entries
                        .into_iter()
                        .map(|entry| {
                            let missing = history_view::media_missing(&root, &entry);
                            (entry, missing)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .await;
            let Some(state) = weak.upgrade() else { return };
            state.loading.set(false);
            match loaded {
                Ok(Ok(entries)) => {
                    *state.items.borrow_mut() = entries
                        .into_iter()
                        .map(|(entry, missing)| Item {
                            card: history_view::card(&entry, missing),
                            entry,
                        })
                        .collect();
                    *state.error.borrow_mut() = None;
                }
                Ok(Err(error)) => {
                    *state.error.borrow_mut() = Some(history_view::load_error(&error.to_string()));
                }
                Err(_) => {
                    *state.error.borrow_mut() =
                        Some(history_view::load_error("the loader stopped"));
                }
            }
            state.rebuild();
        });
    }

    /// Recompute the rows; the list rebinds only the rows on screen.
    fn rebuild(&self) {
        let items = self.items.borrow();
        let filter = self.filter.get();
        let shown: Vec<usize> = (0..items.len())
            .filter(|&index| filter.matches(items[index].entry.kind))
            .collect();
        let mut keys: Vec<String> = vec!["header".into()];
        if !self.loading.get() && !items.is_empty() {
            keys.push("toolbar".into());
        }
        if self.error.borrow().is_some() {
            keys.push("error".into());
        }
        if self.loading.get() {
            keys.push("loading".into());
        } else if items.is_empty() {
            keys.push("empty".into());
        } else if shown.is_empty() {
            keys.push("filtered-empty".into());
        } else {
            let rows = shown.len().div_ceil(self.columns.get());
            keys.extend((0..rows).map(|row| format!("row:{row}")));
        }
        drop(items);
        *self.shown.borrow_mut() = shown;
        self.update_chrome();
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        self.model.splice(0, self.model.n_items(), &refs);
    }

    fn connect_chrome(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.header.back.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                (state.actions.open_preferences)();
            }
        });
        let weak = Rc::downgrade(self);
        self.header.delete_all.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.delete_all();
            }
        });
        let weak = Rc::downgrade(self);
        self.header.cancel.connect_clicked(move |_| {
            let Some(state) = weak.upgrade() else { return };
            if let Some(timer) = state.confirming_all.borrow_mut().take() {
                timer.remove();
            }
            state.update_chrome();
        });
        for (filter, button) in Filter::ALL.into_iter().zip(&self.toolbar.buttons) {
            let weak = Rc::downgrade(self);
            button.0.connect_clicked(move |_| {
                let Some(state) = weak.upgrade() else { return };
                if state.filter.replace(filter) != filter {
                    state.rebuild();
                }
            });
        }
    }

    /// Header and filter state that changes without new rows.
    fn update_chrome(&self) {
        let copy = history_view::copy();
        let items = self.items.borrow();
        let confirming = self.confirming_all.borrow().is_some();
        let busy = self.deleting_all.get();
        let header = &self.header;
        header
            .actions
            .set_visible(!self.loading.get() && !items.is_empty());
        header.cancel.set_visible(confirming);
        header.cancel.set_sensitive(!busy);
        header.delete_all.set_sensitive(!busy);
        header.delete_all_label.set_label(if busy {
            copy.delete_all_busy
        } else if confirming {
            copy.delete_all_confirm
        } else {
            copy.delete_all
        });
        set_class(&header.delete_all, "confirm", confirming);
        header
            .delete_all
            .update_property(&[gtk::accessible::Property::Label(if confirming {
                copy.delete_all_confirm_label
            } else {
                copy.delete_all_label
            })]);
        let filter = self.filter.get();
        for (option, (button, count)) in Filter::ALL.into_iter().zip(&self.toolbar.buttons) {
            let n = items
                .iter()
                .filter(|i| option.matches(i.entry.kind))
                .count();
            count.set_label(&n.to_string());
            button.set_sensitive(option == Filter::All || n > 0);
            set_class(button, "active", option == filter);
            button.update_state(&[gtk::accessible::State::Pressed(if option == filter {
                gtk::AccessibleTristate::True
            } else {
                gtk::AccessibleTristate::False
            })]);
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} {n}",
                option.label()
            ))]);
        }
        if let Some(error) = self.error.borrow().as_deref() {
            self.header.error.set_label(error);
        }
    }

    fn entry(&self, id: &str) -> Option<HistoryEntry> {
        self.items
            .borrow()
            .iter()
            .find(|item| item.entry.id == id)
            .map(|item| item.entry.clone())
    }

    /// Rows were rebound or scrolled: re-place cards, waiting a few frames
    /// again for rows the list has yet to lay out. (Rows far off screen are
    /// never laid out, so the wait is bounded rather than until all are.)
    fn rows_moved(self: &Rc<Self>) {
        self.layout_waits.set(0);
        self.queue_visibility();
    }

    /// Give cards to rows within a screen of the viewport and take them
    /// from the rest, once per frame at most.
    fn queue_visibility(self: &Rc<Self>) {
        if self.visibility_queued.replace(true) {
            return;
        }
        let Some(list) = self.list.borrow().clone() else {
            return;
        };
        // On the next frame, before its layout: rows have this frame's layout.
        let weak = Rc::downgrade(self);
        list.add_tick_callback(move |_, _| {
            if let Some(state) = weak.upgrade() {
                state.visibility_queued.set(false);
                state.update_visibility();
            }
            glib::ControlFlow::Break
        });
    }

    fn update_visibility(self: &Rc<Self>) {
        let Some(list) = self.list.borrow().clone() else {
            return;
        };
        let Some(adjustment) = list.vadjustment() else {
            return;
        };
        // `compute_bounds` against the list is in viewport coordinates.
        let page = adjustment.page_size() as f32;
        let (low, high) = (-page, 2. * page);
        let slots: Vec<Rc<Slot>> = self
            .slots
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        let mut unlaid = false;
        let (near, far): (Vec<_>, Vec<_>) = slots
            .into_iter()
            .filter(|slot| slot.row.get().is_some())
            .partition(|slot| {
                if slot.outer.parent().is_none() {
                    return false;
                }
                match slot.outer.compute_bounds(&list) {
                    // Rows the list has not laid out yet have no height.
                    Some(b) if b.height() > 0. => b.y() + b.height() >= low && b.y() <= high,
                    _ => {
                        unlaid = true;
                        false
                    }
                }
            });
        // Release first so near rows reuse those cards.
        let mut changed = false;
        for slot in far {
            changed |= slot.depopulate(self);
        }
        for slot in near {
            changed |= slot.populate(self);
        }
        // New cards move rows; look again once the list has laid them out,
        // and wait a few frames at most for rows it has not laid out yet.
        let waits = if unlaid {
            self.layout_waits.get() + 1
        } else {
            0
        };
        self.layout_waits.set(waits);
        if changed || (unlaid && waits < 10) {
            self.queue_visibility();
        }
    }

    fn restyle_cards(&self) {
        self.cards.borrow_mut().retain(|card| {
            let Some(card) = card.upgrade() else {
                return false;
            };
            card.restyle(self);
            true
        });
    }

    /// Shipping's two-step Delete: arm for four seconds, delete on the
    /// second click. Missing recordings are removed at once.
    fn delete(self: &Rc<Self>, id: &str) {
        let Some(confirm) = self
            .items
            .borrow()
            .iter()
            .find(|item| item.entry.id == id)
            .map(|item| item.card.delete_requires_confirmation)
        else {
            return;
        };
        let armed = self
            .confirming
            .borrow()
            .as_ref()
            .is_some_and(|(armed, _)| armed == id);
        if confirm && !armed {
            if let Some((_, timer)) = self.confirming.borrow_mut().take() {
                timer.remove();
            }
            let weak = Rc::downgrade(self);
            let timer = glib::timeout_add_local_once(
                std::time::Duration::from_millis(history_view::CONFIRM_TIMEOUT_MS),
                move || {
                    if let Some(state) = weak.upgrade() {
                        state.confirming.borrow_mut().take();
                        state.restyle_cards();
                    }
                },
            );
            *self.confirming.borrow_mut() = Some((id.to_owned(), timer));
            self.restyle_cards();
            return;
        }
        if let Some((_, timer)) = self.confirming.borrow_mut().take() {
            timer.remove();
        }
        let (root, id) = (self.root.clone(), id.to_owned());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let target = id.clone();
            let result = gio::spawn_blocking(move || captures_history::delete(&root, &target))
                .await
                .unwrap_or(Err(captures_history::Error::HistoryUnavailable));
            let Some(state) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    state.items.borrow_mut().retain(|item| item.entry.id != id);
                    state.thumbnails.forget(&id);
                    state.rebuild();
                }
                Err(error) => {
                    state.card_errors.borrow_mut().insert(id, error.to_string());
                    state.restyle_cards();
                }
            }
        });
    }

    fn delete_all(self: &Rc<Self>) {
        if self.deleting_all.get() {
            return;
        }
        if self.confirming_all.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let timer = glib::timeout_add_local_once(
                std::time::Duration::from_millis(history_view::CONFIRM_TIMEOUT_MS),
                move || {
                    if let Some(state) = weak.upgrade() {
                        state.confirming_all.borrow_mut().take();
                        state.update_chrome();
                    }
                },
            );
            *self.confirming_all.borrow_mut() = Some(timer);
            self.update_chrome();
            return;
        }
        if let Some(timer) = self.confirming_all.borrow_mut().take() {
            timer.remove();
        }
        self.deleting_all.set(true);
        self.update_chrome();
        let root = self.root.clone();
        let ids: Vec<String> = self
            .items
            .borrow()
            .iter()
            .map(|item| item.entry.id.clone())
            .collect();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                ids.iter()
                    .try_for_each(|id| captures_history::delete(&root, id))
                    .map_err(|error| error.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("the delete worker stopped".into()));
            let Some(state) = weak.upgrade() else { return };
            state.deleting_all.set(false);
            if let Err(error) = result {
                *state.error.borrow_mut() = Some(history_view::clear_error(&error));
            }
            state.thumbnails.clear();
            state.reload();
        });
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

fn label(text: &str, class: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(class);
    label.set_xalign(0.);
    label
}

/// `.history-header`: Back to Preferences, title and lede, Delete all.
struct Header {
    root: gtk::Box,
    /// Title copy beside the actions; a column in a compact window.
    row: gtk::Box,
    back: gtk::Button,
    actions: gtk::Box,
    cancel: gtk::Button,
    delete_all: gtk::Button,
    delete_all_label: gtk::Label,
    error: gtk::Label,
}

impl Header {
    fn new() -> Header {
        let copy = history_view::copy();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("history-header-row");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        let heading = gtk::Box::new(gtk::Orientation::Vertical, 0);
        heading.set_hexpand(true);
        let (back, _, _) = icon_button(
            "chevron-left",
            copy.back_to_preferences,
            &["ui-btn", "ghost"],
        );
        back.set_halign(gtk::Align::Start);
        heading.append(&back);
        let title = label(copy.title, "history-title");
        title.set_accessible_role(gtk::AccessibleRole::Heading);
        heading.append(&title);
        let lede = label(copy.lede, "history-lede");
        lede.set_wrap(true);
        lede.set_max_width_chars(60);
        lede.set_width_chars(1);
        // `max-width: 60ch`: the label's 60-char natural width caps it.
        heading.append(&MaxWidth::new(&lede, MaxWidth::NATURAL));
        row.append(&heading);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_valign(gtk::Align::End);
        let cancel = gtk::Button::with_label(copy.cancel);
        cancel.add_css_class("ui-btn");
        cancel.add_css_class("ghost");
        cancel.update_property(&[gtk::accessible::Property::Label(copy.cancel_label)]);
        let (delete_all, _, delete_all_label) =
            icon_button("trash", copy.delete_all, &["ui-btn", "danger"]);
        actions.append(&cancel);
        actions.append(&delete_all);
        row.append(&actions);
        root.append(&row);
        let error = label("", "history-error");
        error.set_wrap(true);
        error.set_accessible_role(gtk::AccessibleRole::Alert);
        Header {
            root,
            row,
            back,
            actions,
            cancel,
            delete_all,
            delete_all_label,
            error,
        }
    }
}

impl Header {
    /// `@media (max-width: 720px) .history-header`: a column, actions at
    /// the start, `gap: var(--s-5)`.
    fn set_compact(&self, compact: bool, gap: i32) {
        self.row.set_orientation(if compact {
            gtk::Orientation::Vertical
        } else {
            gtk::Orientation::Horizontal
        });
        self.row.set_spacing(if compact { gap } else { 24 });
        self.actions.set_halign(if compact {
            gtk::Align::Start
        } else {
            gtk::Align::Fill
        });
    }
}

/// `.history-toolbar` filter pills with counts.
struct Toolbar {
    root: gtk::Box,
    buttons: Vec<(gtk::Button, gtk::Label)>,
}

impl Toolbar {
    fn new() -> Toolbar {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        root.add_css_class("history-toolbar");
        root.update_property(&[gtk::accessible::Property::Label(
            history_view::copy().filter_group_label,
        )]);
        let buttons = Filter::ALL
            .into_iter()
            .map(|filter| {
                let button = gtk::Button::new();
                button.add_css_class("history-filter");
                let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                content.append(&gtk::Label::new(Some(filter.label())));
                let count = gtk::Label::new(Some("0"));
                count.add_css_class("count");
                content.append(&count);
                button.set_child(Some(&content));
                root.append(&button);
                (button, count)
            })
            .collect();
        Toolbar { root, buttons }
    }
}

/// One recycled list row: the shared header/toolbar, a message, or a line
/// of cards. A card line holds cards only while it is within a screen of the
/// viewport; farther rows keep their height and hand their cards back to a
/// shared pool, so widget and texture counts follow the viewport, not the
/// list's retained rows.
struct Slot {
    outer: MaxWidth,
    inner: gtk::Box,
    row: Cell<Option<usize>>,
    cards: RefCell<Vec<Rc<CardView>>>,
    spacers: RefCell<Vec<gtk::Box>>,
}

impl Slot {
    fn new() -> Slot {
        let inner = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        inner.set_homogeneous(true);
        Slot {
            outer: MaxWidth::centered(&inner, SHELL_MAX_WIDTH),
            inner,
            row: Cell::new(None),
            cards: RefCell::default(),
            spacers: RefCell::default(),
        }
    }

    fn clear(&self, state: &State) {
        self.depopulate(state);
        self.row.set(None);
        while let Some(child) = self.inner.first_child() {
            self.inner.remove(&child);
        }
        for class in ["history-grid-row", "last"] {
            self.inner.remove_css_class(class);
        }
        self.inner.set_spacing(0);
        self.inner.set_size_request(-1, -1);
    }

    fn bind(&self, state: &Rc<State>, key: Option<&str>) {
        self.clear(state);
        let Some(key) = key else { return };
        match key {
            "header" => reparent(&state.header.root, &self.inner),
            "toolbar" => reparent(&state.toolbar.root, &self.inner),
            "error" => reparent(&state.header.error, &self.inner),
            "loading" | "empty" | "filtered-empty" => {
                self.inner.append(&empty_state(key));
            }
            row => {
                let row: usize = row
                    .strip_prefix("row:")
                    .and_then(|n| n.parse().ok())
                    .expect("row key");
                self.row.set(Some(row));
                self.inner.add_css_class("history-grid-row");
                if (row + 1) * state.columns.get() >= state.shown.borrow().len() {
                    self.inner.add_css_class("last");
                }
                self.inner
                    .set_spacing(history_view::GRID_GAP.round() as i32);
                // An empty row keeps a card's height until it is populated.
                self.inner
                    .set_size_request(-1, history_view::CARD_HEIGHT.round() as i32);
                state.rows_moved();
            }
        }
    }

    /// Attach cards for this row's entries, from the shared pool. Returns
    /// whether anything changed.
    fn populate(&self, state: &Rc<State>) -> bool {
        let Some(row) = self.row.get() else {
            return false;
        };
        if !self.cards.borrow().is_empty() {
            return false;
        }
        let columns = state.columns.get();
        let shown = state.shown.borrow();
        let start = (row * columns).min(shown.len());
        let end = (start + columns).min(shown.len());
        let items = state.items.borrow();
        let mut cards = self.cards.borrow_mut();
        for &index in &shown[start..end] {
            let card = state.pool.borrow_mut().pop().unwrap_or_else(|| {
                let card = CardView::new(state);
                state.cards.borrow_mut().push(Rc::downgrade(&card));
                card
            });
            card.bind(state, &items[index]);
            card.show_thumbnail(state);
            self.inner.append(&card.root);
            cards.push(card);
        }
        let mut spacers = self.spacers.borrow_mut();
        for slot in 0..columns - (end - start) {
            if spacers.len() <= slot {
                spacers.push(gtk::Box::new(gtk::Orientation::Vertical, 0));
            }
            // A spacer may still be here from an earlier pass.
            reparent(&spacers[slot], &self.inner);
        }
        true
    }

    /// Return this row's cards (and their thumbnails) to the pool. Returns
    /// whether anything changed.
    fn depopulate(&self, state: &State) -> bool {
        let changed = !self.cards.borrow().is_empty();
        for card in self.cards.borrow_mut().drain(..) {
            self.inner.remove(&card.root);
            card.unbind(state);
            state.pool.borrow_mut().push(card);
        }
        for spacer in self.spacers.borrow().iter() {
            if spacer.parent().is_some() {
                self.inner.remove(spacer);
            }
        }
        changed
    }
}

fn reparent(widget: &impl IsA<gtk::Widget>, parent: &gtk::Box) {
    if let Some(old) = widget.parent().and_downcast::<gtk::Box>() {
        old.remove(widget);
    }
    parent.append(widget);
}

/// `.history-empty`: icon, title and body.
fn empty_state(kind: &str) -> gtk::Box {
    let copy = history_view::copy();
    let (title, body) = match kind {
        "loading" => (copy.loading, None),
        "empty" => (copy.empty_title, Some(copy.empty_body)),
        _ => (
            "No captures in this filter",
            Some("Choose All to see your other captures."),
        ),
    };
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("history-empty");
    root.set_hexpand(true);
    root.set_valign(gtk::Align::Center);
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.set_halign(gtk::Align::Center);
    inner.set_valign(gtk::Align::Center);
    inner.set_vexpand(true);
    let icon_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    icon_box.add_css_class("history-empty-icon");
    icon_box.set_halign(gtk::Align::Center);
    let icon = Icon::new("history", 26).with_stroke(1.6);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_vexpand(true);
    icon_box.append(&icon);
    inner.append(&icon_box);
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("history-empty-title");
    heading.set_accessible_role(gtk::AccessibleRole::Heading);
    inner.append(&heading);
    if let Some(body) = body {
        let body = gtk::Label::new(Some(body));
        body.add_css_class("history-empty-body");
        inner.append(&body);
    }
    root.append(&inner);
    root
}

type CardCommand<'a> = dyn Fn(&Rc<State>, &Rc<CardView>, HistoryEntry) + 'a;

/// One `.history-card`, rebound to whichever entry its row shows.
struct CardView {
    id: RefCell<String>,
    root: gtk::Box,
    wrap: gtk::Overlay,
    open: gtk::Button,
    thumbnail: Thumbnail,
    delete: gtk::Button,
    missing: gtk::Label,
    date: gtk::Label,
    details: gtk::Label,
    warning: gtk::Label,
    actions: [(gtk::Button, Icon, gtk::Label); 2],
    action_kinds: RefCell<Vec<CardAction>>,
    error: gtk::Label,
    /// A thumbnail request is in flight.
    loading: Cell<bool>,
}

impl CardView {
    fn new(state: &Rc<State>) -> Rc<CardView> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("history-card");
        root.set_overflow(gtk::Overflow::Hidden);
        root.set_hexpand(true);

        let wrap = glib::Object::builder::<gtk::Overlay>()
            .property("accessible-role", gtk::AccessibleRole::Group)
            .build();
        wrap.add_css_class("history-image-wrap");
        let open = gtk::Button::new();
        open.add_css_class("history-image-open");
        let thumbnail = Thumbnail::default();
        thumbnail.set_hexpand(true);
        thumbnail.set_vexpand(true);
        open.set_child(Some(&thumbnail));
        open.set_tooltip_text(Some("Drag this capture into an editor or another app"));
        wrap.set_child(Some(&open));
        let missing = gtk::Label::new(Some(history_view::copy().missing));
        missing.add_css_class("history-missing-label");
        missing.set_halign(gtk::Align::Center);
        missing.set_valign(gtk::Align::Center);
        missing.set_can_target(false);
        wrap.add_overlay(&missing);
        let delete = gtk::Button::new();
        delete.add_css_class("history-delete");
        let trash = Icon::new("trash", 16).with_stroke(1.7);
        delete.set_child(Some(&trash));
        delete.set_halign(gtk::Align::End);
        delete.set_valign(gtk::Align::Start);
        wrap.add_overlay(&delete);
        root.append(&wrap);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
        body.add_css_class("history-card-body");
        let date = label("", "history-date");
        date.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let details = label("", "history-details");
        let warning = label("", "history-warning");
        let actions_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions_box.add_css_class("history-actions");
        actions_box.set_homogeneous(true);
        let actions = [
            icon_button("edit", "Edit", &["ui-btn", "primary"]),
            icon_button("restore", "Restore", &["ui-btn"]),
        ];
        for (button, _, text) in &actions {
            text.set_ellipsize(gtk::pango::EllipsizeMode::End);
            actions_box.append(button);
        }
        let error = label("", "history-card-error");
        error.set_wrap(true);
        error.set_accessible_role(gtk::AccessibleRole::Alert);
        body.append(&date);
        body.append(&details);
        body.append(&warning);
        body.append(&actions_box);
        body.append(&error);
        root.append(&body);

        let card = Rc::new(CardView {
            id: RefCell::default(),
            root,
            wrap,
            open,
            thumbnail,
            delete,
            missing,
            date,
            details,
            warning,
            actions,
            action_kinds: RefCell::default(),
            error,
            loading: Cell::new(false),
        });
        card.connect(state);
        card
    }

    fn connect(self: &Rc<Self>, state: &Rc<State>) {
        let (weak_card, weak_state) = (Rc::downgrade(self), Rc::downgrade(state));
        let with = move |f: &CardCommand<'_>| {
            let (Some(state), Some(card)) = (weak_state.upgrade(), weak_card.upgrade()) else {
                return;
            };
            let id = card.id.borrow().clone();
            if let Some(entry) = state.entry(&id) {
                f(&state, &card, entry);
            }
        };
        let with = Rc::new(with);
        for slot in 0..2 {
            let with = with.clone();
            self.actions[slot].0.connect_clicked(move |_| {
                with(&|state, card, entry| {
                    let action = card.action_kinds.borrow().get(slot).copied();
                    if let Some(action) = action {
                        run_action(state, card, &entry, action);
                    }
                });
            });
        }
        let open = with.clone();
        self.open.connect_clicked(move |_| {
            open(&|state, card, entry| {
                if !card.action_kinds.borrow().is_empty() {
                    run_action(state, card, &entry, CardAction::Edit);
                }
            });
        });
        let delete = with.clone();
        self.delete.connect_clicked(move |_| {
            delete(&|state, _, entry| state.delete(&entry.id));
        });
        // Secondary click: the card's shipping command menu.
        let click = gtk::GestureClick::new();
        click.set_button(gdk::BUTTON_SECONDARY);
        let menu = with.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            menu(&|state, card, entry| card.show_menu(state, &entry, x, y));
        });
        self.root.add_controller(click);
        // Drag the capture's file out, like shipping's OS file drag.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY);
        let source = with.clone();
        drag.connect_prepare(move |_, _, _| {
            let file = RefCell::new(None);
            source(&|state, card, entry| {
                if card.action_kinds.borrow().is_empty() {
                    return;
                }
                let path = media_path(&state.root, &entry);
                *file.borrow_mut() = Some(gio::File::for_path(path));
            });
            let uri = file.into_inner()?.uri();
            Some(gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from_owned(format!("{uri}\r\n").into_bytes()),
            ))
        });
        self.open.add_controller(drag);
    }

    fn bind(self: &Rc<Self>, state: &Rc<State>, item: &Item) {
        let card = &item.card;
        let previous = self.id.replace(item.entry.id.clone());
        if previous != item.entry.id {
            state.thumbnails.release(&previous);
            self.thumbnail.set_texture(None);
            self.loading.set(false);
        }
        self.date.set_label(&card.date);
        self.details.set_label(&card.details);
        self.warning.set_visible(card.warning.is_some());
        self.warning
            .set_label(card.warning.as_deref().unwrap_or_default());
        self.missing.set_visible(card.missing);
        set_class(&self.wrap, "history-image-missing", card.missing);
        self.thumbnail
            .update_property(&[gtk::accessible::Property::Label(card.image_label)]);
        self.open.set_sensitive(card.open_label.is_some());
        self.open
            .update_property(&[gtk::accessible::Property::Label(
                card.open_label.unwrap_or(card.image_label),
            )]);
        *self.action_kinds.borrow_mut() = card.actions.clone();
        let actions_box = self.actions[0].0.parent().expect("actions box");
        actions_box.set_visible(!card.actions.is_empty());
        for (slot, (button, icon, text)) in self.actions.iter().enumerate() {
            let Some(action) = card.actions.get(slot) else {
                continue;
            };
            text.set_label(action.label());
            icon.set_name(action.icon().unwrap_or("folder"));
            button.set_tooltip_text(action.tooltip());
            button.update_property(&[gtk::accessible::Property::Label(action.label())]);
            set_class(button, "primary", slot == 0);
        }
        self.restyle(state);
    }

    fn unbind(&self, state: &State) {
        let id = self.id.take();
        state.thumbnails.release(&id);
        self.thumbnail.set_texture(None);
        self.loading.set(false);
    }

    /// Load this card's thumbnail (it is near the viewport).
    fn show_thumbnail(self: &Rc<Self>, state: &Rc<State>) {
        if self.thumbnail.has_texture() || self.loading.get() {
            return;
        }
        self.loading.set(true);
        let weak = Rc::downgrade(self);
        let id = self.id.borrow().clone();
        let scale = self.root.scale_factor().max(1);
        let width = state.column_width.get().ceil() as i32;
        let size = (width.max(1) * scale, 168 * scale);
        let key = id.clone();
        state
            .thumbnails
            .request(&state.root, &key, size, move |texture| {
                if let Some(card) = weak.upgrade()
                    && *card.id.borrow() == id
                {
                    card.loading.set(false);
                    card.thumbnail.set_texture(Some(texture));
                }
            });
    }

    /// Delete confirmation and inline errors, which change without rebinding.
    fn restyle(&self, state: &State) {
        let id = self.id.borrow();
        let Some(card) = state
            .items
            .borrow()
            .iter()
            .find(|item| item.entry.id == *id)
            .map(|item| item.card.clone())
        else {
            return;
        };
        let confirming = state
            .confirming
            .borrow()
            .as_ref()
            .is_some_and(|(armed, _)| *armed == *id);
        set_class(&self.delete, "confirm", confirming);
        let (label, title) = if confirming {
            (card.delete_confirm_label, card.delete_confirm_title)
        } else {
            (card.delete_label, card.delete_label)
        };
        self.delete
            .update_property(&[gtk::accessible::Property::Label(label)]);
        self.delete.set_tooltip_text(Some(title));
        let error = state.card_errors.borrow().get(&*id).cloned();
        self.error.set_visible(error.is_some());
        self.error.set_label(error.as_deref().unwrap_or_default());
    }

    fn show_menu(self: &Rc<Self>, state: &Rc<State>, entry: &HistoryEntry, x: f64, y: f64) {
        let card = state
            .items
            .borrow()
            .iter()
            .find(|item| item.entry.id == entry.id)
            .map(|item| item.card.clone());
        let Some(card) = card else { return };
        if card.menu.is_empty() {
            return;
        }
        let menu = gtk::Popover::new();
        menu.add_css_class("captures-menu");
        menu.set_has_arrow(false);
        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        menu.set_child(Some(&menu_box));
        menu.set_parent(&self.root);
        // A fresh popover per menu, removed once it closes.
        menu.connect_closed(|menu| {
            let menu = menu.clone();
            glib::idle_add_local_once(move || menu.unparent());
        });
        for action in card.menu {
            let button = gtk::Button::with_label(action.label());
            button.add_css_class("menu-item");
            if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                label.set_xalign(0.);
            }
            let (weak_state, weak_card) = (Rc::downgrade(state), Rc::downgrade(self));
            let entry = entry.clone();
            let popover = menu.downgrade();
            button.connect_clicked(move |_| {
                if let Some(menu) = popover.upgrade() {
                    menu.popdown();
                }
                let (Some(state), Some(card)) = (weak_state.upgrade(), weak_card.upgrade()) else {
                    return;
                };
                run_action(&state, &card, &entry, action);
            });
            menu_box.append(&button);
        }
        menu.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        menu.popup();
    }

    fn fail(&self, state: &State, error: String) {
        state
            .card_errors
            .borrow_mut()
            .insert(self.id.borrow().clone(), error);
        self.restyle(state);
    }
}

pub fn media_path(root: &Path, entry: &HistoryEntry) -> PathBuf {
    if entry.kind.is_recording() {
        entry
            .recording_media_path(root)
            .unwrap_or_else(|| root.join(&entry.id))
    } else {
        root.join(&entry.id).join("capture.png")
    }
}

fn run_action(state: &Rc<State>, card: &Rc<CardView>, entry: &HistoryEntry, action: CardAction) {
    state.card_errors.borrow_mut().remove(&entry.id);
    card.restyle(state);
    match action {
        CardAction::Edit => (state.actions.edit)(entry),
        CardAction::Restore => (state.actions.restore)(entry),
        CardAction::Copy => {
            let path = media_path(&state.root, entry);
            let weak = Rc::downgrade(card);
            let weak_state = Rc::downgrade(state);
            glib::spawn_future_local(async move {
                let texture = gio::spawn_blocking(move || {
                    gdk::Texture::from_filename(&path).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|_| Err("the copy worker stopped".into()));
                let (Some(card), Some(state)) = (weak.upgrade(), weak_state.upgrade()) else {
                    return;
                };
                match texture {
                    Ok(texture) => card.root.clipboard().set_texture(&texture),
                    Err(error) => card.fail(&state, format!("Couldn’t copy the image: {error}")),
                }
            });
        }
        CardAction::SaveImage | CardAction::SaveFile => {
            let window = card.root.root().and_downcast::<gtk::Window>();
            let weak = Rc::downgrade(card);
            let weak_state = Rc::downgrade(state);
            save_copy(
                window.as_ref(),
                action.label(),
                media_path(&state.root, entry),
                move |error| {
                    if let (Some(card), Some(state)) = (weak.upgrade(), weak_state.upgrade()) {
                        card.fail(&state, error);
                    }
                },
            );
        }
        CardAction::ShowInFolder => {
            let path = entry
                .saved_path
                .as_ref()
                .map(PathBuf::from)
                .unwrap_or_else(|| media_path(&state.root, entry));
            reveal(&path);
        }
    }
}

/// Ask where to save a copy of `source`, then copy it there.
pub fn save_copy(
    window: Option<&gtk::Window>,
    title: &str,
    source: PathBuf,
    failed: impl Fn(String) + 'static,
) {
    #[allow(deprecated)] // `GtkFileDialog` needs GTK 4.10.
    let chooser = gtk::FileChooserNative::new(
        Some(title),
        window,
        gtk::FileChooserAction::Save,
        Some("Save"),
        Some("Cancel"),
    );
    #[allow(deprecated)]
    chooser.set_current_name(
        &source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "capture.png".into()),
    );
    // GtkNativeDialog does not keep itself alive: the handler holds the
    // chooser until it responds, then drops it.
    let keep = std::rc::Rc::new(std::cell::RefCell::new(Some(chooser.clone())));
    #[allow(deprecated)]
    chooser.connect_response(move |chooser, response| {
        let target = chooser.file().and_then(|file| file.path());
        chooser.destroy();
        keep.borrow_mut().take();
        let (Some(target), gtk::ResponseType::Accept) = (target, response) else {
            return;
        };
        if let Err(error) = std::fs::copy(&source, &target) {
            failed(format!("Couldn’t save: {error}"));
        }
    });
    #[allow(deprecated)]
    chooser.show();
}

/// Select the file in the desktop's file manager
/// (`org.freedesktop.FileManager1.ShowItems`), or open its folder.
pub fn reveal(path: &Path) {
    let uri = gio::File::for_path(path).uri();
    let folder = path
        .parent()
        .map(|parent| gio::File::for_path(parent).uri());
    gio::bus_get(gio::BusType::Session, gio::Cancellable::NONE, move |bus| {
        let open_folder = move || {
            if let Some(folder) = folder {
                let _ = gio::AppInfo::launch_default_for_uri(&folder, gio::AppLaunchContext::NONE);
            }
        };
        let Ok(bus) = bus else {
            open_folder();
            return;
        };
        bus.call(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1",
            "ShowItems",
            Some(&(vec![uri.to_string()], "").to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            2_000,
            gio::Cancellable::NONE,
            move |result| {
                if result.is_err() {
                    open_folder();
                }
            },
        );
    });
}

type Ready = Box<dyn FnOnce(gdk::Texture)>;

/// Thumbnail textures decoded off the UI thread at display size, with a
/// small LRU so scrolling back does not decode again.
#[derive(Default)]
struct Thumbnails {
    cache: RefCell<VecDeque<(String, gdk::Texture)>>,
    /// Ids a bound card still shows; workers skip anything scrolled away.
    wanted: Arc<Mutex<HashSet<String>>>,
    waiting: RefCell<HashMap<String, Vec<Ready>>>,
}

impl Thumbnails {
    fn request(
        self: &Rc<Self>,
        root: &Path,
        id: &str,
        (width, height): (i32, i32),
        ready: impl FnOnce(gdk::Texture) + 'static,
    ) {
        let id = id.to_owned();
        {
            let mut cache = self.cache.borrow_mut();
            if let Some(index) = cache.iter().position(|(key, _)| *key == id) {
                let hit = cache.remove(index).expect("indexed");
                ready(hit.1.clone());
                cache.push_front(hit);
                return;
            }
        }
        self.wanted.lock().expect("wanted").insert(id.clone());
        let mut waiting = self.waiting.borrow_mut();
        if let Some(callbacks) = waiting.get_mut(&id) {
            callbacks.push(Box::new(ready));
            return;
        }
        waiting.insert(id.clone(), vec![Box::new(ready)]);
        drop(waiting);
        let path = root.join(&id).join("preview.png");
        let wanted = self.wanted.clone();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let key = id.clone();
            let decoded = gio::spawn_blocking(move || {
                if !wanted.lock().expect("wanted").contains(&key) {
                    return None;
                }
                let pixbuf =
                    gdk::gdk_pixbuf::Pixbuf::from_file_at_scale(&path, width, height, true).ok()?;
                Some(gdk::Texture::for_pixbuf(&pixbuf))
            })
            .await
            .ok()
            .flatten();
            let Some(thumbnails) = weak.upgrade() else {
                return;
            };
            let callbacks = thumbnails.waiting.borrow_mut().remove(&id);
            let Some(texture) = decoded else { return };
            let mut cache = thumbnails.cache.borrow_mut();
            cache.push_front((id, texture.clone()));
            cache.truncate(THUMBNAIL_CACHE);
            drop(cache);
            for ready in callbacks.into_iter().flatten() {
                ready(texture.clone());
            }
        });
    }

    fn release(&self, id: &str) {
        if !id.is_empty() {
            self.wanted.lock().expect("wanted").remove(id);
            self.waiting.borrow_mut().remove(id);
        }
    }

    fn forget(&self, id: &str) {
        self.release(id);
        self.cache.borrow_mut().retain(|(key, _)| key != id);
    }

    fn clear(&self) {
        self.cache.borrow_mut().clear();
    }
}
