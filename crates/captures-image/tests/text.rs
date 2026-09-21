use std::sync::Arc;

use captures_image::text::{TextRenderer, TextStyle};

fn renderer() -> TextRenderer {
    TextRenderer::new([
        Arc::from(include_bytes!("shaping-regular.ttf").as_slice()),
        Arc::from(include_bytes!("shaping-bold.ttf").as_slice()),
        Arc::from(include_bytes!("shaping-italic.ttf").as_slice()),
    ])
    .unwrap()
}

fn style() -> TextStyle<'static> {
    TextStyle {
        family: "Captures Shaping Test",
        size: 100.,
        bold: false,
        italic: false,
        color: [41, 137, 203, 128],
    }
}

#[test]
fn measurement_shares_shaping_validation_but_not_the_raster_extent_limit() {
    let mut engine = renderer();
    for text in ["fi", "f i", "A\u{301}", "אב", " ", ""] {
        assert_eq!(
            engine.measure_line(text, &style()).unwrap(),
            engine.render_line(text, &style()).unwrap().advance
        );
    }
    let long = "L".repeat(240);
    assert_eq!(engine.measure_line(&long, &style()).unwrap(), 16_800.);
    assert!(engine.render_line(&long, &style()).is_err());
    for text in ["☃", "A\nL", &"L".repeat(4097)] {
        assert!(engine.measure_line(text, &style()).is_err());
    }
    assert_eq!(engine.measure_line("fi", &style()).unwrap(), 45.);
}

#[test]
fn ligatures_use_shaped_advances_not_individual_characters() {
    let mut renderer = renderer();
    let fi = renderer.render_line("fi", &style()).unwrap();
    // Font GSUB replaces advances 400+200 with a 450-unit glyph, at 100/1000 scale.
    assert_eq!(fi.advance, 45.);
    assert_eq!(renderer.render_line("f i", &style()).unwrap().advance, 90.);
    assert!(fi.pixels.pixels().any(|p| p.0 == style().color));
    assert!(fi.pixels.pixels().all(|p| p[3] <= 128));
    assert_eq!(fi.bounds.width, fi.pixels.width() as f32);
    assert_eq!(fi.bounds.height, fi.pixels.height() as f32);
}

#[test]
fn overlapping_kerned_glyphs_apply_line_opacity_once() {
    let text = renderer().render_line("AA", &style()).unwrap();
    // Two 700-unit advances with a -600-unit pair adjustment, at size100.
    assert!((text.advance - 80.).abs() < 0.001);
    assert!(text.pixels.pixels().any(|p| p[3] == 128));
    assert!(text.pixels.pixels().all(|p| p[3] <= 128));
}

#[test]
fn combining_marks_do_not_advance_but_extend_painted_bounds() {
    let mut renderer = renderer();
    let a = renderer.render_line("A", &style()).unwrap();
    let acute = renderer.render_line("A\u{301}", &style()).unwrap();
    assert_eq!(a.advance, 70.);
    assert_eq!(acute.advance, 70.);
    // GPOS anchors mark at y=800; its outline reaches 1000, above the base's 700.
    assert!((a.bounds.y - acute.bounds.y - 30.).abs() <= 1.);
    assert!(acute.pixels.get_pixel(25, 8)[3] > 0);
    assert_eq!(acute.pixels.get_pixel(50, 8)[3], 0);
}

#[test]
fn bidi_reorders_asymmetric_glyphs_and_preserves_logical_advance() {
    let text = renderer().render_line("אב", &style()).unwrap();
    assert_eq!(text.advance, 140.); // Aleph 900 + Bet 500, at size 100.
    // Visual order is Bet (wide, advance 50) then Aleph (narrow, advance 90).
    let y = (text.baseline - 20. - text.bounds.y) as u32;
    assert_eq!(text.pixels.get_pixel(30, y).0, style().color);
    assert_eq!(text.pixels.get_pixel(47, y)[3], 0);
    assert_eq!(text.pixels.get_pixel(60, y).0, style().color);
    assert!(text.pixels.width() <= 71);
}

