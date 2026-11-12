//! Send Feedback: shipping's `Feedback.tsx` and its `.feedback*` rules as one
//! Direct2D surface. Copy, categories and limits come from
//! `captures_app::feedback`; the context and sending come from
//! `captures-feedback`, always on a worker. Only the resident (`--live`) app
//! sends, to the shared endpoint and only when Send is pressed; other launches
//! are a fixture whose Send stays off, like the GTK host without an endpoint.
use std::cell::RefCell;
use std::sync::mpsc;

use captures_app::app_windows;
use captures_app::feedback as copy;
use captures_feedback::{FeedbackContext, FeedbackDraft};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::DirectWrite::DWRITE_HIT_TEST_METRICS;
use windows::Win32::Graphics::Gdi::{ScreenToClient, ValidateRect};
use windows::Win32::UI::Accessibility::{UiaReturnRawElementProvider, UiaRootObjectId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;
use windows_numerics::Matrix3x2;

use crate::gfx::{self, Gfx, Style, Text, rect};
use crate::panel_uia::{self, Act, Item, Role};
use crate::textfield::{self, Change, Editor, Key, TextField};
use crate::tokens::Tokens;
use crate::{Options, appearance};

const CLASS: PCWSTR = w!("CapturesFeedback");
const WM_DONE: u32 = WM_APP + 61;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// `.feedback-message { min-height: 148px }`
const MESSAGE_HEIGHT: f32 = 148.;
/// `.feedback-meta > div { grid-template-columns: 112px … }`
const META_LABEL: f32 = 112.;

thread_local! {
    static FEEDBACK: RefCell<Option<Feedback>> = const { RefCell::new(None) };
}

pub fn exists() -> bool {
    FEEDBACK.with(|f| f.try_borrow().map_or(true, |f| f.is_some()))
}

fn with_feedback<R>(f: impl FnOnce(&mut Feedback) -> R) -> Option<R> {
    FEEDBACK.with(|s| s.try_borrow_mut().ok()?.as_mut().map(f))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Category(usize),
    Message,
    Contact,
    Send,
}

const ORDER: [Target; 6] = [
    Target::Category(0),
    Target::Category(1),
    Target::Category(2),
    Target::Message,
    Target::Contact,
    Target::Send,
];

enum Status {
    Idle,
    Sending,
    Sent,
    Error(String),
}

enum Done {
    Context(FeedbackContext),
    Sent(std::result::Result<(), String>),
}

/// Text and rects from the last layout, in page coordinates.
struct Lay {
    left: f32,
    shell: f32,
    eyebrow: (Text, f32),
    title: (Text, f32),
    intro: (Text, f32),
    meta_card: D2D_RECT_F,
    meta_title: (Text, f32),
    meta: Vec<(Text, Text, f32)>,
    /// `.settings-card > * + *` rules, as `(left, right, y)`.
    dividers: Vec<(f32, f32, f32)>,
    form_card: D2D_RECT_F,
    category_label: (Text, f32),
    categories: Vec<(Text, Text, D2D_RECT_F)>,
    message_label: (Text, f32),
    message_box: D2D_RECT_F,
    message_text: Text,
    message_placeholder: Text,
    contact_label: (Text, f32),
    badge: Text,
    contact_box: D2D_RECT_F,
    help: (Text, f32),
    note: Option<(Text, D2D_RECT_F, &'static str, Option<&'static str>)>,
    send_label: Text,
    send: D2D_RECT_F,
    height: f32,
}

pub struct Feedback {
    hwnd: HWND,
    g: Gfx,
    t: Tokens,
    options: Options,
    scale: f32,
    size: (f32, f32),
    composition: Option<(
        IDCompositionTarget,
        IDCompositionVisual2,
        IDCompositionSurface,
    )>,
    /// Sending is possible (the resident app); otherwise a fixture.
    live: bool,
    category: usize,
    message: Editor,
    contact: TextField,
    context: Option<FeedbackContext>,
    status: Status,
    focus: Option<Target>,
    /// Keyboard focus: buttons draw `:focus-visible` rings.
    focus_visible: bool,
    hover: Option<Target>,
    pressed: Option<Target>,
    dragging: Option<Target>,
    tracking: bool,
    scroll: f32,
    message_scroll: f32,
    /// Keep the message caret in view at the next layout.
    follow: bool,
    reveal: Option<Target>,
    high_surrogate: Option<u16>,
    done: (mpsc::Sender<Done>, mpsc::Receiver<Done>),
    lay: Option<Lay>,
}

fn contains(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// `(x, y, height)` of the caret before `position` in `text`'s layout, whose
/// characters are `units`. At the end, measure the last character's trailing
/// edge (or the next line after a final line break).
fn caret_point(text: &Text, units: &[u16], position: usize, line: f32) -> (f32, f32, f32) {
    let at_end = position >= units.len() && position > 0;
    let (index, trailing) = if at_end {
        (position - 1, true)
    } else {
        (position, false)
    };
    let (mut x, mut y) = (0., 0.);
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    unsafe {
        let _ =
            text.layout
                .HitTestTextPosition(index as u32, trailing, &mut x, &mut y, &mut metrics);
    }
    let height = if metrics.height > 0. {
        metrics.height
    } else {
        line
    };
    if at_end && units[index] == u16::from(b'\n') {
        return (0., y + height, height);
    }
    (x, y, height)
}

/// The display position nearest `(x, y)` in `text`'s layout.
fn hit_point(text: &Text, x: f32, y: f32) -> usize {
    let (mut trailing, mut inside) = (BOOL(0), BOOL(0));
    let mut metrics = DWRITE_HIT_TEST_METRICS::default();
    unsafe {
        let _ = text
            .layout
            .HitTestPoint(x, y, &mut trailing, &mut inside, &mut metrics);
    }
    metrics.textPosition as usize
        + if trailing.as_bool() {
            metrics.length as usize
        } else {
            0
        }
}

impl Feedback {
    fn new(hwnd: HWND, options: &Options, g: Gfx, t: Tokens, live: bool) -> Self {
        let mut message = Editor::default();
        message.multiline = true;
        message.max_len = Some(copy::MESSAGE_LIMIT);
        let mut contact = TextField::new(Style::Input, copy::CONTACT_PLACEHOLDER);
        contact.editor.max_len = Some(copy::CONTACT_LIMIT);
        Self {
            hwnd,
            g,
            t,
            options: options.clone(),
            scale: 1.,
            size: (copy::WINDOW_WIDTH, copy::WINDOW_HEIGHT),
            composition: None,
            live,
            category: 0,
            message,
            contact,
            context: None,
            status: Status::Idle,
            focus: None,
            focus_visible: false,
            hover: None,
            pressed: None,
            dragging: None,
            tracking: false,
            scroll: 0.,
            message_scroll: 0.,
            follow: false,
            reveal: None,
            high_surrogate: None,
            done: mpsc::channel(),
            lay: None,
        }
    }

    fn sending(&self) -> bool {
        matches!(self.status, Status::Sending)
    }

    fn message_text(&self) -> String {
        String::from_utf16_lossy(self.message.units())
    }

    fn contact_text(&self) -> String {
        String::from_utf16_lossy(self.contact.editor.units())
    }

    fn can_send(&self) -> bool {
        self.live
            && self.context.is_some()
            && copy::can_submit(&self.message_text(), &self.contact_text(), self.sending())
    }

    fn enabled(&self, target: Target) -> bool {
        match target {
            Target::Category(_) => true,
            Target::Message | Target::Contact => !self.sending(),
            Target::Send => self.can_send(),
        }
    }

    /// Run `job` on a worker; its result arrives as `WM_DONE`.
    fn spawn(&self, job: impl FnOnce() -> Done + Send + 'static) {
        let send = self.done.0.clone();
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let _ = send.send(job());
            unsafe {
                let _ = PostMessageW(Some(HWND(hwnd as _)), WM_DONE, WPARAM(0), LPARAM(0));
            }
        });
    }

    fn submit(&mut self) {
        if !self.can_send() {
            return;
        }
        let contact = self.contact_text().trim().to_owned();
        let draft = FeedbackDraft {
            message: self.message_text().trim().to_owned(),
            contact: (!contact.is_empty()).then_some(contact),
            category: copy::CATEGORIES[self.category].id.to_owned(),
        };
        self.status = Status::Sending;
        if matches!(
            self.focus,
            Some(Target::Message | Target::Contact | Target::Send)
        ) {
            self.set_focus(None, false);
        }
        self.spawn(move || Done::Sent(captures_feedback::native::submit(draft)));
    }

    fn finish(&mut self) {
        while let Ok(done) = self.done.1.try_recv() {
            match done {
                Done::Context(context) => self.context = Some(context),
                Done::Sent(Ok(())) => {
                    // Shipping clears the message once it is sent.
                    self.message.set_text("");
                    self.message_scroll = 0.;
                    self.status = Status::Sent;
                }
                Done::Sent(Err(error)) => self.status = Status::Error(error),
            }
        }
    }

    fn set_focus(&mut self, target: Option<Target>, visible: bool) {
        self.focus = target;
        self.focus_visible = visible && target.is_some();
        if visible {
            self.reveal = target;
        }
        if !self.hwnd.is_invalid() {
            textfield::ime_enable(
                self.hwnd,
                matches!(target, Some(Target::Message | Target::Contact)),
            );
        }
    }

    fn activate(&mut self, target: Target) {
        match target {
            Target::Category(i) => self.category = i,
            Target::Send => self.submit(),
            _ => {}
        }
    }

    fn editor(&mut self, field: Target) -> &mut Editor {
        if field == Target::Message {
            &mut self.message
        } else {
            &mut self.contact.editor
        }
    }

    fn edited(&mut self, field: Target, change: Change) {
        if field == Target::Message {
            self.follow = true;
            if change.text && matches!(self.status, Status::Sent | Status::Error(_)) {
                self.status = Status::Idle;
            }
        }
    }

    // ------------------------------------------------------------ layout ----

    fn measure(&mut self) -> Result<()> {
        let (g, t) = (&self.g, &self.t);
        let n = |name: &str| t.n(name);
        let (w, h) = self.size;
        let shell = (w - 2. * n("s-8")).clamp(1., copy::SHELL_MAX_WIDTH);
        let left = n("s-8") + ((w - 2. * n("s-8")) - shell).max(0.) / 2.;
        let mut y = n("s-9");

        // `.feedback-header`
        let eyebrow = g.text_sized(
            t,
            &copy::EYEBROW.to_uppercase(),
            n("text-2xs"),
            "weight-semibold",
            Some("tracking-wide"),
        )?;
        let eyebrow_y = y;
        y += eyebrow.height + n("s-2");
        let title = g.text(t, Style::RecordingTitle, copy::TITLE, shell)?;
        let title_y = y;
        y += title.height + n("s-4");
        let intro_w = (56. * g.ch(t, Style::CardDescription)?).min(shell);
        let intro = g.text(t, Style::CardDescription, copy::INTRO, intro_w)?;
        let intro_y = y;
        y += intro.height + n("s-6");

        // `.feedback-meta-card`
        let pad = n("s-6");
        let inner = (shell - 2. - 2. * pad).max(1.);
        let x = left + 1. + pad;
        let meta_top = y;
        let mut cy = y + 1. + pad;
        let meta_title = g.text(t, Style::RowTitle, copy::META_TITLE, inner)?;
        let meta_title_y = cy;
        let mut dividers = Vec::new();
        cy += meta_title.height + n("s-4");
        dividers.push((x, x + inner, cy));
        cy += 1. + n("s-5");
        let (version, system) = match &self.context {
            Some(c) => (
                c.app_version.clone(),
                copy::system_label(&c.os, &c.os_version, &c.arch),
            ),
            None => (copy::LOADING.into(), copy::LOADING.into()),
        };
        let value_w = (inner - META_LABEL - n("s-4")).max(1.);
        let mut meta = Vec::new();
        for (i, (label, value)) in [
            (copy::APP_VERSION_LABEL, version),
            (copy::SYSTEM_LABEL, system),
        ]
        .into_iter()
        .enumerate()
        {
            if i > 0 {
                cy += n("s-3");
            }
            let dt = g.text(t, Style::Subtitle, label, META_LABEL)?;
            let dd = g.text(t, Style::Mono, &value, value_w)?;
            g.ellipsis(&dd, value_w)?;
            let row = cy;
            cy += dt.height.max(dd.height);
            meta.push((dt, dd, row));
        }
        let meta_card = rect(left, meta_top, left + shell, cy + pad + 1.);
        y = meta_card.bottom + n("s-6");

        // The form card: Category, Message and Contact.
        let form_top = y;
        let mut cy = y + 1. + pad;
        let category_label = g.text(t, Style::Button, copy::CATEGORY_LABEL, inner)?;
        let category_label_y = cy;
        cy += category_label.height + n("s-3");
        let cols = if app_windows::compact(w) { 2 } else { 3 };
        let gap = n("s-3");
        let col_w = (inner - gap * (cols - 1) as f32) / cols as f32;
        let cat_pad = n("s-4");
        let text_w = (col_w - 2. * cat_pad - 2.).max(1.);
        let mut texts = Vec::new();
        for category in copy::CATEGORIES {
            texts.push((
                g.text(t, Style::RowTitle, category.label, text_w)?,
                g.text(t, Style::Note, category.description, text_w)?,
            ));
        }
        let mut categories = Vec::new();
        for (row, chunk) in texts.chunks(cols).enumerate() {
            if row > 0 {
                cy += gap;
            }
            let height = chunk
                .iter()
                .map(|(a, b)| 2. + 2. * cat_pad + a.height + 3. + b.height)
                .fold(62_f32, f32::max);
            for (col, _) in chunk.iter().enumerate() {
                let cx = x + col as f32 * (col_w + gap);
                categories.push(rect(cx, cy, cx + col_w, cy + height));
            }
            cy += height;
        }
        let categories = texts
            .into_iter()
            .zip(categories)
            .map(|((a, b), r)| (a, b, r))
            .collect();
        cy += n("s-5");
        dividers.push((x, x + inner, cy));
        cy += 1. + n("s-5");

        let message_label = g.text(t, Style::Button, copy::MESSAGE_LABEL, inner)?;
        let message_label_y = cy;
        cy += message_label.height + n("s-3");
        let message_w = (inner - 2. - 2. * n("s-5")).max(1.);
        let display = String::from_utf16_lossy(&self.message.display());
        let message_text = g.text(t, Style::SetupLede, &display, message_w)?;
        let placeholder = copy::CATEGORIES[self.category].placeholder;
        let message_placeholder = g.text(t, Style::SetupLede, placeholder, message_w)?;
        let message_box = rect(x, cy, x + inner, cy + MESSAGE_HEIGHT);
        cy = message_box.bottom + n("s-5");
        dividers.push((x, x + inner, cy));
        cy += 1. + n("s-5");

        let contact_label = g.text(t, Style::Button, copy::CONTACT_LABEL, inner)?;
        let badge = g.text_sized(
            t,
            &copy::OPTIONAL_BADGE.to_uppercase(),
            n("text-2xs"),
            "weight-medium",
            Some("tracking-wide"),
        )?;
        let contact_label_y = cy;
        cy += contact_label.height.max(badge.height + 2.) + n("s-3");
        self.contact
            .relayout(g, t, (inner - 2. - 2. * n("s-5")).max(1.))?;
        let contact_h = n("h-lg").max(2. + 2. * n("s-4") + self.contact.line_height());
        let contact_box = rect(x, cy, x + inner, cy + contact_h);
        cy = contact_box.bottom + n("s-3");
        let help = g.text(t, Style::CardDescription, copy::CONTACT_HELP, inner)?;
        let help_y = cy;
        cy += help.height;
        let form_card = rect(left, form_top, left + shell, cy + pad + 1.);
        y = form_card.bottom + n("s-6");

        // `.feedback-actions`
        let send_label = g.text(
            t,
            Style::Strong,
            if self.sending() {
                copy::SENDING
            } else {
                copy::SEND
            },
            1e4,
        )?;
        let send_w = send_label.width + 2. * n("s-6") + 2.;
        let send_h = n("h-lg");
        let note = match &self.status {
            Status::Error(error) => Some((error.clone(), "danger-text", Some("danger-surface"))),
            Status::Sent => Some((copy::SENT.into(), "positive-text", Some("positive-surface"))),
            _ if !self.live => Some((copy::FIXTURE_DISABLED.into(), "text-subtle", None)),
            _ => None,
        };
        let note_w = (shell - send_w - n("s-5")).max(1.);
        let note = note
            .map(|(text, fg, bg)| {
                g.text(t, Style::CardDescription, &text, note_w - 2. * n("s-4"))
                    .map(|text| (text, fg, bg))
            })
            .transpose()?;
        let note_h = note
            .as_ref()
            .map_or(0., |(text, _, _)| text.height + 2. * n("s-3"));
        let footer_h = send_h.max(note_h);
        let send = rect(
            left + shell - send_w,
            y + (footer_h - send_h) / 2.,
            left + shell,
            y + (footer_h + send_h) / 2.,
        );
        let note = note.map(|(text, fg, bg)| {
            let top = y + (footer_h - note_h) / 2.;
            (text, rect(left, top, left + note_w, top + note_h), fg, bg)
        });
        let height = y + footer_h + n("s-11");

        // Keep the message caret in its box.
        let view = MESSAGE_HEIGHT - 2. - 2. * n("s-4");
        if std::mem::take(&mut self.follow) {
            let (_, top, line) = caret_point(
                &message_text,
                &self.message.display(),
                self.message.display_caret(),
                self.t.n("text-md") * self.t.n("leading-normal"),
            );
            if top < self.message_scroll {
                self.message_scroll = top;
            } else if top + line > self.message_scroll + view {
                self.message_scroll = top + line - view;
            }
            self.reveal = self.reveal.or(Some(Target::Message));
        }
        self.message_scroll = self
            .message_scroll
            .clamp(0., (message_text.height - view).max(0.));

        let lay = Lay {
            left,
            shell,
            eyebrow: (eyebrow, eyebrow_y),
            title: (title, title_y),
            intro: (intro, intro_y),
            meta_card,
            meta_title: (meta_title, meta_title_y),
            meta,
            dividers,
            form_card,
            category_label: (category_label, category_label_y),
            categories,
            message_label: (message_label, message_label_y),
            message_box,
            message_text,
            message_placeholder,
            contact_label: (contact_label, contact_label_y),
            badge,
            contact_box,
            help: (help, help_y),
            note,
            send_label,
            send,
            height,
        };
        if let Some(target) = self.reveal.take() {
            let r = Self::rect_of(&lay, target);
            let margin = n("s-6");
            if r.top - margin < self.scroll {
                self.scroll = r.top - margin;
            } else if r.bottom + margin > self.scroll + h {
                self.scroll = r.bottom + margin - h;
            }
        }
        self.scroll = self.scroll.clamp(0., (lay.height - h).max(0.));
        self.lay = Some(lay);
        Ok(())
    }

    fn rect_of(lay: &Lay, target: Target) -> D2D_RECT_F {
        match target {
            Target::Category(i) => lay.categories[i].2,
            Target::Message => lay.message_box,
            Target::Contact => lay.contact_box,
            Target::Send => lay.send,
        }
    }

    /// The target under a client point.
    fn hit(&self, x: f32, y: f32) -> Option<Target> {
        let lay = self.lay.as_ref()?;
        let y = y + self.scroll;
        ORDER
            .into_iter()
            .find(|&target| contains(&Self::rect_of(lay, target), x, y))
    }

    /// The message position under a client point.
    fn message_hit(&self, x: f32, y: f32) -> usize {
        let Some(lay) = &self.lay else { return 0 };
        let (ox, oy) = self.message_origin(lay);
        let position = hit_point(&lay.message_text, x - ox, y + self.scroll - oy);
        self.message.text_position(position)
    }

    /// The message text's origin in page coordinates.
    fn message_origin(&self, lay: &Lay) -> (f32, f32) {
        (
            lay.message_box.left + 1. + self.t.n("s-5"),
            lay.message_box.top + 1. + self.t.n("s-4") - self.message_scroll,
        )
    }

    fn contact_hit(&self, x: f32) -> usize {
        let Some(lay) = &self.lay else { return 0 };
        self.contact
            .hit(x - lay.contact_box.left - 1. - self.t.n("s-5"))
    }

    /// Up or Down in the message: the same x on the line above or below.
    fn vertical(&mut self, up: bool, shift: bool) {
        let Some(lay) = &self.lay else { return };
        if self.message.composing() {
            return;
        }
        let text = &lay.message_text;
        let (x, y, line) = caret_point(
            text,
            &self.message.display(),
            self.message.display_caret(),
            self.t.n("text-md") * self.t.n("leading-normal"),
        );
        let target = if up { y - line / 2. } else { y + line * 1.5 };
        let position = if target < 0. {
            0
        } else if target > text.height {
            self.message.len()
        } else {
            self.message.text_position(hit_point(text, x, target))
        };
        let change = self.message.move_to(position, shift);
        self.edited(Target::Message, change);
    }

    // ------------------------------------------------------ automation ----

    /// The automation key of a target.
    fn uia_key(target: Target) -> u32 {
        match target {
            Target::Category(i) => 10 + i as u32,
            Target::Message => 20,
            Target::Contact => 21,
            Target::Send => 40,
        }
    }

    fn uia_target(key: u32) -> Option<Target> {
        ORDER.into_iter().find(|&t| Self::uia_key(t) == key)
    }

    /// The parts in shipping's DOM order: the `<h1>` and intro, the
    /// `<h2>` details card, the category `radiogroup`, the message
    /// `<textarea>`, the contact `<input>`, the status or alert line and
    /// Send, in client coordinates.
    fn uia_items(&self) -> Vec<Item> {
        let Some(lay) = &self.lay else {
            return Vec::new();
        };
        let n = |name: &str| self.t.n(name);
        let at = |x: f32, (text, y): &(Text, f32)| {
            rect(
                x,
                y - self.scroll,
                x + text.width,
                y + text.height - self.scroll,
            )
        };
        let page =
            |r: D2D_RECT_F| rect(r.left, r.top - self.scroll, r.right, r.bottom - self.scroll);
        let x = lay.left + 1. + n("s-6");
        let mut items = vec![
            Item::new(0, Role::Heading(1), copy::TITLE, at(lay.left, &lay.title)),
            Item::new(1, Role::Text, copy::INTRO, at(lay.left, &lay.intro)),
            Item::new(
                2,
                Role::Heading(2),
                copy::META_TITLE,
                at(x, &lay.meta_title),
            ),
        ];
        let (version, system) = match &self.context {
            Some(c) => (
                c.app_version.clone(),
                copy::system_label(&c.os, &c.os_version, &c.arch),
            ),
            None => (copy::LOADING.into(), copy::LOADING.into()),
        };
        for (i, ((label, value), (dt, dd, y))) in [
            (copy::APP_VERSION_LABEL, version),
            (copy::SYSTEM_LABEL, system),
        ]
        .into_iter()
        .zip(&lay.meta)
        .enumerate()
        {
            let line = dt.height.max(dd.height);
            let r = rect(
                x,
                y - self.scroll,
                lay.meta_card.right - 1. - n("s-6"),
                y + line - self.scroll,
            );
            items.push(Item::new(
                3 + i as u32,
                Role::Text,
                format!("{label}: {value}"),
                r,
            ));
        }
        let group = lay
            .categories
            .iter()
            .map(|(_, _, r)| *r)
            .reduce(|a, b| {
                rect(
                    a.left.min(b.left),
                    a.top.min(b.top),
                    a.right.max(b.right),
                    a.bottom.max(b.bottom),
                )
            })
            .unwrap_or(lay.form_card);
        items.push(Item::new(5, Role::Group, "Feedback category", page(group)));
        for (i, (_, _, r)) in lay.categories.iter().enumerate() {
            let category = &copy::CATEGORIES[i];
            let target = Target::Category(i);
            let mut radio = Item::new(Self::uia_key(target), Role::Radio, category.label, page(*r));
            radio.parent = Some(5);
            radio.help = category.description.into();
            radio.selected = self.category == i;
            radio.focused = self.focus == Some(target);
            items.push(radio);
        }
        for (target, name, help, value, r) in [
            (
                Target::Message,
                copy::MESSAGE_LABEL,
                copy::CATEGORIES[self.category].placeholder,
                self.message_text(),
                lay.message_box,
            ),
            (
                Target::Contact,
                copy::CONTACT_LABEL,
                copy::CONTACT_HELP,
                self.contact_text(),
                lay.contact_box,
            ),
        ] {
            let mut edit = Item::new(Self::uia_key(target), Role::Edit, name, page(r));
            edit.help = help.into();
            edit.value = Some(value);
            edit.enabled = self.enabled(target);
            edit.focused = self.focus == Some(target);
            items.push(edit);
        }
        if let Some((text, r, _, _)) = &lay.note {
            let role = match self.status {
                Status::Error(_) => Role::Alert,
                Status::Sent => Role::Status,
                _ => Role::Text,
            };
            let name = match &self.status {
                Status::Error(error) => error.clone(),
                Status::Sent => copy::SENT.into(),
                _ => copy::FIXTURE_DISABLED.into(),
            };
            let _ = text;
            items.push(Item::new(30, role, name, page(*r)));
        }
        let mut send = Item::new(
            40,
            Role::Button,
            if self.sending() {
                copy::SENDING
            } else {
                copy::SEND
            },
            page(lay.send),
        );
        send.enabled = self.can_send();
        send.focused = self.focus == Some(Target::Send);
        items.push(send);
        items
    }

    /// Apply an automation request through the paths clicks and typing use.
    fn uia_act(&mut self, key: u32, act: Act) {
        let Some(target) = Self::uia_target(key) else {
            return;
        };
        match act {
            Act::Invoke | Act::Select => self.activate(target),
            Act::SetValue(text) if matches!(target, Target::Message | Target::Contact) => {
                let change = self.editor(target).set_text(&text);
                self.edited(target, change);
            }
            Act::Focus => {
                self.set_focus(Some(target), true);
                unsafe {
                    let _ = SetFocus(Some(self.hwnd));
                }
            }
            _ => {}
        }
        let _ = self.render();
    }

    // ------------------------------------------------------------ paint ----

    fn resize(&mut self) -> Result<()> {
        let Some(dcomp) = self.g.dcomp.clone() else {
            return Ok(());
        };
        let surface = self.g.surface(self.pixels().0, self.pixels().1)?;
        unsafe {
            if self.composition.is_none() {
                let target = dcomp.CreateTargetForHwnd(self.hwnd, true)?;
                let visual = dcomp.CreateVisual()?;
                target.SetRoot(&visual)?;
                self.composition = Some((target, visual, surface.clone()));
            }
            if let Some((_, visual, current)) = self.composition.as_mut() {
                visual.SetContent(&surface)?;
                *current = surface;
            }
        }
        self.render()
    }

    fn pixels(&self) -> (u32, u32) {
        (
            (self.size.0 * self.scale).round().max(1.) as u32,
            (self.size.1 * self.scale).round().max(1.) as u32,
        )
    }

    fn render(&mut self) -> Result<()> {
        self.measure()?;
        let Some((_, _, surface)) = self.composition.clone() else {
            return Ok(());
        };
        let px = self.pixels();
        let scale = (
            px.0 as f32 / self.size.0.max(1.),
            px.1 as f32 / self.size.1.max(1.),
        );
        gfx::draw_surface(&surface, px, scale, 0., |dc| self.paint(dc))?;
        unsafe { self.g.dcomp.as_ref().ok_or_else(Error::empty)?.Commit() }
    }

    /// Paint the window from the last layout.
    pub fn paint(&self, dc: &ID2D1DeviceContext) -> Result<()> {
        let Some(lay) = &self.lay else { return Ok(()) };
        let (w, h) = self.size;
        gfx::fill(dc, rect(0., 0., w, h), self.t.color("surface-canvas"))?;
        let mut base = Matrix3x2::identity();
        unsafe {
            dc.GetTransform(&mut base);
            dc.SetTransform(&(Matrix3x2::translation(0., -self.scroll) * base));
        }
        let result = self.paint_page(dc, lay, base);
        unsafe { dc.SetTransform(&base) };
        result
    }

    fn ring(&self, dc: &ID2D1DeviceContext, r: D2D_RECT_F, radius: f32, full: bool) -> Result<()> {
        let accent = self.t.color_alpha("theme-accent", 0.55);
        if full {
            gfx::ring(dc, r, radius, 0., 2., self.t.color("surface-canvas"))?;
            gfx::ring(dc, r, radius, 2., 4., accent)
        } else {
            gfx::ring(dc, r, radius, 0., 2., accent)
        }
    }

    fn paint_page(&self, dc: &ID2D1DeviceContext, lay: &Lay, base: Matrix3x2) -> Result<()> {
        let t = &self.t;
        let c = |name: &str| t.color(name);
        let n = |name: &str| t.n(name);
        let left = lay.left;
        gfx::text(dc, &lay.eyebrow.0, left, lay.eyebrow.1, c("text-subtle"))?;
        gfx::text(dc, &lay.title.0, left, lay.title.1, c("text"))?;
        gfx::text(dc, &lay.intro.0, left, lay.intro.1, c("text-subtle"))?;

        for card in [lay.meta_card, lay.form_card] {
            gfx::shadow(dc, card, n("r-xl"), t.shadow("shadow-sm"))?;
            gfx::fill_round(dc, card, n("r-xl"), c("surface-raised"))?;
            gfx::border(dc, card, n("r-xl"), 1., c("border-subtle"))?;
        }
        let x = left + 1. + n("s-6");
        for &(a, b, y) in &lay.dividers {
            gfx::fill(dc, rect(a, y, b, y + 1.), c("border-subtle"))?;
        }
        gfx::text(dc, &lay.meta_title.0, x, lay.meta_title.1, c("text"))?;
        for (dt, dd, y) in &lay.meta {
            let line = dt.height.max(dd.height);
            gfx::text(dc, dt, x, y + (line - dt.height) / 2., c("text-subtle"))?;
            let vx = x + META_LABEL + n("s-4");
            gfx::text(dc, dd, vx, y + (line - dd.height) / 2., c("text"))?;
        }

        gfx::text(
            dc,
            &lay.category_label.0,
            x,
            lay.category_label.1,
            c("text-muted"),
        )?;
        let radius = n("r-md");
        for (i, (strong, small, r)) in lay.categories.iter().enumerate() {
            let active = self.category == i;
            let hover = self.hover == Some(Target::Category(i));
            let (fill, border, width) = if active {
                ("surface-selected", "theme-accent", 2.)
            } else if hover {
                ("surface-hover", "border-strong", 1.)
            } else {
                ("surface-canvas", "border", 1.)
            };
            gfx::fill_round(dc, *r, radius, c(fill))?;
            gfx::border(dc, *r, radius, width, c(border))?;
            let (tx, ty) = (r.left + 1. + n("s-4"), r.top + 1. + n("s-4"));
            gfx::text(dc, strong, tx, ty, c("text"))?;
            gfx::text(dc, small, tx, ty + strong.height + 3., c("text-subtle"))?;
            if self.focus_visible && self.focus == Some(Target::Category(i)) {
                self.ring(dc, *r, radius, true)?;
            }
        }

        // Fields: `.ui-input` with hover, focus and the placeholder.
        let field = |target: Target, r: D2D_RECT_F| -> Result<()> {
            let focused = self.focus == Some(target);
            let border = if focused {
                "theme-accent"
            } else if self.hover == Some(target) && !self.sending() {
                "border-strong"
            } else {
                "control-border"
            };
            gfx::fill_round(dc, r, radius, c("surface-field"))?;
            gfx::border(dc, r, radius, 1., c(border))?;
            if focused {
                self.ring(dc, r, radius, false)?;
            }
            Ok(())
        };
        gfx::text(
            dc,
            &lay.message_label.0,
            x,
            lay.message_label.1,
            c("text-muted"),
        )?;
        let r = lay.message_box;
        field(Target::Message, r)?;
        let clip = rect(r.left + 1., r.top + 1., r.right - 1., r.bottom - 1.);
        unsafe { dc.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED) };
        let drawn = self.paint_message(dc, lay);
        unsafe { dc.PopAxisAlignedClip() };
        drawn?;

        let (label, ly) = (&lay.contact_label.0, lay.contact_label.1);
        let line = label.height.max(lay.badge.height + 2.);
        gfx::text(
            dc,
            label,
            x,
            ly + (line - label.height) / 2.,
            c("text-muted"),
        )?;
        let bx = x + label.width + n("s-2");
        let badge = rect(
            bx,
            ly + (line - lay.badge.height - 2.) / 2.,
            bx + lay.badge.width + 2. * n("s-3"),
            ly + (line + lay.badge.height + 2.) / 2.,
        );
        gfx::fill_round(
            dc,
            badge,
            (badge.bottom - badge.top) / 2.,
            c("surface-sunken"),
        )?;
        gfx::text(
            dc,
            &lay.badge,
            bx + n("s-3"),
            badge.top + 1.,
            c("text-subtle"),
        )?;
        let r = lay.contact_box;
        field(Target::Contact, r)?;
        let focused = self.focus == Some(Target::Contact);
        let tx = r.left + 1. + n("s-5");
        let box_w = r.right - r.left - 2. - 2. * n("s-5");
        unsafe {
            let at = Matrix3x2::translation(0., r.top - self.scroll) * base;
            dc.SetTransform(&at);
        }
        let drawn = self
            .contact
            .draw(dc, t, (tx, box_w, r.bottom - r.top), focused)
            .and_then(|()| {
                let (a, b) = self.contact.editor.selection();
                if focused && a == b {
                    let (cx, cy, ch) = self.contact.caret_box(tx, r.bottom - r.top);
                    gfx::fill(dc, rect(cx, cy, cx + 1., cy + ch), c("text"))?;
                }
                Ok(())
            });
        unsafe { dc.SetTransform(&(Matrix3x2::translation(0., -self.scroll) * base)) };
        drawn?;
        gfx::text(dc, &lay.help.0, x, lay.help.1, c("text-subtle"))?;

        if let Some((text, r, fg, bg)) = &lay.note {
            if let Some(bg) = bg {
                gfx::fill_round(dc, *r, radius, c(bg))?;
            }
            gfx::text(dc, text, r.left + n("s-4"), r.top + n("s-3"), c(fg))?;
        }
        // `.feedback-submit`: the accent action.
        let enabled = self.can_send();
        let alpha = if enabled { 1. } else { 0.45 };
        let fill = if enabled && self.hover == Some(Target::Send) {
            "theme-accent-hover"
        } else {
            "theme-accent"
        };
        if enabled {
            gfx::shadow(dc, lay.send, radius, t.shadow("shadow-sm"))?;
        }
        gfx::fill_round(dc, lay.send, radius, t.color_alpha(fill, alpha))?;
        let label = &lay.send_label;
        gfx::text(
            dc,
            label,
            lay.send.left + (lay.send.right - lay.send.left - label.width) / 2.,
            lay.send.top + (lay.send.bottom - lay.send.top - label.height) / 2.,
            t.color_alpha("theme-accent-ink", alpha),
        )?;
        if self.focus_visible && self.focus == Some(Target::Send) {
            self.ring(dc, lay.send, radius, true)?;
        }
        let _ = lay.shell;
        Ok(())
    }

    fn paint_message(&self, dc: &ID2D1DeviceContext, lay: &Lay) -> Result<()> {
        let c = |name: &str| self.t.color(name);
        let (ox, oy) = self.message_origin(lay);
        if self.message.len() == 0 && !self.message.composing() {
            gfx::text(dc, &lay.message_placeholder, ox, oy, c("text-faint"))?;
        } else {
            gfx::text(dc, &lay.message_text, ox, oy, c("text"))?;
        }
        if self.focus != Some(Target::Message) {
            return Ok(());
        }
        let (a, b) = self.message.selection();
        if a != b && !self.message.composing() {
            let mut buffer = [DWRITE_HIT_TEST_METRICS::default(); 64];
            let mut count = 0;
            let ok = unsafe {
                lay.message_text.layout.HitTestTextRange(
                    a as u32,
                    (b - a) as u32,
                    ox,
                    oy,
                    Some(&mut buffer),
                    &mut count,
                )
            };
            if ok.is_ok() {
                // `::selection`: accent fill, accent ink over it.
                for m in &buffer[..(count as usize).min(64)] {
                    let r = rect(m.left, m.top, m.left + m.width.max(3.), m.top + m.height);
                    gfx::fill(dc, r, c("theme-accent"))?;
                    unsafe { dc.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) };
                    let drawn = gfx::text(dc, &lay.message_text, ox, oy, c("theme-accent-ink"));
                    unsafe { dc.PopAxisAlignedClip() };
                    drawn?;
                }
            }
        } else {
            let (x, y, h) = caret_point(
                &lay.message_text,
                &self.message.display(),
                self.message.display_caret(),
                self.t.n("text-md") * self.t.n("leading-normal"),
            );
            gfx::fill(dc, rect(ox + x, oy + y, ox + x + 1., oy + y + h), c("text"))?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ input ----

    fn key(&mut self, key: VIRTUAL_KEY) {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        if key == VK_TAB {
            let order: Vec<_> = ORDER.into_iter().filter(|&t| self.enabled(t)).collect();
            let i = self.focus.and_then(|t| order.iter().position(|&x| x == t));
            let next = i.map_or(if shift { order.len() - 1 } else { 0 }, |i| {
                if shift {
                    (i + order.len() - 1) % order.len()
                } else {
                    (i + 1) % order.len()
                }
            });
            self.set_focus(Some(order[next]), true);
            return;
        }
        let Some(focus) = self.focus else { return };
        if matches!(focus, Target::Category(_) | Target::Send) {
            if key == VK_RETURN || key == VK_SPACE {
                self.focus_visible = true;
                self.activate(focus);
            }
            return;
        }
        if self.sending() {
            return;
        }
        let edit = match key {
            VK_LEFT => Some(Key::Left),
            VK_RIGHT => Some(Key::Right),
            VK_HOME => Some(Key::Home),
            VK_END => Some(Key::End),
            VK_BACK => Some(Key::Backspace),
            VK_DELETE => Some(Key::Delete),
            VK_A if ctrl => Some(Key::SelectAll),
            VK_Z if ctrl => Some(if shift { Key::Redo } else { Key::Undo }),
            VK_Y if ctrl => Some(Key::Redo),
            _ => None,
        };
        let change = if let Some(edit) = edit {
            self.editor(focus).key(edit, shift, ctrl)
        } else if focus == Target::Message && (key == VK_UP || key == VK_DOWN) {
            return self.vertical(key == VK_UP, shift);
        } else if focus == Target::Message && key == VK_RETURN {
            self.message.insert("\n")
        } else if ctrl && key == VK_C {
            textfield::copy(self.hwnd, &self.editor(focus).selected_text());
            return;
        } else if ctrl && key == VK_X {
            match self.editor(focus).cut() {
                Some(text) => {
                    textfield::copy(self.hwnd, &text);
                    Change {
                        text: true,
                        caret: true,
                    }
                }
                None => return,
            }
        } else if ctrl && key == VK_V {
            match textfield::paste(self.hwnd) {
                Some(text) => self.editor(focus).insert(&text),
                None => return,
            }
        } else {
            return;
        };
        self.edited(focus, change);
    }

    fn char(&mut self, unit: u16) {
        let Some(field @ (Target::Message | Target::Contact)) = self.focus else {
            return;
        };
        if self.sending() {
            return;
        }
        if (0xd800..=0xdbff).contains(&unit) {
            self.high_surrogate = Some(unit);
            return;
        }
        if unit < 32 || unit == 127 {
            return;
        }
        let mut units = Vec::new();
        units.extend(self.high_surrogate.take());
        units.push(unit);
        let change = self.editor(field).insert(&String::from_utf16_lossy(&units));
        self.edited(field, change);
    }

    fn pointer(&mut self, msg: u32, x: f32, y: f32) {
        let hit = self.hit(x, y);
        if msg == WM_MOUSEMOVE {
            self.hover = hit;
            if !self.tracking {
                unsafe {
                    let _ = TrackMouseEvent(&mut TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        ..Default::default()
                    });
                }
                self.tracking = true;
            }
            match self.dragging {
                Some(Target::Message) => {
                    let at = self.message_hit(x, y);
                    let change = self.message.move_to(at, true);
                    self.edited(Target::Message, change);
                }
                Some(Target::Contact) => {
                    let at = self.contact_hit(x);
                    self.contact.editor.move_to(at, true);
                }
                _ => {}
            }
            return;
        }
        if msg == WM_LBUTTONUP {
            let pressed = self.pressed.take();
            self.dragging = None;
            unsafe {
                let _ = ReleaseCapture();
            }
            if let Some(target) = pressed
                && pressed == hit
            {
                self.activate(target);
            }
            return;
        }
        // Down or double-click.
        let Some(target) = hit.filter(|&t| self.enabled(t)) else {
            return;
        };
        unsafe {
            let _ = SetFocus(Some(self.hwnd));
            SetCapture(self.hwnd);
        }
        self.set_focus(Some(target), false);
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        let at = match target {
            Target::Message => self.message_hit(x, y),
            Target::Contact => self.contact_hit(x),
            _ => {
                self.pressed = Some(target);
                return;
            }
        };
        self.dragging = Some(target);
        let editor = self.editor(target);
        let change = if msg == WM_LBUTTONDBLCLK {
            let (a, b) = editor.word_at(at);
            editor.select(a, b)
        } else {
            editor.move_to(at, shift)
        };
        self.edited(target, change);
    }

    fn wheel(&mut self, delta: f32, x: f32, y: f32) {
        if self.hit(x, y) == Some(Target::Message)
            && let Some(lay) = &self.lay
        {
            let view = MESSAGE_HEIGHT - 2. - 2. * self.t.n("s-4");
            let max = (lay.message_text.height - view).max(0.);
            let next = (self.message_scroll - delta).clamp(0., max);
            if next != self.message_scroll {
                self.message_scroll = next;
                return;
            }
        }
        self.scroll -= delta;
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
        match msg {
            WM_DONE => self.finish(),
            WM_SIZE if wp.0 != SIZE_MINIMIZED as usize => {
                self.size = (
                    (lp.0 as u32 & 0xffff) as f32 / self.scale,
                    ((lp.0 as u32 >> 16) & 0xffff) as f32 / self.scale,
                );
                let _ = self.resize();
                return Some(LRESULT(0));
            }
            WM_SETTINGCHANGE | WM_THEMECHANGED => {
                let settings =
                    captures_settings::load(&self.options.settings_file).unwrap_or_default();
                self.t = appearance::tokens(self.options.appearance.as_deref(), &settings);
                appearance::title_bar(self.hwnd, &self.t);
            }
            WM_GETMINMAXINFO => unsafe {
                let (x, y) = crate::host::window_size(
                    self.hwnd,
                    copy::WINDOW_MIN_WIDTH,
                    copy::WINDOW_MIN_HEIGHT,
                );
                (*(lp.0 as *mut MINMAXINFO)).ptMinTrackSize = POINT { x, y };
                return None;
            },
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_LBUTTONUP => {
                let (x, y) = (
                    (lp.0 as u16 as i16) as f32 / self.scale,
                    ((lp.0 >> 16) as u16 as i16) as f32 / self.scale,
                );
                let before = (self.hover, self.focus, self.dragging.is_some());
                self.pointer(msg, x, y);
                // A plain move over nothing changes nothing worth painting.
                if msg == WM_MOUSEMOVE
                    && before == (self.hover, self.focus, false)
                    && self.dragging.is_none()
                {
                    return Some(LRESULT(0));
                }
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hover = None;
            }
            WM_CAPTURECHANGED => {
                self.pressed = None;
                self.dragging = None;
            }
            WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => unsafe {
                let cursor = match self.hover {
                    Some(Target::Message | Target::Contact) if !self.sending() => IDC_IBEAM,
                    Some(Target::Category(_)) => IDC_HAND,
                    Some(Target::Send) if self.can_send() => IDC_HAND,
                    _ => IDC_ARROW,
                };
                SetCursor(LoadCursorW(None, cursor).ok());
                return Some(LRESULT(1));
            },
            WM_MOUSEWHEEL => {
                let mut point = POINT {
                    x: lp.0 as u16 as i16 as i32,
                    y: (lp.0 >> 16) as u16 as i16 as i32,
                };
                unsafe {
                    let _ = ScreenToClient(self.hwnd, &mut point);
                }
                let delta =
                    (wp.0 >> 16) as u16 as i16 as f32 / 120. * appearance::wheel_lines() * 100.
                        / 3.;
                self.wheel(
                    delta,
                    point.x as f32 / self.scale,
                    point.y as f32 / self.scale,
                );
            }
            WM_KEYDOWN => self.key(VIRTUAL_KEY(wp.0 as u16)),
            WM_CHAR => self.char(wp.0 as u16),
            WM_IME_COMPOSITION => {
                let Some(field @ (Target::Message | Target::Contact)) = self.focus else {
                    return None;
                };
                let hwnd = self.hwnd;
                let change = textfield::ime_composition(hwnd, lp.0 as u32, self.editor(field));
                self.edited(field, change);
            }
            WM_IME_ENDCOMPOSITION => {
                if let Some(field @ (Target::Message | Target::Contact)) = self.focus {
                    let change = self.editor(field).set_composition(&[], 0);
                    self.edited(field, change);
                }
            }
            WM_KILLFOCUS => self.set_focus(None, false),
            _ => return None,
        }
        let _ = self.render();
        Some(LRESULT(0))
    }
}

