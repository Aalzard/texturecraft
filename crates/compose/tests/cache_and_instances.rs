//! The below-stack cache and blocked compositing give exactly the unoptimized result; instance
//! layers draw their source; masks combine with height.

use texturecraft_compose::{BelowCache, Composite, Inputs, PixelStore, all_tiles, composite_tiles, composite_tiles_cached};
use texturecraft_core::{
    BlendMode, Channel, Effect, EffectKind, Fill, Filter, Generator, GeneratorKind, HeightMask, Layer, LayerId, LayerKind, Mask, MaskBackground, Procedural,
    ProceduralKind, Project, Projection, Source, TextureSet,
};
use texturecraft_geom::{Mesh, UvGBuffer, primitives};
use texturecraft_texel::{TILE, to_u16};

const RES: u32 = 256;

struct Fixture {
    project: Project,
    mesh: Mesh,
    gbuf: UvGBuffer,
    pixels: PixelStore,
}

fn fixture() -> Fixture {
    let mesh = primitives::sphere(1.0, 32, 16);
    let gbuf = UvGBuffer::new(&mesh, 0, RES, RES);
    let mut project = Project::new("t", "builtin:sphere");
    project.texture_sets.push(TextureSet::new("A", 0, RES));
    project.texture_sets.push(TextureSet::new("B", 0, RES));
    Fixture { project, mesh, gbuf, pixels: PixelStore::default() }
}

fn procedural(kind: ProceduralKind, low: f32, high: f32) -> Source {
    Source::Procedural { pattern: Procedural::new(kind), low: [low; 3], high: [high; 3] }
}

fn add(f: &mut Fixture, ts: usize, kind: LayerKind, mask: Option<Mask>) -> LayerId {
    let id = f.project.alloc_id();
    let mut l = Layer::new(id, "l", kind, &Channel::DEFAULT_SET);
    l.mask = mask;
    f.project.insert_layer(ts, l, None, false).unwrap()
}

fn fill(sources: &[(Channel, Source)]) -> LayerKind {
    let mut f = Fill::default();
    f.sources = sources.iter().cloned().collect();
    LayerKind::Fill(f)
}

fn effect(f: &mut Fixture, kind: EffectKind) -> Effect {
    Effect::new(f.project.alloc_id(), kind)
}

/// Procedural fills, a generator mask with a blur, a folder, painted pixels; returns the top
/// paint layer.
fn heavy_stack(f: &mut Fixture) -> LayerId {
    add(
        f,
        0,
        fill(&[(Channel::BaseColor, procedural(ProceduralKind::Clouds, 0.1, 0.9)), (Channel::Height, procedural(ProceduralKind::Cells, -0.3, 0.3))]),
        None,
    );
    let mut mask = Mask::new(MaskBackground::Black);
    mask.effects.push(effect(f, EffectKind::Generator(Generator::new(GeneratorKind::Position))));
    mask.effects.push(effect(f, EffectKind::Filter(Filter::Blur { radius: 40.0 })));
    add(f, 0, fill(&[(Channel::Roughness, procedural(ProceduralKind::Grunge, 0.0, 1.0))]), Some(mask));
    let group = add(f, 0, LayerKind::Group { children: vec![], collapsed: false }, None);
    for b in f.project.layer_mut(group).unwrap().channels.values_mut() {
        b.mode = BlendMode::Passthrough;
    }
    let inner = f.project.alloc_id();
    let l = Layer::new(inner, "inner", fill(&[(Channel::Metallic, procedural(ProceduralKind::Scratches, 0.0, 1.0))]), &Channel::DEFAULT_SET);
    f.project.insert_layer(0, l, Some(group), true).unwrap();
    let mut blurred = Mask::new(MaskBackground::Black);
    blurred.effects.push(effect(f, EffectKind::Fill { source: procedural(ProceduralKind::Checker, 0.0, 1.0), projection: Projection::default() }));
    blurred.effects.push(effect(f, EffectKind::Filter(Filter::Blur { radius: 24.0 })));
    let top = add(f, 0, LayerKind::Paint, Some(blurred));
    let img = f.pixels.layer_mut(top, Channel::BaseColor, RES);
    for y in 40..200 {
        for x in 30..220 {
            img.set(x, y, &[to_u16(0.4), to_u16(0.1), 0, to_u16(0.5)]);
        }
    }
    top
}

