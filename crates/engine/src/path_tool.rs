//! The Path tool: control points placed on the surface (3D view) or in texture space (2D view)
//! and a smooth curve through them, committed as one brush stroke, or, when closed and filled,
//! as an antialiased lasso fill of the visible surface (3D) or of the UV region (2D).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use texturecraft_geom::glam::{Vec2, Vec3};
use texturecraft_paint::{Lasso, fill_coverage, spline};

use crate::commands::{CommandSpec, always, bad, boolp};
use crate::session::Tool;
use crate::tool_cmds::{points_param, set_tool, viewport_of};
use crate::{EngineError, Result, Session, cmd};

/// Control points kept at most.
pub const MAX_PATH_POINTS: usize = 4096;
/// Curve sampling step in viewport pixels (3D view) and texture units (2D view).
const STEP_PX: f32 = 2.0;
const STEP_UV: f32 = 0.001;

/// The curve being edited with the Path tool.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PathState {
    /// Control points: surface positions (3D view) or texture coordinates with z = 0 (2D view).
    pub points: Vec<Vec3>,
    /// The points are texture coordinates (placed in the 2D view).
    pub uv: bool,
    /// The curve loops back to its first point.
    pub closed: bool,
    /// A closed path fills its interior instead of stroking the curve.
    pub fill: bool,
}

impl PathState {
    /// Append a control point; a point placed in the other view starts a new path.
    pub fn push(&mut self, p: Vec3, uv: bool) {
        if uv != self.uv {
            self.points.clear();
            self.uv = uv;
        }
        if self.points.len() < MAX_PATH_POINTS {
            self.points.push(p);
        }
    }
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "tool.path",
        "Path",
        ["Tools"],
        Some("7"),
        "{points?: [[x,y],...] viewport pixels on the mesh | uvPoints?: [[u,v],...], viewport?: [w,h], closed?, fill?, commit?: bool, clear?: bool} — selects the Path tool and edits its curve; commit paints the curve with the current brush, or fills a closed, filled path",
        always,
        path
    )]
}

fn path(s: &mut Session, p: &Value) -> Result<Value> {
    const CMD: &str = "tool.path";
    if s.tool.tool != Tool::Path {
        set_tool(s, Tool::Path)?;
    }
    let vp = viewport_of(s, p);
    if boolp(p, "clear") == Some(true) {
        s.tool.path.points.clear();
    }
    let placed = match (points_param(CMD, p, "points")?, points_param(CMD, p, "uvPoints")?) {
        (Some(pts), _) => Some((
            pts.iter()
                .enumerate()
                .map(|(i, q)| s.pick(*q, vp).map(|h| h.pos).ok_or_else(|| bad(CMD, format!("point {i} misses the mesh"))))
                .collect::<Result<Vec<_>>>()?,
            false,
        )),
        (None, Some(pts)) => Some((pts.iter().map(|q| q.clamp(Vec2::ZERO, Vec2::ONE).extend(0.0)).collect(), true)),
        (None, None) => None,
    };
    if let Some((points, uv)) = placed {
        if points.len() > MAX_PATH_POINTS {
            return Err(bad(CMD, format!("at most {MAX_PATH_POINTS} control points")));
        }
        s.tool.path.points = points;
        s.tool.path.uv = uv;
    }
    if let Some(v) = boolp(p, "closed") {
        s.tool.path.closed = v;
    }
    if let Some(v) = boolp(p, "fill") {
        s.tool.path.fill = v;
    }
    s.revision = s.revision.wrapping_add(1);
    let committed = if boolp(p, "commit") == Some(true) {
        let r = s.commit_path(vp)?;
        s.update();
        r
    } else {
        Value::Null
    };
    let st = &s.tool.path;
    Ok(json!({ "points": st.points.len(), "uv": st.uv, "closed": st.closed, "fill": st.fill, "committed": committed }))
}

