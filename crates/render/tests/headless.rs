use texturecraft_compose::{DisplayKind, ImageRes};
use texturecraft_geom::{OrbitCamera, primitives};
use texturecraft_render::headless::Headless;
use texturecraft_render::renderer::TAA_SAMPLES;
use texturecraft_render::{Frame, Shade, ViewKind};

/// The UI's key light (`viewport::shade`).
const LIGHT: [f32; 4] = [0.5, 0.8, 0.4, 2.5];

struct Img {
    w: u32,
    px: Vec<u8>,
}

impl Img {
    fn at(&self, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2]]
    }
    fn luma(&self, x: u32, y: u32) -> f32 {
        let [r, g, b] = self.at(x, y);
        0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32
    }
    /// Mean luma over a rectangle.
    fn mean(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> f32 {
        let mut sum = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                sum += self.luma(x, y);
            }
        }
        sum / ((x1 - x0) * (y1 - y0)) as f32
    }
}

/// A headless renderer with the material ball framed in a 480×360 view (None without a GPU).
fn setup() -> Option<(Headless, Frame)> {
    let Ok(mut h) = Headless::new() else {
        eprintln!("skipping: no GPU adapter");
        return None;
    };
    let mesh = primitives::material_ball();
    h.renderer.set_mesh(&h.device, &h.queue, &mesh, 1);
    h.renderer.set_texture_sets(&h.device, &h.queue, &[(256, 0), (256, 1)], 2);
    let mut cam = OrbitCamera { yaw: 0.6, pitch: 0.25, ..OrbitCamera::default() };
    let (lo, hi) = mesh.bounds();
    cam.frame(lo, hi);
    let frame = Frame { view_id: 1, size: [480, 360], kind: ViewKind::Scene, camera: cam, shade: Shade::default(), visible: vec![true, true] };
    Some((h, frame))
}

fn render(h: &mut Headless, frame: &Frame, name: &str) -> Img {
    let (w, hgt, px) = h.render(frame).expect("readback");
    assert_eq!((w, hgt), (frame.size[0], frame.size[1]));
    if let Ok(dir) = std::env::var("TC_SNAPSHOT_DIR") {
        image::save_buffer(format!("{dir}/{name}.png"), &px, w, hgt, image::ColorType::Rgba8).expect("save snapshot");
    }
    Img { w, px }
}

fn shaded(frame: &Frame, f: impl FnOnce(&mut Shade)) -> Frame {
    let mut out = frame.clone();
    f(&mut out.shade);
    out
}

#[test]
fn renders_the_material_ball() {
    let Some((mut h, frame)) = setup() else { return };
    let img = render(&mut h, &frame, "headless_ball");
    assert_ne!(img.at(240, 140), img.at(5, 5), "mesh should differ from background");
    // UV view of set 0.
    let uv = Frame { view_id: 2, kind: ViewKind::Uv { ts: 0, zoom: 1.0, pan: [0.0, 0.0] }, ..frame.clone() };
    let uv_img = render(&mut h, &uv, "headless_uv");
    assert_eq!(uv_img.px.len(), img.px.len());
    let wire = render(&mut h, &shaded(&uv, |s| s.wireframe = true), "headless_uv_wire");
    assert_ne!(wire.px, uv_img.px, "the UV wireframe draws");
}

#[test]
fn exposure_brightens_the_material_but_not_data_views() {
    let Some((mut h, frame)) = setup() else { return };
    let base = render(&mut h, &frame, "exposure_0");
    let bright = render(&mut h, &shaded(&frame, |s| s.exposure_ev = 1.0), "exposure_plus1");
    let (b0, b1) = (base.mean(200, 100, 280, 180), bright.mean(200, 100, 280, 180));
    assert!(b1 > b0 + 10.0, "exposure +1 brightens the ball ({b0} → {b1})");
    assert_eq!(base.at(5, 5), bright.at(5, 5), "the flat backdrop ignores exposure");
    let ch = shaded(&frame, |s| {
        s.view_mode = 1;
        s.channel = 0;
    });
    let a = render(&mut h, &ch, "channel_base_color");
    let b = render(&mut h, &shaded(&ch, |s| s.exposure_ev = 2.0), "channel_base_color_ev2");
    assert_eq!(a.px, b.px, "data views ignore exposure");
    // Default base color 188 (sRGB) shows unchanged in the channel view.
    let [r, ..] = a.at(240, 140);
    assert!(r.abs_diff(188) <= 1, "base color channel shows the raw value, got {r}");
}

