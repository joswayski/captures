//! Resolve the shipping editor's four font stacks to embeddable native faces.
use anyhow::{Context, Result};
use captures_image::TextFont;
use font_kit::{
    family_name::FamilyName,
    handle::Handle,
    properties::{Properties, Style, Weight},
    source::SystemSource,
};
use std::sync::Arc;

pub fn resolve(family: &str, bold: bool, italic: bool) -> Result<(Arc<[u8]>, TextFont)> {
    let titles: &[&str] = match family {
        "rounded" => &["SF Pro Rounded", "Arial Rounded MT Bold"],
        "serif" => &["Georgia", "Times New Roman"],
        "mono" => &["SFMono-Regular", "Consolas"],
        _ if cfg!(target_os = "macos") => &[".AppleSystemUIFont", "SF Pro Text"],
        _ if cfg!(target_os = "windows") => &["Segoe UI"],
        _ => &[],
    };
    let mut families = titles
        .iter()
        .map(|name| FamilyName::Title((*name).into()))
        .collect::<Vec<_>>();
    families.push(match family {
        "serif" => FamilyName::Serif,
        "mono" => FamilyName::Monospace,
        _ => FamilyName::SansSerif,
    });
    let mut properties = Properties::new();
    properties.weight = if bold { Weight::BOLD } else { Weight::NORMAL };
    properties.style = if italic { Style::Italic } else { Style::Normal };
    let handle = SystemSource::new().select_best_match(&families, &properties)?;
    let collection_index = match &handle {
        Handle::Path { font_index, .. } | Handle::Memory { font_index, .. } => *font_index,
    };
    let face = handle.load()?;
    let actual = face.properties();
    let bytes = face
        .copy_font_data()
        .context("system font cannot be embedded in the draft")?;
    // CoreText extracts a selected TTC face into a standalone SFNT, whereas
    // FreeType/DirectWrite preserve the collection. Index the returned bytes,
    // not the source handle's container unconditionally.
    let collection_index = embedded_index(&bytes, collection_index);
    Ok((
        Arc::from(bytes.as_slice()),
        TextFont {
            family: family.into(),
            collection_index,
            bold: actual.weight.0 >= 600.,
            italic: actual.style != Style::Normal,
        },
    ))
}

fn embedded_index(bytes: &[u8], source_index: u32) -> u32 {
    if bytes.starts_with(b"ttcf") {
        source_index
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_variants_are_embedded_with_their_selected_face_and_family() {
        for family in ["system", "serif", "mono", "rounded"] {
            let (bytes, regular) = resolve(family, false, false).unwrap();
            assert!(!bytes.is_empty());
            assert_eq!(regular.family, family);
            let (bold_bytes, bold) = resolve(family, true, true).unwrap();
            assert!(!bold_bytes.is_empty());
            assert_eq!(bold.family, family);
            // The test orb's monospace family has a real bold face but no
            // italic face. Italic must legitimately fall back to synthesis;
            // requiring a native italic would test the installed font set.
            if family == "mono" {
                assert!(bold.bold);
                assert!(bytes != bold_bytes || regular.collection_index != bold.collection_index);
            }
        }
    }

    #[test]
    fn extracted_coretext_face_uses_zero_while_collections_keep_the_index() {
        assert_eq!(embedded_index(b"ttcf\0\x01\0\0", 3), 3);
        assert_eq!(embedded_index(b"\0\x01\0\0", 3), 0);
        assert_eq!(embedded_index(b"OTTO", 3), 0);
    }
}
