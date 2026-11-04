//! Layer-panel thumbnails: they equal a plain CPU evaluation of the same sub-stack at 64 px,
//! follow edits, drop stale and vanished work, and only render what was asked for.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use texturecraft_engine::compose::{Composite, Inputs, ThumbImage, ThumbTarget, all_tiles, composite_tiles, mask_image};
use texturecraft_engine::core::{Channel, ChannelKind, LayerId};
use texturecraft_engine::geom::UvGBuffer;
use texturecraft_engine::geom::primitives;
use texturecraft_engine::texel::linear_to_srgb;
use texturecraft_engine::thumbs::{CACHE_ENTRIES, ThumbService};
use texturecraft_engine::{Session, execute};

const N: u32 = 64;

fn session() -> Session {
    Session::new_project("t", primitives::sphere(1.0, 32, 16), "builtin:sphere", None, 256).unwrap()
}

fn id(v: Value, key: &str) -> LayerId {
    LayerId(v[key].as_u64().unwrap())
}

/// A fill layer with two content effects (invert, then posterize) and a masked generator.
struct Stack {
    layer: LayerId,
    invert: LayerId,
    posterize: LayerId,
    gen_: LayerId,
}

fn stack(s: &mut Session) -> Stack {
    let layer = id(execute(s, "layer.add.fill", &json!({"color": "#336699", "roughness": 0.2})).unwrap(), "layer");
    let invert = id(execute(s, "effect.add.filter", &json!({"target": "content", "filter": "invert"})).unwrap(), "effect");
    let posterize = id(execute(s, "effect.add.filter", &json!({"target": "content", "filter": "posterize"})).unwrap(), "effect");
    execute(s, "mask.add.black", &Value::Null).unwrap();
    let gen_ = id(execute(s, "effect.add.generator", &json!({"generator": "position", "level": 0.5})).unwrap(), "effect");
    Stack { layer, invert, posterize, gen_ }
}

fn inputs<'a>(s: &'a Session, project: &'a texturecraft_engine::core::Project, gbuf: &'a UvGBuffer) -> Inputs<'a> {
    let none = Default::default();
    // Leaked: the oracle only lives for the test.
    let baked: &'a _ = Box::leak(Box::new(s.doc.baked.get(&0).cloned().unwrap_or(none)));
    Inputs { project, ts: 0, mesh: &s.mesh, gbuf, pixels: &s.doc.pixels, baked, preview: None, images: &s.images, bounds: s.mesh.bounds() }
}

/// The reference: the same sub-stack built by hand and composited at 64 px with the ordinary
/// evaluator, then encoded for display.
fn oracle(s: &Session, target: ThumbTarget) -> ThumbImage {
    let gbuf = UvGBuffer::new(&s.mesh, 0, N, N);
    let mut p = s.doc.project.clone();
    let (layer, upto, channel) = match target {
        ThumbTarget::Content { layer, channel } => (layer, None, channel),
        ThumbTarget::Mask { layer } => (layer, None, Channel::BaseColor),
        ThumbTarget::Stage { effect, channel } => (p.effect(effect).unwrap().0, Some(effect), channel),
    };
    p.texture_sets[0].stack = vec![layer];
    let l = p.layers.get_mut(&layer).unwrap();
    l.visible = true;
    let in_mask = upto.is_some_and(|e| l.mask.as_ref().is_some_and(|m| m.effects.iter().any(|x| x.id == e))) || matches!(target, ThumbTarget::Mask { .. });
    if in_mask {
        let m = l.mask.as_mut().unwrap();
        m.enabled = true;
        if let Some(e) = upto {
            let k = m.effects.iter().position(|x| x.id == e).unwrap();
            m.effects.truncate(k + 1);
        }
        let gray = mask_image(&inputs(s, &p, &gbuf), layer, N).unwrap();
        let rgba = gray.iter().enumerate().flat_map(|(i, g)| [*g, *g, *g, if gbuf.covered(i) { 255 } else { 0 }]).collect();
        return ThumbImage { width: N, height: N, rgba };
    }
    l.mask = None;
    l.channels.insert(channel, texturecraft_engine::core::ChannelBlend::default());
    if let Some(e) = upto {
        let k = l.content_effects.iter().position(|x| x.id == e).unwrap();
        l.content_effects.truncate(k + 1);
    }
    let channels = p.texture_set(0).unwrap().channel_list();
    let mut comp = Composite::new(&channels, N);
    composite_tiles(&inputs(s, &p, &gbuf), &mut comp, &all_tiles(N, N));
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let mut rgba = Vec::new();
    for y in 0..N {
        for x in 0..N {
            let v = comp.value(channel, x, y);
            let px = match channel.kind() {
                ChannelKind::Color => v.map(|c| byte(linear_to_srgb(c.clamp(0.0, 1.0)))),
                ChannelKind::Signed => v.map(|c| byte(c * 0.5 + 0.5)),
                _ => v.map(byte),
            };
            rgba.extend_from_slice(&[px[0], px[1], px[2], if gbuf.covered((y * N + x) as usize) { 255 } else { 0 }]);
        }
    }
    ThumbImage { width: N, height: N, rgba }
}