/// Send Feedback's parts for UI Automation, while the window is open.
pub fn uia_snapshot() -> Option<panel_uia::Snapshot> {
    with_feedback(|f| panel_uia::Snapshot {
        hwnd: f.hwnd,
        scale: f.scale,
        size: f.size,
        name: copy::WINDOW_TITLE,
        focusable: true,
        items: f.uia_items(),
    })
}

pub fn uia_act(key: u32, act: Act) -> Option<()> {
    with_feedback(|f| f.uia_act(key, act))
}

pub fn register(instance: HINSTANCE) -> Result<()> {
    unsafe {
        RegisterClassW(&WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        });
    }
    Ok(())
}

/// Open Send Feedback, or bring it forward when it is already open.
pub fn open(options: &Options) -> Result<()> {
    if let Some(hwnd) = with_feedback(|f| f.hwnd) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(());
    }
    let settings = captures_settings::load(&options.settings_file).unwrap_or_default();
    let tokens = appearance::tokens(options.appearance.as_deref(), &settings);
    let g = Gfx::new(&tokens, true)?;
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP,
            CLASS,
            &HSTRING::from(copy::WINDOW_TITLE),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            copy::WINDOW_WIDTH as i32,
            copy::WINDOW_HEIGHT as i32,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let mut feedback = Feedback::new(hwnd, options, g, tokens, options.live);
        feedback.scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.;
        appearance::title_bar(hwnd, &feedback.t);
        textfield::ime_enable(hwnd, false);
        // Shipping loads the context after the window shows.
        feedback.spawn(|| Done::Context(captures_feedback::native::context()));
        FEEDBACK.with(|f| *f.borrow_mut() = Some(feedback));
        let (w, h) = options
            .size
            .filter(|_| options.scene == crate::Scene::Feedback)
            .map_or((copy::WINDOW_WIDTH, copy::WINDOW_HEIGHT), |(w, h)| {
                (w as f32, h as f32)
            });
        let (ww, wh) = crate::host::window_size(hwnd, w, h);
        // Sizing sends WM_SIZE, which creates the surface and paints.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            ww,
            wh,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// `--scene feedback --screenshot`: the fixture form; `--select` takes
