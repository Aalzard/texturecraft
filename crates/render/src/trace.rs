//! Progressive CPU path tracing for Rendering mode: the textured mesh under the environment,
//! with the raster viewport's material conventions (metallic/roughness or specular/glossiness
//! GGX, specular level,
//! height-to-normal slope rule, normal channel blended with RNM, emissive, opacity) but real
//! multi-bounce light transport. Each [`Tracer::pass`] adds one sample per pixel.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use rayon::prelude::*;
use texturecraft_compose::{Composite, ImageRes};
use texturecraft_core::project::{DisplaySettings, ToneMapping};
use texturecraft_core::Channel;
use texturecraft_geom::glam::{Vec2, Vec3};
use texturecraft_geom::{Bvh, Mesh, OrbitCamera};

use crate::ibl::EnvImage;

/// One texture set's composited channels.
#[derive(Clone)]
pub struct TraceSet {
    /// Mesh material the set covers.
    pub material: usize,
    pub composite: Composite,
    pub height_strength: f32,
}

/// Everything a pass reads (cheap to clone: shared or copy-on-write data).
#[derive(Clone)]
pub struct TraceScene {
    pub mesh: Arc<Mesh>,
    pub bvh: Arc<Bvh>,
    pub sets: Vec<TraceSet>,
    pub env: Arc<EnvImage>,
    /// Environment rotation around +Y (radians), as in the viewport.
    pub env_rotation: f32,
    /// Texture set per triangle (`usize::MAX`: none, default material).
    tri_set: Arc<Vec<usize>>,
    /// Importance sampling of the environment (None for an empty image).
    light: Option<Arc<EnvSampler>>,
    /// Specular/glossiness shading (Specular color is F0, base color is diffuse).
    pub spec_gloss: bool,
    /// Ray offset off surfaces, scaled to the mesh.
    eps: f32,
}

impl TraceScene {
    pub fn new(mesh: Arc<Mesh>, bvh: Arc<Bvh>, sets: Vec<TraceSet>, env: Arc<EnvImage>, env_rotation: f32) -> TraceScene {
        let mut tri_set = vec![usize::MAX; mesh.triangle_count()];
        let set_of = |material: usize| sets.iter().position(|s| s.material == material).unwrap_or(usize::MAX);
        if mesh.submeshes.is_empty() {
            tri_set.fill(set_of(0));
        }
        for sm in &mesh.submeshes {
            let set = set_of(sm.material);
            let first = sm.start as usize / 3;
            for t in tri_set.iter_mut().skip(first).take(sm.count as usize / 3) {
                *t = set;
            }
        }
        let (lo, hi) = mesh.bounds();
        let eps = ((hi - lo).length() * 2e-5).max(1e-6);
        let light = EnvSampler::new(&env).map(Arc::new);
        TraceScene { mesh, bvh, sets, env, env_rotation, tri_set: Arc::new(tri_set), light, spec_gloss: false, eps }
    }

    /// The scene as `display` shows it: the imported environment `env_image` (when the display
    /// names a resource and it decodes) or the built-in one, its rotation and the shading mode.
    pub fn for_display(mesh: Arc<Mesh>, bvh: Arc<Bvh>, sets: Vec<TraceSet>, display: &DisplaySettings, env_image: Option<&ImageRes>) -> TraceScene {
        let imported = env_image.and_then(|i| EnvImage::from_rgba(i.width, i.height, &i.rgba, i.srgb));
        let env = imported.unwrap_or_else(|| crate::ibl::environment(&display.environment, 1024));
        let mut scene = TraceScene::new(mesh, bvh, sets, Arc::new(env), display.env_rotation.to_radians());
        scene.spec_gloss = display.spec_gloss;
        scene
    }

    /// World direction to environment space.
    fn to_env(&self, d: Vec3) -> Vec3 {
        let (s, c) = self.env_rotation.sin_cos();
        Vec3::new(c * d.x - s * d.z, d.y, s * d.x + c * d.z)
    }

    fn env(&self, d: Vec3) -> Vec3 {
        self.env.sample(self.to_env(d))
    }

    /// Solid-angle density of [`Self::sample_env`] picking world direction `d`.
    fn env_pdf(&self, d: Vec3) -> f32 {
        self.light.as_ref().map_or(0.0, |l| l.pdf(self.to_env(d)))
    }

    /// A world direction toward the environment, chosen by brightness, with its density.
    fn sample_env(&self, rng: &mut Rng) -> Option<(Vec3, f32)> {
        let (e, pdf) = self.light.as_ref()?.sample(rng.next(), rng.next(), rng.next(), rng.next())?;
        let (s, c) = self.env_rotation.sin_cos();
        Some((Vec3::new(c * e.x + s * e.z, e.y, -s * e.x + c * e.z), pdf))
    }
}

