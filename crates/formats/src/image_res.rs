//! Decoding imported image resources (fills, alphas, stencils).

use crate::{FormatError, MAX_IMAGE_SIZE, Result};

/// A decoded image: RGBA as 0..1 floats in the file's own encoding (8/16-bit files stay
/// sRGB-encoded; `srgb` says so), row-major, top row first.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<[f32; 4]>,
    /// True for 8/16-bit images (sRGB-encoded); false for float images (linear).
    pub srgb: bool,
}

/// Decode PNG, JPEG, TGA, TIFF, BMP, WEBP, EXR or HDR bytes.
pub fn decode_image(bytes: &[u8]) -> Result<DecodedImage> {
    let reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| FormatError::Malformed { kind: "image", msg: e.to_string() })?;
    let (w, h) = reader.into_dimensions().map_err(|e| FormatError::Malformed { kind: "image", msg: e.to_string() })?;
    if w == 0 || h == 0 || w > MAX_IMAGE_SIZE || h > MAX_IMAGE_SIZE {
        return Err(FormatError::TooLarge(format!("image is {w}x{h} (limit {MAX_IMAGE_SIZE})")));
    }
    let img = crate::guard("image", || image::load_from_memory(bytes).map_err(|e| FormatError::Malformed { kind: "image", msg: e.to_string() }))?;
    let srgb = !matches!(img, image::DynamicImage::ImageRgb32F(_) | image::DynamicImage::ImageRgba32F(_));
    let f = img.into_rgba32f();
    let (width, height) = f.dimensions();
    let rgba = f.pixels().map(|p| p.0).collect();
    Ok(DecodedImage { width, height, rgba, srgb })
}

/// PNG bytes of an `rgb` (sRGB) image whose alpha is `coverage` (`width × height`, top row first):
/// drawn text as an image resource.
pub fn coverage_png(width: u32, height: u32, coverage: &[u8], rgb: [u8; 3]) -> Result<Vec<u8>> {
    let rgba: Vec<u8> = coverage.iter().flat_map(|a| [rgb[0], rgb[1], rgb[2], *a]).collect();
    let img = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| FormatError::Invalid(format!("{} coverage values for {width}x{height}", coverage.len())))?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| FormatError::Invalid(e.to_string()))?;
    Ok(png)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_png_and_rejects_garbage() {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_fn(4, 2, |x, _| image::Rgba([x as u8 * 60, 0, 0, 255]));
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let d = decode_image(&png).unwrap();
        assert_eq!((d.width, d.height), (4, 2));
        assert!(d.srgb);
        assert!((d.rgba[1][0] - 60.0 / 255.0).abs() < 1e-3);
        assert!(decode_image(b"not an image").is_err());
        assert!(decode_image(&png[..png.len() / 2]).is_err());
        let text = decode_image(&coverage_png(2, 1, &[0, 255], [255; 3]).unwrap()).unwrap();
        assert_eq!(text.rgba, vec![[1.0, 1.0, 1.0, 0.0], [1.0; 4]]);
        assert!(coverage_png(2, 2, &[0], [255; 3]).is_err());
    }
}
