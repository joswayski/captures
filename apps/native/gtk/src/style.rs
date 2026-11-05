//! Captures styling for GTK widgets, generated from the shipping tokens.
//!
//! Every Captures class starts from `all: unset`, so the desktop's GTK theme
//! never leaks into the product chrome; only text rendering, input and
//! accessibility come from GTK. Rules mirror `styles/windows.css` and
//! `styles/primitives.css`, written with `var(--token)` and resolved here
//! because GTK 4.6 has no CSS custom properties. GTK `min-height` sizes the
//! content box, so border-box heights from shipping subtract padding and
//! borders.
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gdk;

use crate::switch::TokenSwitch;
use crate::tokens::Tokens;

const CSS: &str = r#"
window.captures {
  background: var(--surface-canvas);
  color: var(--text);
}
.captures label { all: unset; }
.captures .preferences-nav {
  padding: var(--s-6) var(--s-5);
  border-right: 1px solid var(--border-subtle);
  background: var(--surface-sunken);
}
.captures .preferences-nav-brand { padding: 0 var(--s-3); }
.captures .brand-mark {
  min-width: 26px;
  min-height: 26px;
  border-radius: var(--r-md);
  color: var(--theme-accent-ink);
  background: var(--theme-accent);
}
.captures .brand-title {
  font-size: var(--text-md);
  font-weight: var(--weight-semibold);
  letter-spacing: var(--brand-tracking);
}
.captures button.nav-item {
  all: unset;
  min-height: var(--h-md);
  padding: 0 var(--s-4);
  border-radius: var(--r-md);
  color: var(--text-subtle);
  font-size: var(--text-md);
  font-weight: var(--weight-medium);
  transition: color var(--dur-2), background-color var(--dur-2);
}
.captures button.nav-item:hover { color: var(--text); background: var(--surface-hover); }
.captures button.nav-item.active { color: var(--text); background: var(--surface-active); }
.captures button.nav-item:focus-visible { box-shadow: inset 0 0 0 2px var(--focus); }
.captures .preferences-header {
  min-height: 38px;
  padding: var(--s-6) var(--s-8);
  border-bottom: 1px solid var(--border-subtle);
  background: var(--surface-canvas);
}
.captures button.preferences-history-button {
  all: unset;
  min-height: var(--h-md);
  padding: 0 var(--s-5);
  border: 1px solid var(--border);
  border-radius: var(--r-md);
  color: var(--text);
  background: var(--surface-raised);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  transition: background-color var(--dur-2);
}
.captures button.preferences-history-button:hover { background: var(--surface-hover); }
.captures button.preferences-history-button:focus-visible { box-shadow: 0 0 0 3px var(--focus); }
.captures .header-title {
  font-size: var(--text-xl);
  font-weight: var(--weight-semibold);
  letter-spacing: var(--header-tracking);
}
.captures .header-description {
  margin-top: 2px;
  color: var(--text-subtle);
  font-size: var(--text-sm);
}
.captures .preferences-sections { padding: var(--s-8) var(--s-8) var(--s-12); }
.captures .settings-card {
  padding: var(--s-6);
  border: 1px solid var(--border-subtle);
  border-radius: var(--r-xl);
  background: var(--surface-raised);
  box-shadow: var(--shadow-sm);
}
.captures .card-title {
  font-size: var(--text-lg);
  font-weight: var(--weight-semibold);
  letter-spacing: var(--card-tracking);
}
.captures .card-description, .captures .setting-description {
  color: var(--text-subtle);
  font-size: var(--text-sm);
  line-height: var(--leading-snug);
}
.captures .setting-row { padding-top: var(--s-5); border-top: 1px solid var(--border-subtle); }
.captures .setting-row.first { padding-top: var(--s-1); border-top-width: 0; }
.captures .setting-title {
  color: var(--text);
  font-size: var(--text-md);
  font-weight: var(--weight-medium);
}
.captures .setting-row.disabled .setting-copy { opacity: 0.55; }

/* `.ui-segmented`: the thumb is drawn by the widget so it can slide. */
.captures .ui-segmented {
  padding: 3px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--r-lg);
  background: var(--surface-sunken);
}
.captures .ui-segmented > button {
  all: unset;
  min-height: var(--h-sm);
  padding: 0 var(--s-5);
  border-radius: var(--r-sm);
  color: var(--text-subtle);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  transition: color var(--dur-3) var(--ease-standard);
}
.captures .ui-segmented > button:hover,
.captures .ui-segmented > button.active { color: var(--text); }
.captures .ui-segmented > button:focus-visible { box-shadow: 0 0 0 2px var(--focus); }

