use std::sync::Arc;

use captures_image::{BlendMode, Document, Layer, PixelRect, Point, Shape, render};
use image::{Rgba, RgbaImage};

fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

fn layer(shape: Shape) -> Layer {
    Layer {
        id: 42,
        shape,
        color: [240, 30, 80, 255],
        stroke_width: 2.0,
        fill: None,
        rotation_degrees: 0.0,
        blend_mode: BlendMode::Normal,
    }
}

fn document(layers: Vec<Layer>) -> Document {
    Document {
        source: Arc::new(RgbaImage::from_pixel(80, 60, Rgba([0, 0, 0, 0]))),
        crop: None,
        layers,
    }
}

#[test]
fn crop_uses_source_coordinates_after_layer_composition() {
    let mut rectangle = layer(Shape::Rectangle {
        origin: point(22.0, 13.0),
        width: 14.0,
        height: 8.0,
    });
    rectangle.fill = Some([255, 0, 0, 255]);
    rectangle.stroke_width = 0.0;
    let mut doc = document(vec![rectangle]);
    doc.crop = Some(PixelRect {
        x: 19,
        y: 10,
        width: 25,
        height: 15,
    });
    let image = render(&doc).unwrap();
    assert_eq!(image.dimensions(), (25, 15));
    assert_eq!(image.get_pixel(4, 4).0, [255, 0, 0, 255]);
    assert_eq!(image.get_pixel(0, 0).0, [0, 0, 0, 0]);
    assert_eq!(image.get_pixel(20, 12).0, [0, 0, 0, 0]);
    assert_eq!(doc.source.dimensions(), (80, 60));
    assert_eq!(doc.source.get_pixel(23, 14).0, [0, 0, 0, 0]);
}

#[test]
fn composition_preserves_straight_alpha_and_layer_order() {
    let mut red = layer(Shape::Rectangle {
        origin: point(5.0, 5.0),
        width: 30.0,
        height: 20.0,
    });
    red.stroke_width = 0.0;
    red.fill = Some([255, 0, 0, 128]);
    let mut blue = red.clone();
    blue.shape = Shape::Rectangle {
        origin: point(20.0, 10.0),
        width: 30.0,
        height: 20.0,
    };
    blue.fill = Some([0, 0, 255, 128]);
    let image = render(&document(vec![red.clone(), blue.clone()])).unwrap();
    assert_eq!(image.get_pixel(10, 10).0, [255, 0, 0, 128]);
    assert_eq!(image.get_pixel(40, 20).0, [0, 0, 255, 128]);
    // 1/2 blue over 1/2 red: alpha ~= 3/4; straight RGB ~= 1/3 red, 2/3 blue.
    let overlap = image.get_pixel(25, 15).0;
    assert!((i16::from(overlap[0]) - 85).abs() <= 1, "{overlap:?}");
    assert!((i16::from(overlap[2]) - 170).abs() <= 1, "{overlap:?}");
    assert_eq!(overlap[3], 192);
    let reverse = render(&document(vec![blue, red])).unwrap();
    assert!(reverse.get_pixel(25, 15)[0] > reverse.get_pixel(25, 15)[2]);
}

#[test]
fn untouched_pixels_keep_hidden_rgb_and_fractional_alpha_exactly() {
    let source = RgbaImage::from_fn(80, 60, |x, y| Rgba([x as u8, y as u8, 73, (x % 5) as u8]));
    let mut doc = document(vec![]);
    doc.source = Arc::new(source.clone());
    assert_eq!(render(&doc).unwrap(), source);
    doc.layers.push(layer(Shape::Line {
        start: point(5.0, 40.0),
        end: point(35.0, 40.0),
    }));
    let rendered = render(&doc).unwrap();
    for x in 0..80 {
        assert_eq!(rendered.get_pixel(x, 5), source.get_pixel(x, 5));
    }
}