#[test]
fn explicit_faces_and_bearings_survive_rasterization() {
    let mut engine = renderer();
    let regular = engine.render_line("L", &style()).unwrap();
    let bold = engine
        .render_line(
            "L",
            &TextStyle {
                bold: true,
                ..style()
            },
        )
        .unwrap();
    let italic = engine
        .render_line(
            "L",
            &TextStyle {
                italic: true,
                ..style()
            },
        )
        .unwrap();
    assert_eq!(regular.advance, 70.);
    assert_eq!(bold.advance, 80.);
    assert_eq!(italic.advance, 70.);
    assert!(italic.bounds.x <= -9.);
    assert!(italic.bounds.width > regular.bounds.width);
    assert_eq!(
        engine.render_line("L", &style()).unwrap().pixels,
        regular.pixels
    );
    assert_eq!(
        renderer().render_line("L", &style()).unwrap().pixels,
        regular.pixels
    );
}

#[test]
fn spaces_keep_advance_without_ink_and_empty_text_is_valid() {
    let mut renderer = renderer();
    let spaces = renderer.render_line("   ", &style()).unwrap();
    assert!((spaces.advance - 90.).abs() < 0.001);
    assert_eq!(spaces.pixels.dimensions(), (0, 0));
    let empty = renderer.render_line("", &style()).unwrap();
    assert_eq!(empty.advance, 0.);
    assert_eq!(empty.pixels.dimensions(), (0, 0));
}

#[test]
fn color_glyphs_keep_palette_rgb_and_multiply_requested_opacity() {
    let mut renderer =
        TextRenderer::new([Arc::from(include_bytes!("shaping-color.ttf").as_slice())]).unwrap();
    let text = renderer.render_line("A", &style()).unwrap();
    // COLR uses half-alpha red above an opaque green overlap, then layer alpha128.
    // Swash's COLR blitter uses /256, losing up to one output alpha unit.
    assert!(
        text.pixels
            .pixels()
            .any(|p| p.0[..3] == [255, 0, 0] && p[3].abs_diff(64) <= 1)
    );
    assert!(
        text.pixels
            .pixels()
            .any(|p| p.0[..3] == [0, 255, 0] && p[3].abs_diff(128) <= 1)
    );
    assert!(
        renderer
            .render_line(
                "A",
                &TextStyle {
                    color: [20, 30, 40, 0],
                    ..style()
                }
            )
            .unwrap()
            .pixels
            .pixels()
            .all(|p| p[3] == 0)
    );
}

#[test]
fn embedded_bitmap_glyphs_are_already_straight_alpha() {
    let mut renderer =
        TextRenderer::new([Arc::from(include_bytes!("shaping-bitmap.ttf").as_slice())]).unwrap();
    let text = renderer.render_line("A", &style()).unwrap();
    assert_eq!(text.pixels.dimensions(), (2, 2));
    assert!(text.pixels.pixels().all(|p| p.0 == [90, 140, 190, 64]));
}

#[test]
fn rejects_missing_fonts_glyphs_invalid_metrics_and_oversized_lines_then_recovers() {
    assert!(TextRenderer::new([]).is_err());
    assert!(TextRenderer::new([Arc::from(b"invalid".as_slice())]).is_err());
    let mut renderer = renderer();
    assert!(
        renderer
            .render_line(
                "L",
                &TextStyle {
                    family: "host-installed-only",
                    ..style()
                }
            )
            .is_err()
    );
    assert!(renderer.render_line("unavailable", &style()).is_err());
    for size in [0., -1., f32::NAN, f32::INFINITY, 512.1] {
        assert!(
            renderer
                .render_line("L", &TextStyle { size, ..style() })
                .is_err()
        );
    }
    for text in [
        "L\nL",
        "L\rL",
        "L\u{000b}L",
        "L\u{000c}L",
        "L\u{0085}L",
        "L\u{2028}L",
        "L\u{2029}L",
        &"L".repeat(4097),
    ] {
        assert!(renderer.render_line(text, &style()).is_err());
    }
    assert!(renderer.render_line(&"L".repeat(4096), &style()).is_err());
    assert!(
        renderer
            .render_line(
                &"L".repeat(4096),
                &TextStyle {
                    size: 0.1,
                    ..style()
                }
            )
            .is_ok()
    );
    assert!(
        renderer
            .render_line(
                "L",
                &TextStyle {
                    size: 512.,
                    ..style()
                }
            )
            .is_ok()
    );
    assert_eq!(renderer.render_line("fi", &style()).unwrap().advance, 45.);
}
