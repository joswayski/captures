//! The quick format bar over a selected text layer under Select
//! (`captures_app::editor_chrome::text_quick_bar`): Font, Smaller / size /
//! Larger, then Bold, Italic and the alignments. A solid raised card inside
//! the editor window, so it follows the appearance setting. Every control
//! sends the same [`Edit::Text`] the inspector's text fields send.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use captures_app::editor::TextElement;
use captures_app::editor_chrome::{text_format, text_quick_bar as bar};
use captures_app::editor_session::TextPatch;
use captures_app::editor_text;
use gtk::prelude::*;

use super::controls::{Choice, pressed};
use super::inspector::Edit;
use crate::widgets::Icon;

type Send = Rc<RefCell<Option<Box<dyn Fn(Edit)>>>>;

pub(super) struct QuickBar {
    pub(super) root: gtk::Box,
    font: Rc<Choice>,
    size: gtk::Label,
    smaller: gtk::Button,
    larger: gtk::Button,
    bold: gtk::Button,
    italic: gtk::Button,
    align: Vec<(&'static str, gtk::Button)>,
    /// The label the bar shows, for the toggles and size steps.
    current: Rc<RefCell<Option<TextElement>>>,
    send: Send,
}

fn button(child: &impl IsA<gtk::Widget>, name: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("format-button");
    button.add_css_class("quick-bar-button");
    button.set_child(Some(child));
    button.set_size_request(bar::CONTROL as i32, bar::CONTROL as i32);
    button.set_tooltip_text(Some(name));
    button.update_property(&[gtk::accessible::Property::Label(name)]);
    button
}

fn icon(name: &str) -> Icon {
    let icon = Icon::new(name, text_format::ICON as i32).with_stroke(1.8);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon
}

fn separator() -> gtk::Separator {
    let line = gtk::Separator::new(gtk::Orientation::Vertical);
    line.add_css_class("quick-bar-separator");
    line.set_margin_top(bar::GAP as i32);
    line.set_margin_bottom(bar::GAP as i32);
    line
}

fn emit(send: &Send, patch: TextPatch) {
    if let Some(send) = send.borrow().as_ref() {
        send(Edit::Text { patch, live: None });
    }
}

impl QuickBar {
    pub(super) fn new() -> Self {
        // The card, and its row of controls inset by the bar's padding.
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        card.add_css_class("editor-text-quick-bar");
        card.set_accessible_role(gtk::AccessibleRole::Group);
        card.update_property(&[gtk::accessible::Property::Label(bar::LABEL)]);
        card.set_halign(gtk::Align::Start);
        card.set_valign(gtk::Align::Start);
        card.set_visible(false);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, bar::GAP as i32);
        let padding = bar::PADDING as i32;
        root.set_margin_top(padding);
        root.set_margin_bottom(padding);
        root.set_margin_start(padding);
        root.set_margin_end(padding);
        card.append(&root);
        let send: Send = Rc::default();
        let current: Rc<RefCell<Option<TextElement>>> = Rc::default();

        let font = {
            let send = send.clone();
            Choice::new(None, bar::FONT, false, move |family| {
                emit(
                    &send,
                    TextPatch {
                        font_family: Some(family),
                        ..Default::default()
                    },
                );
            })
        };
        font.root.add_css_class("quick-bar-font");
        font.root
            .set_size_request(bar::FONT_WIDTH as i32, bar::CONTROL as i32);
        font.fill_height();
        font.set_tooltip(bar::FONT);
        root.append(&font.root);
        root.append(&separator());

        let smaller = button(&icon("minus"), bar::SMALLER);
        let larger = button(&icon("plus"), bar::LARGER);
        for (step, larger) in [(&smaller, false), (&larger, true)] {
            let (send, current) = (send.clone(), current.clone());
            step.connect_clicked(move |_| {
                let Some(size) = current.borrow().as_ref().map(|label| label.font_size) else {
                    return;
                };
                let next = bar::step(size, larger);
                if next != size {
                    emit(
                        &send,
                        TextPatch {
                            font_size: Some(next),
                            ..Default::default()
                        },
                    );
                }
            });
        }
        let size = gtk::Label::new(None);
        size.add_css_class("quick-bar-size");
        size.set_size_request(bar::SIZE_WIDTH as i32, bar::CONTROL as i32);
        size.set_tooltip_text(Some(bar::SIZE));
        root.append(&smaller);
        root.append(&size);
        root.append(&larger);
        root.append(&separator());

        let bold_glyph = gtk::Label::new(Some("B"));
        let bold = button(&bold_glyph, text_format::BOLD);
        let italic_glyph = gtk::Label::new(None);
        italic_glyph.set_markup("<i>I</i>");
        let italic = button(&italic_glyph, text_format::ITALIC);
        for (toggle, is_bold) in [(&bold, true), (&italic, false)] {
            let (send, current) = (send.clone(), current.clone());
            toggle.connect_clicked(move |_| {
                let Some(label) = current.borrow().clone() else {
                    return;
                };
                let mut patch = TextPatch::default();
                if is_bold {
                    patch.bold = Some(!label.bold);
                } else {
                    patch.italic = Some(!label.italic);
                }
                emit(&send, patch);
            });
            root.append(toggle);
        }
        let align: Vec<_> = text_format::ALIGN
            .iter()
            .map(|(value, name, glyph)| {
                let align = button(&icon(glyph), name);
                let send = send.clone();
                align.connect_clicked(move |_| {
                    emit(
                        &send,
                        TextPatch {
                            align: Some((*value).to_owned()),
                            ..Default::default()
                        },
                    );
                });
                root.append(&align);
                (*value, align)
            })
            .collect();
        QuickBar {
            root: card,
            font,
            size,
            smaller,
            larger,
            bold,
            italic,
            align,
            current,
            send,
        }
    }

    pub(super) fn connect(&self, send: impl Fn(Edit) + 'static) {
        *self.send.borrow_mut() = Some(Box::new(send));
    }

    /// Show `label`'s font, size, emphasis and alignment.
    pub(super) fn sync(&self, label: &TextElement, fonts: &BTreeMap<String, String>) {
        *self.current.borrow_mut() = Some(label.clone());
        self.font
            .set_options(editor_text::font_family_options(fonts));
        self.font.set(&label.font_family);
        let size = format!("{}", label.font_size.round());
        self.size.set_label(&size);
        self.size
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{}: {size}",
                bar::SIZE
            ))]);
        self.smaller
            .set_sensitive(bar::step(label.font_size, false) != label.font_size);
        self.larger
            .set_sensitive(bar::step(label.font_size, true) != label.font_size);
        pressed(&self.bold, label.bold);
        pressed(&self.italic, label.italic);
        for (value, button) in &self.align {
            pressed(button, label.align == *value);
        }
    }

    /// The bar's natural size, for placing it.
    pub(super) fn size(&self) -> (i32, i32) {
        let (_, natural) = self.root.preferred_size();
        (natural.width(), natural.height())
    }
}