fn inputs<'a>(f: &'a Fixture, ts: usize, images: &'a texturecraft_compose::ImageStore, baked: &'a texturecraft_compose::BakedMaps) -> Inputs<'a> {
    Inputs { project: &f.project, ts, mesh: &f.mesh, gbuf: &f.gbuf, pixels: &f.pixels, baked, preview: None, images, bounds: f.mesh.bounds() }
}

fn run(f: &Fixture, ts: usize, tiles: &[(u32, u32)], below: Option<&mut BelowCache>) -> Composite {
    let (images, baked) = (Default::default(), Default::default());
    let mut out = Composite::new(&f.project.texture_sets[ts].channel_list(), RES);
    composite_tiles_cached(&inputs(f, ts, &images, &baked), &mut out, tiles, below);
    out
}

fn reference(f: &Fixture, ts: usize) -> Composite {
    let (images, baked) = (Default::default(), Default::default());
    let mut out = Composite::new(&f.project.texture_sets[ts].channel_list(), RES);
    composite_tiles(&inputs(f, ts, &images, &baked), &mut out, &all_tiles(RES));
    out
}

#[test]
fn below_cache_and_blocks_match_the_full_composite_exactly() {
    let mut f = fixture();
    let top = heavy_stack(&mut f);
    let want = reference(&f, 0);
    // Tile by tile (each its own block and neighbourhood region) gives the same bits.
    let mut one_by_one = Composite::new(&f.project.texture_sets[0].channel_list(), RES);
    let (images, baked) = (Default::default(), Default::default());
    for t in all_tiles(RES) {
        composite_tiles(&inputs(&f, 0, &images, &baked), &mut one_by_one, &[t]);
    }
    assert!(one_by_one == want, "per-tile composites differ from one pass");

    let mut cache = BelowCache::default();
    cache.set_hot(Some(top));
    let half: Vec<(u32, u32)> = all_tiles(RES).into_iter().filter(|t| (t.0 + t.1) % 2 == 0).collect();
    // Cold, half warm and fully warm caches.
    let mut out = run(&f, 0, &half, Some(&mut cache));
    assert!(!cache.is_empty(), "the stack below the hot layer is cached");
    composite_tiles_cached(&inputs(&f, 0, &images, &baked), &mut out, &all_tiles(RES), Some(&mut cache));
    assert!(out == want, "a partly cached composite differs");
    let warm = run(&f, 0, &all_tiles(RES), Some(&mut cache));
    assert!(warm == want, "a cached composite differs");

    // Painting the hot layer keeps the cache; editing below it empties it.
    f.pixels.layer_mut(top, Channel::BaseColor, RES).set(5, 5, &[to_u16(1.0), 0, 0, to_u16(1.0)]);
    let n = cache.len();
    assert!(run(&f, 0, &all_tiles(RES), Some(&mut cache)) == reference(&f, 0));
    assert_eq!(cache.len(), n);
    let low = f.project.texture_sets[0].stack[0];
    if let LayerKind::Fill(fl) = &mut f.project.layer_mut(low).unwrap().kind {
        fl.sources.insert(Channel::Roughness, Source::Uniform([0.9; 3]));
    }
    assert!(run(&f, 0, &all_tiles(RES), Some(&mut cache)) == reference(&f, 0), "a stale cache was used");
    // A hot layer inside a folder caches below the folder.
    let group = f.project.texture_sets[0].stack[2];
    let inner = f.project.layer(group).unwrap().children()[0];
    cache.set_hot(Some(inner));
    run(&f, 0, &all_tiles(RES), Some(&mut cache));
    assert!(!cache.is_empty());
    assert!(run(&f, 0, &all_tiles(RES), Some(&mut cache)) == reference(&f, 0));
}