#[test]
fn rotation_matches_rendered_geometry_and_hit_testing() {
    let mut rectangle = layer(Shape::Rectangle {
        origin: point(20.0, 20.0),
        width: 30.0,
        height: 10.0,
    });
    rectangle.fill = Some([240, 30, 80, 255]);
    rectangle.stroke_width = 0.0;
    rectangle.rotation_degrees = 90.0;
    let bounds = rectangle.bounds().unwrap();
    for (actual, expected) in [
        (bounds.x, 30.0),
        (bounds.y, 10.0),
        (bounds.width, 10.0),
        (bounds.height, 30.0),
    ] {
        assert!((actual - expected).abs() < 0.001, "{bounds:?}");
    }
    assert!(rectangle.hit_test(point(35.0, 13.0), 0.0));
    assert!(!rectangle.hit_test(point(23.0, 25.0), 0.0));
    let rendered = render(&document(vec![rectangle])).unwrap();
    assert_eq!(rendered.get_pixel(35, 13)[3], 255);
    assert_eq!(rendered.get_pixel(23, 25)[3], 0);
}

#[test]
fn unfilled_shapes_do_not_select_or_paint_their_interiors() {
    for shape in [
        Shape::Rectangle {
            origin: point(10.0, 10.0),
            width: 40.0,
            height: 20.0,
        },
        Shape::Ellipse {
            origin: point(10.0, 10.0),
            width: 40.0,
            height: 20.0,
        },
    ] {
        let mut outline = layer(shape);
        assert!(!outline.hit_test(point(30.0, 20.0), 0.0));
        assert!(outline.hit_test(point(50.5, 20.0), 0.0));
        assert!(!outline.hit_test(point(51.5, 20.0), 0.0));
        assert!(outline.hit_test(point(51.5, 20.0), 1.0));
        assert_eq!(
            render(&document(vec![outline.clone()]))
                .unwrap()
                .get_pixel(30, 20)[3],
            0
        );
        outline.fill = Some([0, 120, 30, 255]);
        assert!(outline.hit_test(point(30.0, 20.0), 0.0));
        assert_eq!(
            render(&document(vec![outline]))
                .unwrap()
                .get_pixel(30, 20)
                .0,
            [0, 120, 30, 255]
        );
    }
}

#[test]
fn freehand_dots_segments_and_arrowheads_are_not_bounding_boxes() {
    let dot = layer(Shape::Freehand(vec![point(9.5, 11.5)]));
    assert!(dot.hit_test(point(10.0, 11.5), 0.0));
    assert!(!dot.hit_test(point(11.0, 11.5), 0.0));
    assert_eq!(
        render(&document(vec![dot])).unwrap().get_pixel(9, 11)[3],
        255
    );
    let bent = layer(Shape::Freehand(vec![
        point(10.0, 10.0),
        point(40.0, 10.0),
        point(40.0, 30.0),
    ]));
    assert!(bent.hit_test(point(30.0, 10.5), 0.0));
    assert!(!bent.hit_test(point(25.0, 20.0), 0.0));
    let arrow = layer(Shape::Arrow {
        start: point(10.0, 20.0),
        end: point(50.0, 20.0),
    });
    assert!(arrow.hit_test(point(40.0, 25.0), 0.0));
    assert!(!arrow.hit_test(point(40.0, 29.0), 0.0));
    assert!(render(&document(vec![arrow])).unwrap().get_pixel(41, 24)[3] > 0);
}

#[test]
fn explicit_font_renders_asymmetric_glyphs_with_alpha_and_rotation() {
    let mut text = layer(Shape::Text {
        origin: point(10.0, 5.0),
        text: "L".into(),
        font_size: 20.0,
        font_data: Arc::from(include_bytes!("test-font.ttf").as_slice()),
        style: Default::default(),
    });
    text.color = [50, 150, 250, 128];
    let rendered = render(&document(vec![text.clone()])).unwrap();
    // The test font is a 600×800 L at 1000 units/em: a 3px vertical
    // stem and 3px bottom foot at this size. These expectations come from
    // the fixture outline, not fontdue metrics or the renderer's bounds.
    assert_eq!(rendered.get_pixel(11, 8)[3], 128);
    assert_eq!(rendered.get_pixel(20, 19)[3], 128);
    assert_eq!(rendered.get_pixel(20, 8)[3], 0);
    assert!((i16::from(rendered.get_pixel(11, 8)[2]) - 250).abs() <= 1);
    text.rotation_degrees = 90.0;
    let rotated = render(&document(vec![text])).unwrap();
    assert_eq!(rotated.get_pixel(20, 8)[3], 128);
    assert_eq!(rotated.get_pixel(20, 17)[3], 0);
}

