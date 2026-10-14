use image::{DynamicImage, GenericImageView, ImageFormat};
use texturecraft_core::{ChannelFormat, Project, TextureSet};
use texturecraft_texel::{to_u16, to_u16_signed};

use super::*;
use crate::presets::{builtin_preset, builtin_presets, preset_from_json, preset_to_json};

const RES: u32 = 64;

struct Fx {
    project: Project,
    comp: Composite,
    baked: BakedMaps,
    coverage: Vec<bool>,
}

impl Fx {
    fn new(channels: &[Channel]) -> Fx {
        let mut project = Project::new("My Project", "builtin:sphere");
        let mut ts = TextureSet::new("Body Set", 0, RES);
        ts.channels = channels.iter().map(|c| (*c, ChannelFormat::default_for(*c))).collect();
        project.texture_sets.push(ts);
        let mut fx = Fx { project, comp: Composite::new(channels, RES), baked: BakedMaps::default(), coverage: vec![true; (RES * RES) as usize] };
        for y in 0..RES {
            for x in 0..RES {
                fx.set(Channel::BaseColor, x, y, [x as f32 / 63.0, y as f32 / 63.0, 0.2]);
                fx.set(Channel::Roughness, x, y, [0.25; 3]);
                fx.set(Channel::Metallic, x, y, [0.75; 3]);
                fx.set(Channel::Normal, x, y, [0.5, 0.5, 1.0]);
                fx.set(Channel::Height, x, y, [0.0; 3]);
            }
        }
        fx
    }

    fn set(&mut self, c: Channel, x: u32, y: u32, v: [f32; 3]) {
        let Some(img) = self.comp.channels.get_mut(&c) else { return };
        let vals: Vec<u16> = v.iter().take(c.components()).map(|v| if c.kind() == ChannelKind::Signed { to_u16_signed(*v) } else { to_u16(*v) }).collect();
        img.set(x, y, &vals);
    }

    fn set_all(&mut self, c: Channel, f: impl Fn(u32, u32) -> [f32; 3]) {
        for y in 0..RES {
            for x in 0..RES {
                self.set(c, x, y, f(x, y));
            }
        }
    }

    fn input(&self) -> ExportInput<'_> {
        ExportInput { project: &self.project, ts: 0, composite: &self.comp, baked: &self.baked, coverage: &self.coverage, mesh_name: "sphere" }
    }

    fn render(&self, opts: &ExportOptions) -> Vec<ExportedFile> {
        render_maps(&self.input(), opts).unwrap()
    }

    fn render_preset(&self, name: &str) -> Vec<ExportedFile> {
        self.render(&ExportOptions::new(builtin_preset(name).unwrap()))
    }
}

fn find<'a>(files: &'a [ExportedFile], suffix: &str) -> &'a ExportedFile {
    files.iter().find(|f| f.name.contains(suffix)).unwrap_or_else(|| panic!("no {suffix} in {:?}", files.iter().map(|f| &f.name).collect::<Vec<_>>()))
}

fn decode(f: &ExportedFile) -> DynamicImage {
    let ext = f.name.rsplit('.').next().unwrap();
    image::load_from_memory_with_format(&f.bytes, ImageFormat::from_extension(ext).unwrap()).unwrap()
}

fn default_channels() -> Vec<Channel> {
    Channel::DEFAULT_SET.to_vec()
}

#[test]
fn gltf_png_matches_the_composite() {
    let fx = Fx::new(&default_channels());
    let files = fx.render_preset("glTF PBR Metal Roughness");
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["sphere_Body_Set_BaseColor.png", "sphere_Body_Set_OcclusionRoughnessMetallic.png", "sphere_Body_Set_Normal.png"]);
    let base = decode(&files[0]);
    assert_eq!(base.color(), image::ColorType::Rgb8, "no opacity channel → no alpha");
    for (x, y) in [(0, 0), (10, 40), (63, 63), (32, 7)] {
        let v = fx.comp.value(Channel::BaseColor, x, y);
        let px = base.get_pixel(x, y);
        for k in 0..3 {
            let want = linear_to_srgb(v[k]) * 255.0;
            assert!((px[k] as f32 - want).abs() <= 1.0, "base ({x},{y})[{k}] {} vs {want}", px[k]);
        }
    }
    let orm = decode(&files[1]).get_pixel(5, 5);
    assert_eq!(orm[0], 255, "no AO anywhere → white");
    assert!((orm[1] as i32 - 64).abs() <= 1 && (orm[2] as i32 - 191).abs() <= 1, "{orm:?}");
    let n = decode(&files[2]).get_pixel(5, 5);
    assert!((n[0] as i32 - 128).abs() <= 1 && (n[1] as i32 - 128).abs() <= 1 && n[2] == 255, "{n:?}");
    // Emissive appears only when the set has the channel.
    let mut channels = default_channels();
    channels.push(Channel::Emissive);
    let fx = Fx::new(&channels);
    assert!(fx.render_preset("glTF PBR Metal Roughness").iter().any(|f| f.name.ends_with("_Emissive.png")));
}

