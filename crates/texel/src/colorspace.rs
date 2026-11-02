//! Built-in colour spaces as plain maths: sRGB, linear Rec.709, ACEScg and ACES2065-1.
//!
//! The conversion matrices are derived at first use from the published primaries and white points
//! (ITU-R BT.709, ACES S-2014-004 for AP1, SMPTE ST 2065-1 for AP0) with Bradford chromatic
//! adaptation, so nothing is copied from a colour-management library. TextureCraft's colour
//! channels are linear Rec.709 internally; this module converts them for export.
//!
//! Only colour is converted. Data (normal, height, roughness...) has no colour space and never
//! goes through here.

use std::sync::OnceLock;

use crate::linear_to_srgb;

/// The colour space of exported colour images.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorSpace {
    /// sRGB-encoded (display-referred, 0–1), Rec.709 primaries.
    #[default]
    Srgb,
    /// Scene-linear Rec.709 / sRGB primaries, D65.
    LinearRec709,
    /// Scene-linear ACES AP1 primaries.
    AcesCg,
    /// Scene-linear ACES AP0 primaries (the archival interchange space).
    Aces2065,
}

impl ColorSpace {
    pub const ALL: [ColorSpace; 4] = [ColorSpace::Srgb, ColorSpace::LinearRec709, ColorSpace::AcesCg, ColorSpace::Aces2065];

    pub fn name(self) -> &'static str {
        match self {
            ColorSpace::Srgb => "sRGB",
            ColorSpace::LinearRec709 => "Linear Rec.709",
            ColorSpace::AcesCg => "ACEScg",
            ColorSpace::Aces2065 => "ACES2065-1",
        }
    }

    /// A space by name, ignoring case, spaces and punctuation ("linear rec709", "ap1", "srgb_texture").
    pub fn from_name(name: &str) -> Option<ColorSpace> {
        let key: String = name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Some(match key.as_str() {
            "srgb" | "srgbtexture" => ColorSpace::Srgb,
            "linearrec709" | "linearsrgb" | "linear" | "rec709" => ColorSpace::LinearRec709,
            "acescg" | "ap1" => ColorSpace::AcesCg,
            "aces20651" | "aces2065" | "ap0" | "aces" => ColorSpace::Aces2065,
            _ => return None,
        })
    }

    /// Whether values are scene-linear light.
    pub fn is_linear(self) -> bool {
        self != ColorSpace::Srgb
    }

    /// The red, green and blue primaries and the white point as CIE xy chromaticities.
    pub fn chromaticities(self) -> [[f64; 2]; 4] {
        let p = self.primaries();
        [p.r, p.g, p.b, p.white]
    }

    /// Linear RGB of this space to CIE XYZ adapted (Bradford) to the D50 white of the ICC profile
    /// connection space: the colorant matrix of a matrix/curve ICC profile. Columns are the
    /// red, green and blue colorants.
    pub fn to_xyz_d50(self) -> Option<[[f64; 3]; 3]> {
        let p = self.primaries();
        Some(mul(&adapt(p.white, D50)?, &rgb_to_xyz(&p)?))
    }

    fn primaries(self) -> Primaries {
        match self {
            ColorSpace::Srgb | ColorSpace::LinearRec709 => REC709,
            ColorSpace::AcesCg => AP1,
            ColorSpace::Aces2065 => AP0,
        }
    }

    /// Linear Rec.709 `rgb` as values in this space: the primaries are converted, and sRGB
    /// also gets its transfer curve (clamped to 0–1). Linear spaces keep values outside 0–1.
    /// Non-finite input becomes 0.
    pub fn from_linear_rec709(self, rgb: [f32; 3]) -> [f32; 3] {
        let rgb = rgb.map(|v| if v.is_finite() { v } else { 0.0 });
        let out = match matrix_to(self) {
            Some(m) => [0, 1, 2].map(|i| m[i][0] * rgb[0] + m[i][1] * rgb[1] + m[i][2] * rgb[2]),
            None => rgb,
        };
        if self == ColorSpace::Srgb { out.map(linear_to_srgb) } else { out }
    }
}

type M3 = [[f64; 3]; 3];

#[derive(Clone, Copy)]
struct Primaries {
    r: [f64; 2],
    g: [f64; 2],
    b: [f64; 2],
    white: [f64; 2],
}

const D65: [f64; 2] = [0.3127, 0.3290];
const D60_ACES: [f64; 2] = [0.32168, 0.33767];
/// The white of the ICC profile connection space.
const D50: [f64; 2] = [0.3457, 0.3585];
const REC709: Primaries = Primaries { r: [0.640, 0.330], g: [0.300, 0.600], b: [0.150, 0.060], white: D65 };
const AP1: Primaries = Primaries { r: [0.713, 0.293], g: [0.165, 0.830], b: [0.128, 0.044], white: D60_ACES };
const AP0: Primaries = Primaries { r: [0.7347, 0.2653], g: [0.0, 1.0], b: [0.0001, -0.0770], white: D60_ACES };

const BRADFORD: M3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

fn apply(m: &M3, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn inverse(m: &M3) -> Option<M3> {
    let c = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    if det.abs() < 1e-12 || !det.is_finite() {
        return None;
    }
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = c(j, i) / det;
        }
    }
    Some(o)
}

fn xy_to_xyz(xy: [f64; 2]) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

/// RGB (linear) to XYZ for a set of primaries, scaled so white maps to the white point.
fn rgb_to_xyz(p: &Primaries) -> Option<M3> {
    let (r, g, b) = (xy_to_xyz(p.r), xy_to_xyz(p.g), xy_to_xyz(p.b));
    let cols = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let s = apply(&inverse(&cols)?, xy_to_xyz(p.white));
    Some([[r[0] * s[0], g[0] * s[1], b[0] * s[2]], [r[1] * s[0], g[1] * s[1], b[1] * s[2]], [r[2] * s[0], g[2] * s[1], b[2] * s[2]]])
}

