//! Bounding volume hierarchy over a mesh's triangles: closest-hit and any-hit ray casts, and
//! box queries (used by picking, baking and brush stamps).

use glam::{Vec3, Vec3A};

use crate::Mesh;

#[derive(Clone, Copy, Debug)]
struct Node {
    lo: Vec3A,
    hi: Vec3A,
    /// Leaf: first triangle in `order`; inner: index of the left child (right = left + 1).
    first: u32,
    /// Triangles in a leaf; 0 for inner nodes.
    count: u32,
}

/// A ray hit: distance, triangle and barycentrics (weights of vertices 1 and 2).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub tri: u32,
    pub u: f32,
    pub v: f32,
}

#[derive(Clone, Debug)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangle vertices in BVH order.
    tris: Vec<[Vec3; 3]>,
    /// Original triangle index of each entry in `tris`.
    order: Vec<u32>,
}

const LEAF: usize = 4;

/// Traversal stack of (node, entry distance): inline for typical depths (no allocation per
/// ray), spilling to the heap for degenerate trees.
struct Stack {
    inline: [(u32, f32); 64],
    len: usize,
    spill: Vec<(u32, f32)>,
}

impl Default for Stack {
    fn default() -> Self {
        Stack { inline: [(0, 0.0); 64], len: 0, spill: Vec::new() }
    }
}

impl Stack {
    fn push(&mut self, v: (u32, f32)) {
        match self.inline.get_mut(self.len) {
            Some(slot) if self.spill.is_empty() => {
                *slot = v;
                self.len += 1;
            }
            _ => self.spill.push(v),
        }
    }

    fn pop(&mut self) -> Option<(u32, f32)> {
        self.spill.pop().or_else(|| {
            self.len = self.len.checked_sub(1)?;
            self.inline.get(self.len).copied()
        })
    }
}
const BINS: usize = 12;

fn grow(lo: &mut Vec3, hi: &mut Vec3, p: Vec3) {
    *lo = lo.min(p);
    *hi = hi.max(p);
}

fn area(lo: Vec3, hi: Vec3) -> f32 {
    let d = (hi - lo).max(Vec3::ZERO);
    d.x * d.y + d.y * d.z + d.z * d.x
}

impl Bvh {
    pub fn new(mesh: &Mesh) -> Bvh {
        let n = mesh.triangle_count();
        let mut tris = Vec::with_capacity(n);
        for t in 0..n {
            let Some([a, b, c]) = mesh.tri(t) else { continue };
            let p = |i: usize| mesh.positions.get(i).copied().unwrap_or(Vec3::ZERO);
            tris.push([p(a), p(b), p(c)]);
        }
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let cent: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut nodes = vec![Node { lo: Vec3A::ZERO, hi: Vec3A::ZERO, first: 0, count: tris.len() as u32 }];
        let mut stack = vec![0usize];
        while let Some(ni) = stack.pop() {
            let Node { first, count, .. } = nodes[ni];
            let (f, c) = (first as usize, count as usize);
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            let (mut clo, mut chi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for &t in &order[f..f + c] {
                for p in tris[t as usize] {
                    grow(&mut lo, &mut hi, p);
                }
                grow(&mut clo, &mut chi, cent[t as usize]);
            }
            nodes[ni].lo = lo.into();
            nodes[ni].hi = hi.into();
            if c <= LEAF {
                continue;
            }
            // Binned SAH along the widest centroid axis.
            let ext = chi - clo;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            if ext[axis] <= 1e-12 {
                continue;
            }
            let mut bins = [(0u32, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)); BINS];
            let bin_of = |p: Vec3| (((p[axis] - clo[axis]) / ext[axis] * BINS as f32) as usize).min(BINS - 1);
            for &t in &order[f..f + c] {
                let b = &mut bins[bin_of(cent[t as usize])];
                b.0 += 1;
                for p in tris[t as usize] {
                    grow(&mut b.1, &mut b.2, p);
                }
            }
            let (mut best, mut best_cost) = (0usize, f32::MAX);
            for split in 1..BINS {
                let side = |r: std::ops::Range<usize>| {
                    r.fold((0u32, Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(n, l, h), i| (n + bins[i].0, l.min(bins[i].1), h.max(bins[i].2)))
                };
                let (nl, ll, lh) = side(0..split);
                let (nr, rl, rh) = side(split..BINS);
                if nl == 0 || nr == 0 {
                    continue;
                }
                let cost = nl as f32 * area(ll, lh) + nr as f32 * area(rl, rh);
                if cost < best_cost {
                    best_cost = cost;
                    best = split;
                }
            }
            if best == 0 || best_cost >= c as f32 * area(lo, hi) {
                continue;
            }
            let slice = &mut order[f..f + c];
            let mut i = 0;
            for j in 0..slice.len() {
                if bin_of(cent[slice[j] as usize]) < best {
                    slice.swap(i, j);
                    i += 1;
                }
            }
            if i == 0 || i == c {
                continue;
            }
            let left = nodes.len();
            nodes.push(Node { lo: Vec3A::ZERO, hi: Vec3A::ZERO, first, count: i as u32 });
            nodes.push(Node { lo: Vec3A::ZERO, hi: Vec3A::ZERO, first: first + i as u32, count: (c - i) as u32 });
            nodes[ni].first = left as u32;
            nodes[ni].count = 0;
            stack.push(left);
            stack.push(left + 1);
        }
        let tris = order.iter().map(|t| tris[*t as usize]).collect();
        Bvh { nodes, tris, order }
    }

    pub fn is_empty(&self) -> bool {
        self.tris.is_empty()
    }

    /// The flattened tree (for GPU traversal), root first: bounds, then for a leaf the range
    /// `first..first + count` of [`Bvh::triangles`], for an inner node (`count` 0) the children
    /// `first` and `first + 1`.
    pub fn nodes(&self) -> impl ExactSizeIterator<Item = (Vec3, Vec3, u32, u32)> + '_ {
        self.nodes.iter().map(|n| (n.lo.into(), n.hi.into(), n.first, n.count))
    }