#[test]
fn exr_values_are_linear() {
    let fx = Fx::new(&default_channels());
    for (depth, tol) in [(32u8, 1e-4f32), (16, 2e-3)] {
        let mut opts = ExportOptions::new(builtin_preset("glTF PBR Metal Roughness").unwrap());
        opts.format_override = Some(FileFormat::Exr);
        opts.bit_depth_override = Some(depth);
        let files = fx.render(&opts);
        let base = find(&files, "_BaseColor.exr");
        let img = decode(base).to_rgb32f();
        for (x, y) in [(0, 0), (10, 40), (63, 63)] {
            let v = fx.comp.value(Channel::BaseColor, x, y);
            let px = img.get_pixel(x, y);
            for k in 0..3 {
                assert!((px[k] - v[k]).abs() <= tol.max(v[k] * tol), "depth {depth} ({x},{y})[{k}]: {} vs {}", px[k], v[k]);
            }
        }
    }
    // A single-channel map is written as a luminance (Y) EXR.
    let mut opts = ExportOptions::new(builtin_preset("Separate maps").unwrap());
    opts.format_override = Some(FileFormat::Exr);
    let files = fx.render(&opts);
    let rough = find(&files, "_roughness.exr");
    use exr::prelude::{ReadChannels, ReadLayers};
    let img = exr::prelude::read()
        .no_deep_data()
        .largest_resolution_level()
        .all_channels()
        .first_valid_layer()
        .all_attributes()
        .from_buffered(std::io::Cursor::new(&rough.bytes))
        .unwrap();
    let ch = &img.layer_data.channel_data.list;
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].name.to_string(), "Y");
    assert!((ch[0].sample_data.value_by_flat_index(100).to_f32() - 0.25).abs() < 1e-3);
}

#[test]
fn directx_flips_green() {
    let mut fx = Fx::new(&default_channels());
    let n = Vec3::new(0.3, 0.4, 0.0);
    let n = Vec3::new(n.x, n.y, (1.0 - n.length_squared()).sqrt());
    fx.set_all(Channel::Normal, |_, _| (n * 0.5 + Vec3::splat(0.5)).to_array());
    let gl = decode(find(&fx.render_preset("glTF PBR Metal Roughness"), "_Normal.png")).to_rgb8();
    let dx = decode(find(&fx.render_preset("Unreal Engine (Packed)"), "_Normal.png")).to_rgb8();
    let (a, b) = (gl.get_pixel(9, 9), dx.get_pixel(9, 9));
    assert!((a[0] as i32 - ((n.x * 0.5 + 0.5) * 255.0).round() as i32).abs() <= 1);
    assert!((a[1] as i32 - ((n.y * 0.5 + 0.5) * 255.0).round() as i32).abs() <= 1, "{a:?}");
    assert_eq!(a[0], b[0]);
    assert!((a[1] as i32 + b[1] as i32 - 255).abs() <= 1, "{a:?} vs {b:?}");
    assert_eq!(a[2], b[2]);
    // The normal-format override applies to every normal output.
    let mut opts = ExportOptions::new(builtin_preset("glTF PBR Metal Roughness").unwrap());
    opts.normal_format = Some(NormalFormat::DirectX);
    assert_eq!(decode(find(&fx.render(&opts), "_Normal.png")).to_rgb8(), dx);
}

#[test]
fn height_becomes_normal_detail() {
    let mut fx = Fx::new(&[Channel::BaseColor, Channel::Height]);
    // Height rises to the right and down the image.
    fx.set_all(Channel::Height, |x, y| [(x as f32 + y as f32) / 160.0; 3]);
    fx.project.texture_sets[0].height_strength = 8.0;
    let files = fx.render_preset("Separate maps");
    let n = decode(find(&files, "_normal.png")).to_rgb8();
    let p = n.get_pixel(30, 30);
    assert!(p[0] < 120, "slope up to the right tilts the normal left: {p:?}");
    assert!(p[1] > 136, "slope up toward the image bottom tilts an OpenGL normal up: {p:?}");
    let h = decode(find(&files, "_height.png"));
    assert_eq!(h.color(), image::ColorType::L16);
    let v = h.to_luma16().get_pixel(30, 30)[0] as f32 / 65535.0;
    assert!((v - (60.0 / 160.0 * 0.5 + 0.5)).abs() < 1e-3, "height is stored as h·0.5+0.5: {v}");
    fx.project.texture_sets[0].height_strength = 0.0;
    let flat = decode(find(&fx.render_preset("Separate maps"), "_normal.png")).to_rgb8();
    assert_eq!(flat.get_pixel(30, 30).0, [128, 128, 255]);
}

