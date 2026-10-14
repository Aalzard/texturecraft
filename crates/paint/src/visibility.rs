//! Which surface points the paint camera can see: a software-rasterized eye-distance buffer,
//! rendered once at the start of a stroke.

use rayon::prelude::*;
use texturecraft_geom::glam::{Mat4, Vec3, Vec4};
use texturecraft_geom::{Mesh, OrbitCamera};

const BAND: usize = 16;

/// Distance from the eye to the nearest surface per screen pixel (f32::MAX where empty).
#[derive(Clone, Debug)]
pub struct EyeDepth {
    pub width: u32,
    pub height: u32,
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub dist: Vec<f32>,
    /// Tolerance in world units (≈ 0.4% of the scene radius).
    pub bias: f32,
}

impl EyeDepth {
    /// Rasterize `mesh` seen by `camera` into a `width × height` buffer (sizes capped at 4096).
    pub fn render(mesh: &Mesh, camera: &OrbitCamera, width: u32, height: u32) -> EyeDepth {
        let (w, h) = (width.clamp(1, 4096), height.clamp(1, 4096));
        let aspect = w as f32 / h as f32;
        let vp = camera.view_proj(aspect);
        let eye = camera.eye();
        // Project every vertex once.
        let clip: Vec<Vec4> = mesh.positions.iter().map(|p| vp * p.extend(1.0)).collect();
        let screen: Vec<Option<(f32, f32)>> =
            clip.iter().map(|c| (c.w > 1e-5).then(|| ((c.x / c.w * 0.5 + 0.5) * w as f32, (0.5 - c.y / c.w * 0.5) * h as f32))).collect();
        let dist_v: Vec<f32> = mesh.positions.iter().map(|p| p.distance(eye)).collect();
        let bands = (h as usize).div_ceil(BAND);
        let mut bins: Vec<Vec<u32>> = vec![Vec::new(); bands];
        for t in 0..mesh.triangle_count() {
            let Some([a, b, c]) = mesh.tri(t) else { continue };
            let (Some(Some(pa)), Some(Some(pb)), Some(Some(pc))) = (screen.get(a), screen.get(b), screen.get(c)) else { continue };
            let y0 = pa.1.min(pb.1).min(pc.1).floor().max(0.0);
            let y1 = pa.1.max(pb.1).max(pc.1).ceil().min(h as f32);
            if y1 <= y0 || !y0.is_finite() || !y1.is_finite() {
                continue;
            }
            let (b0, b1) = (y0 as usize / BAND, ((y1 as usize).saturating_sub(1)) / BAND);
            for bin in bins.iter_mut().take(b1 + 1).skip(b0) {
                bin.push(t as u32);
            }
        }
        let mut dist = vec![f32::MAX; (w * h) as usize];
        dist.par_chunks_mut(BAND * w as usize).enumerate().for_each(|(bi, rows)| {
            let row0 = bi * BAND;
            let nrows = rows.len() / w as usize;
            for &t in bins.get(bi).map(Vec::as_slice).unwrap_or(&[]) {
                let Some([a, b, c]) = mesh.tri(t as usize) else { continue };
                let (Some(Some(pa)), Some(Some(pb)), Some(Some(pc))) = (screen.get(a), screen.get(b), screen.get(c)) else { continue };
                let (da, db, dc) = (dist_v[a], dist_v[b], dist_v[c]);
                let area = (pb.0 - pa.0) * (pc.1 - pa.1) - (pb.1 - pa.1) * (pc.0 - pa.0);
                if area.abs() < 1e-9 {
                    continue;
                }
                let x0 = pa.0.min(pb.0).min(pc.0).floor().max(0.0) as usize;
                let x1 = (pa.0.max(pb.0).max(pc.0).ceil().min(w as f32)) as usize;
                let y0 = (pa.1.min(pb.1).min(pc.1).floor().max(row0 as f32)) as usize;
                let y1 = (pa.1.max(pb.1).max(pc.1).ceil().min((row0 + nrows) as f32)) as usize;
                for y in y0.max(row0)..y1 {
                    for x in x0..x1.min(w as usize) {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((pb.0 - px) * (pc.1 - py) - (pb.1 - py) * (pc.0 - px)) / area;
                        let w1 = ((pc.0 - px) * (pa.1 - py) - (pc.1 - py) * (pa.0 - px)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let d = w0 * da + w1 * db + w2 * dc;
                        if let Some(slot) = rows.get_mut((y - row0) * w as usize + x)
                            && d < *slot
                        {
                            *slot = d;
                        }
                    }
                }
            }
        });
        EyeDepth { width: w, height: h, view_proj: vp, eye, dist, bias: mesh.radius() * 0.004 }
    }

    /// Whether world point `p` is visible (not hidden behind other geometry, inside the view).
    pub fn visible(&self, p: Vec3) -> bool {
        let c = self.view_proj * p.extend(1.0);
        if c.w <= 1e-6 {
            return false;
        }
        let (nx, ny) = (c.x / c.w, c.y / c.w);
        if nx.abs() > 1.0 || ny.abs() > 1.0 {
            return false;
        }
        let x = (((nx * 0.5 + 0.5) * self.width as f32) as u32).min(self.width - 1);
        let y = (((0.5 - ny * 0.5) * self.height as f32) as u32).min(self.height - 1);
        // Conservative: the nearest surface among the pixel and its neighbours.
        let mut near = f32::MAX;
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (sx, sy) = (x as i32 + dx, y as i32 + dy);
                if sx < 0 || sy < 0 || sx >= self.width as i32 || sy >= self.height as i32 {
                    continue;
                }
                if let Some(d) = self.dist.get((sy as u32 * self.width + sx as u32) as usize) {
                    near = near.min(*d);
                }
            }
        }
        // Allow a bias that grows with distance (depth buffer resolution).
        p.distance(self.eye) <= near + self.bias + near * 0.002
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use texturecraft_geom::primitives;

    #[test]
    fn front_of_sphere_is_visible_back_is_not() {
        let m = primitives::sphere(1.0, 64, 32);
        let mut cam = OrbitCamera::default();
        cam.yaw = 0.0;
        cam.pitch = 0.0;
        cam.frame(Vec3::splat(-1.0), Vec3::splat(1.0));
        let d = EyeDepth::render(&m, &cam, 256, 256);
        let toward = (cam.eye() - cam.target).normalize();
        assert!(d.visible(toward * 1.0));
        assert!(!d.visible(-toward * 1.0));
        assert!(!d.visible(Vec3::new(100.0, 0.0, 0.0)));
    }
}
