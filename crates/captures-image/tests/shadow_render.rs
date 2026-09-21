use std::{collections::BTreeMap, sync::Arc};

use captures_image::{BlendMode, Document, DropShadow, Layer, Point, Shape, render_with_shadows};
use image::{ImageFormat, Pixel, Rgba, RgbaImage};

struct ReferenceCase {
    name: &'static str,
    document: Document,
    shadow: DropShadow,
    expected: &'static [u8],
}

fn rounded_layer(
    geometry: (Point, f32, f32, f32),
    colors: ([u8; 4], [u8; 4]),
    stroke_width: f32,
    rotation_degrees: f32,
    blend_mode: BlendMode,
) -> Layer {
    let (origin, width, height, radius) = geometry;
    let (fill, stroke) = colors;
    Layer {
        id: 7,
        shape: Shape::RoundedRectangle {
            origin,
            width,
            height,
            radius,
        },
        color: stroke,
        stroke_width,
        fill: Some(fill),
        rotation_degrees,
        rotation_origin: Some(Point {
            x: origin.x + width / 2.0,
            y: origin.y + height / 2.0,
        }),
        blend_mode,
    }
}

fn solid_document(width: u32, height: u32, background: [u8; 4], layer: Layer) -> Document {
    Document {
        source: Arc::new(RgbaImage::from_pixel(width, height, Rgba(background))),
        crop: None,
        layers: vec![layer],
    }
}

fn bitmap_layer() -> Layer {
    let mut pixels = RgbaImage::new(4, 3);
    pixels.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    pixels.put_pixel(2, 1, Rgba([0, 255, 0, 128]));
    pixels.put_pixel(3, 0, Rgba([0, 0, 255, 0]));
    Layer {
        id: 7,
        shape: Shape::Image {
            origin: Point { x: 7., y: 9. },
            width: 4.,
            height: 3.,
            pixels: Arc::new(pixels),
        },
        color: [255, 255, 255, 128],
        fill: None,
        stroke_width: 0.,
        rotation_degrees: 0.,
        rotation_origin: Some(Point { x: 7., y: 9. }),
        blend_mode: BlendMode::Normal,
    }
}

#[test]
fn bitmap_shadow_uses_pixel_alpha_and_opacity_without_repainting_the_source() {
    let shadow = DropShadow {
        color: [0, 0, 255, 128],
        blur: 0.,
        offset_x: 11.,
        offset_y: -4.,
    };
    let document = solid_document(32, 24, [0; 4], bitmap_layer());
    let rendered = render_with_shadows(&document, &BTreeMap::from([(7, shadow)])).unwrap();
    // Intrinsic alpha × layer opacity; a second crisp pass would produce 192/112.
    assert_eq!(rendered.get_pixel(7, 9).0, [255, 0, 0, 128]);
    assert_eq!(rendered.get_pixel(9, 10).0, [0, 255, 0, 64]);
    // Shadow alpha multiplies the already-opacity-weighted coverage once more.
    assert_eq!(rendered.get_pixel(18, 5).0, [0, 0, 255, 64]);
    assert_eq!(rendered.get_pixel(20, 6).0, [0, 0, 255, 32]);
    assert_eq!(
        rendered.get_pixel(21, 5).0,
        [0; 4],
        "invisible RGB casts no shadow"
    );
    assert_eq!(rendered.pixels().filter(|p| p[3] != 0).count(), 4);
    assert!(document.source.pixels().all(|p| p.0 == [0; 4]));
}

#[test]
fn bitmap_shadow_rotates_source_about_custom_pivot_but_offsets_in_canvas_space() {
    let mut layer = bitmap_layer();
    layer.rotation_degrees = 90.;
    let shadow = DropShadow {
        color: [0, 0, 255, 128],
        blur: 0.,
        offset_x: 11.,
        offset_y: -4.,
    };
    let rendered = render_with_shadows(
        &solid_document(32, 24, [0; 4], layer),
        &BTreeMap::from([(7, shadow)]),
    )
    .unwrap();
    assert_eq!(rendered.get_pixel(6, 9).0, [255, 0, 0, 128]);
    assert_eq!(rendered.get_pixel(5, 11).0, [0, 255, 0, 64]);
    assert_eq!(rendered.get_pixel(17, 5).0, [0, 0, 255, 64]);
    assert_eq!(rendered.get_pixel(16, 7).0, [0, 0, 255, 32]);
    assert_eq!(rendered.pixels().filter(|p| p[3] != 0).count(), 4);
}