#[test]
fn post_effects() {
    let Some((mut h, frame)) = setup() else { return };
    let base = render(&mut h, &frame, "post_off");
    // Strengths without the master switch change nothing.
    let gated = render(
        &mut h,
        &shaded(&frame, |s| {
            s.vignette = 1.0;
            s.bloom = 1.0;
            s.grain = 1.0;
            s.sharpen = 1.0;
        }),
        "post_gated",
    );
    assert_eq!(base.px, gated.px, "effects are off without post_effects");
    let on = |f: fn(&mut Shade)| {
        shaded(&frame, |s| {
            s.post_effects = true;
            f(s);
        })
    };
    let vignette = render(&mut h, &on(|s| s.vignette = 1.0), "post_vignette");
    assert!(vignette.mean(0, 0, 20, 20) < base.mean(0, 0, 20, 20) - 5.0, "vignette darkens the corners");
    assert!((vignette.luma(240, 180) - base.luma(240, 180)).abs() < 1.5, "vignette keeps the center");
    // A bright environment and +2 EV so highlights pass the bloom threshold.
    let hot = |s: &mut Shade| {
        s.exposure_ev = 2.0;
        s.env_opacity = 1.0;
    };
    let hot_base = render(&mut h, &shaded(&frame, hot), "post_hot");
    let bloom = render(
        &mut h,
        &on(|s| {
            s.exposure_ev = 2.0;
            s.env_opacity = 1.0;
            s.bloom = 1.0;
        }),
        "post_bloom",
    );
    assert!(bloom.mean(0, 0, 480, 360) > hot_base.mean(0, 0, 480, 360) + 0.5, "bloom adds light");
    let sharpen = render(&mut h, &on(|s| s.sharpen = 1.0), "post_sharpen");
    assert_ne!(sharpen.px, base.px, "sharpen changes edges");
    let grain = render(&mut h, &on(|s| s.grain = 1.0), "post_grain");
    assert_ne!(grain.px, base.px, "grain adds noise");
    let all = render(
        &mut h,
        &on(|s| {
            s.bloom = 0.6;
            s.vignette = 0.5;
            s.sharpen = 0.4;
            s.grain = 0.3;
        }),
        "post_all",
    );
    assert_eq!(all.px.len(), base.px.len());
}

#[test]
fn shadows_darken_the_key_light() {
    let Some((mut h, frame)) = setup() else { return };
    let lit = shaded(&frame, |s| s.light = LIGHT);
    let a = render(&mut h, &lit, "shadows_off");
    let b = render(&mut h, &shaded(&lit, |s| s.shadows = true), "shadows_on");
    let (mut darker, mut brighter) = (0, 0);
    for y in 0..360 {
        for x in 0..480 {
            let d = b.luma(x, y) - a.luma(x, y);
            darker += (d < -8.0) as u32;
            brighter += (d > 2.0) as u32;
        }
    }
    assert!(darker > 1000, "the ball shadows its pedestal ({darker} darker pixels)");
    assert_eq!(brighter, 0, "shadows never brighten");
    // From above, where the pedestal top shows the ball's shadow.
    let mut top = shaded(&lit, |s| s.shadows = true);
    top.camera.pitch = 0.9;
    render(&mut h, &top, "shadows_on_top");
    // Rotating the environment turns the key light and its shadow.
    let turned = render(&mut h, &shaded(&lit, |s| {
        s.shadows = true;
        s.env_rotation_deg = 120.0;
    }), "shadows_env_rotated");
    assert_ne!(turned.px, b.px);
}

#[test]
fn wireframe_3d_overlays_the_mesh() {
    let Some((mut h, frame)) = setup() else { return };
    let a = render(&mut h, &frame, "wire3d_off");
    let b = render(&mut h, &shaded(&frame, |s| s.wireframe_3d = true), "wire3d_on");
    assert_ne!(a.mean(200, 100, 280, 180), b.mean(200, 100, 280, 180), "edges draw over the ball");
    assert_eq!(a.at(5, 5), b.at(5, 5), "the background stays clean");
}

