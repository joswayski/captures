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

use captures_app::preview::{self, PreviewDropLanding, PreviewStack};
use captures_app::preview_motion::ExitKind;
use captures_history::HistoryEntry;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::history::{media_path, reveal, save_copy};
use crate::tokens::Tokens;
use crate::widgets::{Icon, Thumbnail, icon_button};

mod pile;
use pile::{Pile, PileCard};
#[cfg(feature = "x11")]
mod x11;

pub const TITLE: &str = "Captures Previews";

pub struct Actions {
    pub edit: Box<dyn Fn(&HistoryEntry)>,
    /// A preview's Delete removed a History entry from disk.
    pub changed: Box<dyn Fn()>,
    /// Whether an editor window is open on this artifact.
    pub editing: Box<dyn Fn(&str) -> bool>,
}

pub struct Previews {
    pub window: gtk::ApplicationWindow,
    state: Rc<State>,
}

struct Card {
    entry: HistoryEntry,
    root: gtk::Overlay,
    dim: gtk::Box,
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
    /// Playing its exit: the look stays as it was when the exit began.
    exiting: Cell<bool>,
}

struct State {
    history_root: PathBuf,
    window: gtk::ApplicationWindow,
    stack: RefCell<PreviewStack>,
    cards: RefCell<Vec<Rc<Card>>>,
    pile: Pile,
    minimize: gtk::Button,
    actions: Actions,
    /// The current file drag landed on this stack (shipping rejects it).
    self_drop: Cell<bool>,
    #[cfg(feature = "x11")]
    placement: captures_settings::MiniPreviewPlacement,
    #[cfg(feature = "x11")]
    placer: Option<Rc<x11::Placer>>,
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

pub fn build(
    app: &gtk::Application,
    tokens: &Tokens,
    placement: captures_settings::MiniPreviewPlacement,
    history_root: PathBuf,
    actions: Actions,
) -> Previews {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(TITLE)
        .decorated(false)
        .resizable(false)
        .build();
    window.add_css_class("captures");
    window.add_css_class("preview-stack-window");

    // The shared control gutter beside the stack's anchored edge.
    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
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
    let pile = Pile::new(tokens, &controls);
    pile.update_property(&[gtk::accessible::Property::Label("Previews")]);
    pile.set_top_anchor(placement.is_top());
    pile.set_right_anchor(placement.is_right());
    window.set_child(Some(&pile));

    let state = Rc::new(State {
        history_root,
        window: window.clone(),
        stack: RefCell::default(),
        cards: RefCell::default(),
        pile: pile.clone(),
        minimize: minimize.clone(),
        actions,
        self_drop: Cell::new(false),
        #[cfg(feature = "x11")]
        placement,
        #[cfg(feature = "x11")]
        placer: x11::Placer::new(&gtk::prelude::WidgetExt::display(&window)).map(Rc::new),
    });
    #[cfg(feature = "x11")]
    place_on_x11(&state);
    let weak = Rc::downgrade(&state);
    clear.connect_clicked(move |_| {
        if let Some(state) = weak.upgrade() {
            state.clear_all();
        }
    });
    let weak = Rc::downgrade(&state);
    pile.connect_exited(move || {
        if let Some(state) = weak.upgrade() {
            state.relayout();
        }
    });
    // A card's file dropped back on the stack is refused with a shake.
    let drop = gtk::DropTargetAsync::new(None, gdk::DragAction::COPY);
    drop.connect_accept(|_, drop| drop.drag().is_some());
    let weak = Rc::downgrade(&state);
    drop.connect_drop(move |_, drop, _, _| {
        if let Some(state) = weak.upgrade() {
            state.self_drop.set(true);
        }
        drop.finish(gdk::DragAction::empty());
        true
    });
    pile.add_controller(drop);
    let weak = Rc::downgrade(&state);
    pile.connect_expand(move || {
        if let Some(state) = weak.upgrade()
            && state.stack.borrow().is_collapsed()
        {
            state.toggle_collapsed();
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

/// Shipping's stack placement on X11: skip the taskbar and pager before the
/// first map, pin above once the window manager takes the window on, and
/// keep the anchored edge put as the window resizes.
#[cfg(feature = "x11")]
fn place_on_x11(state: &Rc<State>) {
    if state.placer.is_none() {
        return;
    }
    let weak = Rc::downgrade(state);
    state.window.connect_realize(move |window| {
        let Some(surface) = window.surface().and_downcast::<gdk4_x11::X11Surface>() else {
            return;
        };
        surface.set_skip_taskbar_hint(true);
        surface.set_skip_pager_hint(true);
        let Some(state) = weak.upgrade() else {
            return;
        };
        state.place(&surface, true);
        let weak = Rc::downgrade(&state);
        surface.connect_notify_local(Some("mapped"), move |surface, _| {
            if let Some(state) = weak.upgrade()
                && surface.is_mapped()
            {
                state.place(surface, true);
            }
        });
        let (weak, pinned) = (Rc::downgrade(&state), surface.downgrade());
        if let Some(placer) = state.placer.as_ref() {
            placer.watch_managed(&surface, move || {
                if let (Some(state), Some(surface)) = (weak.upgrade(), pinned.upgrade())
                    && let Some(placer) = state.placer.as_ref()
                {
                    placer.pin(&surface);
                    // The window manager may have placed it on its own.
                    state.place(&surface, false);
                }
            });
        }
        let weak = Rc::downgrade(&state);
        surface.connect_height_notify(move |surface| {
            if let Some(state) = weak.upgrade() {
                state.place(surface, false);
            }
        });
    });
}

impl Previews {
    /// History's Restore: add `entry` as the newest card, or show it again.
    pub fn restore(&self, entry: HistoryEntry) {
        self.state.restore(entry);
    }

    pub fn toggle_collapsed(&self) {
        self.state.toggle_collapsed();
    }

    /// Shipping's `thumbnail-editor-active` ring follows the editor windows.
    pub fn sync_editing(&self) {
        self.state.sync_editing();
    }
}

impl State {
    fn restore(self: &Rc<Self>, entry: HistoryEntry) {
        if self.stack.borrow_mut().insert(entry.id.clone()) {
            let card = self.card(entry);
            self.pile.add_card(PileCard {
                id: card.entry.id.clone(),
                root: card.root.clone().upcast(),
                dim: card.dim.clone().upcast(),
                image: card.image.clone(),
            });
            self.cards.borrow_mut().push(card);
        }
        self.sync_editing();
        // A restore always shows the whole stack.
        self.stack.borrow_mut().set_collapsed(false);
        self.relayout();
        self.window.present();
    }

    fn sync_editing(&self) {
        for card in self.cards.borrow().iter() {
            if (self.actions.editing)(&card.entry.id) {
                card.root.add_css_class("editor-active");
            } else {
                card.root.remove_css_class("editor-active");
            }
        }
    }

    /// Close or Delete: play the card's exit in its slot, then it leaves.
    /// The card keeps its locked hover look, as shipping's exit lock.
    fn leave(&self, id: &str, kind: Option<ExitKind>) {
        if !self.stack.borrow_mut().remove(id) {
            return;
        }
        let card = self
            .cards
            .borrow()
            .iter()
            .find(|card| card.entry.id == id)
            .cloned();
        self.cards.borrow_mut().retain(|card| card.entry.id != id);
        let playing = card.zip(kind).is_some_and(|(card, kind)| {
            card.exiting.set(true);
            card.image.set_dimmed(true);
            self.pile
                .begin_exit(id, kind, 0., true, card.entry.saved_path.is_some())
        });
        if !playing {
            self.pile.remove_card(id);
        }
        self.relayout();
    }

    /// Clear all: every card streaks out as it looks now, the bottom card
    /// first, without the survivors settling.
    fn clear_all(&self) {
        let ids = self.stack.borrow().ids().to_vec();
        self.stack.borrow_mut().remove_all(&ids);
        let top = self.pile.is_top_anchor();
        let cards: Vec<_> = self.cards.borrow_mut().drain(..).collect();
        for card in cards {
            let index = ids.iter().position(|id| *id == card.entry.id).unwrap_or(0);
            card.exiting.set(true);
            let delay = captures_app::preview_motion::clear_delay_ms(ids.len(), index, top);
            let saved = card.entry.saved_path.is_some();
            if !self
                .pile
                .begin_exit(&card.entry.id, ExitKind::Dismiss, delay, false, saved)
            {
                self.pile.remove_card(&card.entry.id);
            }
        }
        self.relayout();
    }

    #[cfg(feature = "x11")]
    fn place(&self, surface: &gdk4_x11::X11Surface, follow_pointer: bool) {
        let stack = self.stack.borrow();
        if let Some(placer) = self.placer.as_ref() {
            placer.place(
                surface,
                stack.ids().len(),
                stack.is_collapsed(),
                self.placement,
                follow_pointer,
            );
        }
    }

    fn toggle_collapsed(&self) {
        let collapsed = !self.stack.borrow().is_collapsed();
        self.stack.borrow_mut().set_collapsed(collapsed);
        self.relayout();
    }

    /// The pile flies between the list and the collapsed 3D pose; the
    /// non-resizable window follows its height.
    fn relayout(&self) {
        let stack = self.stack.borrow();
        if stack.ids().is_empty() {
            // Exiting cards finish first.
            if self.pile.is_empty() {
                self.window.set_visible(false);
            }
            return;
        }
        let collapsed = stack.is_collapsed();
        self.pile.set_collapsed(collapsed);
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
            pile::CARD_WIDTH as i32,
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
        // The collapsed pile's depth shade, under every control.
        let dim = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dim.add_css_class("preview-card-dim");
        dim.set_can_target(false);
        dim.set_opacity(0.);
        root.add_overlay(&dim);

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
            dim,
            image,
            chip,
            controls,
            hovered: Cell::new(false),
            focused: Cell::new(false),
            lock_origin: Cell::new(None),
            exiting: Cell::new(false),
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

        // File drag out, as History's cards. The card's image hides under a
        // still copy of it (`thumbnail-file-dragging`); an accepted copy
        // elsewhere dismisses the card, a drop back on the stack shakes it.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::COPY);
        let path = media_path(&self.history_root, &card.entry);
        let grab = Rc::new(Cell::new((0., 0.)));
        let at = grab.clone();
        drag.connect_prepare(move |_, x, y| {
            at.set((x, y));
            let uri = gio::File::for_path(&path).uri();
            Some(gdk::ContentProvider::for_bytes(
                "text/uri-list",
                &glib::Bytes::from_owned(format!("{uri}\r\n").into_bytes()),
            ))
        });
        let (weak, weak_card) = (Rc::downgrade(self), Rc::downgrade(&card));
        drag.connect_drag_begin(move |source, _| {
            let (Some(state), Some(card)) = (weak.upgrade(), weak_card.upgrade()) else {
                return;
            };
            state.self_drop.set(false);
            let still = gtk::WidgetPaintable::new(Some(&card.image)).current_image();
            let (x, y) = grab.get();
            source.set_icon(Some(&still), x as i32, y as i32);
            card.root.add_css_class("file-dragging");
        });
        let cancelled = Rc::new(Cell::new(false));
        let flag = cancelled.clone();
        drag.connect_drag_cancel(move |_, _, _| {
            flag.set(true);
            false
        });
        let (weak, weak_card) = (Rc::downgrade(self), Rc::downgrade(&card));
        drag.connect_drag_end(move |_, _, _| {
            let accepted = !cancelled.replace(false);
            let (Some(state), Some(card)) = (weak.upgrade(), weak_card.upgrade()) else {
                return;
            };
            card.root.remove_css_class("file-dragging");
            let landing = if state.self_drop.replace(false) {
                PreviewDropLanding::PreviewStack
            } else {
                PreviewDropLanding::External
            };
            match preview::preview_drag_outcome(accepted, landing) {
                preview::PreviewDragOutcome::Reject => state.pile.reject(&card.entry.id),
                preview::PreviewDragOutcome::Dismiss => state.leave(&card.entry.id, None),
                preview::PreviewDragOutcome::Retain => {}
            }
        });
        card.image.add_controller(drag);

        let id = card.entry.id.clone();
        let weak = Rc::downgrade(self);
        close.connect_clicked(move |_| {
            if let Some(state) = weak.upgrade() {
                state.leave(&id, Some(ExitKind::Dismiss));
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
                    let width = (pile::CARD_WIDTH * 2.) as i32;
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
        let weak = Rc::downgrade(card);
        glib::spawn_future_local(async move {
            let texture = gio::spawn_blocking(move || {
                gdk::Texture::from_filename(&path).map_err(|e| e.to_string())
            })
            .await
            .unwrap_or_else(|_| Err("the copy worker stopped".into()));
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
                    state.leave(&id, Some(ExitKind::Dust));
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
        if self.exiting.get() {
            return;
        }
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