#[test]
fn reoriented_normal_mapping_identities() {
    let d = Vec3::new(0.2, -0.3, 0.9).normalize();
    assert!((rnm(Vec3::Z, d) - d).length() < 1e-5);
    assert!((rnm(d, Vec3::Z) - d).length() < 1e-5);
    assert!(rnm(Vec3::new(0.0, 0.0, -1.0), d).is_finite());
}

#[test]
fn ao_mixes_baked_and_channel() {
    let mut channels = default_channels();
    channels.push(Channel::AmbientOcclusion);
    let mut fx = Fx::new(&channels);
    fx.set_all(Channel::AmbientOcclusion, |_, _| [0.5; 3]);
    fx.baked = BakedMaps { resolution: RES, maps: [(BakedMap::AmbientOcclusion, std::sync::Arc::new(vec![0.5; (RES * RES) as usize]))].into() };
    fx.project.texture_sets[0].baked.insert(BakedMap::AmbientOcclusion);
    let orm = decode(find(&fx.render_preset("glTF PBR Metal Roughness"), "Metallic.png")).to_rgb8();
    assert!((orm.get_pixel(3, 3)[0] as i32 - 64).abs() <= 1);
    let mesh_maps = fx.render_preset("Mesh maps");
    assert_eq!(mesh_maps.len(), 1, "only baked maps are exported");
    let ao = decode(&mesh_maps[0]);
    assert_eq!(mesh_maps[0].name, "sphere_Body_Set_ambient_occlusion.png");
    assert_eq!(ao.color(), image::ColorType::L16);
}

#[test]
fn templates_are_sanitized_and_collisions_suffixed() {
    let fx = Fx::new(&default_channels());
    let rough = |file: &str| MapSpec {
        file: file.into(),
        format: FileFormat::Png,
        bit_depth: 8,
        channels: vec![(Dest::L, SourceRef::Channel { channel: Channel::Roughness, component: 0 })],
    };
    let preset = ExportPreset {
        name: "t".into(),
        maps: vec![
            rough("$project/..\\$textureSet:$channel ($colorSpace)"),
            rough("$project/..\\$textureSet:$channel ($colorSpace)"),
            rough("con"),
            rough("A"),
            rough("a"),
            rough("..."),
        ],
    };
    let opts = ExportOptions::new(preset);
    let planned = list_outputs(&fx.project, 0, "sphere", &opts).unwrap();
    let names: Vec<&str> = planned.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["My_Project_.._Body_Set_roughness__linear_.png", "My_Project_.._Body_Set_roughness__linear__2.png", "_con.png", "A.png", "a_2.png", "texture.png"]
    );
    assert!(planned[0].warnings.is_empty());
    assert_eq!(planned[1].warnings.len(), 1);
    assert_eq!(planned[4].warnings.len(), 1);
    let rendered: Vec<String> = fx.render(&opts).into_iter().map(|f| f.name).collect();
    assert_eq!(rendered, names);
}

#[test]
fn padding_modes() {
    let mut fx = Fx::new(&default_channels());
    fx.coverage = (0..RES * RES).map(|i| i % RES < 32).collect();
    let rough_at = |fx: &Fx, padding: Padding, x: u32| {
        let mut opts = ExportOptions::new(builtin_preset("Separate maps").unwrap());
        opts.padding = padding;
        decode(find(&fx.render(&opts), "_roughness.png")).to_luma8().get_pixel(x, 10)[0]
    };
    assert_eq!(rough_at(&fx, Padding::None, 10), 64);
    assert_eq!(rough_at(&fx, Padding::None, 40), 0);
    assert_eq!(rough_at(&fx, Padding::Infinite, 63), 64);
    assert_eq!(rough_at(&fx, Padding::Dilation(2), 33), 64);
    assert_eq!(rough_at(&fx, Padding::Dilation(2), 40), 0);
    assert_eq!(rough_at(&fx, Padding::DilationDefault(2), 33), 64);
    assert_eq!(rough_at(&fx, Padding::DilationDefault(2), 40), 128, "roughness default is 0.5");
    let bad = Fx { coverage: vec![true; 10], ..Fx::new(&default_channels()) };
    assert!(render_maps(&bad.input(), &ExportOptions::new(builtin_preset("Separate maps").unwrap())).is_err());
}