#[test]
fn instances_draw_their_source_across_texture_sets() {
    let mut f = fixture();
    let top = heavy_stack(&mut f);
    // Set B instances every top-level layer of set A: same composite.
    for id in f.project.texture_sets[0].stack.clone() {
        add(&mut f, 1, LayerKind::Instance { source: id }, None);
    }
    let a = reference(&f, 0);
    assert!(reference(&f, 1).channels == a.channels, "instances draw like their sources");
    // An edit to the source shows in the instance.
    f.pixels.layer_mut(top, Channel::BaseColor, RES).set(100, 100, &[to_u16(1.0), 0, 0, to_u16(1.0)]);
    let (a, b) = (reference(&f, 0), reference(&f, 1));
    assert!(b.channels == a.channels);
    // The instance's own mask overrides the source's.
    let inst = *f.project.texture_sets[1].stack.last().unwrap();
    f.project.layer_mut(inst).unwrap().mask = Some(Mask::new(MaskBackground::Black));
    let b = reference(&f, 1);
    let (x, y) = (100, 100);
    assert!((a.value(Channel::BaseColor, x, y)[0] - b.value(Channel::BaseColor, x, y)[0]).abs() > 0.1);
    // A hidden instance or a missing source draws nothing.
    let mut g = fixture();
    let src = add(&mut g, 0, fill(&[(Channel::Roughness, Source::Uniform([0.9; 3]))]), None);
    let inst = add(&mut g, 1, LayerKind::Instance { source: src }, None);
    assert!((reference(&g, 1).value(Channel::Roughness, 128, 128)[0] - 0.9).abs() < 1e-3);
    g.project.layer_mut(inst).unwrap().visible = false;
    assert!((reference(&g, 1).value(Channel::Roughness, 128, 128)[0] - 0.5).abs() < 1e-3);
    g.project.layer_mut(inst).unwrap().visible = true;
    g.project.layers.remove(&src);
    assert!((reference(&g, 1).value(Channel::Roughness, 128, 128)[0] - 0.5).abs() < 1e-3);
}

#[test]
fn mask_with_height_follows_the_layer_height() {
    let mut f = fixture();
    // A white layer over the default gray, half masked, its height falling from 0.25 (left) to
    // -0.25 (right).
    let mut ramp = Procedural::new(ProceduralKind::Gradient);
    ramp.scale = 1.0;
    let height = Source::Procedural { pattern: ramp, low: [0.25; 3], high: [-0.25; 3] };
    let id = add(&mut f, 0, fill(&[(Channel::BaseColor, Source::Uniform([1.0; 3])), (Channel::Height, height)]), None);
    let mut mask = Mask::new(MaskBackground::Black);
    mask.effects.push(effect(&mut f, EffectKind::Fill { source: Source::Uniform([0.5; 3]), projection: Projection::default() }));
    f.project.layer_mut(id).unwrap().mask = Some(mask);
    let plain = reference(&f, 0);
    f.project.layer_mut(id).unwrap().height_mask = HeightMask { enabled: true, contrast: 1.0, offset: 0.0 };
    let hm = reference(&f, 0);
    let row = TILE * 2;
    let (left, right) = (16, RES - 16);
    let v = |c: &Composite, x| c.value(Channel::BaseColor, x, row)[0];
    assert!((v(&plain, left) - 0.75).abs() < 0.01 && (v(&plain, right) - 0.75).abs() < 0.01, "plain mask: 50 % white over gray");
    assert!(v(&hm, left) > 0.84, "high parts show first: {}", v(&hm, left));
    assert!(v(&hm, right) < 0.66, "low parts later: {}", v(&hm, right));
    // Without a mask there is nothing to combine.
    f.project.layer_mut(id).unwrap().mask.as_mut().unwrap().enabled = false;
    assert!((v(&reference(&f, 0), left) - 1.0).abs() < 1e-3);
}