/// Piecewise-constant importance sampling of an equirect environment by luminance and sin(theta).
struct EnvSampler {
    width: usize,
    height: usize,
    /// Cumulative row probabilities (last = 1).
    rows: Vec<f32>,
    /// Cumulative probabilities within each row (last of each row = 1).
    cols: Vec<f32>,
    /// Probability of each pixel.
    prob: Vec<f32>,
}

impl EnvSampler {
    fn new(env: &EnvImage) -> Option<EnvSampler> {
        let (w, h) = (env.width as usize, env.height as usize);
        if w == 0 || h == 0 || env.data.len() < w * h {
            return None;
        }
        let mut weight: Vec<f32> = env.data.iter().take(w * h).map(|c| luminance(Vec3::from(*c)).max(0.0)).collect();
        for (y, row) in weight.chunks_mut(w).enumerate() {
            let sin = (PI * (y as f32 + 0.5) / h as f32).sin();
            row.iter_mut().for_each(|v| *v = (*v + 1e-4) * sin);
        }
        let total: f32 = weight.iter().sum();
        if !total.is_finite() || total <= 0.0 {
            return None;
        }
        let prob: Vec<f32> = weight.iter().map(|v| v / total).collect();
        let mut rows = Vec::with_capacity(h);
        let mut cols = Vec::with_capacity(w * h);
        let mut acc_rows = 0.0;
        for row in prob.chunks(w) {
            let sum: f32 = row.iter().sum();
            acc_rows += sum;
            rows.push(acc_rows);
            let mut acc = 0.0;
            for p in row {
                acc += if sum > 0.0 { p / sum } else { 1.0 / w as f32 };
                cols.push(acc);
            }
        }
        Some(EnvSampler { width: w, height: h, rows, cols, prob })
    }

    /// Environment-space direction and its solid-angle density.
    fn sample(&self, u0: f32, u1: f32, u2: f32, u3: f32) -> Option<(Vec3, f32)> {
        let y = self.rows.partition_point(|c| *c < u0 * self.rows.last().copied().unwrap_or(1.0)).min(self.height - 1);
        let row = self.cols.get(y * self.width..(y + 1) * self.width)?;
        let x = row.partition_point(|c| *c < u1 * row.last().copied().unwrap_or(1.0)).min(self.width - 1);
        let d = crate::ibl::dir_of((x as f32 + u2) / self.width as f32, (y as f32 + u3) / self.height as f32);
        let pdf = self.pdf(d);
        (pdf > 0.0).then_some((d, pdf))
    }

    fn pdf(&self, d: Vec3) -> f32 {
        let uv = crate::ibl::uv_of(d);
        let x = ((uv.x * self.width as f32) as usize).min(self.width - 1);
        let y = ((uv.y * self.height as f32) as usize).min(self.height - 1);
        let sin = (PI * (y as f32 + 0.5) / self.height as f32).sin();
        let p = self.prob.get(y * self.width + x).copied().unwrap_or(0.0);
        if sin <= 1e-6 { 0.0 } else { p * (self.width * self.height) as f32 / (2.0 * PI * PI * sin) }
    }
}

/// How the accumulated radiance becomes display pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceDisplay {
    /// Exposure multiplier (2^EV).
    pub exposure: f32,
    pub aces: bool,
    /// How much of the environment shows behind the mesh (0: studio backdrop).
    pub env_opacity: f32,
    /// Filter the remaining noise (edge-aware; keeps texture detail).
    pub denoise: bool,
}

impl TraceDisplay {
    /// Exposure, tone mapping and background as the viewport shows them.
    pub fn from_display(d: &DisplaySettings) -> TraceDisplay {
        TraceDisplay { exposure: d.env_exposure.exp2(), aces: d.tone_mapping == ToneMapping::AcesFilmic, env_opacity: d.env_opacity, denoise: false }
    }
}

/// Light transport settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceSettings {
    /// Diffuse/specular bounces after the camera hit.
    pub bounces: u32,
}

impl Default for TraceSettings {
    fn default() -> Self {
        TraceSettings { bounces: 6 }
    }
}

#[derive(Clone, Copy, Default)]
struct Px {
    /// Radiance summed over samples that hit the mesh.
    hit: Vec3,
    /// Sum of the squared luminance of those samples (for the variance).
    hit_lum2: f32,
    /// First-hit reflectance, shading normal and distance, summed (denoiser guides).
    albedo: Vec3,
    normal: Vec3,
    depth: f32,
    /// Environment radiance summed over samples that missed it.
    env: Vec3,
    hits: f32,
}