/// Frames of `svc` until every target is current (or a timeout).
fn settle(svc: &mut ThumbService, s: &Session, targets: &[ThumbTarget], clock: &mut f64) {
    let t0 = Instant::now();
    loop {
        *clock += 0.2;
        svc.begin_frame(s, *clock);
        for t in targets {
            svc.get(s, *t);
        }
        svc.poll(s);
        if targets.iter().all(|t| svc.is_fresh(*t)) {
            return;
        }
        assert!(t0.elapsed() < Duration::from_secs(60), "thumbnails never settled");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn image(svc: &mut ThumbService, s: &Session, t: ThumbTarget) -> ThumbImage {
    (*svc.get(s, t).image.expect("a picture")).clone()
}

#[test]
fn thumbnails_equal_a_cpu_evaluation_of_the_same_sub_stack() {
    let mut s = session();
    let st = stack(&mut s);
    let base = Channel::BaseColor;
    let targets = [
        ThumbTarget::Content { layer: st.layer, channel: base },
        ThumbTarget::Content { layer: st.layer, channel: Channel::Roughness },
        ThumbTarget::Mask { layer: st.layer },
        ThumbTarget::Stage { effect: st.invert, channel: base },
        ThumbTarget::Stage { effect: st.posterize, channel: base },
        ThumbTarget::Stage { effect: st.gen_, channel: base },
    ];
    let (mut svc, mut clock) = (ThumbService::new(), 0.0);
    settle(&mut svc, &s, &targets, &mut clock);
    for t in targets {
        assert_eq!(image(&mut svc, &s, t), oracle(&s, t), "{t:?}");
    }
    // Sanity on the values themselves: the first stage is #336699 inverted (in linear light).
    let inv = image(&mut svc, &s, targets[3]);
    let px = inv.rgba.chunks_exact(4).find(|p| p[3] == 255).unwrap();
    let expect = |byte: f32| (linear_to_srgb(1.0 - texturecraft_engine::texel::srgb_to_linear(byte / 255.0)) * 255.0 + 0.5) as i32;
    assert!((px[0] as i32 - expect(51.0)).abs() <= 1 && (px[2] as i32 - expect(153.0)).abs() <= 1, "{px:?}");
    // The mask thumbnail is gray and not flat (a position gradient on a black mask).
    let m = image(&mut svc, &s, targets[2]);
    let grays: Vec<u8> = m.rgba.chunks_exact(4).filter(|p| p[3] == 255).map(|p| p[0]).collect();
    assert!(grays.iter().max() != grays.iter().min());
}

#[test]
fn earlier_stages_never_show_later_effects_and_hidden_effects_are_bypassed() {
    let mut s = session();
    let st = stack(&mut s);
    let base = Channel::BaseColor;
    let (first, second) = (ThumbTarget::Stage { effect: st.invert, channel: base }, ThumbTarget::Stage { effect: st.posterize, channel: base });
    let (mut svc, mut clock) = (ThumbService::new(), 0.0);
    settle(&mut svc, &s, &[first, second], &mut clock);
    let (a, b) = (image(&mut svc, &s, first), image(&mut svc, &s, second));
    // Hide the second effect: its stage now shows the first one's result; the first is unchanged.
    execute(&mut s, "effect.set", &json!({"effect": st.posterize.0, "visible": false})).unwrap();
    settle(&mut svc, &s, &[first, second], &mut clock);
    assert_eq!(image(&mut svc, &s, first), a);
    assert_eq!(image(&mut svc, &s, second), a);
    assert_ne!(a, b, "posterize should change something for the test to mean anything");
    // And the content thumbnail (everything) follows.
    let all = ThumbTarget::Content { layer: st.layer, channel: base };
    settle(&mut svc, &s, &[all], &mut clock);
    assert_eq!(image(&mut svc, &s, all), a);
}

#[test]
fn editing_invalidates_and_unrelated_changes_do_not() {
    let mut s = session();
    let st = stack(&mut s);
    let t = ThumbTarget::Content { layer: st.layer, channel: Channel::BaseColor };
    let (mut svc, mut clock) = (ThumbService::new(), 0.0);
    settle(&mut svc, &s, &[t], &mut clock);
    let before = image(&mut svc, &s, t);
    let rendered = svc.rendered;
    // A camera move changes the session revision but no thumbnail input: nothing is re-rendered.
    execute(&mut s, "camera.set", &json!({"yaw": 0.7})).ok();
    s.revision += 1;
    for _ in 0..3 {
        clock += 0.3;
        svc.begin_frame(&s, clock);
        svc.get(&s, t);
        svc.poll(&s);
    }
    assert!(svc.is_fresh(t));
    assert_eq!(svc.rendered, rendered, "an unrelated change re-rendered a thumbnail");
    // An edit of the layer re-renders it, and the new picture is the new oracle.
    execute(&mut s, "effect.set", &json!({"effect": st.invert.0, "visible": false})).unwrap();
    clock += 0.3;
    svc.begin_frame(&s, clock);
    assert!(!svc.is_fresh(t));
    settle(&mut svc, &s, &[t], &mut clock);
    let after = image(&mut svc, &s, t);
    assert_ne!(before, after);
    assert_eq!(after, oracle(&s, t));
    // Undo brings the first picture back.
    execute(&mut s, "edit.undo", &Value::Null).unwrap();
    settle(&mut svc, &s, &[t], &mut clock);
    assert_eq!(image(&mut svc, &s, t), before);
}

#[test]
fn deleting_a_layer_with_a_pending_thumbnail_is_harmless() {
    let mut s = session();
    let st = stack(&mut s);
    let (a, b) = (ThumbTarget::Content { layer: st.layer, channel: Channel::BaseColor }, ThumbTarget::Mask { layer: st.layer });
    let mut svc = ThumbService::new();
    // Queue the work and hand it to the worker, then delete the layer before collecting.
    svc.begin_frame(&s, 1.0);
    svc.get(&s, a);
    svc.get(&s, b);
    svc.poll(&s);
    execute(&mut s, "layer.remove", &json!({"layer": st.layer.0})).unwrap();
    let mut clock = 2.0;
    for _ in 0..200 {
        clock += 0.2;
        svc.begin_frame(&s, clock);
        // The row is gone, so nobody asks for it any more; the pending result is dropped.
        svc.poll(&s);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!svc.is_fresh(a) && !svc.is_fresh(b));
    assert_eq!(svc.cached(), 0);
    assert!(!svc.busy());
    // Asking for a layer that no longer exists renders nothing and does not panic either.
    settle_gone(&mut svc, &s, a, &mut clock);
}

fn settle_gone(svc: &mut ThumbService, s: &Session, t: ThumbTarget, clock: &mut f64) {
    for _ in 0..20 {
        *clock += 0.2;
        svc.begin_frame(s, *clock);
        svc.get(s, t);
        svc.poll(s);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(svc.get(s, t).image.is_none());
}

#[test]
fn a_result_of_an_older_document_never_replaces_a_newer_one() {
    let mut s = session();
    let st = stack(&mut s);
    let t = ThumbTarget::Content { layer: st.layer, channel: Channel::BaseColor };
    let mut svc = ThumbService::new();
    svc.begin_frame(&s, 1.0);
    svc.get(&s, t);
    svc.poll(&s); // the job of the first document is running
    execute(&mut s, "effect.set", &json!({"effect": st.invert.0, "visible": false})).unwrap();
    let mut clock = 2.0;
    settle(&mut svc, &s, &[t], &mut clock);
    // Whatever the first job returned, what is shown is the current document.
    assert_eq!(image(&mut svc, &s, t), oracle(&s, t));
}

#[test]
fn only_requested_rows_render_and_the_cache_is_bounded() {
    let mut s = session();
    let layers: Vec<LayerId> = (0..(CACHE_ENTRIES + 30)).map(|_| id(execute(&mut s, "layer.add.paint", &Value::Null).unwrap(), "layer")).collect();
    let target = |l: LayerId| ThumbTarget::Content { layer: l, channel: Channel::BaseColor };
    let mut svc = ThumbService::new();
    // A frame asks for six rows, the next frame (scrolled) for only one of them.
    svc.begin_frame(&s, 1.0);
    for l in &layers[..6] {
        svc.get(&s, target(*l));
    }
    svc.poll(&s);
    let mut clock = 2.0;
    for _ in 0..300 {
        clock += 0.2;
        svc.begin_frame(&s, clock);
        svc.get(&s, target(layers[0]));
        svc.poll(&s);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(svc.is_fresh(target(layers[0])));
    // At most the rows already on their way (the in-flight limit) finished besides it.
    let done = layers[..6].iter().filter(|l| svc.is_fresh(target(**l))).count();
    assert!(done <= 3, "{done} of the scrolled-away rows were rendered");
    // Scrolling through every layer keeps the cache within its limit.
    for chunk in layers.chunks(12) {
        let ts: Vec<ThumbTarget> = chunk.iter().map(|l| target(*l)).collect();
        settle(&mut svc, &s, &ts, &mut clock);
        assert!(svc.cached() <= CACHE_ENTRIES + 12, "{}", svc.cached());
    }
    clock += 0.2;
    svc.begin_frame(&s, clock);
    svc.poll(&s);
    assert!(svc.cached() <= CACHE_ENTRIES, "{}", svc.cached());
}