#[test]
fn bitmap_shadow_retains_off_canvas_sources_and_uses_the_layer_blend_mode() {
    let mut layer = bitmap_layer();
    if let Shape::Image { origin, .. } = &mut layer.shape {
        origin.x = -6.;
    }
    layer.blend_mode = BlendMode::Multiply;
    let shadow = DropShadow {
        color: [255, 0, 255, 255],
        blur: 0.,
        offset_x: 8.,
        offset_y: -4.,
    };
    let rendered = render_with_shadows(
        &solid_document(16, 20, [100, 200, 80, 255], layer),
        &BTreeMap::from([(7, shadow)]),
    )
    .unwrap();
    // The red source is entirely clipped, but its half-opacity magenta shadow
    // multiplies the opaque backdrop at x=2, y=5 (not a source-over magenta patch).
    assert_eq!(rendered.get_pixel(2, 5).0, [100, 100, 80, 255]);
    assert_eq!(rendered.get_pixel(4, 6).0, [100, 150, 80, 255]);
    assert_eq!(rendered.get_pixel(5, 5).0, [100, 200, 80, 255]);
}

#[test]
fn bitmap_shadow_scales_blurs_and_bounds_work_by_the_output_not_authored_size() {
    let mut layer = bitmap_layer();
    if let Shape::Image {
        origin,
        width,
        height,
        pixels,
    } = &mut layer.shape
    {
        *origin = Point {
            x: -10_000_000.,
            y: -10_000_000.,
        };
        *width = 20_000_020.;
        *height = 20_000_020.;
        *pixels = Arc::new(RgbaImage::from_pixel(2, 3, Rgba([255, 0, 0, 255])));
    }
    layer.color[3] = 0; // Invisible source must not cast a shadow.
    let shadow = DropShadow {
        color: [0, 0, 255, 255],
        blur: 4.,
        offset_x: 2.,
        offset_y: -3.,
    };
    let mut document = solid_document(8, 6, [0; 4], layer);
    let hidden = render_with_shadows(&document, &BTreeMap::from([(7, shadow)])).unwrap();
    assert!(hidden.pixels().all(|p| p[3] == 0));
    document.layers[0].color[3] = 128;
    let rendered = render_with_shadows(&document, &BTreeMap::from([(7, shadow)])).unwrap();
    // Uniform huge coverage: half-red source over half-blue shadow, not clipped
    // to the source image dimensions or to the output before blur/offset.
    for pixel in rendered.pixels() {
        for (actual, expected) in pixel.0.into_iter().zip([170, 0, 85, 192]) {
            assert!(actual.abs_diff(expected) <= 1, "{pixel:?}");
        }
    }
    let invalid = DropShadow {
        blur: f32::NAN,
        ..shadow
    };
    assert!(render_with_shadows(&document, &BTreeMap::from([(7, invalid)])).is_err());
}

