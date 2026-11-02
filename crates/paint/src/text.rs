//! Text as a coverage image, for brush alphas and decals: each line shaped with harfrust
//! (kerning, ligatures, joined scripts), drawn from the font's outlines (skrifa) and filled with
//! a non-zero scanline rasterizer, lines centered.

use harfrust::{FontRef, ShapeOptions, ShaperData, UnicodeBuffer};
use skrifa::MetadataProvider;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};

/// Longest text drawn (characters).
pub const MAX_TEXT_CHARS: usize = 1000;
/// Largest side of a text image (texels); bigger layouts are drawn smaller to fit.
pub const MAX_TEXT_SIDE: u32 = 4096;
/// Vertical samples per texel row.
const SUBROWS: usize = 4;

/// A shaped glyph: its id and pen position (font units).
type Placed = (u32, f32, f32);

/// Coverage 0..=255 per texel, rows top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub struct TextImage {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

/// Draw `text` (one or more lines) with face `index` of `font` at `px` texels per em, cropped to
/// the ink with a margin of a tenth of an em.
pub fn render_text(font: &[u8], index: u32, text: &str, px: f32) -> Result<TextImage, String> {
    let text = text.trim_end();
    if text.trim().is_empty() {
        return Err("the text is empty".into());
    }
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("the text is longer than {MAX_TEXT_CHARS} characters"));
    }
    let font = FontRef::from_index(font, index).map_err(|e| format!("not a usable font: {e}"))?;
    // Layout in font units; `scale` turns them into texels.
    let m = font.metrics(Size::unscaled(), LocationRef::default());
    let upem = f32::from(m.units_per_em.max(1));
    let line = (m.ascent - m.descent + m.leading).max(upem * 0.5);
    let data = ShaperData::new(&font);
    let shaper = data.shaper(&font).build();
    let mut lines: Vec<(f32, Vec<Placed>)> = Vec::new();
    for t in text.lines() {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(t);
        buffer.guess_segment_properties();
        let shaped = shaper.shape(buffer, ShapeOptions::new());
        let mut x = 0.0;
        let mut glyphs = Vec::new();
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            glyphs.push((info.glyph_id, x + pos.x_offset as f32, pos.y_offset as f32));
            x += pos.x_advance as f32;
        }
        lines.push((x, glyphs));
    }
    let widest = lines.iter().map(|(w, _)| *w).fold(0.0, f32::max);
    let px = px.clamp(4.0, 1024.0);
    // Outlines in texels (y down) at the shaped positions, lines centered on the widest.
    let outlines = font.outline_glyphs();
    let mut pen = Edges { scale: px / upem, ..Edges::default() };
    for (row, (line_width, glyphs)) in lines.iter().enumerate() {
        let left = (widest - line_width) * 0.5;
        for &(id, x, y) in glyphs {
            let Some(glyph) = outlines.get(skrifa::GlyphId::new(id)) else { continue };
            pen.origin = ((left + x) * pen.scale, (line * row as f32 - y) * pen.scale);
            // A glyph that can't be drawn is left out rather than failing the text.
            let _ = glyph.draw(DrawSettings::unhinted(Size::unscaled(), LocationRef::default()), &mut pen);
            pen.close();
        }
    }
    // The image is the ink's bounds plus the margin (accents and descenders never clip), scaled
    // down when that is too large.
    let points = || pen.edges.iter().flat_map(|e| [(e[0], e[1]), (e[2], e[3])]);
    let lo = points().fold((f32::MAX, f32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1)));
    let hi = points().fold((f32::MIN, f32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1)));
    if pen.edges.is_empty() || !(lo.0.is_finite() && hi.0.is_finite() && lo.1.is_finite() && hi.1.is_finite()) {
        return Err("the font draws nothing for this text".into());
    }
    let margin = px * 0.1;
    let (w, h) = (hi.0 - lo.0 + 2.0 * margin, hi.1 - lo.1 + 2.0 * margin);
    let fit = (MAX_TEXT_SIDE as f32 / w.max(h)).min(1.0);
    let edges: Vec<[f32; 4]> = pen
        .edges
        .iter()
        .map(|e| [(e[0] - lo.0 + margin) * fit, (e[1] - lo.1 + margin) * fit, (e[2] - lo.0 + margin) * fit, (e[3] - lo.1 + margin) * fit])
        .collect();
    let (width, height) = ((w * fit).ceil().max(1.0) as u32, (h * fit).ceil().max(1.0) as u32);
    Ok(TextImage { width, height, coverage: fill(&edges, width as usize, height as usize) })
}

/// Line segments (texel space, y down) collected from glyph outlines: font units times `scale`
/// from `origin`, curves flattened.
#[derive(Default)]
struct Edges {
    scale: f32,
    origin: (f32, f32),
    start: (f32, f32),
    at: (f32, f32),
    edges: Vec<[f32; 4]>,
}

impl Edges {
    fn map(&self, x: f32, y: f32) -> (f32, f32) {
        (self.origin.0 + x * self.scale, self.origin.1 - y * self.scale)
    }

    fn segment(&mut self, to: (f32, f32)) {
        if to != self.at {
            self.edges.push([self.at.0, self.at.1, to.0, to.1]);
        }
        self.at = to;
    }

