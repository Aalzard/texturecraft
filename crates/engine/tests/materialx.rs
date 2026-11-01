//! `file.exportTextures` with `materialX: true`: a `.mtlx` OpenPBR material next to the textures.

use serde_json::{Value, json};
use texturecraft_engine::geom::primitives;
use texturecraft_engine::{Session, execute};

const OPENPBR: &str = "OpenPBR (separate maps)";

fn temp(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("tc_mtlx_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn export(s: &mut Session, dir: &std::path::Path, preset: &str, mtlx: bool) -> Value {
    execute(s, "file.exportTextures", &json!({"dir": dir.display().to_string(), "preset": preset, "materialX": mtlx})).unwrap()
}

fn files(r: &Value) -> Vec<String> {
    r["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect()
}

#[test]
fn writes_a_material_that_names_the_exported_textures() {
    let mut s = Session::new_project("t", primitives::material_ball(), "builtin:material_ball", None, 128).unwrap();
    let dir = temp("ball");
    let r = export(&mut s, &dir, OPENPBR, true);
    let all = files(&r);
    let mtlx: Vec<&String> = all.iter().filter(|f| f.ends_with(".mtlx")).collect();
    assert_eq!(mtlx.len(), s.set_count(), "one material per texture set: {all:?}");
    for path in mtlx {
        let set = path.rsplit(['/', '\\']).next().unwrap().trim_end_matches(".mtlx").to_string();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("<open_pbr_surface") && text.contains("<surfacematerial"), "{text}");
        // Every texture the document points at was written beside it, and belongs to its set.
        let mut refs = 0;
        for part in text.split("name=\"file\" type=\"filename\" value=\"").skip(1) {
            let file = part.split('"').next().unwrap();
            assert!(dir.join(file).is_file(), "{file} is not in {all:?}");
            assert!(file.starts_with(&set), "{file} is not a texture of {set}");
            refs += 1;
        }
        assert!(refs >= 3, "base colour, roughness and normal at least: {text}");
        assert!(text.contains("name=\"base_color\"") && text.contains("name=\"geometry_normal\""), "{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn off_by_default_and_warns_without_openpbr_maps() {
    let mut s = Session::new_project("t", primitives::material_ball(), "builtin:material_ball", None, 128).unwrap();
    let dir = temp("off");
    assert!(!files(&export(&mut s, &dir, OPENPBR, false)).iter().any(|f| f.ends_with(".mtlx")));
    let _ = std::fs::remove_dir_all(&dir);
    let dir = temp("other");
    let r = export(&mut s, &dir, "Document channels + Normal + AO", true);
    assert!(!files(&r).iter().any(|f| f.ends_with(".mtlx")));
    assert!(r["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("OpenPBR")), "{r}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn udim_sets_use_the_udim_token() {
    let mut s = Session::new_project("t", primitives::udim_plane(), "builtin:udim_plane", None, 128).unwrap();
    let dir = temp("udim");
    let r = export(&mut s, &dir, OPENPBR, true);
    let all = files(&r);
    let mtlx = all.iter().find(|f| f.ends_with(".mtlx")).unwrap_or_else(|| panic!("no .mtlx in {all:?}"));
    let text = std::fs::read_to_string(mtlx).unwrap();
    assert!(text.contains(".&lt;UDIM&gt;.png"), "{text}");
    assert!(!text.contains(".1001.") && !text.contains(".1002."), "{text}");
    assert!(all.iter().any(|f| f.ends_with(".1001.png")) && all.iter().any(|f| f.ends_with(".1002.png")));
    let _ = std::fs::remove_dir_all(&dir);
}