fn reference_cases() -> Vec<ReferenceCase> {
    let zero_blur = rounded_layer(
        (Point { x: 22.0, y: 18.0 }, 42.0, 27.0, 5.0),
        ([235, 53, 91, 114], [24, 99, 220, 141]),
        7.0,
        0.0,
        BlendMode::Normal,
    );
    let rotated = rounded_layer(
        (Point { x: 31.0, y: 24.0 }, 47.0, 31.0, 5.0),
        ([246, 108, 42, 118], [31, 42, 77, 144]),
        6.0,
        23.0,
        BlendMode::Multiply,
    );
    let edge = rounded_layer(
        (Point { x: -31.0, y: 17.0 }, 27.0, 28.0, 4.0),
        ([238, 183, 31, 165], [15, 30, 60, 184]),
        5.0,
        0.0,
        BlendMode::Normal,
    );
    let screen = rounded_layer(
        (Point { x: 20.0, y: 17.0 }, 45.0, 30.0, 5.0),
        ([215, 38, 83, 73], [242, 202, 68, 94]),
        8.0,
        0.0,
        BlendMode::Screen,
    );

    let mut rotated_source = RgbaImage::from_pixel(112, 88, Rgba([207, 181, 144, 255]));
    for y in 13..69 {
        for x in 17..95 {
            // Browser reference: rgba(73,126,211,.62) over the tan background.
            let mut destination = rotated_source.get_pixel(x, y).to_owned();
            destination.blend(&Rgba([73, 126, 211, 158]));
            rotated_source.put_pixel(x, y, destination);
        }
    }
    vec![
        ReferenceCase {
            name: "zero-blur-crisp-only",
            document: solid_document(96, 72, [0, 0, 0, 0], zero_blur.clone()),
            shadow: DropShadow {
                color: [30, 190, 88, 0],
                blur: 0.0,
                offset_x: 9.0,
                offset_y: -5.0,
            },
            expected: include_bytes!("fixtures/shadows/zero-blur-crisp-only.png"),
        },
        ReferenceCase {
            name: "zero-blur-fill-stroke",
            document: solid_document(96, 72, [0, 0, 0, 0], zero_blur),
            shadow: DropShadow {
                color: [30, 190, 88, 145],
                blur: 0.0,
                offset_x: 9.0,
                offset_y: -5.0,
            },
            expected: include_bytes!("fixtures/shadows/zero-blur-fill-stroke.png"),
        },
        ReferenceCase {
            name: "rotated-blurred-multiply",
            document: Document {
                source: Arc::new(rotated_source),
                crop: None,
                layers: vec![rotated],
            },
            shadow: DropShadow {
                color: [145, 35, 214, 173],
                blur: 14.0,
                offset_x: -9.0,
                offset_y: 6.0,
            },
            expected: include_bytes!("fixtures/shadows/rotated-blurred-multiply.png"),
        },
        ReferenceCase {
            name: "edge-offcanvas",
            document: solid_document(84, 64, [0, 0, 0, 0], edge),
            shadow: DropShadow {
                color: [23, 117, 232, 189],
                blur: 8.0,
                offset_x: 34.0,
                offset_y: -4.0,
            },
            expected: include_bytes!("fixtures/shadows/edge-offcanvas.png"),
        },
        ReferenceCase {
            name: "screen-partial-alpha",
            document: solid_document(92, 70, [38, 54, 81, 255], screen),
            shadow: DropShadow {
                color: [42, 203, 176, 156],
                blur: 10.0,
                offset_x: 7.0,
                offset_y: 9.0,
            },
            expected: include_bytes!("fixtures/shadows/screen-partial-alpha.png"),
        },
    ]
}