/* `.theme-options` accent chips. */
.captures button.theme-option {
  all: unset;
  min-height: 32px;
  padding: 0 var(--s-3);
  border: 1px solid transparent;
  border-radius: var(--r-md);
  color: var(--text-muted);
  transition: border-color var(--dur-2) var(--ease-standard),
    background-color var(--dur-2) var(--ease-standard),
    box-shadow var(--dur-2) var(--ease-standard);
}
.captures button.theme-option:hover { background: var(--surface-hover); }
.captures button.theme-option.active {
  border-color: var(--border);
  color: var(--text);
  background: var(--surface-raised);
  box-shadow: var(--shadow-xs);
}
.captures button.theme-option:focus-visible { box-shadow: var(--focus-ring); }
.captures .theme-option-name {
  padding: 1px 0;
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
}
.captures .theme-option-check {
  color: var(--check);
  font-size: 10px;
  font-weight: var(--weight-bold);
  opacity: 0;
}
.captures button.theme-option.active .theme-option-check { opacity: 1; }

/* `.custom-theme-editor`. */
.captures .custom-theme-editor {
  padding: var(--s-5);
  border: 1px solid var(--border);
  border-radius: var(--r-lg);
  background: var(--surface-canvas);
}
.captures .custom-theme-field {
  padding: var(--s-4);
  border: 1px solid var(--border-subtle);
  border-radius: var(--r-md);
  background: var(--surface-raised);
}
.captures .custom-theme-field-label {
  color: var(--text);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
}
.captures .custom-theme-field-description {
  color: var(--text-subtle);
  font-size: var(--text-xs);
  line-height: var(--leading-snug);
}
.captures button.custom-theme-swatch {
  all: unset;
  min-width: 28px;
  min-height: 24px;
  padding: 3px;
  border: 1px solid var(--control-border);
  border-radius: var(--r-sm);
  background: var(--surface-field);
}
.captures button.custom-theme-swatch:focus-visible { box-shadow: var(--focus-ring-tight); }

/* `.ui-btn`: 32 px border box. */
.captures button.ui-btn {
  all: unset;
  min-height: 30px;
  padding: 0 var(--s-5);
  border: 1px solid var(--control-border);
  border-radius: var(--r-md);
  color: var(--text);
  background: var(--control);
  box-shadow: var(--shadow-xs);
  font-size: var(--text-sm);
  font-weight: var(--weight-medium);
  letter-spacing: var(--sm-tracking);
  transition: background-color var(--dur-2) var(--ease-standard),
    border-color var(--dur-2) var(--ease-standard),
    box-shadow var(--dur-2) var(--ease-standard);
}
.captures button.ui-btn:hover { border-color: var(--border-strong); background: var(--control-hover); }
.captures button.ui-btn:focus-visible { box-shadow: var(--focus-ring); }
.captures button.ui-btn:disabled { opacity: 0.45; }
.captures button.ui-btn.small { min-height: 26px; }

/* `.ui-input` text fields. */
.captures entry.ui-input {
  all: unset;
  min-height: 34px;
  padding: 0 var(--s-5);
  border: 1px solid var(--control-border);
  border-radius: var(--r-md);
  color: var(--text);
  background: var(--surface-field);
  font-size: var(--text-md);
  transition: border-color var(--dur-2) var(--ease-standard),
    box-shadow var(--dur-2) var(--ease-standard);
}
.captures entry.ui-input.mono { font-family: monospace; font-size: var(--text-sm); }
.captures entry.ui-input.hex {
  min-height: 30px;
  padding: 0 var(--s-3);
  border-radius: var(--r-sm);
  font-family: monospace;
  font-size: var(--text-xs);
}
.captures entry.ui-input:hover { border-color: var(--border-strong); }
.captures entry.ui-input:focus-within {
  border-color: var(--theme-accent);
  box-shadow: var(--focus-ring-tight);
}
.captures entry.ui-input > text { all: unset; caret-color: var(--text); }
.captures entry.ui-input > text > selection { color: var(--text); background: var(--surface-selected); }

/* `.mini-preview-placement`: 108×72 screen with four 22 px corners. */
.captures .placement-screen {
  min-width: 92px;
  min-height: 56px;
  padding: 7px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--r-md);
  background: var(--surface-sunken);
  box-shadow: var(--shadow-xs);
}
.captures button.placement-corner {
  all: unset;
  min-width: 20px;
  min-height: 20px;
  border: 1px solid var(--border-strong);
  border-radius: var(--r-sm);
  background: var(--surface-raised);
}
.captures button.placement-corner:hover { border-color: var(--theme-accent); }
.captures button.placement-corner.active { border-color: transparent; background: var(--theme-accent); }
.captures button.placement-corner:focus-visible {
  outline: 2px solid var(--theme-accent);
  outline-offset: 2px;
}
.captures .placement-caption { color: var(--text-subtle); font-size: var(--text-sm); }
.captures .placement.disabled { opacity: 0.55; }

