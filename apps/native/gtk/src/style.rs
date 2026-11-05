//! Captures styling for GTK widgets, generated from the shipping tokens.
//!
//! Every Captures class starts from `all: unset`, so the desktop's GTK theme
//! never leaks into the product chrome; only text rendering, input and
//! accessibility come from GTK. Rules mirror `styles/windows.css`.
use gtk::gdk;

use crate::tokens::Tokens;

pub fn css(t: &Tokens) -> String {
    let n = |name: &str| t.number(name);
    let c = |name: &str| t.css(name);
    let md = n("text-md");
    let xl = n("text-xl");
    let lg = n("text-lg");
    format!(
        r#"
window.captures {{
  background: {canvas};
  color: {text};
}}
.captures label {{ all: unset; }}
.captures .preferences-nav {{
  padding: {s6}px {s5}px;
  border-right: 1px solid {border_subtle};
  background: {sunken};
}}
.captures .preferences-nav-brand {{ padding: 0 {s3}px; }}
.captures .brand-mark {{
  min-width: 26px;
  min-height: 26px;
  border-radius: {r_md}px;
  color: {accent_ink};
  background: {accent};
}}
.captures .brand-title {{
  font-size: {md}px;
  font-weight: {semibold};
  letter-spacing: {brand_tracking}px;
}}
.captures button.nav-item {{
  all: unset;
  min-height: {h_md}px;
  padding: 0 {s4}px;
  border-radius: {r_md}px;
  color: {text_subtle};
  font-size: {md}px;
  font-weight: {medium};
  transition: color {dur2}ms, background-color {dur2}ms;
}}
.captures button.nav-item:hover {{ color: {text}; background: {hover}; }}
.captures button.nav-item.active {{ color: {text}; background: {active}; }}
.captures button.nav-item:focus-visible {{ box-shadow: inset 0 0 0 2px {focus}; }}
.captures .preferences-header {{
  min-height: 38px;
  padding: {s6}px {s8}px;
  border-bottom: 1px solid {border_subtle};
  background: {canvas};
}}
.captures button.preferences-history-button {{
  all: unset;
  min-height: {h_md}px;
  padding: 0 {s5}px;
  border: 1px solid {border};
  border-radius: {r_md}px;
  color: {text};
  background: {raised};
  font-size: {sm}px;
  font-weight: {medium};
  transition: background-color {dur2}ms;
}}
.captures button.preferences-history-button:hover {{ background: {hover}; }}
.captures button.preferences-history-button:focus-visible {{ box-shadow: 0 0 0 3px {focus}; }}
.captures .header-title {{
  font-size: {xl}px;
  font-weight: {semibold};
  letter-spacing: {header_tracking}px;
}}
.captures .header-description {{
  margin-top: 2px;
  color: {text_subtle};
  font-size: {sm}px;
}}
.captures .preferences-sections {{ padding: {s8}px {s8}px {s12}px; }}
.captures .settings-card {{
  padding: {s6}px;
  border: 1px solid {border_subtle};
  border-radius: {r_xl}px;
  background: {raised};
  box-shadow: {shadow_sm};
}}
.captures .card-title {{
  font-size: {lg}px;
  font-weight: {semibold};
  letter-spacing: {card_tracking}px;
}}
.captures .card-description, .captures .setting-description {{
  color: {text_subtle};
  font-size: {sm}px;
  line-height: {snug};
}}
.captures .setting-row {{ padding-top: {s5}px; border-top: 1px solid {border_subtle}; }}
.captures .setting-row.first {{ padding-top: {s1}px; border-top-width: 0; }}
.captures .setting-title {{
  color: {text};
  font-size: {md}px;
  font-weight: {medium};
}}
.captures .setting-row.disabled .setting-copy {{ opacity: 0.55; }}
"#,
        canvas = c("surface-canvas"),
        sunken = c("surface-sunken"),
        raised = c("surface-raised"),
        hover = c("surface-hover"),
        active = c("surface-active"),
        text = c("text"),
        text_subtle = c("text-subtle"),
        border_subtle = c("border-subtle"),
        border = c("border"),
        accent = c("theme-accent"),
        accent_ink = c("theme-accent-ink"),
        focus = {
            let mut accent = t.rgba("theme-accent");
            accent.set_alpha(0.55);
            accent.to_string()
        },
        shadow_sm = t.shadow_css("shadow-sm"),
        s1 = n("s-1"),
        s3 = n("s-3"),
        s4 = n("s-4"),
        s5 = n("s-5"),
        s6 = n("s-6"),
        s8 = n("s-8"),
        s12 = n("s-12"),
        r_md = n("r-md"),
        r_xl = n("r-xl"),
        h_md = n("h-md"),
        sm = n("text-sm"),
        md = md,
        lg = lg,
        xl = xl,
        snug = n("leading-snug"),
        medium = n("weight-medium"),
        semibold = n("weight-semibold"),
        dur2 = n("dur-2"),
        brand_tracking = t.em("tracking-tight", md),
        header_tracking = t.em("tracking-tighter", xl),
        card_tracking = t.em("tracking-tight", lg),
    )
}

/// Install (or replace) the Captures stylesheet for the display.
pub fn install(t: &Tokens) -> gtk::CssProvider {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&css(t));
    gtk::style_context_add_provider_for_display(
        &gdk::Display::default().expect("a display"),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER,
    );
    provider
}
