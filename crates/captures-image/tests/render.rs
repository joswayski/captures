use std::sync::Arc;

use captures_image::{Document, Layer, PixelRect, Point, Shape, render};
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
        },
    ] {
        let invalid = layer(shape);
        assert!(invalid.bounds().is_none());
        assert!(!invalid.hit_test(point(1.0, 1.0), 1.0));
        assert!(render(&document(vec![invalid])).is_err());
    }
}