#[test]
fn concave_polygons_close_the_outline_and_fill_only_their_interior() {
    let points = vec![
        point(10.0, 10.0),
        point(50.0, 10.0),
        point(50.0, 20.0),
        point(20.0, 20.0),
        point(20.0, 45.0),
        point(10.0, 45.0),
    ];
    let mut shape = layer(Shape::Polygon(points.clone()));
    assert!(shape.hit_test(point(10.5, 30.0), 0.0)); // Last-to-first closing edge.
    assert!(!shape.hit_test(point(15.0, 30.0), 0.0));
    shape.fill = Some([0, 200, 90, 255]);
    assert!(shape.hit_test(point(15.0, 30.0), 0.0));
    assert!(!shape.hit_test(point(40.0, 30.0), 0.0)); // Inside bounds, outside the L.
    let rendered = render(&document(vec![shape.clone()])).unwrap();
    assert_eq!(rendered.get_pixel(15, 30).0, [0, 200, 90, 255]);
    assert_eq!(rendered.get_pixel(40, 30)[3], 0);
    shape.shape = Shape::Polygon(points.into_iter().rev().collect());
    assert!(shape.hit_test(point(15.0, 30.0), 0.0));
    assert!(!shape.hit_test(point(40.0, 30.0), 0.0));
    assert_eq!(render(&document(vec![shape])).unwrap(), rendered);
    assert!(
        render(&document(vec![layer(Shape::Polygon(vec![point(
            1.0, 1.0
        )]))]))
        .is_err()
    );
}

#[test]
fn image_pixels_keep_orientation_alpha_and_document_layer_order() {
    let pixels = Arc::new(RgbaImage::from_fn(3, 2, |x, y| match (x, y) {
        (0, 0) => Rgba([255, 0, 0, 255]),
        (1, 0) => Rgba([0, 255, 0, 128]),
        (2, 1) => Rgba([0, 0, 255, 255]),
        _ => Rgba([150, 90, 20, 0]),
    }));
    let mut image = layer(Shape::Image {
        origin: point(7.0, 11.0),
        width: 3.0,
        height: 2.0,
        pixels: pixels.clone(),
    });
    image.stroke_width = 20.0; // Images have no outline or fill.
    image.fill = Some([255, 255, 255, 255]);
    image.color = [13, 27, 41, 128]; // RGB must not tint imported pixels.
    let rendered = render(&document(vec![image.clone()])).unwrap();
    assert_eq!(rendered.get_pixel(7, 11).0, [255, 0, 0, 128]);
    assert_eq!(rendered.get_pixel(8, 11).0, [0, 255, 0, 64]);
    assert_eq!(rendered.get_pixel(9, 12).0, [0, 0, 255, 128]);
    assert_eq!(rendered.get_pixel(9, 11)[3], 0);
    assert_eq!(rendered.get_pixel(6, 11)[3], 0);
    let mut cover = layer(Shape::Rectangle {
        origin: point(7.0, 11.0),
        width: 1.0,
        height: 1.0,
    });
    cover.stroke_width = 0.0;
    cover.fill = Some([0, 0, 255, 255]);
    let forward = render(&document(vec![image.clone(), cover.clone()])).unwrap();
    let reverse = render(&document(vec![cover, image])).unwrap();
    assert_eq!(forward.get_pixel(7, 11).0, [0, 0, 255, 255]);
    assert_eq!(reverse.get_pixel(7, 11).0, [128, 0, 127, 255]);
    assert_eq!(pixels.get_pixel(0, 0).0, [255, 0, 0, 255]);
}