    /// Flatten a curve given as a point function of t in 0..1, finer for longer curves.
    fn curve(&mut self, control_length: f32, point: impl Fn(f32) -> (f32, f32)) {
        let steps = (control_length.sqrt() * 2.0).clamp(2.0, 64.0) as usize;
        for i in 1..=steps {
            self.segment(point(i as f32 / steps as f32));
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

impl OutlinePen for Edges {
    fn move_to(&mut self, x: f32, y: f32) {
        self.close();
        self.start = self.map(x, y);
        self.at = self.start;
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.map(x, y);
        self.segment(p);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (p0, c, p) = (self.at, self.map(cx0, cy0), self.map(x, y));
        self.curve(dist(p0, c) + dist(c, p), |t| {
            let s = 1.0 - t;
            (s * s * p0.0 + 2.0 * s * t * c.0 + t * t * p.0, s * s * p0.1 + 2.0 * s * t * c.1 + t * t * p.1)
        });
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (p0, c0, c1, p) = (self.at, self.map(cx0, cy0), self.map(cx1, cy1), self.map(x, y));
        self.curve(dist(p0, c0) + dist(c0, c1) + dist(c1, p), |t| {
            let s = 1.0 - t;
            let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
            (a * p0.0 + b * c0.0 + c * c1.0 + d * p.0, a * p0.1 + b * c0.1 + c * c1.1 + d * p.1)
        });
    }

    fn close(&mut self) {
        self.segment(self.start);
    }
}

/// Non-zero winding coverage of closed `edges` over a `w × h` grid: [`SUBROWS`] scanlines per
/// row, each span's ends spread over the texels they cross.
fn fill(edges: &[[f32; 4]], w: usize, h: usize) -> Vec<u8> {
    let mut acc = vec![0f32; w * h];
    // Edges by their top, and the ones the current scanline may cross.
    let mut sorted: Vec<[f32; 4]> = edges.iter().copied().filter(|e| e[1] != e[3]).collect();
    sorted.sort_by(|a, b| a[1].min(a[3]).total_cmp(&b[1].min(b[3])));
    let (mut next, mut active) = (0, Vec::new());
    let mut crossings: Vec<(f32, i32)> = Vec::new();
    for row in 0..h {
        for sub in 0..SUBROWS {
            let y = row as f32 + (sub as f32 + 0.5) / SUBROWS as f32;
            while let Some(e) = sorted.get(next).filter(|e| e[1].min(e[3]) <= y) {
                active.push(*e);
                next += 1;
            }
            active.retain(|e| e[1].max(e[3]) > y);
            crossings.clear();
            for &[x0, y0, x1, y1] in &active {
                if (y0 <= y) != (y1 <= y) {
                    crossings.push((x0 + (y - y0) / (y1 - y0) * (x1 - x0), if y1 > y0 { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                if winding != 0 {
                    span(&mut acc[row * w..(row + 1) * w], pair[0].0, pair[1].0);
                }
            }
        }
    }
    acc.iter().map(|c| (c / SUBROWS as f32 * 255.0).round().clamp(0.0, 255.0) as u8).collect()
}

/// Add the covered length of `x0..x1` to each texel of `row`.
fn span(row: &mut [f32], x0: f32, x1: f32) {
    let (x0, x1) = (x0.clamp(0.0, row.len() as f32), x1.clamp(0.0, row.len() as f32));
    if x1 <= x0 {
        return;
    }
    for (i, c) in row.iter_mut().enumerate().take(x1.ceil() as usize).skip(x0.floor() as usize) {
        *c += (x1.min(i as f32 + 1.0) - x0.max(i as f32)).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_spread_partial_texels() {
        let mut row = [0.0; 4];
        span(&mut row, 0.5, 2.25);
        assert_eq!(row, [0.5, 1.0, 0.25, 0.0]);
        // A square drawn either way round fills the same texels.
        let square = |cw: bool| {
            let p = [(1.0, 1.0), (3.0, 1.0), (3.0, 3.0), (1.0, 3.0)];
            let p: Vec<_> = if cw { p.to_vec() } else { p.iter().rev().copied().collect() };
            (0..4).map(|i| [p[i].0, p[i].1, p[(i + 1) % 4].0, p[(i + 1) % 4].1]).collect::<Vec<_>>()
        };
        let a = fill(&square(true), 4, 4);
        assert_eq!(a, fill(&square(false), 4, 4));
        assert_eq!((a[5], a[0], a[3 * 4 + 3]), (255, 0, 0), "inside full, outside empty");
    }

    #[test]
    fn text_is_laid_out_and_filled() {
        let font = epaint_default_fonts::UBUNTU_LIGHT;
        let one = render_text(font, 0, "H", 64.0).unwrap();
        let two = render_text(font, 0, "HH", 64.0).unwrap();
        let lines = render_text(font, 0, "H\nH", 64.0).unwrap();
        assert!(two.width > one.width && two.height == one.height);
        assert!(lines.height > one.height && lines.width == one.width, "lines stack");
        let ink = |t: &TextImage| t.coverage.iter().map(|c| u32::from(*c)).sum::<u32>();
        assert!(ink(&two) > ink(&one) * 19 / 10 && ink(&two) < ink(&one) * 21 / 10, "two H, twice the ink");
        assert!(one.coverage.contains(&255) && one.coverage.contains(&0));
        // The margin stays clear; huge sizes are drawn to fit.
        assert!(one.coverage[..one.width as usize].iter().all(|c| *c == 0));
        let big = render_text(font, 0, &"W".repeat(400), 1024.0).unwrap();
        assert!(big.width <= MAX_TEXT_SIDE && big.height >= 1);
    }

    #[test]
    fn bad_input_is_an_error() {
        assert!(render_text(b"not a font", 0, "Hi", 64.0).is_err());
        assert!(render_text(b"", 0, "   ", 64.0).is_err());
        assert!(render_text(b"", 0, &"x".repeat(MAX_TEXT_CHARS + 1), 64.0).is_err());
    }
}
