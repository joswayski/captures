//! Shared Captures tokens, read from the same source as the shipping frontend.
use gpui::{
    App, Div, ElementId, Global, Hsla, Pixels, SharedString, Stateful, div, prelude::*, px, rgba,
};

const DESIGN: &str = include_str!("../../../shared/design.css");
const THEMES: &str = include_str!("../../../shared/themes.css");

#[path = "../../native-ui/src/native/icons.rs"]
mod icons;

/// The same outline geometry as the shipping editor, independent of OS themes.
pub struct Assets;
impl gpui::AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        Ok(path.strip_prefix("icons/").and_then(icons::body).map(|body| {
            std::borrow::Cow::Owned(format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"#
            ).into_bytes())
        }))
    }

    fn list(&self, _: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

pub fn icon(name: &'static str) -> gpui::Svg {
    gpui::svg()
        .path(format!("icons/{name}"))
        .size(metric("--s-6"))
        .flex_shrink_0()
        .text_color(Theme::default().glass_text())
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub dark: bool,
    pub accent: Hsla,
    pub signal: Hsla,
}

impl Global for Theme {}

impl Default for Theme {
    fn default() -> Self {
        Self {
            dark: true,
            accent: color(THEMES, "--theme-accent"),
            signal: color(THEMES, "--theme-signal"),
        }
    }
}

pub fn theme(cx: &App) -> Theme {
    cx.try_global::<Theme>().copied().unwrap_or_default()
}

fn value<'a>(source: &'a str, name: &str) -> &'a str {
    source
        .split_once(&format!("{name}:"))
        .and_then(|(_, tail)| tail.split_once(';'))
        .map(|(value, _)| value.trim())
        .unwrap_or_else(|| panic!("missing Captures token {name}"))
}

fn color(source: &str, name: &str) -> Hsla {
    let text = value(source, name);
    if let Some(alias) = text.strip_prefix("var(").and_then(|s| s.strip_suffix(')')) {
        return color(source, alias);
    }
    if let Some(hex) = text.strip_prefix('#') {
        return rgba((u32::from_str_radix(hex, 16).expect("hex token") << 8) | 255).into();
    }
    let parts: Vec<f32> = text
        .split_once('(')
        .expect("rgb token")
        .1
        .trim_end_matches(')')
        .split(',')
        .map(|v| v.trim().parse().expect("rgb channel"))
        .collect();
    let alpha = parts.get(3).copied().unwrap_or(1.0);
    gpui::Rgba {
        r: parts[0] / 255.,
        g: parts[1] / 255.,
        b: parts[2] / 255.,
        a: alpha,
    }
    .into()
}

pub fn metric(name: &str) -> Pixels {
    px(value(DESIGN, name)
        .trim_end_matches("px")
        .parse()
        .expect("pixel token"))
}

pub fn accent(name: &str, custom: &str) -> Hsla {
    palette(name, custom, "--theme-accent")
}

pub fn signal(name: &str, custom: &str) -> Hsla {
    palette(name, custom, "--theme-signal")
}

fn palette(name: &str, custom: &str, token: &str) -> Hsla {
    if name == "custom"
        && let Some(hex) = custom.strip_prefix('#')
        && hex.len() == 6
        && let Ok(value) = u32::from_str_radix(hex, 16)
    {
        return rgba((value << 8) | 255).into();
    }
    let marker = format!("[data-capture-theme=\"{name}\"] {{");
    let source = THEMES
        .split_once(&marker)
        .map(|(_, body)| body)
        .unwrap_or(THEMES);
    color(source, token)
}

impl Theme {
    pub fn color(self, name: &str) -> Hsla {
        if name == "--theme-accent" {
            return self.accent;
        }
        if name == "--theme-signal" {
            return self.signal;
        }
        if name.starts_with("--glass") {
            return color(DESIGN, name);
        }
        let marker = if self.dark {
            "[data-appearance=\"dark\"] {"
        } else {
            "[data-appearance=\"light\"] {"
        };
        color(
            DESIGN.split_once(marker).expect("appearance tokens").1,
            name,
        )
    }
    pub fn canvas(self) -> Hsla {
        self.color("--surface-canvas")
    }
    pub fn raised(self) -> Hsla {
        self.color("--surface-raised")
    }
    pub fn text(self) -> Hsla {
        self.color("--text")
    }
    pub fn muted(self) -> Hsla {
        self.color("--text-muted")
    }
    pub fn border(self) -> Hsla {
        self.color("--border")
    }
    pub fn glass(self) -> Hsla {
        color(DESIGN, "--glass-strong")
    }
    pub fn glass_text(self) -> Hsla {
        color(DESIGN, "--glass-text")
    }
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(metric("--h-md"))
        .px(metric("--s-5"))
        .gap(metric("--s-4"))
        .rounded(metric("--r-sm"))
        .border_1()
        .border_color(theme.border())
        .bg(theme.raised())
        .text_color(theme.text())
        .text_size(metric("--text-sm"))
        .cursor_pointer()
        .hover(move |s| s.bg(theme.color("--surface-hover")))
        .child(label.into())
}

pub fn root(theme: Theme) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.canvas())
        .text_color(theme.text())
        .font_family("DejaVu Sans")
        .text_size(metric("--text-md"))
}

/// Blocking capture, encoding and filesystem work stays off the platform UI loop.
pub fn job<T: Send + 'static>(
    cx: &mut App,
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
    done: impl FnOnce(Result<T, String>, &mut App) + 'static,
) {
    let task = cx.background_executor().spawn(async move { work() });
    cx.spawn(async move |cx| {
        let result = task.await;
        let _ = cx.update(|cx| done(result, cx));
    })
    .detach();
}

/// GPUI render images store BGRA, unlike the shared capture crate's RGBA buffers.
pub fn render_image(mut image: image::RgbaImage) -> std::sync::Arc<gpui::RenderImage> {
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(image)]))
}

pub fn error(message: impl Into<String>, cx: &mut App) {
    use gpui::{Bounds, Context, Render, Window, WindowBounds, WindowOptions, size};
    struct Notice(String);
    impl Render for Notice {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let p = Theme::default();
            root(p)
                .p(metric("--s-8"))
                .gap(metric("--s-6"))
                .child(div().text_size(metric("--text-xl")).child("Captures"))
                .child(self.0.clone())
                .child(
                    button("close-notice", "Close", p)
                        .on_click(|_, window, _| window.remove_window()),
                )
        }
    }
    let message = message.into();
    eprintln!("Captures: {message}");
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(520.), px(240.)),
            cx,
        ))),
        ..Default::default()
    };
    let _ = cx.open_window(options, |window, cx| {
        window.set_window_title("Captures — action failed");
        cx.new(|_| Notice(message))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_aliases_and_fixed_glass_are_distinct() {
        let dark = Theme::default();
        let light = Theme {
            dark: false,
            ..dark
        };
        assert_ne!(dark.canvas(), light.canvas());
        assert_eq!(dark.glass(), light.glass());
        let light_text: gpui::Rgba = light.text().into();
        assert!((light_text.r - 19. / 255.).abs() < 0.001);
        assert_eq!(metric("--s-6"), px(16.));
    }
}
