//! From pointer samples to stamps: picking, lazy mouse, spacing along the surface, stamp
//! frames, jitter and symmetry.

use texturecraft_geom::glam::{Quat, Vec2, Vec3};
use texturecraft_geom::{Bvh, Mesh, OrbitCamera, surface_at};
use texturecraft_texel::noise::pcg3d;

use crate::brush::{Alignment, BrushSettings};

/// Scene access for picking.
pub struct PickCtx<'a> {
    pub mesh: &'a Mesh,
    pub bvh: &'a Bvh,
    pub camera: &'a OrbitCamera,
    /// Viewport size in pixels.
    pub viewport: Vec2,
}

/// A point on the mesh under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceHit {
    pub pos: Vec3,
    pub nrm: Vec3,
    pub tri: u32,
    pub uv: Vec2,
    pub material: usize,
    pub screen: Vec2,
}

/// Pick the mesh at a viewport pixel (origin top-left).
pub fn pick(ctx: &PickCtx, screen: Vec2) -> Option<SurfaceHit> {
    let vp = ctx.viewport.max(Vec2::ONE);
    let ndc = Vec2::new(screen.x / vp.x * 2.0 - 1.0, 1.0 - screen.y / vp.y * 2.0);
    let ray = ctx.camera.ray(ndc, vp.x / vp.y);
    let hit = ctx.bvh.closest(ray.origin, ray.dir, f32::MAX)?;
    let s = surface_at(ctx.mesh, hit.tri, hit.u, hit.v)?;
    Some(SurfaceHit { pos: s.pos, nrm: s.nrm, tri: hit.tri, uv: s.uv, material: ctx.mesh.material_of(hit.tri as usize).unwrap_or(0), screen })
}

/// One brush dab in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stamp {
    pub center: Vec3,
    pub normal: Vec3,
    pub tangent: Vec3,
    pub bitangent: Vec3,
    pub radius: f32,
    pub flow: f32,
    pub hardness: f32,
    /// Half-depth of the stamp volume relative to the radius.
    pub depth: f32,
    /// Varies procedural alphas per stamp.
    pub seed: u32,
    /// A symmetry copy: it paints where symmetry puts it, whether the camera sees that or not.
    pub copy: bool,
}

/// An in-progress stroke.
#[derive(Clone, Debug)]
pub struct Stroke {
    pub settings: BrushSettings,
    /// Brush radius in world units at full pressure.
    pub radius: f32,
    center: Vec3,
    last: Option<SurfaceHit>,
    dist: f32,
    lazy: Option<Vec2>,
    count: u32,
    last_dir: Option<Vec3>,
    /// Every stamp emitted so far (for replay, inspect and tests).
    pub stamps: Vec<Stamp>,
}

fn rand01(n: u32, k: u32) -> f32 {
    pcg3d(n, k, 0x5EED)[0] as f32 / u32::MAX as f32
}

impl Stroke {
    pub fn new(settings: &BrushSettings, mesh: &Mesh) -> Stroke {
        let s = settings.sanitized();
        let (lo, hi) = mesh.bounds();
        let diag = (hi - lo).length().max(1e-4);
        Stroke { radius: s.size * diag, settings: s, center: (lo + hi) * 0.5, last: None, dist: 0.0, lazy: None, count: 0, last_dir: None, stamps: Vec::new() }
    }

    /// Feed a pointer position (pixels) with pen pressure (1 for a mouse). Returns new stamps.
    pub fn add(&mut self, ctx: &PickCtx, screen: Vec2, pressure: f32) -> Vec<Stamp> {
        let mut p = screen;
        if self.settings.lazy > 0.0 {
            let l = *self.lazy.get_or_insert(screen);
            let d = screen - l;
            let len = d.length();
            let nl = if len > self.settings.lazy { l + d * (1.0 - self.settings.lazy / len) } else { l };
            self.lazy = Some(nl);
            if self.last.is_some() && nl == l {
                return Vec::new();
            }
            p = nl;
        }
        let mut out = Vec::new();
        self.feed(ctx, p, pressure, 0, &mut out);
        if self.settings.particles.enabled {
            out = out.iter().flat_map(|s| crate::particles::emit(ctx, &self.settings.particles, s)).collect();
        }
        let mut all = Vec::with_capacity(out.len() * 2);
        for s in out {
            all.push(s);
            all.extend(self.mirrors(&s));
        }
        self.stamps.extend_from_slice(&all);
        all
    }

    fn radius_at(&self, ctx: &PickCtx, pos: Vec3, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        let scale = self.settings.min_size + (1.0 - self.settings.min_size) * p;
        let r = self.radius * if pressure < 1.0 { scale } else { 1.0 };
        if self.settings.screen_space { r * (pos.distance(ctx.camera.eye()) / ctx.camera.distance.max(1e-4)) } else { r }
    }