/// What one camera sample saw.
struct Sample {
    radiance: Vec3,
    albedo: Vec3,
    normal: Vec3,
    depth: f32,
}

/// The accumulation buffer of one view.
pub struct Tracer {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// First row of the pass in progress (see [`Tracer::pass_rows`]).
    row: u32,
    acc: Vec<Px>,
}

impl Tracer {
    pub fn new(width: u32, height: u32) -> Tracer {
        let (w, h) = (width.clamp(1, 8192), height.clamp(1, 8192));
        Tracer { width: w, height: h, samples: 0, row: 0, acc: vec![Px::default(); (w * h) as usize] }
    }

    /// Add one sample per pixel.
    pub fn pass(&mut self, scene: &TraceScene, cam: &OrbitCamera, settings: &TraceSettings) {
        while !self.pass_rows(scene, cam, settings, self.height) {}
    }

    /// Sample up to `rows` more rows of the current pass; true when that finished the pass (the
    /// new sample then counts in [`Tracer::samples`]). Lets a single-threaded caller spread a
    /// pass over several frames.
    pub fn pass_rows(&mut self, scene: &TraceScene, cam: &OrbitCamera, settings: &TraceSettings, rows: u32) -> bool {
        let (w, h, pass) = (self.width, self.height, self.samples);
        let aspect = w as f32 / h as f32;
        let (y0, y1) = (self.row, self.row.saturating_add(rows.max(1)).min(h));
        let span = self.acc.get_mut(y0 as usize * w as usize..y1 as usize * w as usize).unwrap_or_default();
        span.par_chunks_mut(w as usize).enumerate().for_each(|(dy, row)| {
            let y = y0 as usize + dy;
            for (x, px) in row.iter_mut().enumerate() {
                let mut rng = Rng::new(x as u32, y as u32, pass);
                let ndc = Vec2::new((x as f32 + rng.next()) / w as f32 * 2.0 - 1.0, 1.0 - (y as f32 + rng.next()) / h as f32 * 2.0);
                let ray = cam.ray(ndc, aspect);
                match trace(scene, ray.origin, ray.dir, settings, &mut rng) {
                    Some(s) => {
                        px.hit += s.radiance;
                        px.hit_lum2 += luminance(s.radiance).powi(2);
                        px.albedo += s.albedo;
                        px.normal += s.normal;
                        px.depth += s.depth;
                        px.hits += 1.0;
                    }
                    None => px.env += scene.env(ray.dir),
                }
            }
        });
        if y1 >= h {
            self.row = 0;
            self.samples += 1;
            true
        } else {
            self.row = y1;
            false
        }
    }