#[test]
fn shadows_track_chromium_canvas_references() {
    for case in reference_cases() {
        let expected = image::load_from_memory_with_format(case.expected, ImageFormat::Png)
            .unwrap()
            .into_rgba8();
        let actual =
            render_with_shadows(&case.document, &BTreeMap::from([(7, case.shadow)])).unwrap();
        assert_eq!(actual.dimensions(), expected.dimensions(), "{}", case.name);
        let expected_width = expected.width();

        let mut total_difference = 0_u64;
        let mut maximum_difference = 0_u8;
        let mut maximum_location = (0_u32, 0_u32, 0_usize);
        let mut changed_channels = 0_u64;
        for (pixel_index, (actual, expected)) in actual.pixels().zip(expected.pixels()).enumerate()
        {
            let actual = [
                (u16::from(actual[0]) * u16::from(actual[3]) / 255) as u8,
                (u16::from(actual[1]) * u16::from(actual[3]) / 255) as u8,
                (u16::from(actual[2]) * u16::from(actual[3]) / 255) as u8,
                actual[3],
            ];
            let expected = [
                (u16::from(expected[0]) * u16::from(expected[3]) / 255) as u8,
                (u16::from(expected[1]) * u16::from(expected[3]) / 255) as u8,
                (u16::from(expected[2]) * u16::from(expected[3]) / 255) as u8,
                expected[3],
            ];
            for (channel, (actual, expected)) in actual.into_iter().zip(expected).enumerate() {
                let difference = actual.abs_diff(expected);
                total_difference += u64::from(difference);
                if difference > maximum_difference {
                    maximum_difference = difference;
                    maximum_location = (
                        pixel_index as u32 % expected_width,
                        pixel_index as u32 / expected_width,
                        channel,
                    );
                }
                changed_channels += u64::from(difference != 0);
            }
        }
        let mean_difference = total_difference as f64 / actual.as_raw().len() as f64;
        eprintln!(
            "{}: mean={mean_difference:.4}, max={maximum_difference} at {maximum_location:?}, changed={changed_channels}/{}",
            case.name,
            actual.as_raw().len()
        );
        assert!(
            mean_difference < 1.0,
            "{} mean={mean_difference}",
            case.name
        );
        assert!(
            maximum_difference <= 40,
            "{} max={maximum_difference}",
            case.name
        );
    }
}

#[test]
fn shadow_aware_bounds_contain_every_painted_pixel_without_changing_hit_policy() {
    let layer = rounded_layer(
        (Point { x: 18.0, y: 20.0 }, 34.0, 21.0, 5.0),
        ([230, 40, 90, 170], [20, 80, 220, 210]),
        7.0,
        31.0,
        BlendMode::Normal,
    );
    let shadow = DropShadow {
        color: [17, 190, 73, 160],
        blur: 11.0,
        offset_x: 13.0,
        offset_y: -6.0,
    };
    let plain_bounds = layer.bounds().unwrap();
    let shadow_bounds = layer.bounds_with_shadow(shadow).unwrap();
    assert!(shadow_bounds.x < plain_bounds.x && shadow_bounds.y < plain_bounds.y);
    assert!(shadow_bounds.width > plain_bounds.width && shadow_bounds.height > plain_bounds.height);

    let document = solid_document(96, 80, [0, 0, 0, 0], layer.clone());
    let rendered = render_with_shadows(&document, &BTreeMap::from([(7, shadow)])).unwrap();
    for (x, y, pixel) in rendered.enumerate_pixels() {
        if pixel[3] != 0 {
            assert!(
                x as f32 >= shadow_bounds.x.floor()
                    && x as f32 <= (shadow_bounds.x + shadow_bounds.width).ceil()
                    && y as f32 >= shadow_bounds.y.floor()
                    && y as f32 <= (shadow_bounds.y + shadow_bounds.height).ceil(),
                "painted pixel ({x}, {y}) escaped {shadow_bounds:?}"
            );
        }
    }
    let shadow_only_point = Point {
        x: shadow_bounds.x + 1.0,
        y: shadow_bounds.y + 1.0,
    };
    assert!(!layer.hit_test(shadow_only_point, 0.0));
}

#[test]
fn distant_off_canvas_geometry_has_bounded_shadow_work() {
    let layer = rounded_layer(
        (
            Point {
                x: -10_000_000.0,
                y: -10_000_000.0,
            },
            20_000_020.0,
            20_000_020.0,
            0.0,
        ),
        ([255, 0, 0, 128], [0, 0, 0, 0]),
        0.0,
        0.0,
        BlendMode::Normal,
    );
    let document = solid_document(8, 6, [0, 0, 0, 0], layer);
    let rendered = render_with_shadows(
        &document,
        &BTreeMap::from([(
            7,
            DropShadow {
                color: [0, 0, 0, 128],
                blur: 4.0,
                offset_x: 2.0,
                offset_y: -3.0,
            },
        )]),
    )
    .unwrap();
    assert_eq!(rendered.dimensions(), (8, 6));
}
