use core_foundation::{base::TCFType, string::CFString};
use core_graphics::{
    base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast},
    color_space::CGColorSpace,
    display::CGDisplay,
    image::CGImage,
    window::{kCGWindowImageDefault, kCGWindowListOptionAll},
};
use foreign_types::ForeignType;
use image::RgbaImage;
use std::{ffi::c_void, ptr};

use crate::{CaptureError, CaptureResult};

pub(crate) fn capture_display(id: u32) -> CaptureResult<RgbaImage> {
    // Match xcap's composite and resolution, but retain the CGImage's profile
    // until color conversion. xcap copies its bytes and discards that profile.
    let source = CGDisplay::screenshot(
        CGDisplay::new(id).bounds(),
        kCGWindowListOptionAll,
        0,
        kCGWindowImageDefault,
    )
    .ok_or(CaptureError::SessionUnavailable)?;
    image_to_srgb(&source)
}

/// `vImage_CGImageFormat` (Accelerate/vImage/vImage_Utilities.h).
#[repr(C)]
struct VImageCGImageFormat {
    bits_per_component: u32,
    bits_per_pixel: u32,
    color_space: *const c_void,
    bitmap_info: u32,
    version: u32,
    decode: *const f64,
    rendering_intent: i32,
}

/// `vImage_Buffer` (Accelerate/vImage/vImage_Types.h).
#[repr(C)]
struct VImageBuffer {
    data: *mut c_void,
    height: usize,
    width: usize,
    row_bytes: usize,
}

/// `kvImageNoAllocate`: write into the caller's `vImage_Buffer` storage.
const K_VIMAGE_NO_ALLOCATE: u32 = 512;

#[link(name = "Accelerate", kind = "framework")]
unsafe extern "C" {
    fn vImageBuffer_InitWithCGImage(
        buffer: *mut VImageBuffer,
        format: *mut VImageCGImageFormat,
        background_color: *const f64,
        image: *const c_void,
        flags: u32,
    ) -> isize;
}

fn image_to_srgb(source: &CGImage) -> CaptureResult<RgbaImage> {
    // Both the frozen preview and its final crop use sRGB-tagged PNGs.
    // Converting while the source profile is attached transforms the pixels,
    // rather than relabeling display-native RGB values as sRGB.
    //
    // vImage, not CGContextDrawImage: drawing a CGWindowListCreateImage frame
    // makes Core Graphics decode a full-size copy into its process-wide image
    // cache, which outlives the frame. That left one ~30 MB copy of a Retina
    // display per capture resident until the process quit. vImage produces
    // byte-identical premultiplied sRGB without that cache.
    let name = CFString::new("kCGColorSpaceSRGB");
    let srgb = CGColorSpace::create_with_name(name.as_concrete_TypeRef())
        .ok_or_else(|| CaptureError::Image("sRGB color space is unavailable".into()))?;
    let width = source.width();
    let height = source.height();
    let mut format = VImageCGImageFormat {
        bits_per_component: 8,
        bits_per_pixel: 32,
        color_space: srgb.as_ptr().cast(),
        bitmap_info: kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
        version: 0,
        decode: ptr::null(),
        rendering_intent: 0, // kCGRenderingIntentDefault
    };
    // vImage writes straight into the storage the RgbaImage keeps, so a
    // capture holds one frame-sized buffer instead of converting and copying.
    let row = width * 4;
    let mut pixels = vec![0_u8; row * height];
    let mut buffer = VImageBuffer {
        data: pixels.as_mut_ptr().cast(),
        height,
        width,
        row_bytes: row,
    };
    // SAFETY: `buffer` describes `pixels` (row * height writable bytes), which
    // outlives the call, as do `format`, `source` and `srgb`.
    let error = unsafe {
        vImageBuffer_InitWithCGImage(
            &mut buffer,
            &mut format,
            ptr::null(),
            source.as_ptr().cast(),
            K_VIMAGE_NO_ALLOCATE,
        )
    };
    if error != 0 {
        return Err(CaptureError::Image(format!(
            "sRGB conversion failed (vImage error {error})"
        )));
    }
    // image::RgbaImage uses straight alpha; the conversion above produces
    // premultiplied alpha. Full display composites are normally opaque.
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(width as u32, height as u32, pixels)
        .ok_or_else(|| CaptureError::Image("invalid sRGB capture dimensions".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::{base::kCGRenderingIntentDefault, data_provider::CGDataProvider};
    use std::sync::Arc;

    fn fixture(space: &str, pixels: Vec<u8>, width: usize, height: usize) -> CGImage {
        let name = CFString::new(space);
        let space = CGColorSpace::create_with_name(name.as_concrete_TypeRef()).unwrap();
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            &space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
            &CGDataProvider::from_buffer(Arc::new(pixels)),
            false,
            kCGRenderingIntentDefault,
        )
    }

    #[test]
    fn srgb_capture_preserves_channels_and_row_orientation() {
        let pixels = vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 64, 64, 64, 255,
        ];
        let source = fixture("kCGColorSpaceSRGB", pixels.clone(), 2, 2);
        let image = image_to_srgb(&source).unwrap();
        assert_eq!(image.dimensions(), (2, 2));
        assert_eq!(image.into_raw(), pixels);
    }

    #[test]
    fn display_p3_pixels_are_converted_not_relabelled_as_srgb() {
        // In-gamut Display P3 (180, 100, 80) maps to approximately sRGB
        // (193, 95, 74), not the unchanged, less saturated source triplet.
        let source = fixture("kCGColorSpaceDisplayP3", vec![180, 100, 80, 255], 1, 1);
        let image = image_to_srgb(&source).unwrap();
        let actual = image.get_pixel(0, 0).0;
        for (channel, expected) in actual.into_iter().zip([193_u8, 95, 74, 255]) {
            assert!(
                channel.abs_diff(expected) <= 3,
                "converted pixel: {actual:?}"
            );
        }
    }

    #[test]
    fn capture_returns_straight_alpha() {
        let source = fixture("kCGColorSpaceSRGB", vec![64, 32, 16, 128], 1, 1);
        let image = image_to_srgb(&source).unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [128, 64, 32, 128]);
    }
}
