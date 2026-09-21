//! Offline, redistributable default for the experimental native Text tool.
//! No installed-font scan, download, or substitution for saved draft fonts.

use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock},
};

use captures_history::editor_draft::FontAssets;

pub const NOTICE: &str = include_str!("../fonts/liberation-sans/LICENSE");

/// Reuse immutable bytes across workers. Each session owns its shaping state.
pub fn bundled() -> FontAssets {
    static FONTS: OnceLock<FontAssets> = OnceLock::new();
    FONTS
        .get_or_init(|| FontAssets {
            families: BTreeMap::from([("sans".into(), "Liberation Sans".into())]),
            files: [
                (
                    "regular",
                    include_bytes!("../fonts/liberation-sans/LiberationSans-Regular.ttf")
                        .as_slice(),
                ),
                (
                    "bold",
                    include_bytes!("../fonts/liberation-sans/LiberationSans-Bold.ttf").as_slice(),
                ),
                (
                    "italic",
                    include_bytes!("../fonts/liberation-sans/LiberationSans-Italic.ttf").as_slice(),
                ),
                (
                    "bold-italic",
                    include_bytes!("../fonts/liberation-sans/LiberationSans-BoldItalic.ttf")
                        .as_slice(),
                ),
            ]
            .into_iter()
            .map(|(style, bytes)| (format!("liberation-sans-2-1-5-{style}"), Arc::from(bytes)))
            .collect(),
            notices: NOTICE.into(),
        })
        .clone()
}
