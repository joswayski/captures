//! Floating mini previews (`?view=thumbnail`): History's Restore brings a
//! capture back as a card in one undecorated stack window. Cards follow
//! `styles/mini-preview.css`: a cover thumbnail with its size chip, and on
//! hover or keyboard focus a dimmed, blurred image under Close, Delete, Edit,
//! Copy and Save file (Show in Folder once saved). Membership and copy come
//! from `captures_app::preview`; only the hovered card redraws.
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use captures_app::preview::{self, PreviewStack};
use captures_history::HistoryEntry;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::history::{media_path, reveal, save_copy};
use crate::widgets::{Icon, Thumbnail, icon_button};

pub const TITLE: &str = "Captures Previews";

pub struct Actions {
    pub edit: Box<dyn Fn(&HistoryEntry)>,
    /// A preview's Delete removed a History entry from disk.
    pub changed: Box<dyn Fn()>,
}

pub struct Previews {
    pub window: gtk::ApplicationWindow,
    state: Rc<State>,
}

struct Card {
    entry: HistoryEntry,
    root: gtk::Overlay,
    image: Thumbnail,
    chip: gtk::Label,
    /// Hover-only controls; untargetable while hidden, as shipping's
    /// `pointer-events: none` overlays.
    controls: Vec<gtk::Button>,
    hovered: Cell<bool>,
    focused: Cell<bool>,
    /// Shipping's stale-pointer hover lock: a card that appears under a
    /// resting pointer stays at rest until the pointer really moves.
    lock_origin: Cell<Option<(f64, f64)>>,
}

struct State {
    history_root: PathBuf,
    window: gtk::ApplicationWindow,
    stack: RefCell<PreviewStack>,
    cards: RefCell<Vec<Rc<Card>>>,
    list: gtk::Box,
    minimize: gtk::Button,
    actions: Actions,
}

fn icon_control(icon: &str, label: &str, classes: &[&str]) -> gtk::Button {
    let button = gtk::Button::new();
    for class in classes {
        button.add_css_class(class);
    }
    button.set_child(Some(&Icon::new(icon, 16).with_stroke(1.8)));
    button.set_tooltip_text(Some(label));
    button.update_property(&[gtk::accessible::Property::Label(label)]);
    button
}

