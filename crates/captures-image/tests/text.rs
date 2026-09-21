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
fn outlines_stroke_contours_with_hollow_interiors_round_joins_and_stable_metrics() {
    let mut engine = renderer();
    let filled = engine.render_line("L", &style()).unwrap();
    let outlined = engine.render_outline_line("L", &style(), 8.).unwrap();
    assert_eq!(outlined.advance, 70.);
    assert_eq!(outlined.baseline, filled.baseline);
    assert_eq!(outlined.bounds.x, filled.bounds.x - 4.);
    assert_eq!(outlined.bounds.y, filled.bounds.y - 4.);
    assert_eq!(outlined.bounds.width, filled.bounds.width + 8.);
    assert_eq!(outlined.bounds.height, filled.bounds.height + 8.);
    let pixel = |x: i32, up: i32| {
        outlined
            .pixels
            .get_pixel(
                (x - outlined.bounds.x as i32) as u32,
                (outlined.baseline as i32 - up - outlined.bounds.y as i32) as u32,
            )
            .0
    };
    // The original L has a 15px stem, a 60×15px foot and an 80px ascender.
    // Stroke spans both sides of each contour, not the filled glyph interior.
    assert_eq!(pixel(-3, 40), style().color);
    assert_eq!(pixel(1, 40), style().color);
    assert_eq!(pixel(7, 40)[3], 0);
    assert_eq!(pixel(40, 7)[3], 0);
    assert_eq!(pixel(40, 1), style().color);
    assert_eq!(
        pixel(-4, 84)[3],
        0,
        "round, not square/mitered, outer corner"
    );
    assert_eq!(
        engine
            .render_outline_line("L", &style(), 2.)
            .unwrap()
            .pixels
            .width(),
        filled.pixels.width() + 2
    );
    assert_eq!(
        engine
            .render_outline_line("L", &style(), 8.)
            .unwrap()
            .pixels,
        outlined.pixels
    );
    assert_eq!(
        engine.render_line("L", &style()).unwrap().pixels,
        filled.pixels
    );
}

#[test]
fn outlined_shaping_preserves_faces_marks_bidi_and_single_line_opacity() {
    let mut engine = renderer();
    for (text, advance) in [("fi", 45.), ("A\u{301}", 70.), ("אב", 140.), ("AA", 80.)] {
        let line = engine.render_outline_line(text, &style(), 4.).unwrap();
        assert!((line.advance - advance).abs() < 0.001);
        assert!(line.pixels.pixels().any(|pixel| pixel.0 == style().color));
        assert!(line.pixels.pixels().all(|pixel| pixel[3] <= 128));
    }
    let bold = engine
        .render_outline_line(
            "L",
            &TextStyle {
                bold: true,
                ..style()
            },
            4.,
        )
        .unwrap();
    assert_eq!(bold.advance, 80.);
    let italic = engine
        .render_outline_line(
            "L",
            &TextStyle {
                italic: true,
                ..style()
            },
            8.,
        )
        .unwrap();
    assert_eq!(italic.advance, 70.);
    assert!(italic.bounds.x <= -13.);
    for (text, advance) in [("", 0.), ("   ", 90.)] {
        let line = engine.render_outline_line(text, &style(), 8.).unwrap();
        assert!((line.advance - advance).abs() < 0.001);
        assert_eq!(line.pixels.dimensions(), (0, 0));
    }
}

#[test]
fn outlined_fractional_glyph_positions_move_coverage_not_only_bitmap_bounds() {
    let line = renderer()
        .render_outline_line(
            "ff",
            &TextStyle {
                size: 101.25,
                ..style()
            },
            2.,
        )
        .unwrap();
    // Each f advances 400 font units at 101.25/1000, so the second stem starts
    // at x40.5. Its two-pixel stroke covers x39.5..41.5: half/full/half pixels.
    let row = (line.baseline as i32 - 30 - line.bounds.y as i32) as u32;
    for (x, expected) in [(39, 64_u8), (40, 128), (41, 64), (42, 0)] {
        let actual = line
            .pixels
            .get_pixel((x - line.bounds.x as i32) as u32, row)[3];
        assert!(
            actual.abs_diff(expected) <= 1,
            "x{x}: {actual} != {expected}"
        );
    }
}

#[test]
fn outlined_color_and_bitmap_glyphs_fail_explicitly_without_poisoning_fill() {
    for bytes in [
        include_bytes!("shaping-color.ttf").as_slice(),
        include_bytes!("shaping-bitmap.ttf").as_slice(),
    ] {
        let mut engine = TextRenderer::new([Arc::from(bytes)]).unwrap();
        let before = engine.render_line("A", &style()).unwrap();
        assert!(
            engine
                .render_outline_line("A", &style(), 4.)
                .err()
                .unwrap()
                .contains("monochrome scalable")
        );
        assert_eq!(
            engine.render_line("A", &style()).unwrap().pixels,
            before.pixels
        );
    }
}

#[test]
fn outlined_validation_bounds_and_pixel_budgets_leave_the_renderer_reusable() {
    let mut engine = renderer();
    for width in [0., -1., f32::NAN, f32::INFINITY, 512.1] {
        assert!(engine.render_outline_line("L", &style(), width).is_err());
    }
    assert!(engine.render_outline_line("☃", &style(), 4.).is_err());
    assert!(
        engine
            .render_outline_line(&"L".repeat(240), &style(), 4.)
            .is_err()
    );
    assert!(
        engine
            .render_outline_line(
                &"L".repeat(80),
                &TextStyle {
                    size: 4.,
                    ..style()
                },
                512.
            )
            .err()
            .unwrap()
            .contains("pixel budget")
    );
    assert_eq!(
        engine
            .render_outline_line("fi", &style(), 4.)
            .unwrap()
            .advance,
        45.
    );
    assert_eq!(engine.render_line("L", &style()).unwrap().advance, 70.);
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
