//! IMG module (design §7.2): decode → orientation fix → downscale to the
//! long-edge cap → re-encode JPEG q85. EXIF/GPS can't survive: the JPEG
//! encoder writes no metadata, and the re-encode is unconditional.

use std::io::Cursor;

/// Processed long edge (§7.2 default; a setting can lower it, never raise it
/// past the vision-encoding sweet spot of 1568).
pub const MAX_LONG_EDGE: u32 = 1568;
/// JPEG quality for the processed copy (§7.2).
pub const JPEG_QUALITY: u8 = 85;

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedImage {
    /// JPEG-encoded processed copy (the bytes sent to providers).
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Decode `raw`, apply the EXIF orientation, downscale to `max_edge`, and
/// re-encode as JPEG. Returns an error string for undecodable/mismatched
/// data (callers map it to a user-visible message).
pub fn process(raw: &[u8], max_edge: u32) -> Result<ProcessedImage, String> {
    let reader = image::ImageReader::new(Cursor::new(raw))
        .with_guessed_format()
        .map_err(|e| format!("image: {e}"))?;
    // Orientation lives on the decoder (EXIF), read before decode consumes it.
    let mut decoder = reader.into_decoder().map_err(|e| format!("decode: {e}"))?;
    let orientation =
        image::ImageDecoder::orientation(&mut decoder).map_err(|e| format!("decode: {e}"))?;
    let mut img =
        image::DynamicImage::from_decoder(decoder).map_err(|e| format!("decode: {e}"))?;
    img.apply_orientation(orientation);

    let (w, h) = (img.width(), img.height());
    if w.max(h) > max_edge {
        img = img.resize(max_edge, max_edge, image::imageops::FilterType::Triangle);
    }

    let mut out: Vec<u8> = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
        .encode_image(&img)
        .map_err(|e| format!("encode: {e}"))?;

    let (width, height) = (img.width(), img.height());
    Ok(ProcessedImage {
        bytes: out,
        width,
        height,
    })
}

/// Canonical file extension for an attachment's original copy.
pub fn ext_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

/// Sniff the image MIME from magic bytes (declared mimes from the webview
/// are untrusted, §10.3).
pub fn sniff_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// First frame only for GIF (§7.1): the decoder stops at frame 1 naturally.

/// One stored attachment: original + processed file paths plus the metadata
/// that lands in the `attachments` DB row (§8.2). `id` is caller-linkable;
/// files are deduped by sha256 name (§8.1), so identical uploads share files.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredImage {
    pub id: String,
    pub path: String,
    pub processed_path: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    /// Original byte size (the processed copy is a separate JPEG).
    pub bytes: i64,
    pub sha256: String,
}

/// Process + write both copies under `dir`, named by content hash.
pub fn store(dir: &std::path::Path, raw: &[u8]) -> Result<StoredImage, String> {
    let mime = sniff_mime(raw);
    if !mime.starts_with("image/") {
        return Err(format!("unsupported image type: {mime}"));
    }
    let processed = process(raw, MAX_LONG_EDGE)?;
    let sha = sha256_hex(raw);

    let original_name = format!("{sha}.{}", ext_for_mime(mime));
    let processed_name = format!("{sha}_processed.jpg");
    let original_path = dir.join(&original_name);
    let processed_path = dir.join(&processed_name);

    // Skip writes when the exact same bytes were stored before (dedupe).
    if !original_path.exists() {
        std::fs::write(&original_path, raw).map_err(|e| format!("write: {e}"))?;
    }
    if !processed_path.exists() {
        std::fs::write(&processed_path, &processed.bytes)
            .map_err(|e| format!("write: {e}"))?;
    }

    Ok(StoredImage {
        id: crate::ids::new_id(),
        path: original_path.display().to_string(),
        processed_path: processed_path.display().to_string(),
        mime: mime.to_string(),
        width: processed.width,
        height: processed.height,
        bytes: raw.len() as i64,
        sha256: sha,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use image::ImageEncoder;

    /// Test helper: a real (blank) PNG, also used by tools/fs/read tests.
    pub(crate) fn make_png(w: u32, h: u32) -> Vec<u8> {
        let img = image::DynamicImage::new_rgb8(w, h);
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut std::io::Cursor::new(&mut out))
            .write_image(&img.to_rgb8(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    #[test]
    fn png_roundtrips_through_jpeg() {
        let raw = make_png(64, 32);
        let out = process(&raw, MAX_LONG_EDGE).unwrap();
        assert_eq!(out.width, 64);
        assert_eq!(out.height, 32);
        assert_eq!(sniff_mime(&out.bytes), "image/jpeg");
    }

    #[test]
    fn oversized_images_downscale_to_the_cap() {
        let raw = make_png(3000, 1000);
        let out = process(&raw, 1568).unwrap();
        assert_eq!(out.width, 1568);
        assert_eq!(out.height, 523, "aspect preserved: 1000*1568/3000");

        // Already under the cap → untouched.
        let small = process(&make_png(64, 32), 1568).unwrap();
        assert_eq!((small.width, small.height), (64, 32));
    }

    #[test]
    fn jpeg_reencode_strips_metadata() {
        // A PNG is decoded and re-encoded as JPEG — no EXIF/GPS survives.
        let out = process(&make_png(100, 100), 1568).unwrap();
        let s = out.bytes;
        assert!(s.starts_with(&[0xFF, 0xD8]), "JPEG magic present");
        assert!(!s.windows(4).any(|w| w == b"Exif"), "no EXIF segment");
        assert!(!s.windows(4).any(|w| w == b"GPS "), "no GPS segment");
    }

    #[test]
    fn sniff_covers_the_supported_set() {
        assert_eq!(sniff_mime(&make_png(2, 2)), "image/png");
        assert_eq!(sniff_mime(b"GIF89a"), "image/gif");
        assert_eq!(sniff_mime(b"BM\x00\x00"), "image/bmp");
        assert_eq!(sniff_mime(b"\xff\xd8\xff\xe0"), "image/jpeg");
        assert_eq!(sniff_mime(b"not an image"), "application/octet-stream");
    }

    #[test]
    fn junk_input_is_an_error() {
        assert!(process(b"not an image", 1568).is_err());
    }

    #[test]
    fn ext_map_matches_mime() {
        assert_eq!(ext_for_mime("image/jpeg"), "jpg");
        assert_eq!(ext_for_mime("image/png"), "png");
        assert_eq!(ext_for_mime("image/webp"), "webp");
    }

    #[test]
    fn store_writes_both_copies_and_dedupes_by_hash() {
        let dir = tempfile::tempdir().unwrap();
        let raw = make_png(100, 60);
        let first = store(dir.path(), &raw).unwrap();
        assert_eq!(first.mime, "image/png");
        assert_eq!(first.bytes, raw.len() as i64);
        assert_eq!(first.sha256, sha256_hex(&raw));
        assert!(std::path::Path::new(&first.path).exists());
        assert!(std::path::Path::new(&first.processed_path).exists());
        assert!(first.processed_path.ends_with("_processed.jpg"));

        // Identical bytes → new id, same file names (dedupe, §8.1).
        let second = store(dir.path(), &raw).unwrap();
        assert_ne!(second.id, first.id);
        assert_eq!(second.path, first.path);
        assert_eq!(second.sha256, first.sha256);

        // Non-images are refused.
        assert!(store(dir.path(), b"not an image").is_err());
    }
}