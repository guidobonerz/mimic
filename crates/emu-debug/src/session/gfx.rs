//! Graphics data views (`gfx_list`, `gfx_get`): what the machine offers
//! through `Machine::gfx_views`, decoded by the machine, encoded here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use emu_core::gfx::{GfxOptions, GfxPixels};
use emu_preview::Format;

use super::Session;
use crate::protocol::base64_encode;

impl Session {
    pub(super) fn gfx_list(&self) -> Value {
        let views: Vec<Value> = self
            .machine
            .gfx_views()
            .iter()
            .map(|v| {
                let options: Vec<Value> = v
                    .options
                    .iter()
                    .map(|o| json!({"name": o.name, "help": o.help}))
                    .collect();
                json!({"id": v.id, "kind": v.kind.name(), "name": v.name, "options": options})
            })
            .collect();
        json!(views)
    }

    pub(super) fn gfx_get(
        &self,
        id: &str,
        format: Option<&str>,
        scale: Option<u32>,
        options: &BTreeMap<String, u32>,
    ) -> Result<Value, String> {
        let mut opts = GfxOptions::new();
        for (k, &v) in options {
            opts.set(k, v);
        }
        let view = self
            .machine
            .gfx_view(id, &opts)
            .ok_or_else(|| format!("no graphics view '{id}'"))?;
        let params: serde_json::Map<String, Value> = view
            .params
            .iter()
            .map(|&(k, v)| (k.to_string(), json!(v)))
            .collect();
        let mut out = json!({
            "id": view.id, "kind": view.kind.name(), "name": view.name,
            "space": view.space.0, "addr": view.addr,
            "width": view.width, "height": view.height,
            "params": params, "raw": base64_encode(&view.raw),
        });
        let format = format.unwrap_or("png");
        match format {
            "png" | "qoi" => {
                let f = if format == "png" {
                    Format::Png
                } else {
                    Format::Qoi
                };
                let scale = scale.unwrap_or(1).clamp(1, 8);
                out["format"] = json!(format);
                out["scale"] = json!(scale);
                out["image"] = json!(base64_encode(&emu_preview::encode_gfx(&view, f, scale)));
            }
            "indexed" => match &view.pixels {
                GfxPixels::Indexed { pixels, palette } => {
                    let pal: Vec<[u8; 3]> = palette.iter().map(|c| [c.r, c.g, c.b]).collect();
                    out["format"] = json!("indexed");
                    out["pixels"] = json!(base64_encode(pixels));
                    out["palette"] = json!(pal);
                }
                GfxPixels::Rgb(p) => {
                    let bytes: Vec<u8> = p.iter().flat_map(|c| [c.r, c.g, c.b]).collect();
                    out["format"] = json!("rgb");
                    out["pixels"] = json!(base64_encode(&bytes));
                }
            },
            other => return Err(format!("unknown format '{other}' (png, qoi, indexed)")),
        }
        Ok(out)
    }
}