    fn feed(&mut self, ctx: &PickCtx, screen: Vec2, pressure: f32, depth: u32, out: &mut Vec<Stamp>) {
        let Some(hit) = pick(ctx, screen) else {
            self.last = None;
            return;
        };
        let radius = self.radius_at(ctx, hit.pos, pressure);
        let Some(last) = self.last else {
            out.push(self.stamp(ctx, &hit, radius));
            self.last = Some(hit);
            self.dist = 0.0;
            return;
        };
        let seg = hit.pos - last.pos;
        let len = seg.length();
        if len < 1e-9 {
            return;
        }
        let step = (self.settings.spacing * 2.0 * radius).max(1e-6);
        if len > 4.0 * radius && depth < 4 {
            // Jumped across a silhouette or gap: resample in screen space, never through air.
            let n = ((len / radius).ceil() as u32).clamp(2, 64);
            let a = last.screen;
            for k in 1..=n {
                let s = a + (screen - a) * (k as f32 / n as f32);
                self.feed(ctx, s, pressure, depth + 1, out);
            }
            return;
        }
        self.last_dir = Some(seg / len);
        let mut t = step - self.dist;
        let mut emitted = 0;
        while t <= len && emitted < 4096 {
            let p = last.pos + seg * (t / len);
            // Snap to the surface by re-picking through the projected point.
            let aspect = ctx.viewport.x.max(1.0) / ctx.viewport.y.max(1.0);
            let snapped = ctx
                .camera
                .project(p, aspect)
                .map(|ndc| Vec2::new((ndc.x * 0.5 + 0.5) * ctx.viewport.x, (0.5 - ndc.y * 0.5) * ctx.viewport.y))
                .and_then(|s| pick(ctx, s))
                .filter(|h| h.pos.distance(p) < radius * 2.0);
            let h = snapped.unwrap_or(SurfaceHit { pos: p, ..hit });
            out.push(self.stamp(ctx, &h, radius));
            emitted += 1;
            t += step;
        }
        self.dist = len - (t - step);
        self.last = Some(hit);
    }

    fn stamp(&mut self, ctx: &PickCtx, hit: &SurfaceHit, radius: f32) -> Stamp {
        let s = &self.settings;
        let n = self.count;
        self.count = self.count.wrapping_add(1);
        let jitter = |k: u32, amount: f32| if amount > 0.0 { (rand01(n, k) * 2.0 - 1.0) * amount } else { 0.0 };
        let normal = match s.alignment {
            Alignment::Tangent => hit.nrm,
            Alignment::Camera => -ctx.camera.forward(),
        };
        let cam_up = {
            let f = ctx.camera.forward();
            let right = f.cross(Vec3::Y).try_normalize().unwrap_or(Vec3::X);
            right.cross(f)
        };
        let up_src = if s.follow_path { self.last_dir.unwrap_or(cam_up) } else { cam_up };
        let up = (up_src - normal * normal.dot(up_src)).try_normalize().unwrap_or_else(|| normal.any_orthonormal_vector());
        let tangent0 = up.cross(normal).try_normalize().unwrap_or_else(|| normal.any_orthonormal_vector());
        let angle = (s.angle + jitter(1, s.jitter_angle) * 180.0).to_radians();
        let q = Quat::from_axis_angle(normal, angle);
        let tangent = q * tangent0;
        let bitangent = normal.cross(tangent);
        let r = radius * (1.0 + jitter(2, s.jitter_size)).max(0.05);
        let offset = (tangent * jitter(3, 1.0) + bitangent * jitter(4, 1.0)) * s.jitter_position * radius;
        let flow = (s.flow * (1.0 - jitter(5, s.jitter_flow).abs())).clamp(0.0, 1.0);
        Stamp { center: hit.pos + offset, normal, tangent, bitangent, radius: r, flow, hardness: s.hardness, depth: s.depth, seed: n, copy: false }
    }

    /// Mirror copies of a stamp for the enabled symmetry axes, then radial copies of it and
    /// its mirrors.
    fn mirrors(&self, s: &Stamp) -> Vec<Stamp> {
        let sym = self.settings.symmetry;
        if !sym.any() {
            return Vec::new();
        }

        let mut out = Vec::new();
        for sign in sym.mirror_signs().skip(1) {
            let normal = s.normal * sign;
            let tangent = s.tangent * sign;
            out.push(Stamp { center: self.center + (s.center - self.center) * sign, normal, tangent, bitangent: normal.cross(tangent), copy: true, ..*s });
        }

        if sym.radial > 1 {
            let axis = [Vec3::X, Vec3::Y, Vec3::Z].get(sym.axis as usize).copied().unwrap_or(Vec3::Y);
            let base: Vec<Stamp> = std::iter::once(*s).chain(out.iter().copied()).collect();

            for k in 1..sym.radial {
                let q = Quat::from_axis_angle(axis, f32::from(k) * std::f32::consts::TAU / f32::from(sym.radial));
                out.extend(base.iter().map(|b| Stamp {
                    center: self.center + q * (b.center - self.center),
                    normal: q * b.normal,
                    tangent: q * b.tangent,
                    bitangent: q * b.bitangent,
                    copy: true,
                    ..*b
                }));
            }
        }

        out
    }

