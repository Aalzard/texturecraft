//! Thumbnails: shaded material spheres, smart mask spheres, procedural swatches and brush alpha
//! previews, rendered on the CPU into cached egui textures (no image assets). New shelf
//! thumbnails are rendered lazily, a few per frame, so opening the shelf never stalls a frame.

use std::collections::HashMap;

use egui::{Color32, ColorImage, Painter, Pos2, Rect, TextureHandle, TextureOptions};
use texturecraft_engine::core::ProceduralKind;
use texturecraft_engine::paint::{BrushAlpha, alphas};
use texturecraft_engine::preview::{self, Sample};
use texturecraft_engine::texel::linear_to_srgb;

/// Shelf thumbnails rendered per frame at most (the rest follow on the next frames).
const PER_FRAME: usize = 2;
/// Shelf sphere thumbnail size in pixels.
const SPHERE_PX: usize = 72;

#[derive(Default)]
pub struct Thumbs {
    /// Shelf thumbnails by key (`mat:<id>`, `mask:<id>`, `filter:<id>`, `proc:<kind>`, `alpha:<id>`).
    shelf: HashMap<String, TextureHandle>,
    images: HashMap<String, TextureHandle>,
    /// (frame, thumbnails rendered in it).
    budget: (u64, usize),
}

fn shade(n: [f32; 3], base: [f32; 3], metal: f32, rough: f32) -> [f32; 3] {
    let l = [0.45f32, 0.7, 0.55];
    let len = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
    let l = l.map(|v| v / len);
    let ndl = (n[0] * l[0] + n[1] * l[1] + n[2] * l[2]).max(0.0);
    let ndv = n[2].max(0.0);
    // View along +Z: half vector between light and view.
    let h = [l[0], l[1], l[2] + 1.0];
    let hl = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
    let ndh = ((n[0] * h[0] + n[1] * h[1] + n[2] * h[2]) / hl).max(0.0);
    let shin = 2.0 / (rough * rough * rough * rough).max(1e-3) - 2.0;
    let spec_amt = ndh.powf(shin.clamp(1.0, 4096.0)) * (shin + 2.0) / 8.0;
    // A studio environment seen in the mirror direction: dark floor, bright sky, a horizon that
    // blurs (and the whole environment averages out) as roughness grows.
    let ry = 2.0 * ndv * n[1];
    let w = 0.06 + rough * 0.9;
    let t = ((ry + w) / (2.0 * w)).clamp(0.0, 1.0);
    let sharp = 0.04 + 0.03 * (ry + 1.0) + (0.2 + 0.55 * ry.max(0.0)) * t * t * (3.0 - 2.0 * t);
    let env = sharp + (0.28 - sharp) * (rough * rough);
    let fres = (1.0 - ndv).powi(5);
    let sky = 0.35 + 0.25 * n[1];
    let mut out = [0.0; 3];
    for k in 0..3 {
        let f0 = 0.04 + (base[k] - 0.04) * metal;
        let f = f0 + ((1.0 - rough).max(f0) - f0) * fres;
        let diffuse = base[k] * (1.0 - metal) * (ndl * 0.85 + sky * 0.5);
        out[k] = diffuse * (1.0 - f) + f * env + f0 * spec_amt * ndl * 1.5;
    }
    out
}

fn aces(x: f32) -> f32 {
    let v = x * 0.6;
    ((v * (2.51 * v + 0.03)) / (v * (2.43 * v + 0.59) + 0.14)).clamp(0.0, 1.0)
}

/// A shaded sphere image (sRGB, transparent outside) from per-pixel preview samples (row-major
/// `size²`, see [`preview::sphere_normals`]); height bumps the normals.
pub fn sphere_image(size: usize, samples: &[Option<Sample>]) -> ColorImage {
    let mut img = ColorImage::filled([size, size], Color32::TRANSPARENT);
    let r = (size as f32 * 0.5 - 1.0).max(0.5);
    let h = |x: Option<usize>, y: Option<usize>| {
        let (x, y) = (x.filter(|x| *x < size)?, y?);
        samples.get(y.checked_mul(size)?.checked_add(x)?).copied().flatten().map(|s| s.height)
    };
    for (i, n) in preview::sphere_normals(size).enumerate() {
        let (Some(n), Some(Some(s))) = (n, samples.get(i)) else { continue };
        let (x, y) = (i % size, i / size);
        let c = s.height;
        // Central differences (image y points down).
        let dx = h(Some(x + 1), Some(y)).unwrap_or(c) - h(x.checked_sub(1), Some(y)).unwrap_or(c);
        let dy = h(Some(x), y.checked_sub(1)).unwrap_or(c) - h(Some(x), Some(y + 1)).unwrap_or(c);
        let k = r * 0.5 * 1.5;
        let b = [n.x - dx * k, n.y - dy * k, n.z];
        let len = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt().max(1e-6);
        let col = shade(b.map(|v| v / len), s.base, s.metallic, s.roughness).map(|v| (linear_to_srgb(aces(v * 1.4)) * 255.0 + 0.5) as u8);
        let edge = ((1.0 - (n.x * n.x + n.y * n.y).sqrt()) * r).clamp(0.0, 1.0);
        img.pixels[i] = Color32::from_rgba_unmultiplied(col[0], col[1], col[2], (edge * 255.0) as u8);
    }
    img
}