#[test]
fn output_size_override() {
    let fx = Fx::new(&default_channels());
    for size in [32u32, 128] {
        let mut opts = ExportOptions::new(builtin_preset("Separate maps").unwrap());
        opts.size = Some(size);
        let files = fx.render(&opts);
        let r = find(&files, "_roughness.png");
        assert_eq!((r.width, r.height), (size, size));
        let img = decode(r).to_luma8();
        assert_eq!(img.dimensions(), (size, size));
        assert_eq!(img.get_pixel(size / 2, size / 3)[0], 64);
    }
    let mut opts = ExportOptions::new(builtin_preset("Separate maps").unwrap());
    opts.size = Some(48);
    assert!(render_maps(&fx.input(), &opts).is_err());
}

#[test]
fn every_format_encodes_and_decodes() {
    let mut channels = default_channels();
    channels.push(Channel::Opacity);
    let fx = Fx::new(&channels);
    let gray_alpha = MapSpec {
        file: "la".into(),
        format: FileFormat::Png,
        bit_depth: 8,
        channels: vec![(Dest::L, SourceRef::Channel { channel: Channel::Roughness, component: 0 }), (Dest::A, SourceRef::Constant(0.5))],
    };
    let mut preset = builtin_preset("glTF PBR Metal Roughness").unwrap();
    preset.maps.push(gray_alpha);
    for format in FileFormat::ALL {
        for &depth in format.bit_depths() {
            let mut opts = ExportOptions::new(preset.clone());
            opts.format_override = Some(format);
            opts.bit_depth_override = Some(depth);
            let files = fx.render(&opts);
            assert_eq!(files.len(), 4, "{format:?}");
            for f in &files {
                assert!(f.name.ends_with(format.extension()));
                if format == FileFormat::Exr && f.name.starts_with("la") {
                    continue; // Y/A EXR: covered by `exr_values_are_linear`.
                }
                assert_eq!(decode(f).dimensions(), (RES, RES), "{}", f.name);
            }
            // Roughness 0.25 everywhere survives every format (JPEG within its loss).
            let px = decode(find(&files, "OcclusionRoughnessMetallic.")).to_rgba32f().get_pixel(1, 1)[1];
            assert!((px - 0.25).abs() < 0.01, "{format:?}/{depth}: {px}");
            let base = find(&files, "_BaseColor.");
            match format {
                FileFormat::Jpeg | FileFormat::Hdr => assert!(base.warnings.iter().any(|w| w.contains("alpha")), "{format:?}"),
                _ => assert!(decode(base).color().has_alpha(), "{format:?} keeps opacity"),
            }
        }
    }
}

#[test]
fn list_outputs_matches_render_and_presets_round_trip() {
    let fx = Fx::new(&default_channels());
    for preset in builtin_presets() {
        preset.validate().unwrap();
        assert_eq!(preset_from_json(&preset_to_json(&preset).unwrap()).unwrap(), preset);
        let opts = ExportOptions::new(preset.clone());
        let planned: Vec<String> = list_outputs(&fx.project, 0, "sphere", &opts).unwrap().into_iter().map(|p| p.name).collect();
        let rendered: Vec<String> = fx.render(&opts).into_iter().map(|f| f.name).collect();
        assert_eq!(planned, rendered, "{}", preset.name);
    }
    assert!(builtin_preset("unity hdrp (mask map)").is_some());
    assert!(preset_from_json("{").is_err());
    let dup = r#"{"name":"x","maps":[{"file":"a","format":"Png","bit_depth":8,"channels":[["R",{"Constant":1.0}],["R",{"Constant":0.0}]]}]}"#;
    assert!(preset_from_json(dup).is_err());
    let mixed = r#"{"name":"x","maps":[{"file":"a","format":"Png","bit_depth":8,"channels":[["L",{"Constant":1.0}],["G",{"Constant":0.0}]]}]}"#;
    assert!(preset_from_json(mixed).is_err());
    let ok = r#"{"name":"x","maps":[{"file":"a","format":"Tga","bit_depth":8,"channels":[["L",{"Channel":{"channel":"Metallic"}}]]}]}"#;
    assert!(preset_from_json(ok).is_ok());
    assert!(list_outputs(&fx.project, 5, "m", &ExportOptions::new(builtin_presets().remove(0))).is_err());
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn write_files_is_atomic_and_refuses_paths() {
    let fx = Fx::new(&default_channels());
    let files = fx.render_preset("Unreal Engine (Packed)");
    let dir = std::env::temp_dir().join(format!("tc-formats-export-{}", std::process::id()));
    let paths = write_files(&files, &dir).unwrap();
    assert_eq!(paths.len(), 3);
    for (p, f) in paths.iter().zip(&files) {
        assert_eq!(std::fs::read(p).unwrap(), f.bytes);
    }
    assert!(std::fs::read_dir(&dir).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));
    let evil = ExportedFile { name: "../evil.png".into(), ..files[0].clone() };
    assert!(write_files(&[evil], &dir).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