pub fn build(app: &gtk::Application, history_root: PathBuf, actions: Actions) -> Previews {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(TITLE)
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("captures");
    window.add_css_class("preview-stack-window");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let pad = preview::THUMBNAIL_PADDING as i32;
    root.set_margin_start(pad);
    root.set_margin_end(pad);
    root.set_margin_top(pad);
    let list = gtk::Box::new(gtk::Orientation::Vertical, preview::THUMBNAIL_GAP as i32);
    list.update_property(&[gtk::accessible::Property::Label("Previews")]);
    root.append(&list);
    // The shared control gutter under the stack.
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.set_size_request(-1, preview::THUMBNAIL_CONTROL_GUTTER as i32);
    controls.set_valign(gtk::Align::Start);
    let clear = icon_control(
        "close",
        preview::STACK_CLEAR_ALL_LABEL,
        &["preview-stack-control"],
    );
    clear.set_tooltip_text(Some(preview::STACK_CLEAR_ALL_TOOLTIP));
    let minimize = icon_control(
        "preview-stack",
        preview::STACK_MINIMIZE_LABEL,
        &["preview-stack-control"],
    );
    for button in [&clear, &minimize] {
        button.set_valign(gtk::Align::Center);
        controls.append(button);
    }
    root.append(&controls);
    window.set_child(Some(&root));

    let state = Rc::new(State {
        history_root,
        window: window.clone(),
        stack: RefCell::default(),
        cards: RefCell::default(),
        list,
        minimize: minimize.clone(),
        actions,
    });
    let weak = Rc::downgrade(&state);
    clear.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.clear_all();
        }
    });
    let weak = Rc::downgrade(&state);
    minimize.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.toggle_collapsed();
        }
    });
    let keys = gtk::EventControllerKey::new();
    let weak = Rc::downgrade(&state);
    keys.connect_key_pressed(move |_, key, _, _| {
        let Some(state) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        if key == gdk::Key::Escape && !state.stack.borrow().is_collapsed() {
            state.toggle_collapsed();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    let weak = Rc::downgrade(&state);
    window.connect_focus_visible_notify(move |_| {
        if let Some(state) = weak.upgrade() {
            for card in state.cards.borrow().iter() {
                card.refresh();
            }
        }
    });
    Previews { window, state }
}

impl Previews {
    /// History's Restore: add `entry` as the newest card, or show it again.
    pub fn restore(&self, entry: HistoryEntry) {
        self.state.restore(entry);
    }

    pub fn toggle_collapsed(&self) {
        self.state.toggle_collapsed();
    }
}

impl State {
    fn restore(self: &Rc<Self>, entry: HistoryEntry) {
        if self.stack.borrow_mut().insert(entry.id.clone()) {
            let card = self.card(entry);
            self.list.append(&card.root);
            self.cards.borrow_mut().push(card);
        }
        // A restore always shows the whole stack.
        self.stack.borrow_mut().set_collapsed(false);
        self.relayout();
        self.window.present();
    }

    fn dismiss(&self, id: &str) {
        if !self.stack.borrow_mut().remove(id) {
            return;
        }
        self.cards.borrow_mut().retain(|card| {
            let keep = card.entry.id != id;
            if !keep {
                self.list.remove(&card.root);
            }
            keep
        });
        self.relayout();
    }

    fn clear_all(&self) {
        let ids = self.stack.borrow().ids().to_vec();
        self.stack.borrow_mut().remove_all(&ids);
        for card in self.cards.borrow_mut().drain(..) {
            self.list.remove(&card.root);
        }
        self.relayout();
    }

    fn toggle_collapsed(&self) {
        let collapsed = !self.stack.borrow().is_collapsed();
        self.stack.borrow_mut().set_collapsed(collapsed);
        self.relayout();
    }

    /// Collapsed keeps only the newest card (the shipping 3D pile pose is
    /// not ported); the window shrinks to its content.
    fn relayout(&self) {
        let stack = self.stack.borrow();
        if stack.ids().is_empty() {
            self.window.set_visible(false);
            return;
        }
        let collapsed = stack.is_collapsed();
        let cards = self.cards.borrow();
        let newest = cards.len().saturating_sub(1);
        for (index, card) in cards.iter().enumerate() {
            card.root.set_visible(!collapsed || index == newest);
        }
        let label = if collapsed {
            "Expand previews"
        } else {
            preview::STACK_MINIMIZE_LABEL
        };
        self.minimize.set_tooltip_text(Some(if collapsed {
            "Show all"
        } else {
            preview::STACK_MINIMIZE_HOVER_LABEL
        }));
        self.minimize
            .update_property(&[gtk::accessible::Property::Label(label)]);
        // Undecorated, non-resizable: let the window shrink to the stack.
        self.window.set_default_size(-1, -1);
    }

    fn card(self: &Rc<Self>, entry: HistoryEntry) -> Rc<Card> {
        let image = Thumbnail::default();
        image.set_cover(true);
        image.set_size_request(
            preview::THUMBNAIL_WIDTH as i32,
            preview::THUMBNAIL_CARD_HEIGHT as i32,
        );
        let recording = entry.kind.is_recording();
        image.update_property(&[gtk::accessible::Property::Label(if recording {
            "Recording preview"
        } else {
            "Screenshot preview"
        })]);
        let root = gtk::Overlay::new();
        root.add_css_class("preview-card");
        root.set_overflow(gtk::Overflow::Hidden);
        root.set_child(Some(&image));

        // `.thumbnail-bottom-bar`: size chip and the clipboard status chip.
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bottom.set_valign(gtk::Align::End);
        bottom.set_margin_start(8);
        bottom.set_margin_end(8);
        bottom.set_margin_bottom(8);
        bottom.set_can_target(false);
        let meta = gtk::Label::new(Some(&preview::card_metadata(
            entry.width,
            entry.height,
            entry.size_bytes,
        )));
        meta.add_css_class("preview-meta");
        meta.set_hexpand(true);
        meta.set_halign(gtk::Align::Start);
        let chip = gtk::Label::new(None);
        chip.add_css_class("preview-chip");
        chip.set_accessible_role(gtk::AccessibleRole::Status);
        // A live region stays in the tree and only its text changes: newer
        // GTK does not announce a status node that appears already filled.
        chip.set_opacity(0.);
        bottom.append(&meta);
        bottom.append(&chip);
        root.add_overlay(&bottom);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top.add_css_class("preview-overlay");
        top.set_halign(gtk::Align::Start);
        top.set_valign(gtk::Align::Start);
        top.set_margin_start(8);
        top.set_margin_top(8);
        let close = icon_control("close", "Close", &["preview-icon-button"]);
        let delete = icon_control("trash", "Delete", &["preview-icon-button", "delete"]);
        top.append(&close);
        top.append(&delete);
        root.add_overlay(&top);

        let mut controls = vec![close.clone(), delete.clone()];
        let edit = (!recording).then(|| {
            let edit = icon_control(
                "edit",
                captures_app::preview_chrome::EDIT_LABEL,
                &["preview-icon-button", "preview-overlay"],
            );
            edit.set_halign(gtk::Align::End);
            edit.set_valign(gtk::Align::Start);
            edit.set_margin_end(8);
            edit.set_margin_top(8);
            root.add_overlay(&edit);
            controls.push(edit.clone());
            edit
        });

        // `.thumbnail-main-actions`: a centered 140 px column.
        let main = gtk::Box::new(gtk::Orientation::Vertical, 6);
        main.add_css_class("preview-overlay");
        main.set_size_request(140, -1);
        main.set_halign(gtk::Align::Center);
        main.set_valign(gtk::Align::Center);
        let copy = (!recording).then(|| {
            let (copy, _, _) = icon_button("copy", "Copy", &["preview-main-button"]);
            main.append(&copy);
            controls.push(copy.clone());
            copy
        });
        let saved = entry.saved_path.clone();
        let (second, _, _) = if saved.is_some() {
            icon_button(
                "folder",
                "Show in Folder",
                &["preview-main-button", "primary"],
            )
        } else {
            icon_button("save", "Save file", &["preview-main-button", "primary"])
        };
        main.append(&second);
        controls.push(second.clone());
        root.add_overlay(&main);

        let card = Rc::new(Card {
            entry,
            root,
            image,
            chip,
            controls,
            hovered: Cell::new(false),
            focused: Cell::new(false),
            lock_origin: Cell::new(None),
        });
        card.refresh();
        self.load_thumbnail(&card);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&card);
        let unlocked = Rc::new(Cell::new(false));
        let seen = unlocked.clone();
        motion.connect_enter(move |_, x, y| {
            let Some(card) = weak.upgrade() else { return };
            if seen.get() {
                card.hovered.set(true);
                card.refresh();
            } else {
                card.lock_origin.set(Some((x, y)));
            }
        });
        let weak = Rc::downgrade(&card);
        motion.connect_motion(move |_, x, y| {
            let Some(card) = weak.upgrade() else { return };
            if unlocked.get() {
                if !card.hovered.get() {
                    card.hovered.set(true);
                    card.refresh();
                }
                return;
            }
            let (ox, oy) = card.lock_origin.get().unwrap_or((x, y));
            card.lock_origin.set(Some((ox, oy)));
            if (x - ox).hypot(y - oy) > captures_app::preview_chrome::CARD_HOVER_LOCK_SLOP {
                unlocked.set(true);
                card.hovered.set(true);
                card.refresh();
            }
        });
        let weak = Rc::downgrade(&card);
        motion.connect_leave(move |_| {
            if let Some(card) = weak.upgrade() {
                card.hovered.set(false);
                card.refresh();
            }
        });
        card.root.add_controller(motion);
        let focus = gtk::EventControllerFocus::new();
        let weak = Rc::downgrade(&card);
        focus.connect_enter(move |_| {
            if let Some(card) = weak.upgrade() {
                card.focused.set(true);
                card.refresh();
            }
        });
        let weak = Rc::downgrade(&card);
        focus.connect_leave(move |_| {
            if let Some(card) = weak.upgrade() {
                card.focused.set(false);
                card.refresh();
            }
        });
        card.root.add_controller(focus);

        // File drag out, as History's cards.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY);
        let path = media_path(&self.history_root, &card.entry);
        drag.connect_prepare(move |_, _, _| {
            let uri = gio::File::for_path(&path).uri();
            Some(gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from_owned(format!("{uri}\r\n").into_bytes()),
            ))
        });
        card.image.add_controller(drag);

        let id = card.entry.id.clone();
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.dismiss(&id);
            }
        });
        let weak = Rc::downgrade(self);
        let weak_card = Rc::downgrade(&card);
        delete.connect_clicked(move |_| {
            if let (Some(state), Some(card)) = (weak.upgrade(), weak_card.upgrade()) {
                state.delete(&card);
            }
        });
        if let Some(edit) = edit {
            let weak = Rc::downgrade(self);
            let entry = card.entry.clone();
            edit.connect_clicked(move |_| {
                if let Some(state) = weak.upgrade() {
                    (state.actions.edit)(&entry);
                }
            });
        }
        if let Some(copy) = copy {
            let weak = Rc::downgrade(self);
            let weak_card = Rc::downgrade(&card);
            copy.connect_clicked(move |_| {
                if let (Some(state), Some(card)) = (weak.upgrade(), weak_card.upgrade()) {
                    state.copy(&card);
                }
            });
        }
        let weak = Rc::downgrade(self);
        let weak_card = Rc::downgrade(&card);
        second.connect_clicked(move |_| {
            let (Some(state), Some(card)) = (weak.upgrade(), weak_card.upgrade()) else {
                return;
            };
            match &saved {
                Some(path) => reveal(std::path::Path::new(path)),
                None => {
                    let weak_card = Rc::downgrade(&card);
                    save_copy(
                        Some(state.window.upcast_ref()),
                        "Save file",
                        media_path(&state.history_root, &card.entry),
                        move |error| {
                            if let Some(card) = weak_card.upgrade() {
                                card.notify(&error, true);
                            }
                        },
                    );
                }
            }
        });
        card
    }

    /// Decode `preview.png` at twice the card width off the UI thread; the
    /// full capture is never loaded to draw a card.
    fn load_thumbnail(&self, card: &Rc<Card>) {
        let directory = self.history_root.join(&card.entry.id);
        let weak = Rc::downgrade(card);
        glib::spawn_future_local(async move {
            let pixbuf = gio::spawn_blocking(move || {
                ["preview.png", "capture.png"].iter().find_map(|name| {
                    let width = (preview::THUMBNAIL_WIDTH * 2.) as i32;
                    gdk::gdk_pixbuf::Pixbuf::from_file_at_scale(
                        directory.join(name),
                        width,
                        -1,
                        true,
                    )
                    .ok()
                    .map(|pixbuf| gdk::Texture::for_pixbuf(&pixbuf))
                })
            })
            .await
            .ok()
            .flatten();
            if let (Some(card), Some(texture)) = (weak.upgrade(), pixbuf) {
                card.image.set_texture(Some(texture));
            }
        });
    }

    fn copy(&self, card: &Rc<Card>) {
        let path = media_path(&self.history_root, &card.entry);
        // Temporary CI trace for GTK 4.14's missing Copied chip.
        let trace = std::env::var_os("CAPTURES_GTK_TRACE").is_some();
        if trace {
            eprintln!("trace: copy {}", path.display());
        }
        let weak = Rc::downgrade(card);
        glib::spawn_future_local(async move {
            let texture = gio::spawn_blocking(move || {
                gdk::Texture::from_filename(&path).map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("the copy worker stopped".into()));
            if trace {
                eprintln!("trace: copy decoded {:?}", texture.as_ref().map(|_| ()));
            }
            let Some(card) = weak.upgrade() else { return };
            match texture {
                Ok(texture) => {
                    card.root.clipboard().set_texture(&texture);
                    card.notify("Copied to clipboard", false);
                }
                Err(error) => card.notify(&format!("Couldn’t copy: {error}"), true),
            }
        });
    }

    /// Delete removes the History entry (capture and recovery media);
    /// exported copies elsewhere stay, as History's Delete.
    fn delete(self: &Rc<Self>, card: &Rc<Card>) {
        let root = self.history_root.clone();
        let id = card.entry.id.clone();
        let weak = Rc::downgrade(self);
        let weak_card = Rc::downgrade(card);
        glib::spawn_future_local(async move {
            let target = id.clone();
            let result = gio::spawn_blocking(move || captures_history::delete(&root, &target))
                .await
                .unwrap_or(Err(captures_history::Error::HistoryUnavailable));
            let Some(state) = weak.upgrade() else { return };
            match result {
                Ok(()) => {
                    state.dismiss(&id);
                    (state.actions.changed)();
                }
                Err(error) => {
                    if let Some(card) = weak_card.upgrade() {
                        card.notify(&format!("Couldn’t delete: {error}"), true);
                    }
                }
            }
        });
    }
}

