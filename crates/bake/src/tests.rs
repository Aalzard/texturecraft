use std::sync::atomic::Ordering::Relaxed;

use texturecraft_core::BakeMatch;
use texturecraft_geom::glam::{Vec2, Vec3, Vec4};
use texturecraft_geom::primitives;

use super::*;

fn settings(res: u32, maps: &[BakedMap]) -> BakeSettings {
    BakeSettings { resolution: res, dilation: 0, rays: 64, maps: maps.iter().copied().collect(), ..BakeSettings::default() }
}

fn run(mesh: &Mesh, s: &BakeSettings) -> Result<BakeOutput, BakeError> {
    let bvh = Bvh::new(mesh);
    bake(&BakeRequest { mesh, bvh: &bvh, material: 0, settings: s, high: None, cage: None }, &Progress::new())
}

/// Values of `map` (first component) at texels the UV layout covers, with their surface positions.
fn covered_values(mesh: &Mesh, out: &BakeOutput, map: BakedMap) -> Vec<(Vec3, f32)> {
    let res = out.resolution;
    let g = UvGBuffer::new(mesh, 0, res, res);
    let data = &out.maps[&map];
    (0..(res * res) as usize).filter_map(|i| g.surface(mesh, i).map(|s| (s.pos, data[i * map.components()]))).collect()
}

fn mean(v: impl Iterator<Item = f32>) -> f32 {
    let (s, n) = v.fold((0.0, 0), |(s, n), x| (s + x, n + 1));
    assert!(n > 0, "no samples");
    s / n as f32
}

fn quad() -> Mesh {
    Mesh::from_parts(
        vec![Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(-1.0, 0.0, 1.0)],
        Some(vec![Vec3::Y; 4]),
        vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)],
        vec![0, 2, 1, 0, 3, 2],
        "Quad",
    )
    .unwrap()
}

#[test]
fn open_quad_is_unoccluded() {
    let m = quad();
    let out = run(&m, &settings(32, &[BakedMap::AmbientOcclusion])).unwrap();
    let ao = covered_values(&m, &out, BakedMap::AmbientOcclusion);
    assert!(ao.iter().all(|(_, v)| *v > 0.999), "{:?}", ao.iter().map(|v| v.1).fold(1.0f32, f32::min));
}

#[test]
fn inside_of_a_closed_box_is_occluded() {
    let mut m = primitives::cube(2.0);
    for t in m.indices.chunks_exact_mut(3) {
        t.swap(1, 2);
    }
    for n in &mut m.normals {
        *n = -*n;
    }
    m.compute_tangents();
    let s = BakeSettings { max_distance: 4.0, ..settings(48, &[BakedMap::AmbientOcclusion]) };
    let out = run(&m, &s).unwrap();
    let ao = mean(covered_values(&m, &out, BakedMap::AmbientOcclusion).into_iter().map(|v| v.1));
    assert!(ao < 0.3, "{ao}");
}

#[test]
fn thin_slab_is_thinner_than_a_block() {
    let thick = |m: &Mesh| mean(covered_values(m, &run(m, &settings(48, &[BakedMap::Thickness])).unwrap(), BakedMap::Thickness).into_iter().map(|v| v.1));
    let slab = thick(&primitives::rounded_box(Vec3::new(1.0, 0.05, 1.0), 0.0));
    let block = thick(&primitives::cube(2.0));
    assert!(slab < 0.4 && block > 0.6, "slab {slab}, block {block}");
}