    /// Tone-mapped sRGB pixels, row-major from the top.
    pub fn rgba8(&self, d: &TraceDisplay) -> Vec<u8> {
        let n = self.samples.max(1) as f32;
        let (w, h) = (self.width as f32, self.height as f32);
        let denoised = if d.denoise { self.denoise() } else { Vec::new() };
        let mut out = vec![0u8; self.acc.len() * 4];
        out.par_chunks_mut(4).zip(self.acc.par_iter()).enumerate().for_each(|(i, (o, px))| {
            let uv = Vec2::new(((i as u32 % self.width) as f32 + 0.5) / w, ((i as u32 / self.width) as f32 + 0.5) / h);
            let a = px.hits / n;
            let misses = n - px.hits;
            let back = if misses > 0.0 { backdrop(uv).lerp(tonemap(px.env / misses, d), d.env_opacity.clamp(0.0, 1.0)) } else { backdrop(uv) };
            let mean = denoised.get(i).copied().unwrap_or(px.hit / px.hits.max(1.0));
            let scene = if px.hits > 0.0 { tonemap(mean, d) } else { Vec3::ZERO };
            let c = back.lerp(scene, a);
            for (k, v) in c.to_array().into_iter().enumerate() {
                if let Some(o) = o.get_mut(k) {
                    *o = (texturecraft_texel::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8;
                }
            }
            if let Some(o) = o.get_mut(3) {
                *o = 255;
            }
        });
        out
    }
}

impl Tracer {
    /// Edge-avoiding à-trous filtering of the mesh radiance: lighting is divided by the first-hit
    /// reflectance (so texture detail stays sharp), filtered five times with growing steps, its
    /// weights falling off across normal, depth and luminance edges (scaled by the estimated
    /// noise), then multiplied back. Returns the mean radiance per pixel.
    fn denoise(&self) -> Vec<Vec3> {
        let (w, h) = (self.width as usize, self.height as usize);
        struct G {
            illum: Vec3,
            albedo: Vec3,
            normal: Vec3,
            depth: f32,
            var: f32,
            hit: bool,
        }
        let guides: Vec<G> = self
            .acc
            .par_iter()
            .map(|px| {
                let n = px.hits.max(1.0);
                let albedo = (px.albedo / n).max(Vec3::splat(0.02));
                let mean = px.hit / n;
                let var = (px.hit_lum2 / n - luminance(mean).powi(2)).max(0.0) / n;
                let a = luminance(albedo).max(0.02);
                G { illum: mean / albedo, albedo, normal: px.normal.normalize_or(Vec3::Z), depth: px.depth / n, var: var / (a * a), hit: px.hits > 0.0 }
            })
            .collect();
        const H: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];
        let mut cur: Vec<(Vec3, f32)> = guides.iter().map(|g| (g.illum, g.var)).collect();
        for step in [1usize, 2, 4, 8, 16] {
            let prev = cur;
            cur = (0..w * h)
                .into_par_iter()
                .map(|i| {
                    let (Some(p), Some(&(ip, vp))) = (guides.get(i), prev.get(i)) else { return (Vec3::ZERO, 0.0) };
                    if !p.hit {
                        return (ip, vp);
                    }
                    let (x, y) = ((i % w) as i64, (i / w) as i64);
                    let sigma_l = 4.0 * vp.sqrt() + 1e-3;
                    let (mut sum, mut wsum, mut vsum) = (Vec3::ZERO, 0.0f32, 0.0f32);
                    for (j, hy) in H.iter().enumerate() {
                        for (k, hx) in H.iter().enumerate() {
                            let qx = x + (k as i64 - 2) * step as i64;
                            let qy = y + (j as i64 - 2) * step as i64;
                            if qx < 0 || qy < 0 || qx >= w as i64 || qy >= h as i64 {
                                continue;
                            }
                            let qi = qy as usize * w + qx as usize;
                            let (Some(q), Some(&(iq, vq))) = (guides.get(qi), prev.get(qi)) else { continue };
                            if !q.hit {
                                continue;
                            }
                            let wn = p.normal.dot(q.normal).max(0.0).powi(64);
                            let wz = (-(p.depth - q.depth).abs() / (0.01 * p.depth.max(1e-3) * step as f32)).exp();
                            let wl = (-(luminance(ip) - luminance(iq)).abs() / sigma_l).exp();
                            let wt = hx * hy * wn * wz * wl;
                            sum += iq * wt;
                            wsum += wt;
                            vsum += wt * wt * vq;
                        }
                    }
                    if wsum > 1e-8 { (sum / wsum, vsum / (wsum * wsum)) } else { (ip, vp) }
                })
                .collect();
        }
        cur.iter().zip(&guides).map(|((illum, _), g)| *illum * g.albedo).collect()
    }
}

/// The viewport's flat studio gray with a gentle falloff (matches the post pass).
fn backdrop(uv: Vec2) -> Vec3 {
    let r = (uv * 2.0 - Vec2::ONE).length() * 0.35;
    Vec3::splat(0.0232 * (1.06 - r * r))
}

fn tonemap(hdr: Vec3, d: &TraceDisplay) -> Vec3 {
    let x = hdr.max(Vec3::ZERO) * d.exposure;
    if !d.aces {
        return x.min(Vec3::ONE);
    }
    let v = x * 0.6;
    ((v * (v * 2.51 + 0.03)) / (v * (v * 2.43 + 0.59) + 0.14)).clamp(Vec3::ZERO, Vec3::ONE)
}

/// Small, fast per-sample generator (PCG hash seeded by pixel and pass).
struct Rng(u32);

impl Rng {
    fn new(x: u32, y: u32, pass: u32) -> Rng {
        Rng(texturecraft_texel::noise::pcg3d(x, y, pass.wrapping_mul(7919).wrapping_add(17))[0] | 1)
    }

