//! Offline, redistributable default for the experimental native Text tool.
//! No installed-font scan, download, or substitution for saved draft fonts.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use captures_history::editor_draft::FontAssets;

pub const NOTICE: &str = include_str!("../fonts/liberation/LICENSE");

/// Reuse immutable bytes across workers. Each session owns its shaping state.
pub fn bundled() -> FontAssets {
    static FONTS: OnceLock<FontAssets> = OnceLock::new();
    FONTS
        .get_or_init(|| FontAssets {
            families: BTreeMap::from([
                ("sans".into(), "Liberation Sans".into()),
                ("serif".into(), "Liberation Serif".into()),
                ("mono".into(), "Liberation Mono".into()),
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
            ]
            .into_iter()
            .map(|(family, style, bytes)| {
                (
                    format!("liberation-{family}-2-1-5-{style}"),
                    Arc::from(bytes),
                )
            })
            .collect(),
            notices: NOTICE.into(),
        })
        .clone()
}