#[test]
fn ortho_background_matches_perspective() {
    let Some((mut h, frame)) = setup() else { return };
    let env = shaded(&frame, |s| s.env_opacity = 1.0);
    let persp = render(&mut h, &env, "env_perspective");
    let ortho = render(&mut h, &Frame { camera: OrbitCamera { ortho: true, ..env.camera }, ..env.clone() }, "env_ortho");
    for (x, y) in [(5, 5), (474, 5), (5, 354), (474, 354), (240, 5)] {
        assert_eq!(persp.at(x, y), ortho.at(x, y), "background at {x},{y}");
    }
    assert_ne!(ortho.at(5, 5), ortho.at(240, 5), "the environment varies across the view");
}

/// Frame cost at 1920×1080: `cargo test -p texturecraft-render --release -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_1080p() {
    let Some((mut h, frame)) = setup() else { return };
    h.renderer.set_texture_sets(&h.device, &h.queue, &[(2048, 0), (2048, 1)], 2);
    let frame = Frame { size: [1920, 1080], ..frame };
    let run = |h: &mut Headless, frame: &Frame| {
        for _ in 0..5 {
            h.render(frame);
        }
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let mut enc = h.device.create_command_encoder(&Default::default());
            h.renderer.prepare(&h.device, &h.queue, &mut enc, frame);
            h.queue.submit([enc.finish()]);
            let _ = h.device.poll(texturecraft_render::wgpu::PollType::Wait { submission_index: None, timeout: None });
        }
        t.elapsed().as_secs_f64() * 1000.0 / 100.0
    };
    println!("adapter: {}", h.adapter_name);
    println!("default: {:.3} ms/frame", run(&mut h, &frame));
    let effects = shaded(&frame, |s| {
        s.post_effects = true;
        s.bloom = 0.5;
        s.vignette = 0.5;
        s.sharpen = 0.5;
        s.grain = 0.5;
    });
    println!("post effects: {:.3} ms/frame", run(&mut h, &effects));
    let shadows = shaded(&frame, |s| {
        s.light = LIGHT;
        s.shadows = true;
    });
    println!("shadows (cached map): {:.3} ms/frame", run(&mut h, &shadows));
    let wire = shaded(&frame, |s| s.wireframe_3d = true);
    println!("3D wireframe: {:.3} ms/frame", run(&mut h, &wire));
    let all = shaded(&effects, |s| {
        s.light = LIGHT;
        s.shadows = true;
        s.wireframe_3d = true;
    });
    println!("everything: {:.3} ms/frame", run(&mut h, &all));
    println!("parallax occlusion: {:.3} ms/frame", run(&mut h, &shaded(&frame, |s| s.parallax = true)));
    let taa = shaded(&frame, |s| s.taa = true);
    h.render(&taa);
    let t = std::time::Instant::now();
    for _ in 1..TAA_SAMPLES {
        let mut enc = h.device.create_command_encoder(&Default::default());
        h.renderer.prepare(&h.device, &h.queue, &mut enc, &taa);
        h.queue.submit([enc.finish()]);
        let _ = h.device.poll(texturecraft_render::wgpu::PollType::Wait { submission_index: None, timeout: None });
    }
    println!("TAA accumulation: {:.3} ms/sample", t.elapsed().as_secs_f64() * 1000.0 / f64::from(TAA_SAMPLES - 1));
    // Shadow map re-rendered every frame (light moving).
    let t = std::time::Instant::now();
    for i in 0..100 {
        let f = shaded(&shadows, |s| s.env_rotation_deg = i as f32);
        let mut enc = h.device.create_command_encoder(&Default::default());
        h.renderer.prepare(&h.device, &h.queue, &mut enc, &f);
        h.queue.submit([enc.finish()]);
        let _ = h.device.poll(texturecraft_render::wgpu::PollType::Wait { submission_index: None, timeout: None });
    }
    println!("shadows, light moving: {:.3} ms/frame", t.elapsed().as_secs_f64() * 1000.0 / 100.0);
}

/// Fill the height texture of both 256² sets: `f(x, y)` → (height, F0 r, g, b).
fn upload_height(h: &mut Headless, f: impl Fn(u32, u32) -> [f32; 4]) {
    for ts in 0..2 {
        for (tx, ty) in (0..16).map(|i| (i % 4, i / 4)) {
            let bytes: Vec<u8> = (0..64 * 64).flat_map(|i| f(tx * 64 + i % 64, ty * 64 + i / 64)).flat_map(|v| half::f16::from_f32(v).to_le_bytes()).collect();
            h.renderer.upload_tile(&h.queue, ts, DisplayKind::Height, tx, ty, &bytes);
        }
    }
}

