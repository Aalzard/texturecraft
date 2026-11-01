//! Embeds the optional craft-fonts UI fonts (storytold/craft-fonts) when `CRAFT_FONTS_DIR` points
//! at a checkout: IBM Plex Sans and Mono for the interface (from its Latin catalog) and the CJK
//! faces; without it `CRAFT_FONTS` is empty and the UI uses egui's and the system's fonts. The
//! recipe follows craft-fonts' `docs/integration.md`; no font file is committed here.

use std::fmt::Write as _;
use std::path::PathBuf;

/// The UI faces TextureCraft embeds (family, style) per manifest: desktop, then web (the web build
/// has a download budget: of the CJK faces only the Japanese one, ~4.5 MB).
const MANIFESTS: &[(&str, Faces, Faces)] = &[
    ("fonts/manifest.txt", &[("BIZ UDPGothic", "Regular"), ("Noto Sans CJK SC", "Regular")], &[("BIZ UDPGothic", "Regular")]),
    // The interface font: one variable file for every weight (0.5 MB), and the numbers' face.
    (LATIN_MANIFEST, LATIN, LATIN),
];
/// Newer than the core catalog: a checkout without it keeps the default interface font.
const LATIN_MANIFEST: &str = "fonts/latin-manifest.txt";
const LATIN: Faces = &[("IBM Plex Sans", "Variable"), ("IBM Plex Mono", "Regular")];
/// (family, style) pairs of a manifest.
type Faces = &'static [(&'static str, &'static str)];

fn main() {
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_DIR");
    println!("cargo::rerun-if-env-changed=CRAFT_FONTS_REQUIRED");
    let mut src = String::from("pub static CRAFT_FONTS: &[CraftFont] = &[\n");
    if let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) {
        match MANIFESTS.iter().map(|(rel, desktop, web)| craft_fonts(&dir, rel, desktop, web)).collect::<Result<String, String>>() {
            Ok(entries) => src.push_str(&entries),
            Err(e) if std::env::var_os("CRAFT_FONTS_REQUIRED").is_some() => {
                println!("cargo::error=CRAFT_FONTS_DIR={}: {e}", dir.display());
            }
            Err(e) => println!("cargo::warning=building without craft-fonts: CRAFT_FONTS_DIR={}: {e}", dir.display()),
        }
    }
    src.push_str("];\n");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("craft_fonts.rs");
    if let Err(e) = std::fs::write(&out, src) {
        println!("cargo::error=writing {}: {e}", out.display());
    }
}

/// One `CraftFont { .. }` initialiser per wanted line of manifest `rel`.
fn craft_fonts(dir: &std::path::Path, rel: &str, desktop: &[(&str, &str)], web: &[(&str, &str)]) -> Result<String, String> {
    let manifest = dir.join(rel);
    println!("cargo::rerun-if-changed={}", manifest.display());
    if rel == LATIN_MANIFEST && !manifest.exists() {
        return Ok(String::new());
    }
    let text = std::fs::read_to_string(&manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").is_ok_and(|a| a == "wasm32");
    let wanted = if wasm { web } else { desktop };
    let mut out = String::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split(" | ").map(str::trim).collect();
        let [family, style, file, scripts, ..] = f.as_slice() else {
            return Err(format!("malformed manifest line: {line}"));
        };
        if !wanted.contains(&(*family, *style)) {
            continue;
        }
        let path = dir.join(file).canonicalize().map_err(|e| format!("{file}: {e}"))?;
        println!("cargo::rerun-if-changed={}", path.display());
        let scripts: Vec<String> = scripts.split(',').map(|s| format!("{:?}", s.trim())).collect();
        let _ = writeln!(
            out,
            "    CraftFont {{ family: {family:?}, style: {style:?}, scripts: &[{}], bytes: include_bytes!({:?}) }},",
            scripts.join(", "),
            path.display().to_string(),
        );
    }
    Ok(out)
}
