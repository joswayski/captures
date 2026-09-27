//! Offline, redistributable default for the experimental native Text tool.
//! No installed-font scan, download, or substitution for saved draft fonts.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use captures_history::editor_draft::FontAssets;

pub const NOTICE: &str = concat!(
    include_str!("../fonts/liberation/LICENSE"),
    "\n\n",
    include_str!("../fonts/nunito/OFL.txt")
);

/// Reuse immutable bytes across workers. Each session owns its shaping state.
pub fn bundled() -> FontAssets {
    static FONTS: OnceLock<FontAssets> = OnceLock::new();
    FONTS
        .get_or_init(|| FontAssets {
            families: BTreeMap::from([
                ("sans".into(), "Liberation Sans".into()),
                ("serif".into(), "Liberation Serif".into()),
                ("mono".into(), "Liberation Mono".into()),
                ("rounded".into(), "Nunito".into()),
            ]),
            files: [
                (
                    "sans",
                    "regular",
                    include_bytes!("../fonts/liberation/LiberationSans-Regular.ttf").as_slice(),
                ),
                (
                    "sans",
                    "bold",
                    include_bytes!("../fonts/liberation/LiberationSans-Bold.ttf").as_slice(),
                ),
                (
                    "sans",
                    "italic",
                    include_bytes!("../fonts/liberation/LiberationSans-Italic.ttf").as_slice(),
                ),
                (
                    "sans",
                    "bold-italic",
                    include_bytes!("../fonts/liberation/LiberationSans-BoldItalic.ttf").as_slice(),
                ),
                (
                    "serif",
                    "regular",
                    include_bytes!("../fonts/liberation/LiberationSerif-Regular.ttf").as_slice(),
                ),
                (
                    "serif",
                    "bold",
                    include_bytes!("../fonts/liberation/LiberationSerif-Bold.ttf").as_slice(),
                ),
                (
                    "serif",
                    "italic",
                    include_bytes!("../fonts/liberation/LiberationSerif-Italic.ttf").as_slice(),
                ),
                (
                    "serif",
                    "bold-italic",
                    include_bytes!("../fonts/liberation/LiberationSerif-BoldItalic.ttf").as_slice(),
                ),
                (
                    "mono",
                    "regular",
                    include_bytes!("../fonts/liberation/LiberationMono-Regular.ttf").as_slice(),
                ),
                (
                    "mono",
                    "bold",
                    include_bytes!("../fonts/liberation/LiberationMono-Bold.ttf").as_slice(),
                ),
                (
                    "mono",
                    "italic",
                    include_bytes!("../fonts/liberation/LiberationMono-Italic.ttf").as_slice(),
                ),
                (
                    "mono",
                    "bold-italic",
                    include_bytes!("../fonts/liberation/LiberationMono-BoldItalic.ttf").as_slice(),
                ),
                (
                    "rounded",
                    "regular",
                    include_bytes!("../fonts/nunito/Nunito-Regular.ttf").as_slice(),
                ),
                (
                    "rounded",
                    "bold",
                    include_bytes!("../fonts/nunito/Nunito-Bold.ttf").as_slice(),
                ),
                (
                    "rounded",
                    "italic",
                    include_bytes!("../fonts/nunito/Nunito-Italic.ttf").as_slice(),
                ),
                (
                    "rounded",
                    "bold-italic",
                    include_bytes!("../fonts/nunito/Nunito-BoldItalic.ttf").as_slice(),
                ),
            ]
            .into_iter()
            .map(|(family, style, bytes)| {
                (
                    if family == "rounded" {
                        format!("nunito-rounded-3-601-{style}")
                    } else {
                        format!("liberation-{family}-2-1-5-{style}")
                    },
                    Arc::from(bytes),
                )
            })
            .collect(),
            notices: NOTICE.into(),
        })
        .clone()
}

/// One bundled face for a document family key and trait pair, or `None` for a
/// family this build does not bundle (such as a reopened draft's own font).
/// Hosts use it to draw the inline text editor in the layer's face.
#[must_use]
pub fn bundled_face(family: &str, bold: bool, italic: bool) -> Option<Arc<[u8]>> {
    let style = match (bold, italic) {
        (false, false) => "regular",
        (true, false) => "bold",
        (false, true) => "italic",
        (true, true) => "bold-italic",
    };
    let id = match family {
        "rounded" => format!("nunito-rounded-3-601-{style}"),
        "sans" | "serif" | "mono" => format!("liberation-{family}-2-1-5-{style}"),
        _ => return None,
    };
    bundled().files.get(&id).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_family_has_all_four_faces() {
        for family in bundled().families.keys() {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                assert!(
                    bundled_face(family, bold, italic).is_some(),
                    "{family} {bold} {italic}"
                );
            }
        }
        assert!(bundled_face("Captures Shaping Test", false, false).is_none());
        assert_ne!(
            bundled_face("sans", true, false),
            bundled_face("sans", false, false)
        );
    }
}
