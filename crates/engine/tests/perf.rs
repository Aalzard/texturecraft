//! Compositing benchmarks at 4K and 8K (run with `--ignored --nocapture` in release).

use std::time::{Duration, Instant};

use rayon::prelude::*;
use serde_json::{Value, json};
use texturecraft_engine::compose::{DisplayKind, tile_bytes};
use texturecraft_engine::geom::glam::Vec2;
use texturecraft_engine::geom::primitives;
use texturecraft_engine::{Session, execute};

/// A ~10 layer stack: procedural fills, generators, a blurred mask, a folder and painted layers.
fn stack(res: u32) -> Session {
    let mut s = Session::new_project("perf", primitives::sphere(1.0, 64, 32), "builtin:sphere", None, res).unwrap();
    s.viewport = Vec2::new(800.0, 800.0);
    let run = |s: &mut Session, id: &str, p: Value| execute(s, id, &p).unwrap();
    run(&mut s, "layer.add.fill", json!({"color": [0.6, 0.5, 0.4], "roughness": 0.6}));
    run(&mut s, "layer.fill.set", json!({"channel": "baseColor", "procedural": "clouds", "low": [0.2, 0.2, 0.2], "high": [0.8, 0.7, 0.6]}));
    run(&mut s, "layer.fill.set", json!({"channel": "height", "procedural": "cells", "low": [-0.2, -0.2, -0.2], "high": [0.2, 0.2, 0.2]}));
    run(&mut s, "layer.add.fill", json!({"color": [0.1, 0.3, 0.6]}));
    run(&mut s, "layer.fill.set", json!({"channel": "roughness", "procedural": "grunge"}));
    run(&mut s, "mask.add.black", Value::Null);
    run(&mut s, "effect.add.generator", json!({"generator": "position", "level": 0.5}));
    run(&mut s, "effect.add.filter", json!({"filter": "blur", "radius": 8}));
    run(&mut s, "layer.add.fill", json!({"roughness": 0.3}));
    run(&mut s, "layer.fill.set", json!({"channel": "baseColor", "procedural": "wood"}));
    run(&mut s, "layer.fill.projection", json!({"projection": "triplanar", "scale": 2}));
    run(&mut s, "mask.add.black", Value::Null);
    run(&mut s, "effect.add.fill", json!({"procedural": "scratches", "scale": 6}));
    run(&mut s, "layer.add.fill", json!({"color": [0.2, 0.2, 0.2], "metallic": 1.0}));
    run(&mut s, "mask.add.black", Value::Null);
    run(&mut s, "effect.add.generator", json!({"generator": "dirt"}));
    run(&mut s, "layer.add.fill", json!({"roughness": 0.8}));
    run(&mut s, "layer.fill.set", json!({"channel": "roughness", "procedural": "marble"}));
    run(&mut s, "layer.add.group", Value::Null);
    run(&mut s, "layer.add.fill", json!({"color": [0.9, 0.9, 0.2]}));
    run(&mut s, "mask.add.black", Value::Null);
    run(&mut s, "effect.add.generator", json!({"generator": "direction"}));
    run(&mut s, "effect.add.levels", json!({"inLow": 0.3, "inHigh": 0.7}));
    let top = s.doc.project.texture_sets[0].stack.last().copied().unwrap();
    run(&mut s, "layer.select", json!({"layer": top.0}));
    run(&mut s, "layer.add.paint", Value::Null);
    run(&mut s, "tool.brush", json!({"size": 6, "hardness": 80}));
    run(&mut s, "tool.stroke", json!({"points": [[200, 300], [600, 500]]}));
    run(&mut s, "layer.add.paint", Value::Null);
    s.update();
    s
}

fn upload_bytes(s: &mut Session) -> usize {
    let tiles = s.take_uploads(0);
    let baked = s.baked(0).cloned().unwrap_or_default();
    let comp = &s.caches[0].composite;
    // As the UI does it (in parallel).
    tiles.par_iter().flat_map_iter(|&(tx, ty)| DisplayKind::ALL.map(|k| tile_bytes(comp, &baked, k, tx, ty).len())).sum()
}

fn mb(b: usize) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

fn bench(res: u32) {
    let t = Instant::now();
    let mut s = stack(res);
    println!("[{res}²] setup {:.2?} ({} layers, {} covered tiles)", t.elapsed(), s.doc.project.walk(0).len(), s.caches[0].covered_tiles.len());
    s.caches[0].invalidate_all();
    let t = Instant::now();
    s.update();
    println!("[{res}²] full recomposite (Session::update) {:.2?}", t.elapsed());
    let t = Instant::now();
    let up = upload_bytes(&mut s);
    println!("[{res}²] full display tile conversion {:.2?} ({:.0} MB)", t.elapsed(), mb(up));

    // The UI's budgeted update: a full recomposite spread over frames.
    s.caches[0].invalidate_all();
    let clock = Instant::now();
    let ms = || clock.elapsed().as_secs_f64() * 1000.0;
    let (mut frames, mut worst) = (0, Duration::ZERO);
    while s.pending_tiles() > 0 {
        let t = Instant::now();
        s.update_for(30.0, ms);
        worst = worst.max(t.elapsed());
        frames += 1;
    }
    println!("[{res}²] budgeted full recomposite: {frames} frames, worst frame {worst:.2?}, total {:.2?}", clock.elapsed());

    // Strokes on the top paint layer, twice over the same path: per-frame dirty-tile composite
    // + display conversion, then the commit. Brush sizes are relative to the mesh: the largest
    // one covers a third of the texture per stamp.
    for (size, brush) in [(0.5, "largest brush"), (0.02, "default brush")] {
        s.tool.brush.size = size;
        for pass in ["1st stroke", "2nd stroke"] {
            let (mut comp, mut conv, mut worst, mut tiles) = (Duration::ZERO, Duration::ZERO, Duration::ZERO, 0usize);
            s.begin_stroke(Vec2::new(250.0, 400.0), s.viewport, 1.0).unwrap();
            let frames = 40;
            for i in 0..frames {
                let p = Vec2::new(250.0 + i as f32 * 8.0, 400.0 + (i as f32 * 0.3).sin() * 40.0);
                s.continue_stroke(p, 1.0);
                tiles += s.caches[0].dirty.len();
                let t = Instant::now();
                s.update();
                let c = t.elapsed();
                let t = Instant::now();
                upload_bytes(&mut s);
                let u = t.elapsed();
                comp += c;
                conv += u;
                worst = worst.max(c + u);
            }
            s.end_stroke().unwrap();
            let t = Instant::now();
            s.update();
            let commit = t.elapsed();
            let f = frames as u32;
            println!(
                "[{res}²] {brush}, {pass}: frame composite {:.2?} + display {:.2?} avg, worst {worst:.2?} ({:.0} dirty tiles/frame); commit {commit:.2?}",
                comp / f,
                conv / f,
                tiles as f64 / frames as f64
            );
        }
    }
    let m = s.memory();
    println!("[{res}²] memory: layer pixels {:.1} MB, composites {:.1} MB, below-stack cache {:.1} MB", mb(m.pixels), mb(m.composites), mb(m.cache));
}

#[test]
#[ignore]
fn bench_4k() {
    bench(4096);
}

#[test]
#[ignore]
fn bench_8k() {
    bench(8192);
}
