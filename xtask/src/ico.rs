//! `cargo xtask ico` / `cargo xtask icns`: pack PNG images into a Windows `.ico` or a macOS `.icns`.
//!
//! Both formats store PNG payloads as is (ICO since Vista; ICNS since OS X 10.7), so no image
//! decoding is needed: sizes come from each PNG's IHDR chunk. packaging/icons.sh calls these.

/// Width and height from a PNG's IHDR chunk.
pub fn png_size(png: &[u8]) -> Result<(u32, u32), String> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    let (Some(sig), Some(ihdr), Some(w), Some(h)) = (png.get(..8), png.get(12..16), png.get(16..20), png.get(20..24)) else {
        return Err("not a PNG file".into());
    };
    if sig != SIG || ihdr != b"IHDR" {
        return Err("not a PNG file".into());
    }
    let be = |b: &[u8]| b.iter().fold(0u32, |acc, &x| (acc << 8) | u32::from(x));
    Ok((be(w), be(h)))
}

fn square(png: &[u8]) -> Result<u32, String> {
    match png_size(png)? {
        (w, h) if w == h && w > 0 => Ok(w),
        (w, h) => Err(format!("icon images must be square, got {w}x{h}")),
    }
}

/// An ICO file holding `pngs`, which must be square and at most 256 px.
pub fn pack_ico(pngs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let count = u16::try_from(pngs.len()).ok().filter(|n| *n > 0).ok_or("need 1 to 65535 PNGs")?;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * pngs.len();
    for png in pngs {
        let size = square(png)?;
        // One byte per dimension; 256 is written as 0.
        let dim = match size {
            256 => 0,
            s => u8::try_from(s).map_err(|_| format!("ICO images must be at most 256 px, got {s}"))?,
        };
        out.extend_from_slice(&[dim, dim, 0, 0]); // width, height, palette size, reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&u32::try_from(png.len()).map_err(|_| "PNG too large")?.to_le_bytes());
        out.extend_from_slice(&u32::try_from(offset).map_err(|_| "ICO too large")?.to_le_bytes());
        offset += png.len();
    }
    for png in pngs {
        out.extend_from_slice(png);
    }
    Ok(out)
}

/// ICNS element types holding PNG data, by pixel size (the 1x and 2x slots iconutil writes).
fn icns_types(size: u32) -> Result<&'static [&'static [u8; 4]], String> {
    Ok(match size {
        16 => &[b"icp4"],
        32 => &[b"icp5", b"ic11"],
        64 => &[b"icp6", b"ic12"],
        128 => &[b"ic07"],
        256 => &[b"ic08", b"ic13"],
        512 => &[b"ic09", b"ic14"],
        1024 => &[b"ic10"],
        s => return Err(format!("ICNS images must be 16, 32, 64, 128, 256, 512 or 1024 px, got {s}")),
    })
}

/// An ICNS file holding `pngs`, each stored in every slot of its size.
pub fn pack_icns(pngs: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    if pngs.is_empty() {
        return Err("need at least one PNG".into());
    }
    let mut body = Vec::new();
    for png in pngs {
        for ty in icns_types(square(png)?)? {
            body.extend_from_slice(*ty);
            body.extend_from_slice(&u32::try_from(png.len() + 8).map_err(|_| "PNG too large")?.to_be_bytes());
            body.extend_from_slice(png);
        }
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&u32::try_from(body.len() + 8).map_err(|_| "ICNS too large")?.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

type Packer = fn(&[Vec<u8>]) -> Result<Vec<u8>, String>;

fn run(args: &[&str], ext: &str, pack: Packer) -> Result<(), String> {
    let [out, inputs @ ..] = args else {
        return Err(format!("usage: cargo xtask {ext} <out.{ext}> <in.png>…"));
    };
    let pngs = inputs.iter().map(|p| std::fs::read(p).map_err(|e| format!("read {p}: {e}"))).collect::<Result<Vec<_>, _>>()?;
    let file = pack(&pngs)?;
    std::fs::write(out, &file).map_err(|e| format!("write {out}: {e}"))?;
    println!("{out}: {} images, {} bytes", pngs.len(), file.len());
    Ok(())
}

pub fn run_ico(args: &[&str]) -> Result<(), String> {
    run(args, "ico", pack_ico)
}

pub fn run_icns(args: &[&str]) -> Result<(), String> {
    run(args, "icns", pack_icns)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_png(size: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(&size.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 1, 2, 3, 4]);
        v
    }

    fn u32_le(b: &[u8]) -> u32 {
        u32::from_le_bytes(b.try_into().unwrap())
    }

    #[test]
    fn packs_ico_directory_and_payloads() {
        let (a, b) = (fake_png(16), fake_png(256));
        let ico = pack_ico(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 2, 0]);
        assert_eq!(&ico[6..8], &[16, 16]);
        assert_eq!(u32_le(&ico[14..18]), a.len() as u32);
        assert_eq!(u32_le(&ico[18..22]), 38);
        assert_eq!(&ico[22..24], &[0, 0]); // 256 px is stored as 0
        assert_eq!(u32_le(&ico[34..38]), 38 + a.len() as u32);
        assert_eq!(&ico[38..38 + a.len()], &a[..]);
        assert_eq!(&ico[38 + a.len()..], &b[..]);
    }

    #[test]
    fn packs_icns_elements() {
        let (a, b) = (fake_png(16), fake_png(32));
        let icns = pack_icns(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        // 16 px -> icp4; 32 px -> icp5 and ic11.
        let types: Vec<&[u8]> = vec![&icns[8..12], &icns[8 + 8 + a.len()..12 + 8 + a.len()], &icns[8 + 16 + a.len() + b.len()..12 + 16 + a.len() + b.len()]];
        assert_eq!(types, [b"icp4" as &[u8], b"icp5", b"ic11"]);
        assert_eq!(u32::from_be_bytes(icns[12..16].try_into().unwrap()) as usize, a.len() + 8);
        assert_eq!(icns.len(), 8 + (8 + a.len()) + 2 * (8 + b.len()));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(pack_ico(&[]).is_err());
        assert!(pack_ico(&[fake_png(512)]).is_err());
        assert!(pack_ico(&[b"GIF89a".to_vec()]).is_err());
        assert!(pack_icns(&[fake_png(48)]).is_err());
        assert!(pack_icns(&[]).is_err());
        assert_eq!(png_size(&fake_png(48)).unwrap(), (48, 48));
    }

    #[test]
    fn committed_icons_are_valid() {
        let dir = crate::root().join("assets/app-icon");
        if let Ok(ico) = std::fs::read(dir.join("texturecraft.ico")) {
            assert_eq!(&ico[..4], &[0, 0, 1, 0]);
            assert!(u16::from_le_bytes([ico[4], ico[5]]) >= 4);
        }
        if let Ok(icns) = std::fs::read(dir.join("texturecraft.icns")) {
            assert_eq!(&icns[..4], b"icns");
            assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
        }
    }
}