#[test]
fn image_scaling_rotation_and_crop_share_document_geometry() {
    let mut image = layer(Shape::Image {
        origin: point(10.0, 20.0),
        width: 8.0,
        height: 4.0,
        pixels: Arc::new(RgbaImage::from_fn(4, 2, |x, _| {
            if x < 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            }
        })),
    });
    image.rotation_degrees = 90.0;
    let bounds = image.bounds().unwrap();
    for (actual, expected) in [
        (bounds.x, 12.0),
        (bounds.y, 18.0),
        (bounds.width, 4.0),
        (bounds.height, 8.0),
    ] {
        assert!((actual - expected).abs() < 0.001, "{bounds:?}");
    }
    assert!(image.hit_test(point(13.0, 19.0), 0.0));
    assert!(!image.hit_test(point(10.5, 21.0), 0.0));
    let mut doc = document(vec![image]);
    doc.crop = Some(PixelRect {
        x: 11,
        y: 17,
        width: 7,
        height: 11,
    });
    let rendered = render(&doc).unwrap();
    assert_eq!(rendered.get_pixel(2, 2).0, [255, 0, 0, 255]);
    assert_eq!(rendered.get_pixel(2, 7).0, [0, 0, 255, 255]);
    assert_eq!(rendered.get_pixel(0, 2)[3], 0);
    assert_eq!(rendered.get_pixel(5, 2)[3], 0);
}

#[test]
fn image_resampling_interpolates_premultiplied_color_and_clips_off_canvas() {
    let image = layer(Shape::Image {
        origin: point(-1.0, 3.0),
        width: 4.0,
        height: 2.0,
        pixels: Arc::new(RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 0])
            }
        })),
    });
    let rendered = render(&document(vec![image])).unwrap();
    let edge = rendered.get_pixel(0, 3).0;
    assert_eq!(&edge[..3], &[255, 0, 0]); // Hidden blue cannot create a halo.
    assert!((180..=200).contains(&edge[3]), "{edge:?}"); // 3/4 coverage, not nearest-neighbor.
    let edge = rendered.get_pixel(1, 3).0;
    assert_eq!(&edge[..3], &[255, 0, 0]);
    assert!((55..=70).contains(&edge[3]), "{edge:?}"); // 1/4 coverage.
    assert_eq!(rendered.get_pixel(3, 3)[3], 0);
    assert_eq!(rendered.get_pixel(0, 2)[3], 0);
}

#[test]
fn blend_modes_include_source_backdrop_for_images_shapes_text_and_brushes() {
    // Independently computed channel results for source-over, Cs*Cb,
    // Cs+Cb-Cs*Cb, backdrop-dependent overlay, min and max, respectively.
    let modes: [(BlendMode, [u8; 3]); 6] = [
        (BlendMode::Normal, [192, 96, 32]),
        (BlendMode::Multiply, [48, 60, 28]),
        (BlendMode::Screen, [208, 196, 228]),
        (BlendMode::Overlay, [96, 137, 201]),
        (BlendMode::Darken, [64, 96, 32]),
        (BlendMode::Lighten, [192, 160, 224]),
    ];
    let shapes = [
        (
            Shape::Image {
                origin: point(10.0, 11.0),
                width: 20.0,
                height: 18.0,
                pixels: Arc::new(RgbaImage::from_pixel(2, 2, Rgba([192, 96, 32, 255]))),
            },
            (15, 16),
        ),
        (
            Shape::Rectangle {
                origin: point(10.0, 11.0),
                width: 20.0,
                height: 18.0,
            },
            (15, 16),
        ),
        (
            Shape::Text {
                origin: point(10.0, 5.0),
                text: "L".into(),
                font_size: 20.0,
                font_data: Arc::from(include_bytes!("test-font.ttf").as_slice()),
                style: Default::default(),
            },
            (11, 8),
        ),
        (Shape::Freehand(vec![point(12.5, 12.5)]), (12, 12)),
    ];
    for (mode, expected) in modes {
        for (shape, (x, y)) in &shapes {
            let mut annotation = layer(shape.clone());
            annotation.color = [192, 96, 32, 255];
            annotation.fill = Some(annotation.color);
            annotation.stroke_width = 4.0;
            annotation.blend_mode = mode;
            let mut doc = document(vec![annotation]);
            doc.source = Arc::new(RgbaImage::from_pixel(80, 60, Rgba([64, 160, 224, 255])));
            let rendered = render(&doc).unwrap();
            let actual = rendered.get_pixel(*x, *y).0;
            for (actual, expected) in actual[..3].iter().zip(expected) {
                assert!(
                    (i16::from(*actual) - i16::from(expected)).abs() <= 1,
                    "{mode:?}: {actual} != {expected}"
                );
            }
            assert_eq!(actual[3], 255);
            assert_eq!(rendered.get_pixel(0, 0).0, [64, 160, 224, 255]);
        }
    }
}