    fn next(&mut self) -> f32 {
        // xorshift32
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Shading data at a hit.
struct Surface {
    pos: Vec3,
    /// Geometric normal facing the incoming ray.
    geo: Vec3,
    n: Vec3,
    /// Diffuse albedo and specular reflectance at normal incidence.
    diffuse: Vec3,
    f0: Vec3,
    roughness: f32,
    emissive: Vec3,
    opacity: f32,
}

fn lerp3(a: Vec3, b: Vec3, c: Vec3, u: f32, v: f32) -> Vec3 {
    a * (1.0 - u - v) + b * u + c * v
}

/// Tangent-space normal blending (reoriented normal mapping), as in the viewport shader.
fn rnm(base: Vec3, detail: Vec3) -> Vec3 {
    let t = base + Vec3::Z;
    let u = detail * Vec3::new(-1.0, -1.0, 1.0);
    (t * t.dot(u) - u * t.z).normalize_or(Vec3::Z)
}

fn surface(scene: &TraceScene, tri: u32, u: f32, v: f32, o: Vec3, d: Vec3, t: f32) -> Option<Surface> {
    let m = &scene.mesh;
    let i = tri as usize * 3;
    let idx = [*m.indices.get(i)? as usize, *m.indices.get(i + 1)? as usize, *m.indices.get(i + 2)? as usize];
    let p = idx.map(|k| m.positions.get(k).copied().unwrap_or(Vec3::ZERO));
    let mut geo = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or(Vec3::Y);
    let nrm = idx.map(|k| m.normals.get(k).copied().unwrap_or(geo));
    let mut n_geo = lerp3(nrm[0], nrm[1], nrm[2], u, v).normalize_or(geo);
    if geo.dot(d) > 0.0 {
        geo = -geo;
    }
    if n_geo.dot(geo) < 0.0 {
        n_geo = -n_geo;
    }
    let uvs = idx.map(|k| m.uvs.get(k).copied().unwrap_or(Vec2::ZERO));
    let uv = uvs[0] * (1.0 - u - v) + uvs[1] * u + uvs[2] * v;
    let tan = idx.map(|k| m.tangents.get(k).copied().unwrap_or(texturecraft_geom::glam::Vec4::X));
    let tw = lerp3(tan[0].truncate(), tan[1].truncate(), tan[2].truncate(), u, v);
    let sign = if tan[0].w < 0.0 { -1.0 } else { 1.0 };

    let set = scene.tri_set.get(tri as usize).and_then(|s| scene.sets.get(*s));
    let ch = |c: Channel, dx: i32, dy: i32| -> [f32; 3] {
        let Some(s) = set else { return c.default_value() };
        let res = s.composite.resolution.max(1) as i32;
        let x = ((uv.x * res as f32).floor() as i32 + dx).rem_euclid(res) as u32;
        let y = ((uv.y * res as f32).floor() as i32 + dy).rem_euclid(res) as u32;
        s.composite.value(c, x, y)
    };
    let base = Vec3::from(ch(Channel::BaseColor, 0, 0));
    let k = set.map_or(0.0, |s| s.height_strength * s.composite.resolution as f32 / 16.0);
    let n_height = if k != 0.0 {
        let h = |dx, dy| ch(Channel::Height, dx, dy)[0];
        Vec3::new(-(h(1, 0) - h(-1, 0)) * 0.5 * k, (h(0, 1) - h(0, -1)) * 0.5 * k, 1.0).normalize_or(Vec3::Z)
    } else {
        Vec3::Z
    };
    let detail = (Vec3::from(ch(Channel::Normal, 0, 0)) * 2.0 - Vec3::ONE).normalize_or(Vec3::Z);
    let n_ts = rnm(n_height, detail);
    let tt = (tw - n_geo * n_geo.dot(tw)).normalize_or(n_geo.any_orthonormal_vector());
    let b = n_geo.cross(tt) * sign;
    let mut n = (tt * n_ts.x + b * n_ts.y + n_geo * n_ts.z).normalize_or(n_geo);
    if n.dot(-d) <= 0.0 {
        n = n_geo;
    }
    // Glossiness replaces roughness wherever a set has it (as in the viewport).
    let roughness = match set {
        Some(s) if s.composite.channels.contains_key(&Channel::Glossiness) => 1.0 - ch(Channel::Glossiness, 0, 0)[0],
        _ => ch(Channel::Roughness, 0, 0)[0],
    };
    let (diffuse, f0) = if scene.spec_gloss {
        let f0 = Vec3::from(ch(Channel::Specular, 0, 0)).clamp(Vec3::ZERO, Vec3::ONE);
        (base * (1.0 - f0.max_element()), f0)
    } else {
        let metallic = ch(Channel::Metallic, 0, 0)[0].clamp(0.0, 1.0);
        let level = ch(Channel::SpecularLevel, 0, 0)[0].clamp(0.0, 1.0);
        (base * (1.0 - metallic), Vec3::splat(0.08 * level).lerp(base, metallic))
    };
    Some(Surface {
        pos: o + d * t,
        geo,
        n,
        diffuse,
        f0,
        roughness: roughness.clamp(0.045, 1.0),
        emissive: Vec3::from(ch(Channel::Emissive, 0, 0)),
        opacity: ch(Channel::Opacity, 0, 0)[0].clamp(0.0, 1.0),
    })
}

fn luminance(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

fn schlick(f0: Vec3, cos: f32) -> Vec3 {
    f0 + (Vec3::ONE - f0) * (1.0 - cos.clamp(0.0, 1.0)).powi(5)
}

/// Smith GGX masking for one direction (local frame, z = normal).
fn smith_g1(w: Vec3, a: f32) -> f32 {
    let z = w.z.max(1e-6);
    2.0 * z / (z + (a * a * (1.0 - z * z) + z * z).sqrt())
}

/// GGX visible-normal sampling (local frame, z = normal).
fn sample_vndf(ve: Vec3, a: f32, u1: f32, u2: f32) -> Vec3 {
    let vh = Vec3::new(a * ve.x, a * ve.y, ve.z).normalize_or(Vec3::Z);
    let len2 = vh.x * vh.x + vh.y * vh.y;
    let t1 = if len2 > 0.0 { Vec3::new(-vh.y, vh.x, 0.0) / len2.sqrt() } else { Vec3::X };
    let t2 = vh.cross(t1);
    let r = u1.sqrt();
    let phi = TAU * u2;
    let p1 = r * phi.cos();
    let s = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - s) * (1.0 - p1 * p1).max(0.0).sqrt() + s * r * phi.sin();
    let nh = t1 * p1 + t2 * p2 + vh * (1.0 - p1 * p1 - p2 * p2).max(0.0).sqrt();
    Vec3::new(a * nh.x, a * nh.y, nh.z.max(1e-6)).normalize_or(Vec3::Z)
}

/// The surface's BRDF at one hit, in a local frame (z = shading normal).
struct Bsdf {
    t: Vec3,
    b: Vec3,
    n: Vec3,
    /// View direction, local.
    v: Vec3,
    f0: Vec3,
    diffuse: Vec3,
    /// GGX alpha.
    a: f32,
    /// Fresnel at the view angle.
    fv: Vec3,
    /// Probability of sampling the specular lobe.
    p_spec: f32,
}

impl Bsdf {
    fn new(s: &Surface, v: Vec3) -> Bsdf {
        let n = s.n;
        let t = n.any_orthonormal_vector();
        let b = n.cross(t);
        let vl = Vec3::new(v.dot(t), v.dot(b), v.dot(n).max(1e-4));
        let (f0, diffuse) = (s.f0, s.diffuse);
        let fv = schlick(f0, vl.z);
        let ld = luminance(diffuse * (Vec3::ONE - fv));
        let p_spec = if ld < 1e-5 { 1.0 } else { (luminance(fv) / (luminance(fv) + ld)).clamp(0.1, 0.9) };
        Bsdf { t, b, n, v: vl, f0, diffuse, a: (s.roughness * s.roughness).max(1e-3), fv, p_spec }
    }

    fn local(&self, w: Vec3) -> Vec3 {
        Vec3::new(w.dot(self.t), w.dot(self.b), w.dot(self.n))
    }

    fn ggx_d(&self, h: Vec3) -> f32 {
        let a2 = self.a * self.a;
        let d = h.z * h.z * (a2 - 1.0) + 1.0;
        a2 / (PI * d * d).max(1e-12)
    }

    /// BRDF times cos(theta) toward world direction `w`, and the density of [`Self::sample`]
    /// picking it.
    fn eval(&self, w: Vec3) -> (Vec3, f32) {
        let l = self.local(w);
        if l.z <= 0.0 {
            return (Vec3::ZERO, 0.0);
        }
        let h = (l + self.v).normalize_or(Vec3::Z);
        let d = self.ggx_d(h);
        let g1v = smith_g1(self.v, self.a);
        let spec = schlick(self.f0, l.dot(h)) * d * g1v * smith_g1(l, self.a) / (4.0 * self.v.z);
        let diff = self.diffuse * (Vec3::ONE - self.fv) * (l.z / PI);
        let pdf_spec = g1v * d / (4.0 * self.v.z);
        let pdf_diff = l.z / PI;
        (spec + diff, self.p_spec * pdf_spec + (1.0 - self.p_spec) * pdf_diff)
    }

    /// A world direction from the BRDF lobes (specular by visible normals, diffuse by cosine).
    fn sample(&self, rng: &mut Rng) -> Option<Vec3> {
        let (u0, u1, u2) = (rng.next(), rng.next(), rng.next());
        let l = if u0 < self.p_spec {
            let h = sample_vndf(self.v, self.a, u1, u2);
            h * 2.0 * self.v.dot(h) - self.v
        } else {
            let r = u1.sqrt();
            let phi = TAU * u2;
            Vec3::new(r * phi.cos(), r * phi.sin(), (1.0 - u1).max(0.0).sqrt())
        };
        (l.z > 0.0).then(|| self.t * l.x + self.b * l.y + self.n * l.z)
    }
}

/// Power-heuristic MIS weight of a strategy with density `a` against one with density `b`.
fn mis(a: f32, b: f32) -> f32 {
    let (a2, b2) = (a * a, b * b);
    if a2 + b2 > 0.0 { a2 / (a2 + b2) } else { 0.0 }
}

/// Clamp rare, very bright indirect contributions (fireflies) to keep progressive renders calm.
fn clamp_firefly(c: Vec3) -> Vec3 {
    let lum = luminance(c);
    if lum > 32.0 { c * (32.0 / lum) } else { c }
}

/// Radiance along a camera ray; None when it misses the mesh. Environment light is gathered
/// both by sampling it directly (next-event estimation) and by BRDF sampling, combined with MIS.
fn trace(scene: &TraceScene, mut o: Vec3, mut d: Vec3, settings: &TraceSettings, rng: &mut Rng) -> Option<Sample> {
    let origin = o;
    let mut first: Option<(Vec3, Vec3, f32)> = None;
    let mut thr = Vec3::ONE;
    let mut l = Vec3::ZERO;
    let mut bounce = 0u32;
    // Density of the BRDF sample that produced `d` (None for the camera ray).
    let mut brdf_pdf: Option<f32> = None;
    // Cutout surfaces don't count as bounces; cap them so a stack of cards can't loop forever.
    for _ in 0..settings.bounces + 24 {
        let Some(hit) = scene.bvh.closest(o, d, f32::MAX) else {
            let pdf = brdf_pdf?;
            let w = if scene.light.is_some() { mis(pdf, scene.env_pdf(d)) } else { 1.0 };
            l += clamp_firefly(thr * scene.env(d) * w);
            break;
        };
        let Some(s) = surface(scene, hit.tri, hit.u, hit.v, o, d, hit.t) else { break };
        if rng.next() > s.opacity {
            o = s.pos - s.geo * scene.eps;
            continue;
        }
        if first.is_none() {
            first = Some(((s.diffuse + s.f0).min(Vec3::ONE), s.n, (s.pos - origin).length()));
        }
        l += thr * s.emissive;
        if bounce >= settings.bounces {
            break;
        }
        let bsdf = Bsdf::new(&s, -d);
        // Next-event estimation toward the environment.
        if let Some((wl, pdf_env)) = scene.sample_env(rng)
            && wl.dot(s.geo) > 0.0
        {
            let (f, pdf_brdf) = bsdf.eval(wl);
            if f.max_element() > 0.0 && !scene.bvh.occluded(s.pos + s.geo * scene.eps, wl, f32::MAX) {
                l += clamp_firefly(thr * f * scene.env(wl) * (mis(pdf_env, pdf_brdf) / pdf_env));
            }
        }
        let Some(wi) = bsdf.sample(rng) else { break };
        let (f, pdf) = bsdf.eval(wi);
        if pdf <= 0.0 {
            break;
        }
        thr *= f / pdf;
        brdf_pdf = Some(pdf);
        if bounce >= 2 {
            let p = thr.max_element().clamp(0.05, 0.95);
            if rng.next() > p {
                break;
            }
            thr /= p;
        }
        let side = if wi.dot(s.geo) >= 0.0 { 1.0 } else { -1.0 };
        o = s.pos + s.geo * scene.eps * side;
        d = wi;
        bounce += 1;
    }
    let (albedo, normal, depth) = first?;
    Some(Sample { radiance: if l.is_finite() { l } else { Vec3::ZERO }, albedo, normal, depth })
}

#[cfg(test)]
mod tests {
    use super::*;
    use texturecraft_geom::primitives;

    fn scene(env: &str) -> TraceScene {
        let mesh = Arc::new(primitives::sphere(1.0, 32, 16));
        let bvh = Arc::new(Bvh::new(&mesh));
        let env = Arc::new(crate::ibl::environment(env, 128));
        TraceScene::new(mesh, bvh, Vec::new(), env, 0.0)
    }

    fn camera() -> OrbitCamera {
        let mut cam = OrbitCamera::default();
        cam.target = Vec3::ZERO;
        cam.distance = 4.0;
        cam
    }

    #[test]
    fn renders_the_mesh_over_the_backdrop_and_converges() {
        let s = scene("Studio Soft");
        let mut tr = Tracer::new(48, 32);
        for _ in 0..8 {
            tr.pass(&s, &camera(), &TraceSettings::default());
        }
        assert_eq!(tr.samples, 8);
        let d = TraceDisplay { exposure: 1.0, aces: true, env_opacity: 0.0, denoise: false };
        let img = tr.rgba8(&d);
        assert_eq!(img.len(), 48 * 32 * 4);
        let px = |x: usize, y: usize| &img[(y * 48 + x) * 4..(y * 48 + x) * 4 + 3];
        // The sphere (default material, lit by the studio) is brighter than the dark backdrop corner.
        let center: u32 = px(24, 16).iter().map(|v| *v as u32).sum();
        let corner: u32 = px(0, 0).iter().map(|v| *v as u32).sum();
        assert!(center > corner + 60, "center {center} corner {corner}");
        assert!(tr.acc.iter().all(|p| p.hit.is_finite() && p.env.is_finite()));
    }

    #[test]
    fn environment_shows_behind_the_mesh_with_opacity() {
        let s = scene("Overcast");
        let mut tr = Tracer::new(16, 8);
        tr.pass(&s, &camera(), &TraceSettings::default());
        let corner = |opacity: f32| tr.rgba8(&TraceDisplay { exposure: 1.0, aces: true, env_opacity: opacity, denoise: false })[0];
        assert!(corner(1.0) > corner(0.0) + 40);
    }

    #[test]
    fn light_sampling_matches_brdf_sampling_on_average() {
        let lit = scene("Studio Soft");
        let unlit = TraceScene { light: None, ..lit.clone() };
        let mean = |s: &TraceScene| {
            let mut rng = Rng::new(9, 9, 9);
            let n = 40_000;
            let mut sum = Vec3::ZERO;
            for _ in 0..n {
                sum += trace(s, Vec3::new(0.3, 0.4, 4.0), Vec3::new(-0.07, -0.1, -1.0).normalize(), &TraceSettings { bounces: 3 }, &mut rng).map_or(Vec3::ZERO, |s| s.radiance);
            }
            sum / n as f32
        };
        let (a, b) = (mean(&lit), mean(&unlit));
        assert!((a - b).length() < 0.05 * b.length().max(0.05), "with light sampling {a}, without {b}");
    }

    #[test]
    fn passes_split_into_row_slices_match_whole_passes() {
        let s = scene("Studio Soft");
        let (mut whole, mut sliced) = (Tracer::new(40, 30), Tracer::new(40, 30));
        let d = TraceDisplay { exposure: 1.0, aces: true, env_opacity: 0.0, denoise: false };
        for _ in 0..2 {
            whole.pass(&s, &camera(), &TraceSettings::default());
            let mut done = false;
            while !done {
                done = sliced.pass_rows(&s, &camera(), &TraceSettings::default(), 7);
            }
        }
        assert_eq!(sliced.samples, 2);
        assert_eq!(whole.rgba8(&d), sliced.rgba8(&d));
    }

    #[test]
    fn denoising_lowers_noise_and_keeps_the_mean() {
        let s = scene("Studio Contrast");
        let mut tr = Tracer::new(64, 48);
        for _ in 0..4 {
            tr.pass(&s, &camera(), &TraceSettings::default());
        }
        let raw = tr.rgba8(&TraceDisplay { exposure: 1.0, aces: true, env_opacity: 0.0, denoise: false });
        let clean = tr.rgba8(&TraceDisplay { exposure: 1.0, aces: true, env_opacity: 0.0, denoise: true });
        // Neighbour differences inside the ball (rows 18..30, columns 26..38) drop; the mean stays.
        let stats = |img: &[u8]| {
            let px = |x: usize, y: usize| img[(y * 64 + x) * 4] as f32;
            let (mut diff, mut sum, mut n) = (0.0, 0.0, 0.0);
            for y in 18..30 {
                for x in 26..38 {
                    diff += (px(x, y) - px(x + 1, y)).abs();
                    sum += px(x, y);
                    n += 1.0;
                }
            }
            (diff / n, sum / n)
        };
        let ((d_raw, m_raw), (d_clean, m_clean)) = (stats(&raw), stats(&clean));
        assert!(d_clean < d_raw * 0.6, "noise {d_raw} -> {d_clean}");
        assert!((m_raw - m_clean).abs() < 0.08 * m_raw.max(1.0), "mean {m_raw} -> {m_clean}");
    }

    #[test]
    fn white_furnace_keeps_energy_bounded() {
        // A uniform white environment: a white diffuse sphere must not exceed the environment.
        let mesh = Arc::new(primitives::sphere(1.0, 24, 12));
        let bvh = Arc::new(Bvh::new(&mesh));
        let env = Arc::new(EnvImage { width: 8, height: 4, data: vec![[1.0; 3]; 32] });
        let s = TraceScene::new(mesh, bvh, Vec::new(), env, 0.0);
        let mut rng = Rng::new(1, 2, 3);
        let mut sum = Vec3::ZERO;
        let n = 4000;
        for _ in 0..n {
            sum += trace(&s, Vec3::new(0.0, 0.0, 4.0), Vec3::NEG_Z, &TraceSettings { bounces: 8 }, &mut rng).map_or(Vec3::ZERO, |s| s.radiance);
        }
        let avg = sum / n as f32;
        assert!(avg.max_element() <= 1.05 && avg.min_element() > 0.3, "{avg}");
    }
}