impl Thumbs {
    /// A larger cached texture of an image resource for the stencil overlay (up to 1024 px).
    pub fn stencil(&mut self, ctx: &egui::Context, name: &str, img: &texturecraft_engine::compose::ImageRes) -> TextureHandle {
        let key = format!("stencil:{name}");
        if let Some(t) = self.images.get(&key) {
            return t.clone();
        }
        let scale = (1024.0 / img.width.max(img.height).max(1) as f32).min(1.0);
        let (w, h) = (((img.width as f32 * scale) as usize).max(1), ((img.height as f32 * scale) as usize).max(1));
        let mut ci = ColorImage::filled([w, h], Color32::TRANSPARENT);
        for y in 0..h {
            for x in 0..w {
                let s = img.sample((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let enc = |v: f32| if img.srgb { (v.clamp(0.0, 1.0) * 255.0) as u8 } else { (linear_to_srgb(v) * 255.0) as u8 };
                ci.pixels[y * w + x] = Color32::from_rgba_unmultiplied(enc(s[0]), enc(s[1]), enc(s[2]), (s[3].clamp(0.0, 1.0) * 255.0) as u8);
            }
        }
        let t = ctx.load_texture(key.clone(), ci, TextureOptions::LINEAR);
        self.images.insert(key, t.clone());
        t
    }

    /// Cached preview of an image resource (64 px, nearest downsample).
    pub fn image(&mut self, ctx: &egui::Context, name: &str, img: &texturecraft_engine::compose::ImageRes) -> TextureHandle {
        if let Some(t) = self.images.get(name) {
            return t.clone();
        }
        let n = 64usize;
        let mut ci = ColorImage::filled([n, n], Color32::BLACK);
        for y in 0..n {
            for x in 0..n {
                let s = img.sample((x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32);
                let enc = |v: f32| if img.srgb { (v.clamp(0.0, 1.0) * 255.0) as u8 } else { (linear_to_srgb(v) * 255.0) as u8 };
                ci.pixels[y * n + x] = Color32::from_rgb(enc(s[0]), enc(s[1]), enc(s[2]));
            }
        }
        let t = ctx.load_texture(format!("res_{name}"), ci, TextureOptions::LINEAR);
        self.images.insert(name.to_string(), t.clone());
        t
    }

    /// A cached shelf thumbnail, rendering it with `render` if this frame's budget allows (None
    /// until then; a repaint is requested so it follows shortly).
    fn shelf(&mut self, ctx: &egui::Context, key: String, render: impl FnOnce() -> ColorImage) -> Option<TextureHandle> {
        if let Some(t) = self.shelf.get(&key) {
            return Some(t.clone());
        }
        let frame = ctx.cumulative_pass_nr();
        if self.budget.0 != frame {
            self.budget = (frame, 0);
        }
        if self.budget.1 >= PER_FRAME {
            ctx.request_repaint();
            return None;
        }
        self.budget.1 += 1;
        let t = ctx.load_texture(format!("thumb_{key}"), render(), TextureOptions::LINEAR);
        self.shelf.insert(key, t.clone());
        Some(t)
    }

    /// Sphere thumbnail of built-in material `id`.
    pub fn material(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        self.shelf(ctx, format!("mat:{id}"), || sphere_image(SPHERE_PX, &preview::material(id, SPHERE_PX).unwrap_or_default()))
    }

    /// Sphere thumbnail of smart mask `id`: white where the mask applies, over dark gray.
    pub fn smart_mask(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        self.shelf(ctx, format!("mask:{id}"), || {
            let px = preview::smart_mask(id, SPHERE_PX).unwrap_or_default();
            let samples: Vec<Option<Sample>> =
                px.iter().map(|m| m.map(|m| Sample { base: [0.04 + 0.76 * m; 3], metallic: 0.0, roughness: 0.6, height: 0.0 })).collect();
            sphere_image(SPHERE_PX, &samples)
        })
    }

    /// Sphere thumbnail of library filter `id` applied to a sample material.
    pub fn filter(&mut self, ctx: &egui::Context, id: &str) -> Option<TextureHandle> {
        self.shelf(ctx, format!("filter:{id}"), || sphere_image(SPHERE_PX, &preview::filter(id, SPHERE_PX).unwrap_or_default()))
    }

    /// Grayscale swatch of procedural `k`.
    pub fn procedural(&mut self, ctx: &egui::Context, k: ProceduralKind) -> Option<TextureHandle> {
        use texturecraft_engine::compose::pattern::{Coords, procedural};
        use texturecraft_engine::core::{Procedural, Projection};
        use texturecraft_engine::geom::glam::{Vec2, Vec3};
        self.shelf(ctx, format!("proc:{k:?}"), || {
            let n = 56usize;
            let pr = Procedural { scale: 3.0, ..Procedural::new(k) };
            let mut img = ColorImage::filled([n, n], Color32::BLACK);
            for (i, px) in img.pixels.iter_mut().enumerate() {
                let uv = Vec2::new(((i % n) as f32 + 0.5) / n as f32, ((i / n) as f32 + 0.5) / n as f32);
                let v = procedural(&pr, &Projection::default(), &Coords { uv, pos: Vec3::ZERO, nrm: Vec3::Y });
                *px = Color32::from_gray((v.clamp(0.0, 1.0) * 255.0) as u8);
            }
            img
        })
    }

    /// White-on-black swatch of library alpha `a`.
    pub fn alpha(&mut self, ctx: &egui::Context, a: &alphas::LibraryAlpha) -> Option<TextureHandle> {
        self.shelf(ctx, format!("alpha:{}", a.id), || {
            let n = 64usize;
            ColorImage::new([n, n], a.render(n, 2).into_iter().map(|v| Color32::from_gray((v * 255.0 + 0.5) as u8)).collect())
        })
    }
}

/// A small shaded sphere drawn with a vertex-colored mesh (layer thumbnails, previews).
pub fn paint_sphere(p: &Painter, c: Pos2, r: f32, base: [f32; 3]) {
    let mut mesh = egui::Mesh::default();
    let rings = 10;
    let segs = 28;
    let color = |dx: f32, dy: f32| {
        let d2 = (dx * dx + dy * dy).min(1.0);
        let n = [dx, dy, (1.0 - d2).sqrt()];
        let s = shade(n, base, 0.0, 0.5).map(|v| (linear_to_srgb(aces(v * 1.4)) * 255.0) as u8);
        Color32::from_rgb(s[0], s[1], s[2])
    };
    mesh.colored_vertex(c, color(0.0, 0.0));
    for i in 1..=rings {
        let rr = i as f32 / rings as f32;
        for j in 0..segs {
            let a = j as f32 / segs as f32 * std::f32::consts::TAU;
            let (dx, dy) = (a.cos() * rr, a.sin() * rr);
            mesh.colored_vertex(Pos2::new(c.x + dx * r, c.y - dy * r), color(dx, dy));
        }
    }
    for j in 0..segs {
        let (a, b) = (1 + j, 1 + (j + 1) % segs);
        mesh.add_triangle(0, a as u32, b as u32);
    }
    for i in 1..rings {
        let r0 = 1 + (i - 1) * segs;
        let r1 = 1 + i * segs;
        for j in 0..segs {
            let (a, b) = (r0 + j, r0 + (j + 1) % segs);
            let (c2, d) = (r1 + j, r1 + (j + 1) % segs);
            mesh.add_triangle(a as u32, c2 as u32, d as u32);
            mesh.add_triangle(a as u32, d as u32, b as u32);
        }
    }
    p.add(egui::Shape::mesh(mesh));
}

/// Brush alpha preview drawn as small cells (cheap, no texture).
pub fn paint_alpha(p: &Painter, rect: Rect, a: BrushAlpha, hardness: f32) {
    let n = 20;
    let cell = rect.width() / n as f32;
    for y in 0..n {
        for x in 0..n {
            let lx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let ly = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let v = a.sample(lx, ly, hardness, 7);
            if v > 0.02 {
                let r = Rect::from_min_size(rect.min + egui::vec2(x as f32 * cell, y as f32 * cell), egui::vec2(cell + 0.5, cell + 0.5));
                p.rect_filled(r, 0.0, Color32::from_white_alpha((v * 255.0) as u8));
            }
        }
    }
}
