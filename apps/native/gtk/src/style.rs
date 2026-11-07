//! Captures styling for GTK widgets: `style.css` with every `var(--token)`
//! resolved from the shipping tokens. Only text rendering, input and
//! accessibility come from GTK.
use gtk::gdk;

use crate::tokens::Tokens;

const CSS: &str = include_str!("style.css");

/// Resolve every `var(--name)` in `template` against `lookup`.
fn expand(template: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len() * 2);
    let mut rest = template;
    while let Some(start) = rest.find("var(--") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 6..];
        let end = after.find(')').expect("closed var()");
        let name = &after[..end];
        out.push_str(&lookup(name).unwrap_or_else(|| panic!("unknown CSS token --{name}")));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// `color-mix(in srgb, a (1 - amount), b amount)`.
pub fn mix(a: gdk::RGBA, b: gdk::RGBA, amount: f32) -> gdk::RGBA {
    let lerp = |x: f32, y: f32| x + (y - x) * amount;
    gdk::RGBA::new(
        lerp(a.red(), b.red()),
        lerp(a.green(), b.green()),
        lerp(a.blue(), b.blue()),
        lerp(a.alpha(), b.alpha()),
    )
}

pub fn css(t: &Tokens) -> String {
    let focus = {
        let mut accent = t.rgba("theme-accent");
        accent.set_alpha(0.55);
        accent.to_string()
    };
    let tracking = |token: &str, size: &str| format!("{}px", t.em(token, t.number(size)));
    expand(CSS, |name| {
        Some(match name {
            "focus" => focus.clone(),
            // `--focus-ring` (`styles/base.css`).
            "focus-ring" => format!("0 0 0 2px {}, 0 0 0 4px {focus}", t.css("surface-canvas")),
            "focus-ring-tight" => format!("0 0 0 2px {focus}"),
            "brand-tracking" => tracking("tracking-tight", "text-md"),
            "header-tracking" => tracking("tracking-tighter", "text-xl"),
            "recording-title-tracking" => tracking("tracking-tighter", "text-2xl"),
            "title-tracking" => tracking("tracking-tighter", "text-3xl"),
            "card-tracking" => tracking("tracking-tight", "text-lg"),
            "sm-tracking" => tracking("tracking-tight", "text-sm"),
            "eyebrow-tracking" => tracking("tracking-wide", "text-2xs"),
            "xs-wide-tracking" => tracking("tracking-wide", "text-xs"),
            // `filter: drop-shadow(…)` tokens, as a `box-shadow`.
            name if name.starts_with("tooltip-shadow") => t
                .raw(name)?
                .trim()
                .strip_prefix("drop-shadow(")?
                .strip_suffix(')')?
                .to_owned(),
            // `rgba(var(--theme-accent-rgb), 0.8)`.
            "guidance-feedback-border" => {
                let mut accent = t.rgba("theme-accent");
                accent.set_alpha(captures_app::capture_menu::GUIDANCE_FEEDBACK_BORDER_ALPHA as f32);
                accent.to_string()
            }
            "image-hover" => {
                mix(t.rgba("surface-sunken"), t.rgba("theme-accent"), 0.12).to_string()
            }
            // Shipping's `[data-appearance="dark"]` swap for active quiet
            // buttons: readable accent on light, accent text on dark.
            "accent-active-text" => t.css(if t.rgba("text").red() < 0.5 {
                "theme-accent-readable"
            } else {
                "theme-accent-text"
            }),
            name if name.starts_with("shadow-") => t.shadow_css(name),
            // `--lh-{leading}-{size}`: CSS `line-height: var(--leading-…)` at
            // `font-size: var(--text-…)`, as pixels.
            name if name.starts_with("lh-") => {
                let (leading, size) = name[3..].split_once('-')?;
                let px =
                    t.number(&format!("leading-{leading}")) * t.number(&format!("text-{size}"));
                format!("{px}px")
            }
            name => t
                .color_css(name)
                .or_else(|| t.raw(name).map(str::to_owned))?,
        })
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_token_in_the_stylesheet_resolves() {
        for appearance in ["light", "dark"] {
            let css = css(&Tokens::variant(appearance, "mustard"));
            assert!(!css.contains("var("), "unresolved token in {appearance}");
        }
    }

    #[test]
    fn expand_substitutes_in_place() {
        let out = expand("a: var(--x); b: var(--yy) 1px;", |name| {
            Some(format!("<{name}>"))
        });
        assert_eq!(out, "a: <x>; b: <yy> 1px;");
    }
}