impl Card {
    /// Hover or focus shows the controls over a dimmed image; only this
    /// card's nodes change.
    fn refresh(&self) {
        // Keyboard focus only (`:focus-visible`): the window's initial focus
        // must not reveal the first card's controls.
        let keyboard = self
            .root
            .root()
            .and_downcast::<gtk::Window>()
            .is_some_and(|window| window.gets_focus_visible());
        let active = self.hovered.get() || (self.focused.get() && keyboard);
        if active {
            self.root.add_css_class("hover");
        } else {
            self.root.remove_css_class("hover");
        }
        self.image.set_dimmed(active);
        for button in &self.controls {
            button.set_can_target(active);
        }
    }

    fn notify(self: &Rc<Self>, text: &str, error: bool) {
        if std::env::var_os("CAPTURES_GTK_TRACE").is_some() {
            eprintln!("trace: notify {text}");
        }
        self.chip.set_label(text);
        self.chip
            .update_property(&[gtk::accessible::Property::Label(text)]);
        if error {
            self.chip.add_css_class("error");
        } else {
            self.chip.remove_css_class("error");
        }
        self.chip.set_opacity(1.);
        let weak = Rc::downgrade(self);
        let shown = text.to_owned();
        glib::timeout_add_local_once(Duration::from_millis(2_500), move || {
            if let Some(card) = weak.upgrade()
                && card.chip.label() == shown
            {
                card.chip.set_opacity(0.);
                card.chip.set_label("");
                card.chip
                    .update_property(&[gtk::accessible::Property::Label("")]);
            }
        });
    }
}
