//! Host UI typography from the shipping token font stack.
//!
//! `shared/design.css` names system faces (`-apple-system`, "Segoe UI Variable
//! Text", "Segoe UI", Inter, Roboto, ... `sans-serif`) that the Tauri webview
//! resolves through the OS. egui ships its own faces, so resolve the same stack
//! here and put the system face first. egui's bundled fonts stay as fallbacks
//! for glyphs the system face lacks.
use eframe::egui;
use std::path::PathBuf;

/// Named family for shipping `--weight-semibold` (600) text.
pub const SEMIBOLD: &str = "semibold";

#[derive(Debug, Default, PartialEq)]
struct Faces {
    regular: Option<PathBuf>,
    semibold: Option<PathBuf>,
}

pub fn install(ctx: &egui::Context) {
    let faces = resolve();
    let mut fonts = egui::FontDefinitions::default();
    let fallbacks = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut semibold_family = fallbacks.clone();
    if let Some(bytes) = faces
        .regular
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
    {
        fonts
            .font_data
            .insert("system-ui".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "system-ui".into());
        semibold_family.insert(0, "system-ui".into());
    }
    if let Some(bytes) = faces
        .semibold
        .as_ref()
        .and_then(|path| std::fs::read(path).ok())
    {
        fonts.font_data.insert(
            "system-ui-semibold".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        semibold_family.insert(0, "system-ui-semibold".into());
    }
    fonts
        .families
        .insert(egui::FontFamily::Name(SEMIBOLD.into()), semibold_family);
    // The screenshot editor's inline text box draws in the layer's own
    // bundled face; UI fallbacks cover glyphs that face lacks.
    for family in EDITOR_FAMILIES {
        for (bold, italic) in EDITOR_TRAITS {
            let Some(bytes) = captures_app::editor_fonts::bundled_face(family, bold, italic) else {
                continue;
            };
            let name = editor_face_name(family, bold, italic);
            fonts.font_data.insert(
                name.clone(),
                egui::FontData::from_owned(bytes.to_vec()).into(),
            );
            let mut stack = vec![name.clone()];
            stack.extend(fallbacks.iter().cloned());
            fonts
                .families
                .insert(egui::FontFamily::Name(name.into()), stack);
        }
    }
    ctx.set_fonts(fonts);
}

const EDITOR_FAMILIES: [&str; 4] = ["sans", "serif", "mono", "rounded"];
const EDITOR_TRAITS: [(bool, bool); 4] =
    [(false, false), (true, false), (false, true), (true, true)];

fn editor_face_name(family: &str, bold: bool, italic: bool) -> String {
    format!(
        "editor-text:{family}:{}{}",
        if bold { "bold" } else { "regular" },
        if italic { "-italic" } else { "" }
    )
}

/// The egui family for a text layer's document family key, the draft's font
/// name for it, and traits: the bundled face this build embeds, or the UI face
/// for a draft's own font (and in contexts that never installed the editor
/// faces).
pub fn editor_text_family(
    ctx: &egui::Context,
    family: &str,
    name: Option<&str>,
    bold: bool,
    italic: bool,
) -> egui::FontFamily {
    let named = egui::FontFamily::Name(editor_face_name(family, bold, italic).into());
    if EDITOR_FAMILIES.contains(&family)
        && name.is_none_or(|name| captures_app::editor_fonts::is_bundled_family(family, name))
        && ctx.fonts(|fonts| fonts.definitions().families.contains_key(&named))
    {
        named
    } else {
        egui::FontFamily::Proportional
    }
}

#[cfg(target_os = "windows")]
fn resolve() -> Faces {
    let directory = std::env::var_os("WINDIR")
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from)
        .join("Fonts");
    let first = |names: &[&str]| {
        names
            .iter()
            .map(|name| directory.join(name))
            .find(|path| path.is_file())
    };
    // "Segoe UI Variable Text" on Windows 11, then "Segoe UI".
    Faces {
        regular: first(&["SegUIVar.ttf", "segoeui.ttf"]),
        semibold: first(&["seguisb.ttf"]),
    }
}