/// Mean of red minus blue over the ball.
fn warmth(img: &Img) -> f32 {
    let mut sum = 0.0;
    for y in 100..180 {
        for x in 200..280 {
            let [r, _, b] = img.at(x, y);
            sum += r as f32 - b as f32;
        }
    }
    sum / 6400.0
}

#[test]
fn imported_hdr_environment_lights_the_ball() {
    let Some((mut h, frame)) = setup() else { return };
    let base = render(&mut h, &frame, "env_default");
    // A procedural HDR: a hot orange sky over a dim blue ground (values far above 1).
    let (w, hh) = (256u32, 128u32);
    let rgba = (0..w * hh).map(|i| if i / w < hh / 2 { [12.0, 4.0, 1.0, 1.0] } else { [0.02, 0.05, 0.2, 1.0] }).collect();
    let sky = ImageRes { width: w, height: hh, rgba, srgb: false };
    h.renderer.set_environment(&h.device, &h.queue, "resource:sky.hdr", Some(&sky));
    let lit = render(&mut h, &frame, "env_hdr_resource");
    assert_ne!(lit.px, base.px, "the imported environment changes the lighting");
    assert!(warmth(&lit) > warmth(&base) + 10.0, "the orange sky tints the ball");
    assert!(lit.mean(200, 100, 280, 180) > 20.0, "lit, finite shading (NaNs would render black)");
    h.renderer.set_environment(&h.device, &h.queue, "Studio Soft", None);
    assert_eq!(render(&mut h, &frame, "env_default_again").px, base.px, "built-ins come back");
}

#[test]
fn specular_glossiness_takes_f0_from_the_specular_color() {
    let Some((mut h, frame)) = setup() else { return };
    upload_height(&mut h, |_, _| [0.0, 1.0, 0.6, 0.1]);
    let mr = render(&mut h, &frame, "shader_metal_rough");
    let sg = render(&mut h, &shaded(&frame, |s| s.spec_gloss = true), "shader_spec_gloss_gold");
    assert!(warmth(&sg) > warmth(&mr) + 10.0, "a gold F0 tints the reflections ({} vs {})", warmth(&sg), warmth(&mr));
    assert!(sg.mean(200, 100, 280, 180) > 20.0);
}

#[test]
fn parallax_follows_the_height_channel() {
    let Some((mut h, frame)) = setup() else { return };
    let pom = shaded(&frame, |s| s.parallax = true);
    // A flat height field: parallax changes nothing.
    let (off, on) = (render(&mut h, &frame, "parallax_flat_off"), render(&mut h, &pom, "parallax_flat_on"));
    assert!(off.px.iter().zip(&on.px).all(|(a, b)| a.abs_diff(*b) <= 2), "flat heights keep the UVs");
    upload_height(&mut h, |x, y| [if (x / 16 + y / 16) % 2 == 0 { 0.8 } else { -0.8 }, 0.04, 0.04, 0.04]);
    let (off, on) = (render(&mut h, &frame, "parallax_off"), render(&mut h, &pom, "parallax_on"));
    assert_ne!(off.px, on.px, "a checker height field shifts the textures");
    assert!(on.mean(200, 100, 280, 180) > 20.0);
}

#[test]
fn idle_accumulation_converges_then_stops() {
    let Some((mut h, frame)) = setup() else { return };
    let taa = shaded(&frame, |s| s.taa = true);
    let first = render(&mut h, &taa, "taa_first");
    assert_eq!(first.px, render(&mut h, &frame, "taa_off").px, "the first sample is the plain frame");
    render(&mut h, &taa, "taa_first_again");
    for _ in 1..TAA_SAMPLES {
        assert!(h.renderer.accumulating(1));
        render(&mut h, &taa, "taa_sample");
    }
    assert!(!h.renderer.accumulating(1), "converged after {TAA_SAMPLES} samples");
    let last = render(&mut h, &taa, "taa_converged");
    assert_ne!(last.px, first.px, "jittered samples smooth the edges");
    assert_eq!(render(&mut h, &taa, "taa_idle").px, last.px, "a converged view keeps its image");
    render(&mut h, &shaded(&taa, |s| s.env_rotation_deg = 30.0), "taa_changed");
    assert!(h.renderer.accumulating(1), "a change restarts accumulation");
}