    pub fn last_hit(&self) -> Option<SurfaceHit> {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use texturecraft_geom::primitives;

    fn ctx_parts() -> (Mesh, Bvh, OrbitCamera) {
        let m = primitives::sphere(1.0, 64, 32);
        let b = Bvh::new(&m);
        let mut c = OrbitCamera::default();
        c.pitch = 0.0;
        c.frame(Vec3::splat(-1.0), Vec3::splat(1.0));
        (m, b, c)
    }

    #[test]
    fn spacing_controls_stamp_count() {
        let (m, b, c) = ctx_parts();
        let ctx = PickCtx { mesh: &m, bvh: &b, camera: &c, viewport: Vec2::new(400.0, 400.0) };
        let count = |spacing: f32| {
            let mut s = Stroke::new(&BrushSettings { spacing, ..BrushSettings::default() }, &m);
            let mut n = 0;
            for i in 0..=40 {
                n += s.add(&ctx, Vec2::new(150.0 + i as f32 * 2.5, 200.0), 1.0).len();
            }
            n
        };
        let dense = count(0.05);
        let sparse = count(0.5);
        assert!(dense > sparse * 5, "{dense} vs {sparse}");
        assert!(sparse >= 2);
    }

    #[test]
    fn misses_produce_nothing_and_stamps_lie_on_the_surface() {
        let (m, b, c) = ctx_parts();
        let ctx = PickCtx { mesh: &m, bvh: &b, camera: &c, viewport: Vec2::new(400.0, 400.0) };
        let mut s = Stroke::new(&BrushSettings::default(), &m);
        assert!(s.add(&ctx, Vec2::new(2.0, 2.0), 1.0).is_empty());
        let st = s.add(&ctx, Vec2::new(200.0, 200.0), 1.0);
        assert_eq!(st.len(), 1);
        assert!((st[0].center.length() - 1.0).abs() < 0.01);
        assert!((st[0].tangent.dot(st[0].normal)).abs() < 1e-4);
    }

    #[test]
    fn symmetry_mirrors_stamps() {
        let (m, b, c) = ctx_parts();
        let ctx = PickCtx { mesh: &m, bvh: &b, camera: &c, viewport: Vec2::new(400.0, 400.0) };
        let mut st = BrushSettings::default();
        st.symmetry.x = true;
        let mut s = Stroke::new(&st, &m);
        let out = s.add(&ctx, Vec2::new(250.0, 200.0), 1.0);
        assert_eq!(out.len(), 2);
        assert!((out[0].center.x + out[1].center.x).abs() < 1e-4);

        st.symmetry.y = true;
        st.symmetry.z = true;
        let mut s = Stroke::new(&st, &m);
        assert_eq!(s.add(&ctx, Vec2::new(250.0, 180.0), 1.0).len(), 8);
    }

    #[test]
    fn radial_symmetry_spreads_copies_around_the_axis() {
        let (m, b, c) = ctx_parts();
        let ctx = PickCtx { mesh: &m, bvh: &b, camera: &c, viewport: Vec2::new(400.0, 400.0) };
        let mut st = BrushSettings::default();
        st.symmetry.radial = 4;
        let mut s = Stroke::new(&st, &m);
        let out = s.add(&ctx, Vec2::new(250.0, 180.0), 1.0);
        assert_eq!(out.len(), 4);
        // Same height and distance from the Y axis, a quarter turn apart: they sum to the axis.
        let sum = out.iter().fold(Vec3::ZERO, |a, x| a + x.center);
        assert!(sum.x.abs() < 1e-3 && sum.z.abs() < 1e-3, "{sum}");
        assert!(out.iter().all(|x| (x.center.y - out[0].center.y).abs() < 1e-4 && (x.normal.dot(x.tangent)).abs() < 1e-4));
        // With mirror X as well: each mirror is spread too.
        st.symmetry.x = true;
        let mut s = Stroke::new(&st, &m);
        assert_eq!(s.add(&ctx, Vec2::new(250.0, 180.0), 1.0).len(), 8);
        // Out-of-range settings are clamped.
        st.symmetry = crate::brush::Symmetry { radial: 200, axis: 9, ..st.symmetry };
        let clean = st.sanitized().symmetry;
        assert_eq!((clean.radial, clean.axis), (32, 2));
    }
}
