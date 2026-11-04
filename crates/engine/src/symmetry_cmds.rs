//! Symmetry commands: the mirror shortcut and the "Show symmetry guides" switch. The mirror axes
//! and radial copies themselves are brush settings (`tool.brush`); this module is the quick
//! toggle the viewport bar, the keyboard and agents share.

use serde_json::{Value, json};

use crate::commands::{CommandSpec, always, bad, boolp, strp};
use crate::{Result, Session, cmd};

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "tool.symmetry",
        "Toggle mirror symmetry (X)",
        ["Tools"],
        Some("X"),
        "{axis?: x|y|z (default x), enabled?: bool (toggles when omitted), guides?: bool (show the symmetry guides on the model)}",
        always,
        toggle
    )]
}

fn toggle(s: &mut Session, p: &Value) -> Result<Value> {
    let axis = strp(p, "axis").unwrap_or("x").to_ascii_lowercase();
    let sym = &mut s.tool.brush.symmetry;
    let slot = match axis.as_str() {
        "x" => &mut sym.x,
        "y" => &mut sym.y,
        "z" => &mut sym.z,
        other => return Err(bad("tool.symmetry", format!("unknown axis `{other}` (x, y or z)"))),
    };
    *slot = boolp(p, "enabled").unwrap_or(!*slot);
    if let Some(g) = boolp(p, "guides") {
        s.tool.show_symmetry_guides = g;
    }
    s.revision = s.revision.wrapping_add(1);
    let mut out = crate::tool_cmds::brush_json(s);
    out["symmetryGuides"] = json!(s.tool.show_symmetry_guides);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute;
    use texturecraft_geom::primitives;

    fn session() -> Session {
        Session::new_project("t", primitives::cube(1.0), "builtin:cube", None, 128).unwrap()
    }

    #[test]
    fn toggles_any_combination_of_axes() {
        let mut s = session();
        assert!(!s.tool.brush.symmetry.any());
        let r = execute(&mut s, "tool.symmetry", &json!({})).unwrap();
        assert_eq!(r["symmetry"], json!([true, false, false]));
        execute(&mut s, "tool.symmetry", &json!({"axis": "Z"})).unwrap();
        execute(&mut s, "tool.symmetry", &json!({"axis": "y", "enabled": true})).unwrap();
        assert_eq!((s.tool.brush.symmetry.x, s.tool.brush.symmetry.y, s.tool.brush.symmetry.z), (true, true, true));
        execute(&mut s, "tool.symmetry", &json!({})).unwrap();
        assert!(!s.tool.brush.symmetry.x && s.tool.brush.symmetry.y);
        assert!(execute(&mut s, "tool.symmetry", &json!({"axis": "w"})).is_err());
    }

    #[test]
    fn guides_switch_shows_in_inspect() {
        let mut s = session();
        assert!(s.tool.show_symmetry_guides);
        execute(&mut s, "tool.symmetry", &json!({"axis": "x", "guides": false})).unwrap();
        let v = execute(&mut s, "project.inspect", &json!({})).unwrap();
        assert_eq!(v["tool"]["symmetryGuides"], json!(false));
        assert_eq!(v["tool"]["brush"]["symmetry"], json!([true, false, false]));
    }

    #[test]
    fn x_shortcut_is_used_once() {
        let n = crate::command_specs().iter().filter(|c| c.shortcut == Some("X")).count();
        assert_eq!(n, 1);
    }
}
