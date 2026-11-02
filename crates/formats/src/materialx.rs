//! MaterialX documents for exported texture sets, written from the ASWF MaterialX specification
//! (version 1.39) and the OpenPBR Surface specification. Pure Rust: no MaterialX library is
//! linked, and nothing is read from any other application.
//!
//! [`write`] takes a material's name and the texture files an export produced, each tagged with
//! the OpenPBR input it feeds, and gives the text of a `.mtlx` file: an `open_pbr_surface` shader
//! whose inputs read `tiledimage` nodes, wrapped in a `surfacematerial`. The inputs are the ones
//! the built-in "OpenPBR (separate maps)" export preset writes ([`INPUTS`]); `height` becomes a
//! `displacement` shader on the material and `geometry_normal` goes through a `normalmap` node
//! (OpenGL, +Y up, which is what that node expects).
//!
//! Colour textures (base, fuzz and emission colour) are tagged with the colour space they were
//! exported in ([`colorspace_name`]); every data map is left untagged (linear). UDIM sets write the `<UDIM>` filename token MaterialX defines.
//! Texture file names are relative to the document.

use std::collections::HashSet;
use std::fmt::Write as _;

use texturecraft_texel::ColorSpace;

/// How an OpenPBR input is typed in the document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `color3` from an sRGB texture.
    Color,
    /// `float` from a linear texture.
    Float,
    /// `vector3` through a `normalmap` node.
    Normal,
    /// `float` driving a `displacement` shader.
    Height,
}

/// The OpenPBR inputs the export writes, in document order. Names are the file-name suffixes of
/// the built-in OpenPBR preset's maps (`$mesh_$textureSet_<input>`).
pub const INPUTS: &[(&str, Kind)] = &[
    ("base_color", Kind::Color),
    ("base_metalness", Kind::Float),
    ("specular_roughness", Kind::Float),
    ("specular_roughness_anisotropy", Kind::Float),
    ("coat_weight", Kind::Float),
    ("coat_roughness", Kind::Float),
    ("fuzz_weight", Kind::Float),
    ("fuzz_color", Kind::Color),
    ("fuzz_roughness", Kind::Float),
    ("transmission_weight", Kind::Float),
    ("subsurface_weight", Kind::Float),
    ("emission_color", Kind::Color),
    ("geometry_opacity", Kind::Float),
    ("geometry_normal", Kind::Normal),
    ("height", Kind::Height),
];

/// The file-name prefix of the built-in OpenPBR preset's maps.
const PREFIX: &str = "$mesh_$textureSet_";

/// One exported texture: the OpenPBR input it feeds and the file (relative to the `.mtlx`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub input: String,
    pub file: String,
    /// The MaterialX colour space of the file's values (`srgb_texture`, `lin_rec709`, ...);
    /// used for colour inputs only, data maps are always linear.
    pub colorspace: &'static str,
}

/// The MaterialX colour space of colour maps written in `space`. MaterialX 1.39 defines no
/// ACES2065-1 (AP0) space: it gets `lin_ap0_scene`, its name in later versions of the
/// specification, which 1.39 readers may not know.
pub fn colorspace_name(space: ColorSpace) -> &'static str {
    match space {
        ColorSpace::Srgb => "srgb_texture",
        ColorSpace::LinearRec709 => "lin_rec709",
        ColorSpace::AcesCg => "acescg",
        ColorSpace::Aces2065 => "lin_ap0_scene",
    }
}

/// The OpenPBR input a preset map's file template feeds (`$mesh_$textureSet_base_color` →
/// `base_color`), or None for templates this module has no place for.
pub fn input_of_template(template: &str) -> Option<&'static str> {
    let suffix = template.strip_prefix(PREFIX)?;
    INPUTS.iter().find(|(n, _)| *n == suffix).map(|(n, _)| *n)
}

fn kind_of(input: &str) -> Option<Kind> {
    INPUTS.iter().find(|(n, _)| *n == input).map(|(_, k)| *k)
}