#[cfg(target_os = "linux")]
fn resolve() -> Faces {
    // The stack after the platform faces; WebKitGTK resolves it via fontconfig.
    const STACK: [&str; 5] = ["Inter", "Roboto", "Helvetica Neue", "Arial", "sans-serif"];
    let lookup = |pattern: &str| -> Option<(String, PathBuf)> {
        let output = std::process::Command::new("fc-match")
            .args(["--format=%{family}\n%{file}", pattern])
            .output()
            .ok()?;
        parse_match(&String::from_utf8_lossy(&output.stdout))
    };
    let Some(family) = STACK.iter().find_map(|family| {
        let (matched, _) = lookup(family)?;
        accepts(family, &matched).then_some(*family)
    }) else {
        return Faces::default();
    };
    Faces {
        regular: lookup(&format!("{family}:weight=regular")).map(|(_, path)| path),
        semibold: lookup(&format!("{family}:weight=semibold")).map(|(_, path)| path),
    }
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn resolve() -> Faces {
    Faces::default()
}

/// `fc-match` output: comma-separated family names, then the file path.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_match(output: &str) -> Option<(String, PathBuf)> {
    let (family, file) = output.split_once('\n')?;
    let file = file.trim();
    (!file.is_empty()).then(|| (family.to_owned(), PathBuf::from(file)))
}

/// fontconfig always returns some face; accept it only when it is the
/// requested family, except for the generic `sans-serif` at the end.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn accepts(requested: &str, matched: &str) -> bool {
    requested == "sans-serif"
        || matched
            .split(',')
            .any(|family| family.trim().eq_ignore_ascii_case(requested))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fontconfig_fallbacks_are_not_mistaken_for_requested_families() {
        assert_eq!(
            parse_match("DejaVu Sans\n/usr/share/fonts/DejaVuSans.ttf"),
            Some((
                "DejaVu Sans".to_owned(),
                PathBuf::from("/usr/share/fonts/DejaVuSans.ttf")
            ))
        );
        assert_eq!(parse_match("Inter\n"), None);
        assert!(!accepts("Inter", "DejaVu Sans"));
        assert!(accepts("Inter", "Inter,Inter Display"));
        assert!(accepts("roboto", "Roboto"));
        assert!(accepts("sans-serif", "DejaVu Sans"));
    }

    #[test]
    fn install_always_provides_the_semibold_family() {
        let ctx = egui::Context::default();
        install(&ctx);
        ctx.begin_pass(Default::default());
        let galley = ctx.fonts_mut(|fonts| {
            fonts.layout_no_wrap(
                "Copied".into(),
                egui::FontId::new(12., egui::FontFamily::Name(SEMIBOLD.into())),
                egui::Color32::WHITE,
            )
        });
        assert!(galley.size().x > 0.);
        ctx.end_pass().textures_delta.clear();
    }

    #[test]
    fn editor_text_faces_are_installed_for_every_bundled_family_and_trait() {
        let ctx = egui::Context::default();
        ctx.begin_pass(Default::default());
        assert_eq!(
            editor_text_family(&ctx, "sans", None, false, false),
            egui::FontFamily::Proportional,
            "uninstalled contexts fall back to the UI face"
        );
        ctx.end_pass().textures_delta.clear();
        install(&ctx);
        ctx.begin_pass(Default::default());
        for family in EDITOR_FAMILIES {
            for (bold, italic) in EDITOR_TRAITS {
                let face = editor_text_family(&ctx, family, None, bold, italic);
                assert_ne!(face, egui::FontFamily::Proportional);
                let galley = ctx.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        "Label αβ".into(),
                        egui::FontId::new(24., face),
                        egui::Color32::WHITE,
                    )
                });
                assert!(galley.size().x > 0.);
            }
        }
        assert_eq!(
            editor_text_family(&ctx, "Captures Shaping Test", None, false, false),
            egui::FontFamily::Proportional
        );
        assert_eq!(
            editor_text_family(&ctx, "sans", Some("Captures Shaping Test"), false, false),
            egui::FontFamily::Proportional,
            "a draft's own font under the sans key is not the bundled face"
        );
        assert_ne!(
            editor_text_family(&ctx, "sans", Some("Liberation Sans"), false, false),
            egui::FontFamily::Proportional
        );
        ctx.end_pass().textures_delta.clear();
    }
}