/// NDC (y up) to viewport pixels (origin top-left).
fn to_pixels(ndc: Vec2, viewport: Vec2) -> Vec2 {
    Vec2::new((ndc.x * 0.5 + 0.5) * viewport.x, (0.5 - ndc.y * 0.5) * viewport.y)
}

impl Session {
    /// The path's control points in view space: viewport pixels for a 3D path (None behind the
    /// camera), texture coordinates for a 2D one.
    pub fn path_view_points(&self, viewport: Vec2) -> Vec<Option<Vec2>> {
        let path = &self.tool.path;
        if path.uv {
            return path.points.iter().map(|p| Some(p.truncate())).collect();
        }
        let aspect = viewport.x.max(1.0) / viewport.y.max(1.0);
        path.points.iter().map(|p| self.camera.project(*p, aspect).map(|n| to_pixels(n.truncate(), viewport))).collect()
    }

    /// Paint the path with the current brush (or fill it when closed and filled), then clear it.
    pub fn commit_path(&mut self, viewport: Vec2) -> Result<Value> {
        let (uv, closed) = (self.tool.path.uv, self.tool.path.closed);
        let fill = closed && self.tool.path.fill;
        let pts: Vec<Vec2> = self.path_view_points(viewport).into_iter().flatten().collect();
        if pts.is_empty() {
            return Err(EngineError::Other("the path has no points: click the surface to add some".into()));
        }
        if fill && pts.len() < 3 {
            return Err(EngineError::Other("a filled path needs at least three points".into()));
        }
        let curve = spline(&pts, closed, if uv { STEP_UV } else { STEP_PX });
        let out = if fill {
            json!({ "filledTiles": self.lasso_fill(curve, uv, viewport)? })
        } else if uv {
            json!({ "tiles": self.stroke_uv_points(&curve)? })
        } else {
            json!({ "stamps": self.stroke_points(&curve, viewport)? })
        };
        self.tool.path.points.clear();
        Ok(out)
    }

    /// Fill the inside of `poly`: viewport pixels over the visible surface, or texture
    /// coordinates. Returns the touched tiles.
    fn lasso_fill(&mut self, poly: Vec<Vec2>, uv: bool, viewport: Vec2) -> Result<usize> {
        let ts = self.ts();
        let (gbuf, mut canvas) = self.fill_canvas(ts)?;
        let w = gbuf.width.max(1);
        if uv {
            let lasso = Lasso::new(poly.iter().map(|p| *p * w as f32).collect());
            fill_coverage(&mut canvas, &gbuf, |i, _| lasso.coverage(Vec2::new((i as u32 % w) as f32 + 0.5, (i as u32 / w) as f32 + 0.5)));
        } else {
            let lasso = Lasso::new(poly);
            let view_proj = self.camera.view_proj(viewport.x.max(1.0) / viewport.y.max(1.0));
            let inv = view_proj.inverse();
            let (cull, mesh, bvh) = (self.tool.brush.backface_culling, &self.mesh, &self.bvh);
            fill_coverage(&mut canvas, &gbuf, |i, _| {
                let Some(s) = gbuf.surface(mesh, i) else { return 0.0 };
                let c = view_proj * s.pos.extend(1.0);
                if c.w <= 1e-6 {
                    return 0.0;
                }
                let ndc = Vec2::new(c.x, c.y) / c.w;
                let a = lasso.coverage(to_pixels(ndc, viewport));
                if a <= 0.0 {
                    return 0.0;
                }
                // Exact visibility (a depth buffer flickers on grazing surfaces): nothing between
                // the near plane and the texel.
                let o = inv * ndc.extend(0.0).extend(1.0);
                let origin = o.truncate() / o.w;
                let to = s.pos - origin;
                let d = to.length();
                let dir = to / d.max(1e-9);
                let hidden = (cull && s.nrm.dot(dir) >= 0.0) || bvh.closest(origin, dir, d * 0.999).is_some();
                if hidden { 0.0 } else { a }
            });
        }
        let tiles = canvas.dirty.len();
        self.commit_fill("Path fill", ts, &canvas)?;
        Ok(tiles)
    }
}