/* `.custom-select` trigger and listbox. */
.captures button.select-trigger {
  all: unset;
  min-height: 30px;
  padding: 0 var(--s-4);
  border: 1px solid var(--control-border);
  border-radius: var(--r-md);
  color: var(--text);
  background: var(--surface-field);
  font-size: var(--text-sm);
  transition: border-color var(--dur-2) var(--ease-standard),
    box-shadow var(--dur-2) var(--ease-standard);
}
.captures button.select-trigger:hover { border-color: var(--border-strong); }
.captures button.select-trigger:focus-visible,
.captures button.select-trigger.open {
  border-color: var(--theme-accent);
  box-shadow: var(--focus-ring-tight);
}
.captures button.select-trigger:disabled { opacity: 0.5; }
.captures .select-chevron { color: var(--text-subtle); }
.captures popover.select-popover { all: unset; }
.captures popover.select-popover > arrow { all: unset; }
.captures popover.select-popover > contents {
  all: unset;
  padding: var(--s-2);
  border: 1px solid var(--border);
  border-radius: var(--r-lg);
  background: var(--surface-overlay);
  box-shadow: var(--shadow-lg);
}
.captures .select-option {
  min-height: 16px;
  padding: var(--s-3) var(--s-4);
  border-radius: var(--r-sm);
  color: var(--text-muted);
  font-size: var(--text-sm);
}
.captures .select-option.active { color: var(--text); background: var(--surface-hover); }
.captures .select-option.selected { color: var(--text); font-weight: var(--weight-medium); }
.captures .select-option.selected .select-check { color: var(--check); }
.captures .select-option.disabled { color: var(--text-faint); background: transparent; }

/* `.update-settings-status`. */
.captures .update-status {
  color: var(--text-subtle);
  font-size: var(--text-sm);
  line-height: var(--leading-snug);
}
"#;

/// Resolve every `var(--name)` in `template` against `lookup`.
fn expand(template: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len() * 2);
    let mut rest = template;
    while let Some(start) = rest.find("var(--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let end = after.find(')').expect("closed var()");
        let name = &after[..end];
        out.push_str(&lookup(name).unwrap_or_else(|| panic!("unknown CSS token {name}")));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

pub fn css(t: &Tokens, dark: bool) -> String {
    let focus = {
        let mut accent = t.rgba("theme-accent");
        accent.set_alpha(0.55);
        accent.to_string()
    };
    expand(CSS, |name| {
        Some(match name {
            "focus" => focus.clone(),
            // `--focus-ring` and `--focus-ring-tight` (`styles/base.css`).
            "focus-ring" => format!("0 0 0 2px {}, 0 0 0 4px {focus}", t.css("surface-canvas")),
            "focus-ring-tight" => format!("0 0 0 2px {focus}"),
            // Selected marks: readable accent on light, the accent on dark.
            "check" => t.css(if dark {
                "theme-accent"
            } else {
                "theme-accent-readable"
            }),
            "brand-tracking" => format!("{}px", t.em("tracking-tight", t.number("text-md"))),
            "header-tracking" => format!("{}px", t.em("tracking-tighter", t.number("text-xl"))),
            "card-tracking" => format!("{}px", t.em("tracking-tight", t.number("text-lg"))),
            "sm-tracking" => format!("{}px", t.em("tracking-tight", t.number("text-sm"))),
            name if name.starts_with("shadow-") => t.shadow_css(name),
            name => t.color_css(name).or_else(|| {
                let raw = t.raw(name);
                (!raw.is_empty()).then(|| raw.to_owned())
            })?,
        })
    })
}

type Restyle = Box<dyn Fn(&Rc<Tokens>, &str)>;

/// The live look: tokens, the stylesheet built from them and the widgets
/// that draw from tokens directly. Changing appearance or accent rebuilds
/// the stylesheet in place and repaints those widgets.
pub struct Look {
    tokens: RefCell<Rc<Tokens>>,
    appearance: Cell<&'static str>,
    provider: gtk::CssProvider,
    hooks: RefCell<Vec<Restyle>>,
}

impl Look {
    pub fn install(tokens: Tokens, appearance: &'static str) -> Rc<Look> {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(&css(&tokens, appearance == "dark"));
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("a display"),
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
        Rc::new(Look {
            tokens: RefCell::new(Rc::new(tokens)),
            appearance: Cell::new(appearance),
            provider,
            hooks: RefCell::default(),
        })
    }

    pub fn tokens(&self) -> Rc<Tokens> {
        self.tokens.borrow().clone()
    }

    /// `"light"` or `"dark"`.
    pub fn appearance(&self) -> &'static str {
        self.appearance.get()
    }

    /// Run `hook` after every restyle.
    pub fn on_restyle(&self, hook: impl Fn(&Rc<Tokens>, &str) + 'static) {
        self.hooks.borrow_mut().push(Box::new(hook));
    }

    pub fn track_switch(&self, switch: &TokenSwitch) {
        let weak = gtk::glib::object::ObjectExt::downgrade(switch);
        self.on_restyle(move |tokens, _| {
            if let Some(switch) = weak.upgrade() {
                switch.set_tokens(tokens.clone());
            }
        });
    }

    /// Restyle for saved settings.
    pub fn apply(&self, settings: &captures_settings::AppSettings) {
        let appearance = crate::tokens::resolve_appearance(settings.appearance);
        let tokens = Rc::new(Tokens::for_settings(settings, appearance));
        self.provider
            .load_from_data(&css(&tokens, appearance == "dark"));
        self.appearance.set(appearance);
        *self.tokens.borrow_mut() = tokens.clone();
        for hook in self.hooks.borrow().iter() {
            hook(&tokens, appearance);
        }
    }
}
