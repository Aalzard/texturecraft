// Shared ray casting of the GPU path tracer (trace.wgsl) and the GPU baker (bake.wgsl), each
// concatenated after this file: the BVH buffers (gpu_trace.rs `bvh_rows` layout), Möller–Trumbore
// with the CPU's edge slack, and the stack traversal of `texturecraft_geom::Bvh`. The including
// shader defines `fn accept(i: u32, d: vec3<f32>) -> bool`, which filters the triangles a ray
// may hit (BVH order index, ray direction).

const NONE: u32 = 0xffffffffu;
const STACK: u32 = 64u;
const EDGE: f32 = 1e-5;

// Per node: (lo, first child or triangle), (hi, triangle count; 0 for inner nodes); floats as bits.
@group(0) @binding(1) var<storage, read> nodes: array<vec4<u32>>;
// Per triangle in BVH order: (p0, w), (p1, w), (p2, w), (u32 x4); floats as bits, the w words and
// last row are the including shader's.
@group(0) @binding(2) var<storage, read> tris: array<vec4<u32>>;

fn max3(c: vec3<f32>) -> f32 {
    return max(c.x, max(c.y, c.z));
}

// Duff et al. 2017 orthonormal basis (glam's `any_orthonormal_pair`).
fn onb_t(n: vec3<f32>) -> vec3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    return vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
}

fn onb_b(n: vec3<f32>) -> vec3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    return vec3<f32>(b, s + n.y * n.y * a, -n.y);
}

struct Hit {
    t: f32,
    idx: u32,
    u: f32,
    v: f32,
};

fn slab(n: u32, o: vec3<f32>, inv: vec3<f32>, tmax: f32) -> f32 {
    let lo = bitcast<vec3<f32>>(nodes[n * 2u].xyz);
    let hi = bitcast<vec3<f32>>(nodes[n * 2u + 1u].xyz);
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let tn = max(max3(min(t0, t1)), 0.0);
    let tf = min(min(min(max(t0.x, t1.x), max(t0.y, t1.y)), max(t0.z, t1.z)), tmax);
    return select(-1.0, tn, tn <= tf);
}

// Vertex `k` of triangle `i` (BVH order).
fn tri_p(i: u32, k: u32) -> vec3<f32> {
    return bitcast<vec3<f32>>(tris[i * 4u + k].xyz);
}

// Möller–Trumbore, double sided, with the CPU's edge slack: (t, u, v), t < 0 for a miss.
fn hit_tri(i: u32, o: vec3<f32>, d: vec3<f32>) -> vec3<f32> {
    let miss = vec3<f32>(-1.0);
    let p0 = tri_p(i, 0u);
    let e1 = tri_p(i, 1u) - p0;
    let e2 = tri_p(i, 2u) - p0;
    let p = cross(d, e2);
    let det = dot(e1, p);
    if (abs(det) < 1e-12) {
        return miss;
    }
    let inv = 1.0 / det;
    let s = o - p0;
    let u = dot(s, p) * inv;
    if (u < -EDGE || u > 1.0 + EDGE) {
        return miss;
    }
    let q = cross(s, e1);
    let v = dot(d, q) * inv;
    if (v < -EDGE || u + v > 1.0 + EDGE) {
        return miss;
    }
    let t = dot(e2, q) * inv;
    return select(miss, vec3<f32>(t, u, v), t > 0.0);
}

// Closest accepted hit before `tmax` (or any, for shadow rays) in the BVH rooted at node `root`;
// idx NONE when nothing is hit.
fn traverse_from(root: u32, o: vec3<f32>, d: vec3<f32>, tmax: f32, any_hit: bool) -> Hit {
    var best = Hit(tmax, NONE, 0.0, 0.0);
    // Zero direction components would give 0·∞ in the slab test.
    let sd = select(d, select(vec3<f32>(-1e-20), vec3<f32>(1e-20), d >= vec3<f32>(0.0)), abs(d) < vec3<f32>(1e-20));
    let inv = 1.0 / sd;
    var stack_n: array<u32, STACK>;
    var stack_t: array<f32, STACK>;
    let root_t = slab(root, o, inv, best.t);
    if (root_t < 0.0) {
        return best;
    }
    stack_n[0] = root;
    stack_t[0] = root_t;
    var sp = 1u;
    loop {
        if (sp == 0u) {
            break;
        }
        sp -= 1u;
        let ni = stack_n[sp];
        if (stack_t[sp] > best.t) {
            continue;
        }
        let first = nodes[ni * 2u].w;
        let count = nodes[ni * 2u + 1u].w;
        if (count > 0u) {
            for (var i = first; i < first + count; i++) {
                let h = hit_tri(i, o, d);
                if (h.x > 0.0 && h.x < best.t && accept(i, d)) {
                    best = Hit(h.x, i, h.y, h.z);
                    if (any_hit) {
                        return best;
                    }
                }
            }
        } else {
            // Visit the nearer child first.
            let dl = slab(first, o, inv, best.t);
            let dr = slab(first + 1u, o, inv, best.t);
            if (dl >= 0.0 && dr >= 0.0 && sp + 2u <= STACK) {
                let near_first = dl <= dr;
                stack_n[sp] = select(first, first + 1u, near_first);
                stack_t[sp] = select(dl, dr, near_first);
                stack_n[sp + 1u] = select(first + 1u, first, near_first);
                stack_t[sp + 1u] = select(dr, dl, near_first);
                sp += 2u;
            } else if (dl >= 0.0 && sp < STACK) {
                stack_n[sp] = first;
                stack_t[sp] = dl;
                sp += 1u;
            } else if (dr >= 0.0 && sp < STACK) {
                stack_n[sp] = first + 1u;
                stack_t[sp] = dr;
                sp += 1u;
            }
        }
    }
    return best;
}