/// Bradford chromatic adaptation from one white point to another.
fn adapt(from: [f64; 2], to: [f64; 2]) -> Option<M3> {
    let (cs, cd) = (apply(&BRADFORD, xy_to_xyz(from)), apply(&BRADFORD, xy_to_xyz(to)));
    let scale = [[cd[0] / cs[0], 0.0, 0.0], [0.0, cd[1] / cs[1], 0.0], [0.0, 0.0, cd[2] / cs[2]]];
    Some(mul(&inverse(&BRADFORD)?, &mul(&scale, &BRADFORD)))
}

/// The matrix from linear Rec.709 to the linear primaries of `to` (cached).
fn matrix_to(to: ColorSpace) -> Option<[[f32; 3]; 3]> {
    static CACHE: OnceLock<[Option<[[f32; 3]; 3]>; 4]> = OnceLock::new();
    let table = CACHE.get_or_init(|| {
        ColorSpace::ALL.map(|s| {
            let (from, to) = (REC709, s.primaries());
            let m = mul(&inverse(&rgb_to_xyz(&to)?)?, &mul(&adapt(from.white, to.white)?, &rgb_to_xyz(&from)?));
            Some(m.map(|r| r.map(|v| v as f32)))
        })
    });
    let i = ColorSpace::ALL.iter().position(|s| *s == to)?;
    table.get(i).copied().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::srgb_to_linear;

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn white_stays_white_in_every_linear_space() {
        for s in [ColorSpace::LinearRec709, ColorSpace::AcesCg, ColorSpace::Aces2065] {
            assert!(close(s.from_linear_rec709([1.0; 3]), [1.0; 3], 1e-4), "{s:?}");
        }
        assert!(close(ColorSpace::Srgb.from_linear_rec709([1.0; 3]), [1.0; 3], 1e-5));
    }

    #[test]
    fn rec709_primaries_in_acescg_match_published_values() {
        // Rec.709 red in ACEScg (AP1, D60): the widely published 0.6131 0.0702 0.0206.
        assert!(close(ColorSpace::AcesCg.from_linear_rec709([1.0, 0.0, 0.0]), [0.6131, 0.0702, 0.0206], 2e-3));
        // And in ACES2065-1 (AP0): 0.4397 0.0897 0.0175.
        assert!(close(ColorSpace::Aces2065.from_linear_rec709([1.0, 0.0, 0.0]), [0.4397, 0.0897, 0.0175], 2e-3));
    }

    #[test]
    fn grey_stays_neutral_and_srgb_is_the_transfer_curve() {
        for s in ColorSpace::ALL {
            let o = s.from_linear_rec709([0.18; 3]);
            assert!(close(o, [o[0]; 3], 1e-4), "{s:?} {o:?}");
        }
        assert!(close(ColorSpace::Srgb.from_linear_rec709([0.5; 3]), [linear_to_srgb(0.5); 3], 1e-6));
        assert!((srgb_to_linear(ColorSpace::Srgb.from_linear_rec709([0.25; 3])[0]) - 0.25).abs() < 1e-5);
        assert_eq!(ColorSpace::LinearRec709.from_linear_rec709([0.3, 2.0, -1.0]), [0.3, 2.0, -1.0], "linear Rec.709 is the identity, values above 1 kept");
    }

    #[test]
    fn colorants_adapted_to_d50_add_up_to_the_d50_white_and_match_srgb() {
        for s in ColorSpace::ALL {
            let m = s.to_xyz_d50().unwrap();
            let white = [0, 1, 2].map(|row| m[row][0] + m[row][1] + m[row][2]);
            assert!((white[0] - 0.9642).abs() < 1e-3 && (white[1] - 1.0).abs() < 1e-3 && (white[2] - 0.8249).abs() < 1e-3, "{s:?} {white:?}");
        }
        // The colorants of the published sRGB ICC profile (D50 adapted): 0.4361 0.2225 0.0139 for red.
        let m = ColorSpace::Srgb.to_xyz_d50().unwrap();
        let red = [m[0][0], m[1][0], m[2][0]];
        assert!((red[0] - 0.4361).abs() < 2e-3 && (red[1] - 0.2225).abs() < 2e-3 && (red[2] - 0.0139).abs() < 2e-3, "{red:?}");
        let c = ColorSpace::AcesCg.chromaticities();
        assert_eq!(c[0], [0.713, 0.293]);
        assert_eq!(c[3], [0.32168, 0.33767]);
    }

    #[test]
    fn hostile_values_stay_finite() {
        for s in ColorSpace::ALL {
            assert!(s.from_linear_rec709([f32::NAN, f32::INFINITY, f32::NEG_INFINITY]).iter().all(|v| v.is_finite()), "{s:?}");
        }
    }

    #[test]
    fn names_round_trip_and_aliases_work() {
        for s in ColorSpace::ALL {
            assert_eq!(ColorSpace::from_name(s.name()), Some(s));
        }
        assert_eq!(ColorSpace::from_name("srgb_texture"), Some(ColorSpace::Srgb));
        assert_eq!(ColorSpace::from_name("AP1"), Some(ColorSpace::AcesCg));
        assert_eq!(ColorSpace::from_name("linear"), Some(ColorSpace::LinearRec709));
        assert_eq!(ColorSpace::from_name("nope"), None);
        assert!(!ColorSpace::Srgb.is_linear() && ColorSpace::AcesCg.is_linear());
    }
}