/// A MaterialX name: letters, digits and underscores, not starting with a digit.
pub fn node_name(s: &str) -> String {
    let mut out: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if out.is_empty() || out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

fn image_name(input: &str) -> String {
    format!("image_{}", node_name(input))
}

/// The text of a MaterialX document for `name` with the given textures. `physical_size_cm` goes
/// into the material's `doc` attribute. Inputs this module has no place for are skipped, and so
/// are repeats of one input (the first wins).
pub fn write(name: &str, textures: &[Texture], physical_size_cm: Option<[f32; 2]>) -> String {
    let base = node_name(name);
    let mut seen = HashSet::new();
    let tex: Vec<(&Texture, Kind)> = textures.iter().filter_map(|t| kind_of(&t.input).map(|k| (t, k))).filter(|(t, _)| seen.insert(t.input.as_str())).collect();
    let has = |i: &str| tex.iter().any(|(t, _)| t.input == i);
    let mut x = String::new();
    let _ = writeln!(x, "<?xml version=\"1.0\"?>");
    let _ = writeln!(x, "<materialx version=\"1.39\" colorspace=\"lin_rec709\">");

    let shader = format!("OPBR_{base}");
    let _ = writeln!(x, "  <open_pbr_surface name=\"{}\" type=\"surfaceshader\">", esc(&shader));
    for (t, kind) in &tex {
        let (ty, node) = match kind {
            Kind::Color => ("color3", image_name(&t.input)),
            Kind::Float => ("float", image_name(&t.input)),
            Kind::Normal => ("vector3", "normalmap".to_owned()),
            Kind::Height => continue,
        };
        let _ = writeln!(x, "    <input name=\"{}\" type=\"{ty}\" nodename=\"{node}\" />", t.input);
    }
    if has("emission_color") {
        let _ = writeln!(x, "    <input name=\"emission_luminance\" type=\"float\" value=\"1\" />");
    }
    let _ = writeln!(x, "  </open_pbr_surface>");

    for (t, kind) in &tex {
        let (ty, space) = match kind {
            Kind::Color => ("color3", Some(t.colorspace)),
            Kind::Normal => ("vector3", None),
            Kind::Float | Kind::Height => ("float", None),
        };
        let node = image_name(&t.input);
        let _ = writeln!(x, "  <tiledimage name=\"{node}\" type=\"{ty}\">");
        match space {
            Some(space) => {
                let _ = writeln!(x, "    <input name=\"file\" type=\"filename\" value=\"{}\" colorspace=\"{space}\" />", esc(&t.file));
            }
            None => {
                let _ = writeln!(x, "    <input name=\"file\" type=\"filename\" value=\"{}\" />", esc(&t.file));
            }
        }
        let _ = writeln!(x, "  </tiledimage>");
    }
    if has("geometry_normal") {
        let _ = writeln!(x, "  <normalmap name=\"normalmap\" type=\"vector3\">");
        let _ = writeln!(x, "    <input name=\"in\" type=\"vector3\" nodename=\"{}\" />", image_name("geometry_normal"));
        let _ = writeln!(x, "  </normalmap>");
    }
    if has("height") {
        let _ = writeln!(x, "  <displacement name=\"DISP_{base}\" type=\"displacementshader\">");
        let _ = writeln!(x, "    <input name=\"displacement\" type=\"float\" nodename=\"{}\" />", image_name("height"));
        let _ = writeln!(x, "    <input name=\"scale\" type=\"float\" value=\"1\" />");
        let _ = writeln!(x, "  </displacement>");
    }

    let doc = match physical_size_cm {
        Some([w, h]) if w.is_finite() && h.is_finite() => format!(" doc=\"Tiles every {w} x {h} cm\""),
        _ => String::new(),
    };
    let _ = writeln!(x, "  <surfacematerial name=\"{}\" type=\"material\"{doc}>", esc(&base));
    let _ = writeln!(x, "    <input name=\"surfaceshader\" type=\"surfaceshader\" nodename=\"{}\" />", esc(&shader));
    if has("height") {
        let _ = writeln!(x, "    <input name=\"displacementshader\" type=\"displacementshader\" nodename=\"DISP_{base}\" />");
    }
    let _ = writeln!(x, "  </surfacematerial>");
    let _ = writeln!(x, "</materialx>");
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(input: &str, file: &str) -> Texture {
        Texture { input: input.into(), file: file.into(), colorspace: "srgb_texture" }
    }

    /// Every start tag is closed or self-closed, in order: enough to catch a malformed writer.
    fn balanced(xml: &str) -> bool {
        let mut stack: Vec<String> = Vec::new();
        let mut rest = xml;
        while let Some(i) = rest.find('<') {
            let Some(j) = rest.get(i..).and_then(|r| r.find('>')) else { return false };
            let Some(tag) = rest.get(i + 1..i + j) else { return false };
            rest = rest.get(i + j + 1..).unwrap_or("");
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                if stack.pop().as_deref() != Some(name.trim()) {
                    return false;
                }
            } else if !tag.ends_with('/') {
                stack.push(tag.split_whitespace().next().unwrap_or("").to_owned());
            }
        }
        stack.is_empty()
    }

    #[test]
    fn a_full_material_maps_every_input() {
        let t: Vec<Texture> = INPUTS.iter().map(|(n, _)| tex(n, &format!("stone_{n}.png"))).collect();
        let x = write("Stone", &t, Some([100.0, 50.0]));
        assert!(balanced(&x), "{x}");
        for want in [
            "<materialx version=\"1.39\"",
            "<open_pbr_surface name=\"OPBR_Stone\" type=\"surfaceshader\">",
            "name=\"base_color\" type=\"color3\" nodename=\"image_base_color\"",
            "name=\"base_metalness\" type=\"float\" nodename=\"image_base_metalness\"",
            "name=\"specular_roughness\" type=\"float\" nodename=\"image_specular_roughness\"",
            "name=\"coat_weight\" type=\"float\" nodename=\"image_coat_weight\"",
            "name=\"fuzz_color\" type=\"color3\" nodename=\"image_fuzz_color\"",
            "name=\"transmission_weight\" type=\"float\"",
            "name=\"subsurface_weight\" type=\"float\"",
            "name=\"geometry_opacity\" type=\"float\" nodename=\"image_geometry_opacity\"",
            "name=\"geometry_normal\" type=\"vector3\" nodename=\"normalmap\"",
            "name=\"emission_color\" type=\"color3\" nodename=\"image_emission_color\"",
            "name=\"emission_luminance\" type=\"float\" value=\"1\"",
            "value=\"stone_base_color.png\" colorspace=\"srgb_texture\"",
            "value=\"stone_emission_color.png\" colorspace=\"srgb_texture\"",
            "<normalmap name=\"normalmap\" type=\"vector3\">",
            "<displacement name=\"DISP_Stone\" type=\"displacementshader\">",
            "name=\"displacementshader\"",
            "<surfacematerial name=\"Stone\" type=\"material\" doc=\"Tiles every 100 x 50 cm\">",
        ] {
            assert!(x.contains(want), "missing {want}\n{x}");
        }
        // Data textures carry no colour space, and height is not an OpenPBR input.
        assert!(x.contains("value=\"stone_specular_roughness.png\" />"));
        assert!(!x.contains("name=\"height\""));
    }

    #[test]
    fn only_what_is_there_is_written() {
        let x = write("m", &[tex("specular_roughness", "r.png")], None);
        assert!(balanced(&x));
        assert!(!x.contains("base_color") && !x.contains("normalmap") && !x.contains("displacement") && !x.contains("emission_luminance"));
        assert!(x.contains("<surfacematerial name=\"m\" type=\"material\">"));
        let none = write("m", &[], None);
        assert!(balanced(&none) && none.contains("<open_pbr_surface") && !none.contains("tiledimage"));
    }

    #[test]
    fn colour_inputs_carry_their_files_space_and_data_inputs_none() {
        let lin = |input: &str, file: &str| Texture { colorspace: "lin_rec709", ..tex(input, file) };
        let x = write("m", &[lin("base_color", "c.exr"), lin("specular_roughness", "r.exr")], None);
        assert!(x.contains("value=\"c.exr\" colorspace=\"lin_rec709\"") && x.contains("value=\"r.exr\" />"), "{x}");
    }

    #[test]
    fn unknown_and_repeated_inputs_are_skipped() {
        let x = write("m", &[tex("ambient_occlusion", "ao.png"), tex("base_color", "a.png"), tex("base_color", "b.png")], None);
        assert!(!x.contains("ao.png") && x.contains("a.png") && !x.contains("b.png"));
    }

    #[test]
    fn templates_map_to_inputs() {
        assert_eq!(input_of_template("$mesh_$textureSet_base_color"), Some("base_color"));
        assert_eq!(input_of_template("$mesh_$textureSet_height"), Some("height"));
        assert_eq!(input_of_template("$mesh_$textureSet_orm"), None);
        assert_eq!(input_of_template("base_color"), None);
        for (n, _) in INPUTS {
            assert_eq!(input_of_template(&format!("{PREFIX}{n}")), Some(*n));
        }
    }

    #[test]
    fn udim_tokens_and_hostile_names_are_escaped() {
        assert_eq!(node_name("Rough Stone #2"), "Rough_Stone__2");
        assert_eq!(node_name("9lives"), "_9lives");
        assert_eq!(node_name(""), "_");
        let x = write("a<b>&\"c", &[tex("base_color", "x\"&<'>.<UDIM>.png")], Some([f32::NAN, 1.0]));
        assert!(balanced(&x.replace("&lt;UDIM&gt;", "UDIM")), "{x}");
        assert!(x.contains("value=\"x&quot;&amp;&lt;&apos;&gt;.&lt;UDIM&gt;.png\""));
        assert!(!x.contains("doc="), "a non-finite size is left out");
        assert!(x.contains("OPBR_a_b___c"));
    }
}
