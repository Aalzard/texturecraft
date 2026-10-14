//! Ceilings for automation replies: rendered and screenshot PNGs and JSON payloads.

use serde::Serialize;

use crate::{AutomationError, bad};

/// Largest JSON reply (MCP tool text, one JSON line).
pub const MAX_RESPONSE_BYTES: usize = 8 << 20;
/// Largest PNG returned to a client (before base64).
pub const MAX_PNG_BYTES: usize = 5 << 20;
/// Largest render or screenshot edge returned to a client; larger screenshots are downscaled.
pub const MAX_PNG_SIDE: u32 = 2048;
/// Largest render edge for local CLI renders.
pub const MAX_LOCAL_RENDER_SIDE: u32 = 4096;
/// Largest reply line read from the app (full-window screenshots are big).
pub const MAX_BRIDGE_REPLY_BYTES: usize = 64 << 20;

/// Serialize compactly, refusing replies over [`MAX_RESPONSE_BYTES`].
pub fn json_text(v: &impl Serialize) -> Result<String, AutomationError> {
    let s = serde_json::to_string(v).map_err(|e| AutomationError::Other(format!("cannot encode the reply: {e}")))?;
    if s.len() > MAX_RESPONSE_BYTES {
        return Err(bad(format!("reply is {} bytes, over the {MAX_RESPONSE_BYTES}-byte limit; the operation may have completed", s.len())));
    }
    Ok(s)
}

pub fn check_png(bytes: usize) -> Result<(), AutomationError> {
    if bytes > MAX_PNG_BYTES {
        return Err(bad(format!("PNG is {bytes} bytes, over the {MAX_PNG_BYTES}-byte limit; render smaller")));
    }
    Ok(())
}

pub fn encode_png(w: u32, h: u32, rgba: Vec<u8>) -> Result<Vec<u8>, AutomationError> {
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| AutomationError::Other("render buffer has the wrong size".into()))?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(|e| AutomationError::Other(format!("PNG encoding failed: {e}")))?;
    Ok(png)
}

/// Downscale a PNG (an app screenshot) so its longest edge is at most `max_side`, then check its
/// byte size. Decoding is capped: a small compressed reply can still claim a huge image.
pub fn fit_png(png: Vec<u8>, max_side: u32) -> Result<Vec<u8>, AutomationError> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(1 << 30);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| AutomationError::Other(format!("screenshot is not a valid PNG: {e}")))?;
    let out = if img.width().max(img.height()) > max_side {
        let small = img.thumbnail(max_side, max_side).into_rgba8();
        encode_png(small.width(), small.height(), small.into_raw())?
    } else {
        png
    };
    check_png(out.len())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_png_downscales_and_rejects_garbage() {
        let big = encode_png(3000, 10, vec![200; 3000 * 10 * 4]).unwrap();
        let small = fit_png(big, MAX_PNG_SIDE).unwrap();
        let img = image::load_from_memory(&small).unwrap();
        assert_eq!(img.width(), MAX_PNG_SIDE);
        assert!(fit_png(b"not a png".to_vec(), 64).is_err());
        assert!(json_text(&"x".repeat(MAX_RESPONSE_BYTES)).is_err());
        assert!(check_png(MAX_PNG_BYTES + 1).is_err());
    }
}
