use captures_app::{
    editor::{ImageElement, ImageOrientation, Point},
    editor_image_background::{BrushMode, paint_stroke, remove_color},
};
use image::RgbaImage;
use serde::Deserialize;

#[derive(Deserialize)]
struct Cases {
    image: ImageElement,
    mapping: Vec<Mapping>,
    pixels: Pixels,
    wand: Vec<Wand>,
    brush: Vec<Brush>,
}

#[derive(Deserialize)]
struct Mapping {
    orientation: ImageOrientation,
    rotation: Option<f64>,
    point: Point,
    expected: Option<(u32, u32)>,
}

#[derive(Deserialize)]
struct Pixels {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

#[derive(Deserialize)]
struct Wand {
    name: String,
    seed: (u32, u32),
    tolerance: u8,
    contiguous: bool,
    cleared: Vec<usize>,
}

#[derive(Deserialize)]
struct Brush {
    name: String,
    width: u32,
    height: u32,
    working: Vec<u8>,
    original: Option<Vec<u8>>,
    points: Vec<(u32, u32)>,
    radius: f64,
    hardness: f64,
    mode: BrushMode,
    changed: u64,
    expected: Vec<u8>,
}

fn cases() -> Cases {
    serde_json::from_str(include_str!("editor-image-background-cases.json")).unwrap()
}

#[test]
fn natural_pixel_mapping_matches_shipping_d4_rotation_and_exclusive_edges() {
    let cases = cases();
    for entry in cases.mapping {
        let mut image = cases.image.clone();
        image.orientation = Some(entry.orientation);
        image.base.rotation = entry.rotation;
        assert_eq!(
            image.natural_pixel_at(entry.point),
            entry.expected,
            "{:?}, {:?}, {:?}",
            entry.orientation,
            entry.rotation,
            entry.point
        );
    }
}

#[test]
fn wand_matches_shipping_exact_pixels_not_just_counts() {
    let cases = cases();
    for entry in cases.wand {
        let mut image = RgbaImage::from_raw(
            cases.pixels.width,
            cases.pixels.height,
            cases.pixels.rgba.clone(),
        )
        .unwrap();
        let mut expected = cases.pixels.rgba.clone();
        for index in &entry.cleared {
            expected[index * 4..index * 4 + 4].fill(0);
        }
        assert_eq!(
            remove_color(&mut image, entry.seed, entry.tolerance, entry.contiguous),
            entry.cleared.len() as u64,
            "{}",
            entry.name
        );
        assert_eq!(image.as_raw(), &expected, "{}", entry.name);
    }
}

#[test]
fn brush_matches_shipping_exact_rgba_and_changed_samples() {
    for entry in cases().brush {
        let mut working = RgbaImage::from_raw(entry.width, entry.height, entry.working).unwrap();
        let original = entry
            .original
            .map(|rgba| RgbaImage::from_raw(entry.width, entry.height, rgba).unwrap());
        assert_eq!(
            paint_stroke(
                &mut working,
                original.as_ref(),
                &entry.points,
                entry.radius,
                entry.hardness,
                entry.mode,
            ),
            Ok(entry.changed),
            "{}",
            entry.name
        );
        assert_eq!(working.as_raw(), &entry.expected, "{}", entry.name);
    }
}

#[test]
fn invalid_brush_inputs_do_not_mutate_working_pixels() {
    let initial = vec![10, 20, 30, 255];
    let invalid = [
        (vec![], 1.0, 0.5, BrushMode::Erase, None),
        (vec![(0, 0)], 0.0, 0.5, BrushMode::Erase, None),
        (vec![(0, 0)], f64::NAN, 0.5, BrushMode::Erase, None),
        (vec![(0, 0)], 1.0, f64::INFINITY, BrushMode::Erase, None),
        (vec![(1, 0)], 1.0, 0.5, BrushMode::Erase, None),
        (vec![(0, 0)], 1.0, 0.5, BrushMode::Restore, None),
        (
            vec![(0, 0)],
            1.0,
            0.5,
            BrushMode::Restore,
            Some(RgbaImage::new(2, 1)),
        ),
    ];
    for (points, radius, hardness, mode, original) in invalid {
        let mut working = RgbaImage::from_raw(1, 1, initial.clone()).unwrap();
        assert!(
            paint_stroke(
                &mut working,
                original.as_ref(),
                &points,
                radius,
                hardness,
                mode
            )
            .is_err()
        );
        assert_eq!(working.as_raw(), &initial);
    }
}