    /// Triangle vertices in BVH order, and the original triangle index of each.
    pub fn triangles(&self) -> (&[[Vec3; 3]], &[u32]) {
        (&self.tris, &self.order)
    }

    fn slab(lo: Vec3A, hi: Vec3A, o: Vec3A, inv: Vec3A, tmax: f32) -> Option<f32> {
        let t0 = (lo - o) * inv;
        let t1 = (hi - o) * inv;
        // A ray lying in a slab plane gives 0·∞ = NaN: it is inside that slab, so the axis does
        // not bound the hit.
        let open = t0.is_nan_mask() | t1.is_nan_mask();
        let tn = Vec3A::select(open, Vec3A::NEG_INFINITY, t0.min(t1)).max_element().max(0.0);
        let tf = Vec3A::select(open, Vec3A::INFINITY, t0.max(t1)).min_element().min(tmax);
        (tn <= tf).then_some(tn)
    }

    /// Möller–Trumbore, double sided.
    fn hit_tri(t: &[Vec3; 3], o: Vec3, d: Vec3) -> Option<(f32, f32, f32)> {
        let e1 = t[1] - t[0];
        let e2 = t[2] - t[0];
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let s = o - t[0];
        // A little slack on the edges: rays through a shared edge must not slip between its
        // two triangles to a surface behind.
        const EDGE: f32 = 1e-5;
        let u = s.dot(p) * inv;
        if !(-EDGE..=1.0 + EDGE).contains(&u) {
            return None;
        }
        let q = s.cross(e1);
        let v = d.dot(q) * inv;
        if v < -EDGE || u + v > 1.0 + EDGE {
            return None;
        }
        let tt = e2.dot(q) * inv;
        (tt > 0.0).then_some((tt, u, v))
    }

    fn traverse(&self, o: Vec3, d: Vec3, mut tmax: f32, any: bool, filter: &dyn Fn(u32) -> bool) -> Option<Hit> {
        let root = self.nodes.first().filter(|_| !self.tris.is_empty())?;
        let (oa, inv) = (Vec3A::from(o), Vec3A::ONE / Vec3A::from(d));
        let mut best = None;
        let mut stack = Stack::default();
        stack.push((0, Self::slab(root.lo, root.hi, oa, inv, tmax)?));
        // Entries carry their entry distance: nodes behind a closer hit are skipped unopened.
        while let Some((ni, tn)) = stack.pop() {
            let Some(n) = self.nodes.get(ni as usize).filter(|_| tn <= tmax) else { continue };
            if n.count > 0 {
                let (f, c) = (n.first as usize, n.count as usize);
                for i in f..(f + c).min(self.tris.len()) {
                    let (Some(tri), Some(&orig)) = (self.tris.get(i), self.order.get(i)) else { continue };
                    if let Some((t, u, v)) = Self::hit_tri(tri, o, d)
                        && t < tmax
                        && filter(orig)
                    {
                        tmax = t;
                        best = Some(Hit { t, tri: orig, u, v });
                        if any {
                            return best;
                        }
                    }
                }
            } else {
                // Visit the nearer child first.
                let (l, r) = (n.first, n.first + 1);
                let dl = self.nodes.get(l as usize).and_then(|c| Self::slab(c.lo, c.hi, oa, inv, tmax));
                let dr = self.nodes.get(r as usize).and_then(|c| Self::slab(c.lo, c.hi, oa, inv, tmax));
                match (dl, dr) {
                    (Some(a), Some(b)) if a <= b => {
                        stack.push((r, b));
                        stack.push((l, a));
                    }
                    (Some(a), Some(b)) => {
                        stack.push((l, a));
                        stack.push((r, b));
                    }
                    (Some(a), None) => stack.push((l, a)),
                    (None, Some(b)) => stack.push((r, b)),
                    (None, None) => {}
                }
            }
        }
        best
    }

    /// Closest hit along `o + t·d` for `t` in (0, tmax).
    pub fn closest(&self, o: Vec3, d: Vec3, tmax: f32) -> Option<Hit> {
        self.traverse(o, d, tmax, false, &|_| true)
    }

