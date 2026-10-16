//! Frame timing for performance work: what each frame spent (UI build, scene sync =
//! compositing + display tile conversion), and a scripted stroke that paints through the real
//! frame loop so the numbers match what a user feels. Driven by the control channel
//! (`debug.paintTest`, `debug.frames`).

use std::collections::VecDeque;

use serde_json::{Value, json};
use texturecraft_engine::geom::glam::Vec2;

use crate::TextureCraftApp;

/// Frames kept for statistics.
const KEEP: usize = 600;

#[derive(Clone, Copy, Default)]
struct FrameStat {
    /// Time since the previous frame's UI finished (the real frame interval).
    interval_ms: f32,
    ui_ms: f32,
    sync_ms: f32,
    /// Input, shortcuts, control requests and scripted strokes before the UI is built.
    logic_ms: f32,
    /// The scripted stroke ended (was committed) in this frame.
    stroke_end: bool,
}

impl FrameStat {
    fn json(&self) -> Value {
        json!({ "intervalMs": self.interval_ms, "uiMs": self.ui_ms, "syncMs": self.sync_ms, "logicMs": self.logic_ms })
    }
}

#[derive(Default)]
pub struct Profiler {
    frames: VecDeque<FrameStat>,
    last_end: Option<f64>,
    /// Scene sync time of the frame being built.
    pub sync_ms: f32,
    /// Time in `App::logic` this frame.
    pub logic_ms: f32,
    paint: Option<PaintTest>,
    /// The scripted stroke ended in the frame being built.
    stroke_end: bool,
}

/// A stroke from `from` to `to` (viewport pixels) over `frames` frames.
struct PaintTest {
    from: Vec2,
    to: Vec2,
    frames: u32,
    i: u32,
}

impl Profiler {
    /// Record the frame that just finished building.
    pub fn end_frame(&mut self, now: f64, ui_ms: f32) {
        let interval_ms = self.last_end.map_or(0.0, |t| (now - t) as f32);
        self.last_end = Some(now);
        if self.frames.len() == KEEP {
            self.frames.pop_front();
        }
        self.frames.push_back(FrameStat {
            interval_ms,
            ui_ms,
            sync_ms: std::mem::take(&mut self.sync_ms),
            logic_ms: std::mem::take(&mut self.logic_ms),
            stroke_end: std::mem::take(&mut self.stroke_end),
        });
    }

    /// Averages and worst cases since the last call (then cleared).
    pub fn report(&mut self) -> Value {
        let frames: Vec<FrameStat> = self.frames.drain(..).skip(1).collect();
        let stat = |f: fn(&FrameStat) -> f32| {
            let mut v: Vec<f32> = frames.iter().map(f).collect();
            v.sort_by(f32::total_cmp);
            let avg = v.iter().sum::<f32>() / v.len().max(1) as f32;
            let p95 = v.get(v.len() * 95 / 100).copied().unwrap_or(0.0);
            json!({ "avg": avg, "p95": p95, "max": v.last().copied().unwrap_or(0.0) })
        };
        json!({
            "frames": frames.len(),
            "intervalMs": stat(|f| f.interval_ms),
            "uiMs": stat(|f| f.ui_ms),
            "syncMs": stat(|f| f.sync_ms),
            "logicMs": stat(|f| f.logic_ms),
            // The slowest frame's breakdown: what to look at first.
            "worst": frames.iter().max_by(|a, b| a.interval_ms.total_cmp(&b.interval_ms)).map(FrameStat::json),
            // The frame that committed the scripted stroke and the next one (whose interval
            // includes presenting the commit frame).
            "strokeEnd": frames.iter().position(|f| f.stroke_end).map(|i| frames.iter().skip(i).take(2).map(FrameStat::json).collect::<Vec<_>>()),
            "painting": self.paint.is_some(),
        })
    }
}

/// `debug.paintTest {from: [x, y], to: [x, y], frames?}`: start a scripted stroke.
pub fn start_paint_test(app: &mut TextureCraftApp, p: &Value) -> Result<(), String> {
    let pt = |k: &str| -> Result<Vec2, String> {
        let a = p.get(k).and_then(Value::as_array).ok_or(format!("`{k}` must be [x, y]"))?;
        let n = |i: usize| a.get(i).and_then(Value::as_f64).map(|v| v as f32).ok_or(format!("`{k}` must be [x, y]"));
        Ok(Vec2::new(n(0)?, n(1)?))
    };
    let frames = p.get("frames").and_then(Value::as_u64).unwrap_or(120).clamp(2, 10_000) as u32;
    app.profiler.frames.clear();
    app.profiler.paint = Some(PaintTest { from: pt("from")?, to: pt("to")?, frames, i: 0 });
    Ok(())
}

/// Advance the scripted stroke by one frame (called from the frame loop). Like a pointer
/// release, the frame after the last point commits the stroke.
pub fn drive(app: &mut TextureCraftApp, ctx: &egui::Context) {
    let Some(t) = app.profiler.paint.as_mut() else { return };
    let (i, frames) = (t.i, t.frames);
    let p = t.from + (t.to - t.from) * (i as f32 / (frames - 1) as f32);
    t.i += 1;
    let s = &mut app.session;
    if i == 0 {
        let vp = s.viewport;
        if let Err(e) = s.begin_stroke(p, vp, 1.0) {
            log::warn!("paint test: {e}");
            app.profiler.paint = None;
            return;
        }
    } else if i < frames {
        s.continue_stroke(p, 1.0);
    } else {
        if let Err(e) = s.end_stroke() {
            log::warn!("paint test: {e}");
        }
        app.profiler.paint = None;
        app.profiler.stroke_end = true;
    }
    ctx.request_repaint();
}