#[test]
fn blend_opacity_and_layer_order_preserve_untouched_source_pixels() {
    let mut image = layer(Shape::Image {
        origin: point(10.0, 11.0),
        width: 20.0,
        height: 18.0,
        pixels: Arc::new(RgbaImage::from_pixel(2, 2, Rgba([192, 96, 32, 255]))),
    });
    image.color[3] = 128;
    image.blend_mode = BlendMode::Multiply;
    let mut doc = document(vec![image.clone()]);
    doc.source = Arc::new(RgbaImage::from_fn(80, 60, |x, y| {
        if y > 5 {
            Rgba([64, 160, 224, 255])
        } else {
            Rgba([x as u8, 21, 79, (x % 4) as u8])
        }
    }));
    let result = render(&doc).unwrap();
    for (actual, expected) in result
        .get_pixel(15, 16)
        .0
        .into_iter()
        .zip([56, 110, 126, 255])
    {
        assert!((i16::from(actual) - expected).abs() <= 1);
    }
    for x in 0..80 {
        assert_eq!(result.get_pixel(x, 2), doc.source.get_pixel(x, 2));
    }
    let mut normal = layer(Shape::Rectangle {
        origin: point(10.0, 11.0),
        width: 20.0,
        height: 18.0,
    });
    normal.fill = Some([128, 64, 192, 255]);
    normal.stroke_width = 0.0;
    image.color[3] = 255;
    doc.layers = vec![normal.clone(), image.clone()];
    let result = render(&doc).unwrap();
    for (actual, expected) in result
        .get_pixel(15, 16)
        .0
        .into_iter()
        .zip([96, 24, 24, 255])
    {
        assert!((i16::from(actual) - expected).abs() <= 1);
    }
    doc.layers = vec![image, normal];
    assert_eq!(
        render(&doc).unwrap().get_pixel(15, 16).0,
        [128, 64, 192, 255]
    );
}

#[test]
fn invalid_geometry_crop_and_font_fail_without_mutating_source() {
    for crop in [
        PixelRect {
            x: 79,
            y: 0,
            width: 2,
            height: 1,
        },
        PixelRect {
            x: u32::MAX,
            y: 0,
            width: 2,
            height: 1,
        },
        PixelRect {
            x: 0,
            y: 0,
            width: 1,
            height: 0,
        },
    ] {
        let mut doc = document(vec![]);
        doc.crop = Some(crop);
        assert!(render(&doc).is_err());
    }
    for shape in [
        Shape::Line {
            start: point(f32::NAN, 1.0),
            end: point(5.0, 5.0),
        },
        Shape::Text {
            origin: point(0.0, 0.0),
            text: "hello".into(),
            font_size: 16.0,
            font_data: Arc::from([]),
            style: Default::default(),
        },
        Shape::Image {
            origin: point(0.0, 0.0),
            width: 5.0,
            height: 3.0,
            pixels: Arc::new(RgbaImage::new(0, 0)),
        },
        Shape::Image {
            origin: point(0.0, 0.0),
            width: f32::NAN,
            height: 3.0,
            pixels: Arc::new(RgbaImage::new(2, 1)),
        },
    ] {
        let invalid = layer(shape);
        assert!(invalid.bounds().is_none());
        assert!(!invalid.hit_test(point(1.0, 1.0), 1.0));
        assert!(render(&document(vec![invalid])).is_err());
    }
}