    /// Closest hit on triangles accepted by `filter` (original triangle index).
    pub fn closest_filtered(&self, o: Vec3, d: Vec3, tmax: f32, filter: &dyn Fn(u32) -> bool) -> Option<Hit> {
        self.traverse(o, d, tmax, false, filter)
    }

    /// Whether anything is hit before `tmax` (occlusion rays).
    pub fn occluded(&self, o: Vec3, d: Vec3, tmax: f32) -> bool {
        self.traverse(o, d, tmax, true, &|_| true).is_some()
    }

    /// Call `f` with every triangle (original index) whose bounds overlap the box.
    pub fn query_box(&self, lo: Vec3, hi: Vec3, mut f: impl FnMut(u32)) {
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let Some(n) = self.nodes.get(i as usize) else { continue };
            if self.tris.is_empty() || n.hi.cmplt(lo.into()).any() || n.lo.cmpgt(hi.into()).any() {
                continue;
            }
            if n.count > 0 {
                for k in n.first as usize..(n.first + n.count) as usize {
                    let (Some(t), Some(o)) = (self.tris.get(k), self.order.get(k)) else { continue };
                    let tlo = t[0].min(t[1]).min(t[2]);
                    let thi = t[0].max(t[1]).max(t[2]);
                    if !(thi.cmplt(lo).any() || tlo.cmpgt(hi).any()) {
                        f(*o);
                    }
                }
            } else {
                stack.push(n.first);
                stack.push(n.first + 1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives;

    #[test]
    fn traversal_stack_spills_in_lifo_order() {
        let mut st = Stack::default();
        for v in 0..200 {
            st.push((v, v as f32));
        }
        assert!((0..200).rev().all(|v| st.pop() == Some((v, v as f32))));
        assert_eq!(st.pop(), None);
    }

    #[test]
    fn ray_hits_sphere_at_expected_distance() {
        let m = primitives::sphere(1.0, 64, 32);
        let bvh = Bvh::new(&m);
        let h = bvh.closest(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, 100.0).unwrap();
        assert!((h.t - 4.0).abs() < 0.01, "{}", h.t);
        assert!(bvh.closest(Vec3::new(0.0, 3.0, 5.0), Vec3::NEG_Z, 100.0).is_none());
        // Rays lying exactly in a node's bounding plane (y = 0 is the equator) hit the front.
        // Axis-aligned rays lying in a node's bounding plane (y = 0) still find the front.
        for x in [-0.5f32, -0.4, 0.3] {
            let h = bvh.closest(Vec3::new(x, 0.0, 5.0), Vec3::NEG_Z, 100.0).unwrap();
            assert!(h.t < 5.0, "x {x}: hit the back at t {}", h.t);
        }
        // Rays through shared edges (the equator) hit the front, never slip through to the back.
        let m = primitives::sphere(1.0, 48, 24);
        let bvh = Bvh::new(&m);
        for i in 0..200 {
            let x = i as f32 / 100.0 - 1.0;
            let o = Vec3::new(0.0, 0.0, 5.0);
            let d = (Vec3::new(x * 0.9, 0.0, 0.0) - o).normalize();
            let h = bvh.closest(o, d, 100.0).unwrap();
            assert!(h.t < 5.0, "x {x}: hit the back at t {}", h.t);
        }
        assert!(bvh.occluded(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, 10.0));
        assert!(!bvh.occluded(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z, 3.0));
    }

    #[test]
    fn matches_brute_force() {
        let m = primitives::material_ball();
        let bvh = Bvh::new(&m);
        for i in 0..200 {
            let a = i as f32 * 0.731;
            let o = Vec3::new(a.cos() * 6.0, (i as f32 * 0.37).sin() * 3.0 + 1.0, a.sin() * 6.0);
            let d = (Vec3::new(0.0, 1.0, 0.0) + Vec3::new((a * 3.1).sin(), (a * 1.7).cos(), (a * 2.3).sin()) * 0.6 - o).normalize();
            let fast = bvh.closest(o, d, 1e9).map(|h| h.t);
            let mut slow: Option<f32> = None;
            for t in 0..m.triangle_count() {
                let [x, y, z] = m.tri(t).unwrap();
                if let Some((tt, _, _)) = Bvh::hit_tri(&[m.positions[x], m.positions[y], m.positions[z]], o, d) {
                    slow = Some(slow.map_or(tt, |s: f32| s.min(tt)));
                }
            }
            match (fast, slow) {
                (Some(a), Some(b)) => assert!((a - b).abs() < 1e-4, "{a} {b}"),
                (None, None) => {}
                other => panic!("mismatch {other:?}"),
            }
        }
    }

    #[test]
    fn box_query_finds_overlapping_triangles() {
        let m = primitives::plane(2.0, 4);
        let bvh = Bvh::new(&m);
        let mut found = Vec::new();
        bvh.query_box(Vec3::new(-0.1, -0.1, -0.1), Vec3::new(0.1, 0.1, 0.1), |t| found.push(t));
        assert!(!found.is_empty() && found.len() <= 8, "{found:?}");
        let empty = Bvh::new(&Mesh::default());
        assert!(empty.closest(Vec3::ZERO, Vec3::X, 1.0).is_none());
    }
}