/// `typed` (a message and contact, the message focused), `live` (typed with
/// Send enabled), `sent`, `error` and `idea`.
pub fn render_offscreen(
    options: &Options,
    g: Gfx,
    t: Tokens,
    size: (f32, f32),
) -> Result<Feedback> {
    let select = options.select.as_deref().unwrap_or_default();
    let mut feedback = Feedback::new(HWND::default(), options, g, t, select == "live");
    feedback.size = size;
    feedback.context = Some(captures_feedback::native::context());
    if matches!(select, "typed" | "live" | "error") {
        feedback.message.set_text(
            "The capture bar opened on the wrong display after I unplugged my monitor.\n\nIt happens every time.",
        );
        feedback.contact.editor.set_text("@jose");
    }
    match select {
        "typed" => feedback.focus = Some(Target::Message),
        "idea" => feedback.category = 1,
        "sent" => feedback.status = Status::Sent,
        "error" => {
            feedback.status =
                Status::Error("Feedback could not be sent: network unavailable".into())
        }
        _ => {}
    }
    feedback.measure()?;
    Ok(feedback)
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        // Commit a composition before a click or close moves on.
        if matches!(msg, WM_LBUTTONDOWN | WM_CLOSE | WM_KILLFOCUS)
            && with_feedback(|f| f.message.composing() || f.contact.editor.composing())
                == Some(true)
        {
            textfield::ime_complete(hwnd);
        }
        match msg {
            WM_GETOBJECT if lp.0 as i32 == UiaRootObjectId => {
                return UiaReturnRawElementProvider(
                    hwnd,
                    wp,
                    lp,
                    &panel_uia::node(panel_uia::Window::Feedback, None),
                );
            }
            WM_DESTROY => {
                let _ = UiaReturnRawElementProvider(hwnd, WPARAM(0), LPARAM(0), None);
                FEEDBACK.with(|f| {
                    if let Ok(mut f) = f.try_borrow_mut() {
                        f.take();
                    }
                });
                if !crate::host::exists()
                    && !crate::history::exists()
                    && !crate::editor::exists()
                    && !crate::recording::exists()
                    && !crate::setup::exists()
                    && !crate::live::resident()
                {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            WM_ERASEBKGND => return LRESULT(1),
            WM_PAINT => {
                let _ = ValidateRect(Some(hwnd), None);
                return LRESULT(0);
            }
            WM_IME_SETCONTEXT => {
                return DefWindowProcW(
                    hwnd,
                    msg,
                    wp,
                    LPARAM(
                        lp.0 & !(windows::Win32::UI::Input::Ime::ISC_SHOWUICOMPOSITIONWINDOW
                            as isize),
                    ),
                );
            }
            WM_DPICHANGED => {
                with_feedback(|f| f.scale = (wp.0 & 0xffff) as f32 / 96.);
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                return LRESULT(0);
            }
            _ => {}
        }
        let before = panel_uia::watch(panel_uia::Window::Feedback);
        let handled = with_feedback(|f| f.handle(msg, wp, lp)).flatten();
        if handled.is_some() {
            panel_uia::raise(panel_uia::Window::Feedback, before);
        }
        handled.unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
    }
}
