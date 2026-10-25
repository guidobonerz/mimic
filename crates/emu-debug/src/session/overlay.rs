//! Debug overlays and pictures (`overlay_list`, `overlay_set`,
//! `overlay_get`, `frame`, the optional `stopped_frame` event): the machine
//! describes, `emu_preview` draws on the raw frame.

use serde_json::{json, Value};

use emu_core::overlay::{OverlayWants, Shape};
use emu_core::ScreenId;
use emu_preview::overlay::{OverlaySet, BEAM, GRID};
use emu_preview::{Filter, Format, OwnedFrame};

use super::Session;
use crate::protocol::base64_encode;

fn rgb(c: emu_core::frame::Rgb8) -> Value {
    json!([c.r, c.g, c.b])
}

impl Session {
    /// Share the overlay wishes with the live preview, so the machine
    /// records what either side wants.
    pub fn set_overlay_wants(&mut self, wants: OverlayWants) {
        wants.want(&self.dbg_overlays, true);
        self.overlay_wants.want(&self.dbg_overlays, false);
        self.overlay_wants = wants;
        self.apply_overlays();
    }

    pub(super) fn apply_overlays(&mut self) {
        self.overlay_wants.apply(self.machine.as_mut());
    }

    pub(super) fn overlay_list(&self) -> Value {
        let mut list = vec![
            json!({"id": BEAM, "name": "beam position", "generic": true}),
            json!({"id": GRID, "name": "raster grid (every 8 lines)", "generic": true}),
        ];
        list.extend(
            self.machine
                .overlays()
                .iter()
                .map(|o| json!({"id": o.id, "name": o.name, "generic": false})),
        );
        json!({"overlays": list, "active": self.dbg_overlays})
    }

    /// The debugger's overlays (replacing its earlier choice); recorded
    /// from the next emulated cycle on.
    pub(super) fn overlay_set(&mut self, ids: Vec<String>) -> Result<Value, String> {
        let known: Vec<&str> = self.machine.overlays().iter().map(|o| o.id).collect();
        if let Some(bad) = ids
            .iter()
            .find(|i| *i != BEAM && *i != GRID && !known.contains(&i.as_str()))
        {
            return Err(format!("no overlay '{bad}'"));
        }
        self.overlay_wants.want(&self.dbg_overlays, false);
        self.overlay_wants.want(&ids, true);
        self.dbg_overlays = ids;
        self.apply_overlays();
        Ok(self.overlay_list())
    }

    pub(super) fn overlay_get(&self, id: &str) -> Result<Value, String> {
        if id == BEAM || id == GRID {
            let set = OverlaySet::collect(self.machine.as_ref(), &[id.to_string()]);
            let grid: Vec<Value> = set
                .grid
                .iter()
                .map(|(row, line)| json!({"row": row, "line": line}))
                .collect();
            return Ok(json!({
                "id": id,
                "beam": set.beam.map(|(row, col)| json!({"row": row, "col": col})),
                "grid": grid,
            }));
        }
        let o = self
            .machine
            .overlay(id)
            .ok_or_else(|| format!("no overlay '{id}' (or not switched on with overlay_set)"))?;
        let shapes: Vec<Value> = o
            .shapes
            .iter()
            .map(|s| match s {
                Shape::Rect {
                    x,
                    y,
                    w,
                    h,
                    color,
                    label,
                } => json!({
                    "kind": "rect", "x": x, "y": y, "w": w, "h": h,
                    "color": rgb(*color), "label": label,
                }),
                Shape::Band { y, h, color, label } => json!({
                    "kind": "band", "y": y, "h": h, "color": rgb(*color), "label": label,
                }),
                Shape::Mark { x, y, color, label } => json!({
                    "kind": "mark", "x": x, "y": y, "color": rgb(*color), "label": label,
                }),
                Shape::Cells {
                    x,
                    y,
                    cell_w,
                    cell_h,
                    cols,
                    rows,
                    cells,
                } => json!({
                    "kind": "cells", "x": x, "y": y, "cell_w": cell_w, "cell_h": cell_h,
                    "cols": cols, "rows": rows, "cells": base64_encode(cells),
                }),
            })
            .collect();
        let legend: Vec<Value> = o
            .legend
            .iter()
            .map(|e| json!({"color": rgb(e.color), "text": e.text}))
            .collect();
        Ok(json!({"id": o.id, "name": o.name, "shapes": shapes, "legend": legend}))
    }

    /// The current (possibly partial) picture of `screen` as PNG/QOI, with
    /// `overlays` drawn on the raw frame (main screen only).
    pub(super) fn frame_image(
        &self,
        screen: Option<&str>,
        format: Option<&str>,
        scale: Option<u32>,
        overlays: &[String],
    ) -> Result<Value, String> {
        let m = self.machine.as_ref();
        let (id, main) = match screen {
            None => (ScreenId::MAIN, true),
            Some(name) => {
                let list = m.screens();
                let s = list
                    .iter()
                    .position(|s| s.name == name)
                    .ok_or_else(|| format!("no screen '{name}'"))?;
                (list[s].id, s == 0)
            }
        };
        let fmt = match format.unwrap_or("png") {
            "png" => Format::Png,
            "qoi" => Format::Qoi,
            other => return Err(format!("unknown format '{other}' (png, qoi)")),
        };
        let frame = OwnedFrame::from_screen(m, id, self.running).ok_or("no such screen")?;
        let set = (main && !overlays.is_empty()).then(|| OverlaySet::collect(m, overlays));
        let scale = scale.unwrap_or(1).clamp(1, 8);
        let img = Filter::Raw { scale }.render_with(&frame, set.as_ref());
        Ok(json!({
            "width": img.width, "height": img.height, "scale": scale,
            "format": format.unwrap_or("png"),
            "image": base64_encode(&fmt.encode(&img)),
            "beam": crate::protocol::beam_json(m.beam()),
            "partial": !self.running,
        }))
    }

    /// The `stopped_frame` event: the picture at the stop with the beam
    /// marked and the debugger's overlays.
    pub(super) fn stopped_frame_event(&self) -> Option<Value> {
        let mut ids = self.dbg_overlays.clone();
        if !ids.iter().any(|i| i == BEAM) {
            ids.push(BEAM.to_string());
        }
        let mut v = self.frame_image(None, Some("png"), Some(1), &ids).ok()?;
        v["event"] = json!("stopped_frame");
        Some(v)
    }
}
