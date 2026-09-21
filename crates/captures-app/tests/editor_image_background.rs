use captures_app::{
    editor::{ImageElement, ImageOrientation, Point},
    editor_image_background::remove_color,
};
use image::RgbaImage;
use serde::Deserialize;

#[derive(Deserialize)]
struct Cases {
    image: ImageElement,
    mapping: Vec<Mapping>,
    pixels: Pixels,
    wand: Vec<Wand>,
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
