use image::{GrayImage, RgbaImage};

use crate::CursorImage;

/// GetDIBits expands the AND/XOR monochrome planes to black/white BGRA.
/// Color cursors with alpha contain premultiplied channels; older color and
/// monochrome cursors instead retain the AND mask for destination-aware XOR.
fn from_bitmaps(
    color: Option<RgbaImage>,
    mask: RgbaImage,
    hot_spot: (u32, u32),
) -> Option<CursorImage> {
    let width = mask.width();
    let height = if color.is_some() {
        mask.height()
    } else {
        mask.height() / 2
    };
    if width == 0 || height == 0 {
        return None;
    }
    let mut pixels = match color {
        Some(color) if color.dimensions() == (width, height) => color,
        Some(_) => return None,
        None => image::imageops::crop_imm(&mask, 0, height, width, height).to_image(),
    };
    let has_alpha = pixels.pixels().any(|pixel| pixel[3] != 0);
    let and_mask = if has_alpha {
        for pixel in pixels.pixels_mut() {
            let alpha = u32::from(pixel[3]);
            if alpha > 0 {
                for channel in &mut pixel.0[..3] {
                    *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        None
    } else {
        Some(GrayImage::from_fn(width, height, |x, y| {
            image::Luma([mask.get_pixel(x, y)[0]])
        }))
    };
    Some(CursorImage {
        pixels,
        and_mask,
        // Windows display descriptors and cursor bitmaps are physical pixels,
        // even on a scaled monitor. Do not multiply by monitor DPI again.
        logical_width: f64::from(width),
        logical_height: f64::from(height),
        hot_spot_x: f64::from(hot_spot.0),
        hot_spot_y: f64::from(hot_spot.1),
    })
}

#[cfg(target_os = "windows")]
pub(crate) use win32::pointer_cursor;

#[cfg(target_os = "windows")]
mod win32 {
    use std::{mem::size_of, ptr::null_mut};

    use windows_sys::Win32::{
        Graphics::Gdi::{
            BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS,
            DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP,
        },
        UI::WindowsAndMessaging::{
            CURSOR_SHOWING, CURSORINFO, CopyIcon, DestroyIcon, GetCursorInfo, GetIconInfo, HICON,
            ICONINFO,
        },
    };

    use crate::PointerCursor;

    struct OwnedIcon(HICON);
    impl Drop for OwnedIcon {
        fn drop(&mut self) {
            // SAFETY: this handle is an owned CopyIcon result, not the shared
            // system cursor. It is destroyed exactly once.
            unsafe { DestroyIcon(self.0) };
        }
    }

    struct OwnedBitmaps(ICONINFO);
    impl Drop for OwnedBitmaps {
        fn drop(&mut self) {
            // SAFETY: GetIconInfo allocates both bitmaps for the caller. Neither
            // is selected into a DC; null color handles are valid to skip.
            unsafe {
                DeleteObject(self.0.hbmMask);
                if !self.0.hbmColor.is_null() {
                    DeleteObject(self.0.hbmColor);
                }
            }
        }
    }

    pub(crate) fn pointer_cursor() -> Option<PointerCursor> {
        let mut info = CURSORINFO {
            cbSize: size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: info is an initialized, writable Win32 structure with cbSize.
        if unsafe { GetCursorInfo(&mut info) } == 0 || info.flags & CURSOR_SHOWING == 0 {
            return None;
        }
        Some(PointerCursor {
            position: (info.ptScreenPos.x, info.ptScreenPos.y),
            image: Some(cursor_image(info.hCursor)?),
        })
    }

    fn cursor_image(handle: HICON) -> Option<crate::CursorImage> {
        // SAFETY: handle is a live cursor. CopyIcon gives us a snapshot
        // independent of future cursor changes; owned wrappers release it and
        // GetIconInfo's bitmaps. Its out parameter is initialized and writable.
        unsafe {
            let icon = OwnedIcon(CopyIcon(handle));
            if icon.0.is_null() {
                return None;
            }
            let mut bitmaps = ICONINFO::default();
            if GetIconInfo(icon.0, &mut bitmaps) == 0 {
                return None;
            }
            let bitmaps = OwnedBitmaps(bitmaps);
            let mask = read_bitmap(bitmaps.0.hbmMask)?;
            let color = if bitmaps.0.hbmColor.is_null() {
                None
            } else {
                Some(read_bitmap(bitmaps.0.hbmColor)?)
            };
            super::from_bitmaps(color, mask, (bitmaps.0.xHotspot, bitmaps.0.yHotspot))
        }
    }

    fn read_bitmap(bitmap: HBITMAP) -> Option<image::RgbaImage> {
        let mut object = BITMAP::default();
        // SAFETY: bitmap belongs to the live OwnedBitmaps. GetObjectW writes
        // exactly the BITMAP allocation supplied here.
        if unsafe {
            GetObjectW(
                bitmap,
                size_of::<BITMAP>() as i32,
                (&mut object as *mut BITMAP).cast(),
            )
        } != size_of::<BITMAP>() as i32
        {
            return None;
        }
        let width = u32::try_from(object.bmWidth).ok()?;
        let height = u32::try_from(object.bmHeight).ok()?;
        let len = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(4)?;
        let mut pixels = vec![0; len];
        let mut format = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: object.bmWidth,
                biHeight: -object.bmHeight,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: the requested top-down 32bpp DIB has exactly width*height*4
        // bytes and no palette. GetDIBits is synchronous, and the source bitmap
        // is not selected into a DC. The memory DC is released on every path.
        let rows = unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let rows = GetDIBits(
                dc,
                bitmap,
                0,
                height,
                pixels.as_mut_ptr().cast(),
                &mut format,
                DIB_RGB_COLORS,
            );
            DeleteDC(dc);
            rows
        };
        if rows != height as i32 {
            return None;
        }
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            // Converting a 1/24bpp bitmap does not supply alpha. In particular,
            // monochrome XOR white must not be mistaken for alpha-white.
            if object.bmBitsPixel != 32 {
                pixel[3] = 0;
            }
        }
        image::RgbaImage::from_raw(width, height, pixels)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::{
            Graphics::Gdi::{CreateDIBSection, SelectObject},
            UI::WindowsAndMessaging::{
                DI_NORMAL, DrawIconEx, IDC_ARROW, IDC_HAND, IDC_IBEAM, LoadCursorW,
            },
        };

        #[test]
        fn native_cursor_pixels_match_windows_drawing_without_changing_system_cursor() {
            for id in [IDC_ARROW, IDC_HAND, IDC_IBEAM] {
                // SAFETY: LoadCursorW returns a shared cursor which we only
                // read/copy, never change or destroy. All test DC/DIB resources
                // below are private and freed before assertions can panic.
                let handle = unsafe { LoadCursorW(null_mut(), id) };
                assert!(!handle.is_null());
                let cursor = cursor_image(handle).expect("native cursor bitmap");
                let width = cursor.pixels.width() + 10;
                let height = cursor.pixels.height() + 10;
                for background in [[23, 61, 107], [0, 0, 0], [255, 255, 255]] {
                    let mut expected = image::RgbaImage::from_pixel(
                        width,
                        height,
                        image::Rgba([background[0], background[1], background[2], 255]),
                    );
                    cursor.overlay(
                        &mut expected,
                        (5 + cursor.hot_spot_x as i32, 5 + cursor.hot_spot_y as i32),
                        1.,
                        1.,
                    );
                    let format = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: width as i32,
                            biHeight: -(height as i32),
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    // SAFETY: the DIB is width*height*4 bytes, and GDI writes
                    // synchronously while selected. Deselect it before reading
                    // with GetDIBits and release the bitmap/DC on every result.
                    let (drawn, actual) = unsafe {
                        let dc = CreateCompatibleDC(null_mut());
                        assert!(!dc.is_null());
                        let mut bits = null_mut();
                        let bitmap =
                            CreateDIBSection(dc, &format, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                        assert!(!bitmap.is_null());
                        let buffer = std::slice::from_raw_parts_mut(
                            bits.cast::<u8>(),
                            width as usize * height as usize * 4,
                        );
                        for pixel in buffer.chunks_exact_mut(4) {
                            pixel.copy_from_slice(&[
                                background[2],
                                background[1],
                                background[0],
                                255,
                            ]);
                        }
                        let previous = SelectObject(dc, bitmap);
                        let drawn = DrawIconEx(dc, 5, 5, handle, 0, 0, 0, null_mut(), DI_NORMAL);
                        SelectObject(dc, previous);
                        let actual = read_bitmap(bitmap);
                        DeleteObject(bitmap);
                        DeleteDC(dc);
                        (drawn, actual)
                    };
                    assert_ne!(drawn, 0);
                    let actual = actual.expect("drawn Windows cursor");
                    for (expected, actual) in expected.pixels().zip(actual.pixels()) {
                        // GDI does not preserve destination alpha. Allow one
                        // RGB level for premultiplied vs straight-alpha rounding.
                        for channel in 0..3 {
                            assert!(expected[channel].abs_diff(actual[channel]) <= 1);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::from_bitmaps;

    #[test]
    fn color_cursor_keeps_size_hotspot_and_unpremultiplies_alpha() {
        let color = RgbaImage::from_fn(48, 36, |x, y| {
            if (x, y) == (3, 7) {
                Rgba([40, 70, 10, 128])
            } else {
                Rgba([0, 0, 0, 0])
            }
        });
        let cursor = from_bitmaps(Some(color), RgbaImage::new(48, 36), (3, 7)).unwrap();
        assert_eq!(cursor.pixels.get_pixel(3, 7).0, [80, 139, 20, 128]);
        assert_eq!((cursor.logical_width, cursor.logical_height), (48., 36.));
        assert_eq!((cursor.hot_spot_x, cursor.hot_spot_y), (3., 7.));
        assert!(cursor.and_mask.is_none());
    }

    #[test]
    fn monochrome_cursor_keeps_transparent_black_white_and_inverting_pixels() {
        // Top plane: AND, bottom plane: XOR. GetDIBits leaves alpha at zero.
        let mask = RgbaImage::from_fn(4, 2, |x, y| {
            let value = if y == 0 {
                [255, 0, 0, 255][x as usize]
            } else {
                [0, 0, 255, 255][x as usize]
            };
            Rgba([value, value, value, 0])
        });
        let cursor = from_bitmaps(None, mask, (1, 0)).unwrap();
        assert_eq!(cursor.pixels.dimensions(), (4, 1));
        let mut image = RgbaImage::from_pixel(4, 1, Rgba([23, 61, 107, 255]));
        cursor.overlay(&mut image, (1, 0), 1., 1.);
        assert_eq!(image.get_pixel(0, 0).0, [23, 61, 107, 255]);
        assert_eq!(image.get_pixel(1, 0).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(2, 0).0, [255, 255, 255, 255]);
        assert_eq!(image.get_pixel(3, 0).0, [232, 194, 148, 255]);
    }

    #[test]
    fn legacy_color_cursor_uses_its_mask_instead_of_zero_alpha() {
        let color = RgbaImage::from_pixel(1, 1, Rgba([12, 93, 147, 0]));
        let cursor = from_bitmaps(Some(color), RgbaImage::new(1, 1), (0, 0)).unwrap();
        let mut image = RgbaImage::from_pixel(1, 1, Rgba([23, 61, 107, 255]));
        cursor.overlay(&mut image, (0, 0), 1., 1.);
        assert_eq!(image.get_pixel(0, 0).0, [12, 93, 147, 255]);
    }
}