#[test]
fn sphere_curvature_is_nearly_constant() {
    let m = primitives::sphere(1.0, 64, 32);
    let out = run(&m, &settings(64, &[BakedMap::Curvature])).unwrap();
    let c: Vec<f32> = covered_values(&m, &out, BakedMap::Curvature).into_iter().map(|v| v.1).collect();
    let (lo, hi) = c.iter().fold((1.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
    assert!(lo >= 0.5 && hi < 0.6 && hi - lo < 0.03, "{lo}..{hi}");
}

/// Bevel / edge texels (two coordinates near the half extent) are convex-bright; face centres
/// stay mid-grey.
fn check_box_curvature(m: &Mesh, edge: f32) {
    let s = BakeSettings { dilation: 4, ..settings(128, &[BakedMap::Curvature]) };
    let c = covered_values(m, &run(m, &s).unwrap(), BakedMap::Curvature);
    let near = |p: Vec3, t: f32| p.abs().to_array().iter().filter(|c| **c > t).count();
    let edges: Vec<f32> = c.iter().filter(|(p, _)| near(*p, edge) >= 2).map(|v| v.1).collect();
    let faces: Vec<f32> = c.iter().filter(|(p, _)| near(*p, 0.5) == 1).map(|v| v.1).collect();
    assert!(!edges.is_empty() && !faces.is_empty());
    let min_edge = edges.iter().copied().fold(1.0, f32::min);
    assert!(min_edge > 0.55, "edge min {min_edge}");
    assert!(faces.iter().all(|v| (0.45..=0.55).contains(v)), "face {:?}", faces.iter().copied().fold((1.0f32, 0.0f32), |(a, b), v| (a.min(v), b.max(v))));
}

#[test]
fn rounded_box_bevels_are_convex() {
    check_box_curvature(&primitives::rounded_box(Vec3::ONE, 0.15), 0.9);
}

#[test]
fn cube_edges_are_convex() {
    check_box_curvature(&primitives::cube(2.0), 0.97);
}

#[test]
fn two_runs_are_identical_and_sized() {
    let m = primitives::material_ball();
    let s = BakeSettings { antialiasing: 2, dilation: 2, rays: 8, maps: BakedMap::ALL.into_iter().collect(), ..settings(40, &[]) };
    let a = run(&m, &s).unwrap();
    let b = run(&m, &s).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.maps.len(), BakedMap::ALL.len());
    for (map, data) in &a.maps {
        assert_eq!(data.len(), 40 * 40 * map.components(), "{map:?}");
        assert!(data.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{map:?}");
    }
}

#[test]
fn strips_match_a_single_pass() {
    let m = primitives::material_ball();
    let bvh = Bvh::new(&m);
    let s = BakeSettings { antialiasing: 2, rays: 4, maps: BakedMap::ALL.into_iter().collect(), ..settings(64, &[]) };
    let req = BakeRequest { mesh: &m, bvh: &bvh, material: 0, settings: &s, high: None, cage: None };
    let whole = bake(&req, &Progress::new()).unwrap();
    // 128 × 2 supersampled texels per strip = 1 output row: 64 strips.
    let strips = bake_in_strips(&req, &Progress::new(), 128 * 2).unwrap();
    for (map, a) in &whole.maps {
        let b = &strips.maps[map];
        let off = a.iter().zip(b).filter(|(a, b)| (*a - *b).abs() > 1e-3).count();
        assert!(off * 200 < a.len(), "{map:?}: {off} of {} differ", a.len());
    }
}

#[test]
fn cancel_and_progress() {
    let m = primitives::sphere(1.0, 16, 8);
    let bvh = Bvh::new(&m);
    let s = settings(16, &[BakedMap::AmbientOcclusion, BakedMap::Position]);
    let req = BakeRequest { mesh: &m, bvh: &bvh, material: 0, settings: &s, high: None, cage: None };
    let p = Progress::new();
    p.cancel();
    assert!(matches!(bake(&req, &p), Err(BakeError::Cancelled)));
    let p = Progress::new();
    bake(&req, &p).unwrap();
    assert_eq!(p.done.load(Relaxed), 10_000);
    assert_eq!(p.fraction(), 1.0);
}

#[test]
fn invalid_requests_are_errors() {
    let m = quad();
    let ok = settings(8, &[BakedMap::AmbientOcclusion]);
    let bad = [
        BakeSettings { resolution: 0, ..ok.clone() },
        BakeSettings { resolution: 8193, ..ok.clone() },
        BakeSettings { antialiasing: 3, ..ok.clone() },
        BakeSettings { antialiasing: 0, ..ok.clone() },
        BakeSettings { rays: 0, ..ok.clone() },
        BakeSettings { max_distance: f32::NAN, ..ok.clone() },
        BakeSettings { max_distance: 0.0, ..ok.clone() },
        BakeSettings { max_rear: -1.0, ..ok.clone() },
        BakeSettings { curvature_strength: f32::INFINITY, ..ok.clone() },
    ];
    for s in &bad {
        assert!(matches!(run(&m, s), Err(BakeError::Invalid(_))), "{s:?}");
    }
    let bvh = Bvh::new(&m);
    let req = |mesh: &Mesh, material| bake(&BakeRequest { mesh, bvh: &bvh, material, settings: &ok, high: None, cage: None }, &Progress::new());
    assert!(matches!(req(&m, 1), Err(BakeError::Invalid(_))));
    let mut broken = m.clone();
    broken.indices[0] = 99;
    assert!(matches!(req(&broken, 0), Err(BakeError::Invalid(_))));
    let mut nan = m.clone();
    nan.positions[0].x = f32::NAN;
    assert!(matches!(req(&nan, 0), Err(BakeError::Invalid(_))));
    // An empty mesh bakes to neutral values.
    let empty = Mesh { materials: vec!["m".into()], ..Mesh::default() };
    let out = req(&empty, 0).unwrap();
    assert!(out.maps[&BakedMap::AmbientOcclusion].iter().all(|v| *v == 1.0));
}

#[test]
fn neutral_values_and_padding() {
    // The quad covers only the left half of UV space.
    let mut m = quad();
    for uv in &mut m.uvs {
        uv.x *= 0.5;
    }
    let maps = [BakedMap::AmbientOcclusion, BakedMap::Curvature, BakedMap::Thickness, BakedMap::Normal, BakedMap::Position];
    let out = run(&m, &BakeSettings { dilation: 2, ..settings(16, &maps) }).unwrap();
    let pos = &out.maps[&BakedMap::Position];
    let at = |x: usize, y: usize| &pos[(y * 16 + x) * 3..(y * 16 + x) * 3 + 3];
    assert_eq!(at(9, 5)[0], at(7, 5)[0], "padded texel copies a texel of the last covered column");
    assert_eq!(at(12, 5), [0.0, 0.0, 0.0], "beyond the padding");
    assert_eq!(out.maps[&BakedMap::AmbientOcclusion][5 * 16 + 12], 1.0);
    assert_eq!(out.maps[&BakedMap::Curvature][5 * 16 + 12], 0.5);
    assert_eq!(out.maps[&BakedMap::Thickness][5 * 16 + 12], 1.0);
    assert_eq!(&out.maps[&BakedMap::Normal][(5 * 16 + 12) * 3..(5 * 16 + 12) * 3 + 3], [0.5, 0.5, 1.0]);
}

#[test]
fn normal_and_height_transfer_from_a_high_poly() {
    let low = quad();
    let mut high = quad();
    for p in &mut high.positions {
        p.y += 0.005;
    }
    let tilt = Vec3::new(0.3, 1.0, 0.0).normalize();
    high.normals = vec![tilt; 4];
    high.compute_tangents();
    let (lb, hb) = (Bvh::new(&low), Bvh::new(&high));
    let s = settings(16, &[BakedMap::Normal, BakedMap::Height]);
    let out =
        bake(&BakeRequest { mesh: &low, bvh: &lb, material: 0, settings: &s, high: Some(HighPoly { mesh: &high, bvh: &hb }), cage: None }, &Progress::new())
            .unwrap();
    let i = 8 * 16 + 8;
    let n = &out.maps[&BakedMap::Normal][i * 3..i * 3 + 3];
    // Tangent = +X (u), bitangent = −Z (up the image), normal = +Y.
    assert!((n[0] - (0.5 + 0.5 * tilt.x)).abs() < 1e-3 && (n[1] - 0.5).abs() < 1e-3 && (n[2] - (0.5 + 0.5 * tilt.y)).abs() < 1e-3, "{n:?}");
    let diag = 8.0f32.sqrt();
    let expected = 0.5 + 0.005 / (0.02 * diag);
    assert!((out.maps[&BakedMap::Height][i] - expected).abs() < 1e-3, "{}", out.maps[&BakedMap::Height][i]);
    // Without a high poly both maps are flat.
    let flat = run(&low, &s).unwrap();
    assert_eq!(&flat.maps[&BakedMap::Normal][i * 3..i * 3 + 3], [0.5, 0.5, 1.0]);
    assert_eq!(flat.maps[&BakedMap::Height][i], 0.5);
}

fn transfer(low: &Mesh, high: &Mesh, cage: Option<&Mesh>, s: &BakeSettings) -> Result<BakeOutput, BakeError> {
    let (lb, hb) = (Bvh::new(low), Bvh::new(high));
    bake(&BakeRequest { mesh: low, bvh: &lb, material: 0, settings: s, high: Some(HighPoly { mesh: high, bvh: &hb }), cage }, &Progress::new())
}

/// Texel (x, y) of a `res`² bake of [`quad`] sits over world (2u − 1, 0, 2v − 1).
fn quad_texel(res: u32, x: f32, z: f32) -> usize {
    let t = |c: f32| (((c + 1.0) * 0.5 * res as f32) as usize).min(res as usize - 1);
    t(z) * res as usize + t(x)
}

/// [`quad`]'s footprint with a smooth bump `h · exp(−r²/σ²)`.
fn bumped(h: f32, sigma: f32) -> Mesh {
    let mut m = primitives::plane(2.0, 128);
    for p in &mut m.positions {
        p.y = h * (-(p.x * p.x + p.z * p.z) / (sigma * sigma)).exp();
    }
    m.compute_normals();
    m.compute_tangents();
    m
}

#[test]
fn bump_tilts_the_baked_normal() {
    let (h, sigma) = (0.1f32, 0.4f32);
    let s = BakeSettings { max_frontal: 0.06, max_rear: 0.02, ..settings(64, &[BakedMap::Normal, BakedMap::Height]) };
    let out = transfer(&quad(), &bumped(h, sigma), None, &s).unwrap();
    let normal = &out.maps[&BakedMap::Normal];
    for (x, z) in [(0.3f32, 0.0f32), (-0.3, 0.0), (0.0, 0.3), (0.0, -0.3), (0.25, 0.25)] {
        let i = quad_texel(64, x, z);
        // Analytic normal of the bump at the texel centre.
        let (cx, cz) = (((i % 64) as f32 + 0.5) / 32.0 - 1.0, ((i / 64) as f32 + 0.5) / 32.0 - 1.0);
        let f = h * (-(cx * cx + cz * cz) / (sigma * sigma)).exp();
        let (dx, dz) = (-2.0 * cx / (sigma * sigma) * f, -2.0 * cz / (sigma * sigma) * f);
        let n = Vec3::new(-dx, 1.0, -dz).normalize();
        // Tangent = +X, bitangent = −Z, normal = +Y.
        let expected = [0.5 + 0.5 * n.x, 0.5 - 0.5 * n.z, 0.5 + 0.5 * n.y];
        let got = &normal[i * 3..i * 3 + 3];
        assert!(got.iter().zip(expected).all(|(g, e)| (g - e).abs() < 0.02), "at {x},{z}: {got:?} vs {expected:?}");
        assert!((got[0] - 0.5) * x >= 0.0 && (got[1] - 0.5) * -z >= 0.0, "tilts away from the bump centre at {x},{z}: {got:?}");
    }
    let height = &out.maps[&BakedMap::Height];
    assert!(height[quad_texel(64, 0.0, 0.0)] > height[quad_texel(64, 0.6, 0.6)] + 0.2, "the bump top is higher");
}

#[test]
fn curvature_from_the_normal_map_sees_the_bump() {
    let s = BakeSettings { max_frontal: 0.06, max_rear: 0.02, curvature_from_normal: true, ..settings(64, &[BakedMap::Curvature]) };
    let out = transfer(&quad(), &bumped(0.1, 0.4), None, &s).unwrap();
    assert!(!out.maps.contains_key(&BakedMap::Normal), "the helper normal map is not returned");
    let c = &out.maps[&BakedMap::Curvature];
    assert!(c[quad_texel(64, 0.0, 0.0)] > 0.55, "convex top {}", c[quad_texel(64, 0.0, 0.0)]);
    assert!((c[quad_texel(64, 0.9, 0.9)] - 0.5).abs() < 0.02, "flat corner {}", c[quad_texel(64, 0.9, 0.9)]);
}

/// Two unit boxes 0.2 apart: "A" (material 0, centre x = −0.6) and "B" (material 1, x = +0.6).
fn two_boxes(suffix: &str, colors: Option<[Vec4; 2]>) -> Mesh {
    let part = |name: &str, x: f32, color: Option<Vec4>| {
        let mut b = primitives::cube(1.0);
        b.positions.iter_mut().for_each(|p| p.x += x);
        b.submeshes[0].name = format!("{name}{suffix}");
        b.colors = color.map(|c| vec![c; b.positions.len()]).unwrap_or_default();
        b
    };
    let mut m = part("A", -0.6, colors.map(|c| c[0]));
    m.append(&part("B", 0.6, colors.map(|c| c[1])));
    m
}

#[test]
fn match_by_mesh_name_pairs_the_right_parts() {
    let (red, blue) = (Vec4::new(1.0, 0.0, 0.0, 1.0), Vec4::new(0.0, 0.0, 1.0, 1.0));
    let (low, high) = (two_boxes("_low", None), two_boxes("_HIGH", Some([red, blue])));
    // Rays start 1.3 in front of A's +X face: beyond B, so B's high poly is hit first unless
    // parts are matched by name.
    let s = BakeSettings { max_frontal: 0.5, ..settings(64, &[BakedMap::ColorMap]) };
    let face = |s: &BakeSettings| {
        let out = transfer(&low, &high, None, s).unwrap();
        covered_values(&low, &out, BakedMap::ColorMap)
            .into_iter()
            .filter(|(p, _)| p.x > -0.105 && p.y.abs() < 0.4 && p.z.abs() < 0.4)
            .map(|v| v.1)
            .collect::<Vec<f32>>()
    };
    let always = face(&s);
    assert!(!always.is_empty() && always.iter().all(|r| *r < 0.01), "always: B's (blue) high poly occludes A's face");
    let by_name = face(&BakeSettings { matching: BakeMatch::ByMeshName, ..s.clone() });
    assert!(by_name.iter().all(|r| *r > 0.99), "by name: A's face gets A's red");
    // Parts without a counterpart get nothing.
    let lonely = BakeSettings { matching: BakeMatch::ByMeshName, low_suffix: "_none".into(), ..s };
    assert!(face(&lonely).iter().all(|r| *r == 0.0));
}

#[test]
fn bent_normals_point_away_from_occluders() {
    // A floor (material 0) with a wall standing at x = +1, facing −X (material 1).
    let mut m = quad();
    let wall = Mesh::from_parts(
        vec![Vec3::new(1.0, 0.0, -1.0), Vec3::new(1.0, 0.0, 1.0), Vec3::new(1.0, 2.0, 1.0), Vec3::new(1.0, 2.0, -1.0)],
        Some(vec![-Vec3::X; 4]),
        vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
        vec![0, 2, 1, 0, 3, 2],
        "Wall",
    )
    .unwrap();
    m.append(&wall);
    let s = BakeSettings { max_distance: 2.0, ..settings(32, &[BakedMap::BentNormal, BakedMap::AmbientOcclusion]) };
    let out = run(&m, &s).unwrap();
    let bent = &out.maps[&BakedMap::BentNormal];
    let ao = covered_values(&m, &out, BakedMap::AmbientOcclusion);
    let g = UvGBuffer::new(&m, 0, 32, 32);
    let near: Vec<&[f32]> = (0..32 * 32).filter(|i| g.surface(&m, *i).is_some_and(|s| s.pos.x > 0.6)).map(|i| &bent[i * 3..i * 3 + 3]).collect();
    let far: Vec<&[f32]> = (0..32 * 32).filter(|i| g.surface(&m, *i).is_some_and(|s| s.pos.x < -0.6)).map(|i| &bent[i * 3..i * 3 + 3]).collect();
    let mean_x = |v: &[&[f32]]| mean(v.iter().map(|b| b[0] * 2.0 - 1.0));
    assert!(mean_x(&near) < -0.15, "near the wall the bent normal leans away from it: {}", mean_x(&near));
    assert!(mean_x(&near) < mean_x(&far), "{} vs {}", mean_x(&near), mean_x(&far));
    assert!(near.iter().all(|b| b[1] > 0.6), "still points up");
    let ao_near = mean(ao.iter().filter(|(p, _)| p.x > 0.6).map(|v| v.1));
    assert!(ao_near < mean(ao.iter().filter(|(p, _)| p.x < -0.6).map(|v| v.1)));
}

#[test]
fn opacity_and_color_transfer_where_the_high_poly_is() {
    // The high poly covers x < 0 only, colored green.
    let mut high = quad();
    high.positions.iter_mut().for_each(|p| p.x = p.x.min(0.0));
    high.colors = vec![Vec4::new(0.0, 1.0, 0.0, 0.5); 4];
    let out = transfer(&quad(), &high, None, &settings(32, &[BakedMap::Opacity, BakedMap::ColorMap])).unwrap();
    let (op, col) = (&out.maps[&BakedMap::Opacity], &out.maps[&BakedMap::ColorMap]);
    let (left, right) = (quad_texel(32, -0.5, 0.0), quad_texel(32, 0.5, 0.0));
    assert_eq!((op[left], op[right]), (0.5, 0.0), "vertex alpha where covered, 0 where missed");
    assert_eq!(&col[left * 3..left * 3 + 3], [0.0, 1.0, 0.0]);
    assert_eq!(&col[right * 3..right * 3 + 3], BakedMap::ColorMap.neutral());
    // Without a high poly the low poly covers itself (white, opaque).
    let own = run(&quad(), &settings(32, &[BakedMap::Opacity, BakedMap::ColorMap])).unwrap();
    assert_eq!((own.maps[&BakedMap::Opacity][right], own.maps[&BakedMap::ColorMap][right * 3]), (1.0, 1.0));
}

#[test]
fn ray_distances_and_the_cage_limit_hits() {
    let low = quad();
    let mut high = quad();
    high.positions.iter_mut().for_each(|p| p.y += 0.2);
    let diag = 8.0f32.sqrt();
    let opacity = |s: &BakeSettings, cage: Option<&Mesh>| transfer(&low, &high, cage, s).map(|o| o.maps[&BakedMap::Opacity][quad_texel(16, 0.0, 0.0)]);
    let near = settings(16, &[BakedMap::Opacity]);
    assert_eq!(opacity(&near, None).unwrap(), 0.0, "0.2 above is beyond the default frontal distance");
    assert_eq!(opacity(&BakeSettings { max_frontal: 0.25 / diag, ..near.clone() }, None).unwrap(), 1.0);
    // The rear distance only reaches below the surface.
    let mut below = high.clone();
    below.positions.iter_mut().for_each(|p| p.y -= 0.4);
    let rear = |r: f32| transfer(&low, &below, None, &BakeSettings { max_rear: r, ..near.clone() }).unwrap().maps[&BakedMap::Opacity][quad_texel(16, 0.0, 0.0)];
    assert_eq!((rear(0.1 / diag), rear(0.25 / diag)), (0.0, 1.0));
    // A cage 0.3 above the low poly starts the rays above the high poly.
    let mut cage = low.clone();
    cage.positions.iter_mut().for_each(|p| p.y += 0.3);
    assert_eq!(opacity(&near, Some(&cage)).unwrap(), 1.0);
    // A cage 0.1 above starts below it.
    cage.positions.iter_mut().for_each(|p| p.y -= 0.2);
    assert_eq!(opacity(&near, Some(&cage)).unwrap(), 0.0);
    // The cage must have the low poly's triangles.
    assert!(matches!(opacity(&near, Some(&primitives::cube(1.0))), Err(BakeError::Invalid(_))));
}

#[test]
fn world_space_direction_is_a_facing_ratio() {
    let m = quad();
    let at =
        |d: [f32; 3]| run(&m, &BakeSettings { direction: d, ..settings(8, &[BakedMap::WorldDirection]) }).map(|o| o.maps[&BakedMap::WorldDirection][4 * 8 + 4]);
    assert_eq!(at([0.0, 2.0, 0.0]).unwrap(), 1.0);
    assert_eq!(at([1.0, 0.0, 0.0]).unwrap(), 0.5);
    assert_eq!(at([0.0, -1.0, 0.0]).unwrap(), 0.0);
    assert!(matches!(at([0.0; 3]), Err(BakeError::Invalid(_))));
}

/// `cargo test -p texturecraft-bake --release -- --ignored --nocapture bench`
#[test]
#[ignore]
fn bench_material_ball_2k() {
    let m = primitives::material_ball();
    let bvh = Bvh::new(&m);
    let all = BakeSettings { resolution: 2048, ..BakeSettings::default() };
    for maps in
        [vec![BakedMap::Position], vec![BakedMap::AmbientOcclusion], vec![BakedMap::Thickness], vec![BakedMap::Curvature], all.maps.iter().copied().collect()]
    {
        let s = BakeSettings { maps: maps.into_iter().collect(), ..all.clone() };
        let t = std::time::Instant::now();
        let out = bake(&BakeRequest { mesh: &m, bvh: &bvh, material: 0, settings: &s, high: None, cage: None }, &Progress::new()).unwrap();
        println!("material ball, material 0, {}² × {:?}, {} rays: {:.2?}", out.resolution, out.maps.keys(), s.rays, t.elapsed());
    }
}
