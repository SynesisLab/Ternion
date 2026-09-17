//! Region capture (design §7.1): `Win+Alt+S` opens a transparent overlay;
//! the user drags a rect; the overlay's physical-pixel selection lands in
//! `capture_screen_region`, which BitBlts exactly that rect off the screen
//! DC, converts BGRA→RGBA, and encodes PNG bytes for the IMG pipeline.
//!
//! GDI over Windows.Graphics.Capture (the design's suggestion): WGC cannot
//! capture a sub-region directly and pulls in WinRT frame pools; BitBlt is
//! one syscall deep and deterministic. The function signature stays, so a
//! WGC backend (HDR, protected-content) can slot in later.

/// GDI BitBlt of one screen rect (physical pixels), PNG-encoded for `img::store`.
#[cfg(windows)]
pub fn capture_screen_region(x: i32, y: i32, width: i32, height: i32) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, RGBQUAD, BI_RGB,
        DIB_RGB_COLORS, SRCCOPY,
    };

    if width <= 0 || height <= 0 {
        return Err("empty capture region".into());
    }
    unsafe {
        let screen = GetDC(std::ptr::null_mut() as HWND);
        if screen.is_null() {
            return Err("GetDC(screen) failed".into());
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, width, height);
        let old = SelectObject(mem, bmp as *mut _);
        let blit = BitBlt(mem, 0, 0, width, height, screen, x, y, SRCCOPY);

        // Top-down 32bpp DIB: negative biHeight, no padding needed on 4-byte rows.
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            // All-zero palette: irrelevant for a 32bpp BI_RGB DIB.
            bmiColors: [RGBQUAD {
                rgbBlue: 0,
                rgbGreen: 0,
                rgbRed: 0,
                rgbReserved: 0,
            }],
        };
        let mut buf = vec![0u8; (width as usize) * (height as usize) * 4];
        let copied = GetDIBits(
            mem,
            bmp,
            0,
            height as u32,
            buf.as_mut_ptr().cast(),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut() as HWND, screen);

        if blit == 0 || copied != height {
            return Err("BitBlt/GetDIBits failed".into());
        }
        let rgba = bgra_to_rgba(&buf, width as usize, height as usize);
        encode_png(rgba, width as u32, height as u32)
    }
}

#[cfg(not(windows))]
pub fn capture_screen_region(_x: i32, _y: i32, _w: i32, _h: i32) -> Result<Vec<u8>, String> {
    Err("region capture is Windows-only".into())
}

/// GDI hands back BGRA; the image crate wants RGBA.
pub fn bgra_to_rgba(bgra: &[u8], width: usize, height: usize) -> Vec<u8> {
    let mut rgba = bgra.to_vec();
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2); // B→R, R→B; A stays
    }
    let _ = (width, height); // row length is derivable; kept for clarity at call sites
    rgba
}

/// In-memory PNG (§7.2: `img::store` sniffs magic bytes and runs the full
/// pipeline from encoded file bytes).
pub fn encode_png(rgba: Vec<u8>, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| "capture buffer size mismatch".to_string())?;
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_swaps_to_rgba() {
        // 2×1: red pixel (BGRA = B,G,R,A) and a blue one.
        let bgra = vec![0, 0, 255, 255, 255, 0, 0, 255];
        let rgba = bgra_to_rgba(&bgra, 2, 1);
        assert_eq!(rgba, vec![255, 0, 0, 255, 0, 0, 255, 255]);
    }

    #[test]
    fn png_encode_produces_a_sniffable_image() {
        let rgba = vec![10, 20, 30, 255].repeat(9); // 3×3 solid-color image
        let png = encode_png(rgba, 3, 3).unwrap();
        // img::store sniffs magic bytes — the capture must look like a PNG.
        assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 3));
    }

    #[test]
    fn empty_regions_are_refused() {
        assert!(capture_screen_region(0, 0, 0, 100).is_err());
        assert!(capture_screen_region(0, 0, 100, -1).is_err());
    }
}