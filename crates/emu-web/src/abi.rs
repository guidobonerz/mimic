//! Plain WebAssembly exports for `www/mimic.js` (no generated bindings).
//!
//! Data crosses the boundary through linear memory: the page asks for an
//! input buffer (`mimic_in`, slot 0 for text, slot 1 for bytes), writes
//! into it, and calls a function; results are left in the output buffer
//! (`mimic_out_ptr`, length returned by the call). Text is UTF-8 JSON:
//! `mimic_call` takes `{"cmd": …, …}` and answers `{"ok": true, "result":
//! …}` or `{"ok": false, "error": …}`. The per-frame path has its own
//! functions so no JSON is built 50 times a second.
//!
//! Pointers are handed out as numbers only; this module has no `unsafe`
//! blocks. The JavaScript side re-reads `memory.buffer` after every call,
//! since memory may have grown.

#![allow(unsafe_code)] // `#[no_mangle]` exports only (see the crate docs)

use std::cell::RefCell;

use serde_json::{json, Value};

use crate::app::WebEmu;

#[derive(Default)]
struct Abi {
    emu: WebEmu,
    input: [Vec<u8>; 2],
    out: Vec<u8>,
    audio: Vec<f32>,
}

thread_local! {
    static ABI: RefCell<Abi> = RefCell::new(Abi::default());
}

fn with<R>(f: impl FnOnce(&mut Abi) -> R) -> R {
    ABI.with(|a| f(&mut a.borrow_mut()))
}

/// Make input buffer `slot` (0 text, 1 bytes) `len` bytes long; its address.
#[no_mangle]
pub extern "C" fn mimic_in(slot: u32, len: u32) -> usize {
    with(|a| {
        let buf = &mut a.input[(slot & 1) as usize];
        buf.clear();
        buf.resize(len as usize, 0);
        buf.as_mut_ptr() as usize
    })
}

/// Address of the output buffer of the last call.
#[no_mangle]
pub extern "C" fn mimic_out_ptr() -> usize {
    with(|a| a.out.as_ptr() as usize)
}

/// Run the JSON command in input slot 0 (bytes in slot 1 where a command
/// takes them); the answer's length (it is in the output buffer).
#[no_mangle]
pub extern "C" fn mimic_call() -> u32 {
    with(|a| {
        let answer = match serde_json::from_slice::<Value>(&a.input[0]) {
            Ok(req) => match dispatch(a, &req) {
                Ok(result) => json!({"ok": true, "result": result}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            Err(e) => json!({"ok": false, "error": format!("bad request: {e}")}),
        };
        a.out = answer.to_string().into_bytes();
        a.out.len() as u32
    })
}

/// Bytes a command left besides its JSON (`save_state`): moved into the
/// output buffer; their length.
fn bytes_out(a: &mut Abi, data: Vec<u8>) -> Value {
    let len = data.len();
    a.input[1] = data;
    json!({"bytes": len})
}

fn dispatch(a: &mut Abi, req: &Value) -> Result<Value, String> {
    let s = |k: &str| req[k].as_str().unwrap_or_default().to_string();
    let n = |k: &str| req[k].as_u64().unwrap_or(0);
    match req["cmd"].as_str().unwrap_or_default() {
        "machines" => Ok(a.emu.machines()),
        "files" => Ok(a.emu.files()),
        "add_file" => {
            let data = std::mem::take(&mut a.input[1]);
            a.emu.add_file(&s("path"), data);
            Ok(a.emu.files())
        }
        "remove_file" => {
            a.emu.remove_file(&s("path"));
            Ok(a.emu.files())
        }
        "start" => a.emu.start(&req["config"]),
        "stop" => {
            a.emu.stop();
            Ok(Value::Null)
        }
        "info" => Ok(a.emu.info()),
        "key" => {
            a.emu
                .key(&s("code"), req["down"].as_bool().unwrap_or(false));
            Ok(Value::Null)
        }
        "button" => {
            let ok = a.emu.button(
                n("pad") as u8,
                &s("button"),
                req["down"].as_bool().unwrap_or(false),
            );
            Ok(json!(ok))
        }
        "axis" => {
            let v = req["value"].as_f64().unwrap_or(0.0) as f32;
            Ok(json!(a.emu.axis(n("pad") as u8, &s("axis"), v)))
        }
        "mouse" => {
            let dx = req["dx"].as_i64().unwrap_or(0).clamp(-32768, 32767) as i16;
            let dy = req["dy"].as_i64().unwrap_or(0).clamp(-32768, 32767) as i16;
            a.emu.mouse(dx, dy, n("buttons") as u8);
            Ok(Value::Null)
        }
        "pointer" => {
            let pos = match (req["fx"].as_f64(), req["fy"].as_f64()) {
                (Some(x), Some(y)) => Some((x, y)),
                _ => None,
            };
            a.emu.pointer(pos, n("buttons") as u8);
            Ok(Value::Null)
        }
        "control" => {
            a.emu.control(n("id") as u16, n("value") as u8);
            Ok(a.emu.info()["controls"].clone())
        }
        "insert" => a
            .emu
            .insert(n("slot") as u8, &s("path"))
            .map(|g| json!({"cheats": g})),
        "cheats" => Ok(a.emu.cheats()),
        "cheats_import" => a.emu.cheats_import(&s("format"), &s("text")),
        "cheat_toggle" => Ok(json!(a
            .emu
            .cheat_toggle(n("id") as u32, req["enabled"].as_bool().unwrap_or(true)))),
        "cheat_remove" => Ok(json!(a.emu.cheat_remove(n("id") as u32))),
        "cheats_export" => a.emu.cheats_export().map(|t| json!(t)),
        "search_start" => a.emu.search_start(req["size"].as_u64().unwrap_or(1) as u8),
        "search_next" => {
            let value = req["value"].as_u64().map(|v| v as u32);
            a.emu.search_next(&s("op"), value)
        }
        "search_results" => Ok(a
            .emu
            .search_results(req["max"].as_u64().unwrap_or(50) as usize)),
        "search_freeze" => {
            let label = req["label"].as_str().map(str::to_string);
            a.emu.search_freeze(n("addr") as u32, label)
        }
        "eject" => a.emu.eject(n("slot") as u8).map(|()| Value::Null),
        "reset" => {
            a.emu.reset(req["hard"].as_bool().unwrap_or(false));
            Ok(Value::Null)
        }
        // The state goes to input slot 1, from where `mimic_bytes_ptr` reads.
        "save_state" => {
            let data = a.emu.save_state().ok_or("no machine running")?;
            Ok(bytes_out(a, data))
        }
        "load_state" => {
            let data = std::mem::take(&mut a.input[1]);
            a.emu.load_state(&data).map(|()| Value::Null)
        }
        "profile_args" => crate::profile_args(&s("yaml"), &s("name")).map(|v| json!(v)),
        other => Err(format!("unknown command '{other}'")),
    }
}

/// Address of the bytes a command returned (`save_state`), in slot 1.
#[no_mangle]
pub extern "C" fn mimic_bytes_ptr() -> usize {
    with(|a| a.input[1].as_ptr() as usize)
}

/// Emulate one frame: 0 = fine, 1 = no machine or a fault (the reason
/// via `mimic_call {"cmd": "info"}` / the console).
#[no_mangle]
pub extern "C" fn mimic_run_frame() -> u32 {
    with(|a| u32::from(a.emu.run_frame().is_err()))
}

/// The picture: RGBA bytes, `width × height`.
#[no_mangle]
pub extern "C" fn mimic_frame_ptr() -> usize {
    with(|a| a.emu.frame().map_or(0, |(p, _, _)| p.as_ptr() as usize))
}

#[no_mangle]
pub extern "C" fn mimic_frame_width() -> u32 {
    with(|a| a.emu.frame().map_or(0, |(_, w, _)| w))
}

#[no_mangle]
pub extern "C" fn mimic_frame_height() -> u32 {
    with(|a| a.emu.frame().map_or(0, |(_, _, h)| h))
}

/// Take the sound since the last call: number of f32 values (stereo,
/// interleaved, 48 kHz) at `mimic_audio_ptr`.
#[no_mangle]
pub extern "C" fn mimic_audio() -> u32 {
    with(|a| {
        a.audio = a.emu.take_audio();
        a.audio.len() as u32
    })
}

#[no_mangle]
pub extern "C" fn mimic_audio_ptr() -> usize {
    with(|a| a.audio.as_ptr() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(req: Value) -> Value {
        let text = req.to_string();
        let _ = mimic_in(0, text.len() as u32);
        with(|a| a.input[0].copy_from_slice(text.as_bytes()));
        let len = mimic_call() as usize;
        with(|a| serde_json::from_slice(&a.out[..len]).unwrap())
    }

    #[test]
    fn the_exports_drive_a_machine() {
        let r = call(json!({"cmd": "machines"}));
        assert_eq!(r["ok"], true);
        let r = call(
            json!({"cmd": "start", "config": {"machine": "c64", "options": {"roms": "stub"}}}),
        );
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(mimic_run_frame(), 0);
        assert_eq!((mimic_frame_width(), mimic_frame_height()), (384, 272));
        assert_ne!(mimic_frame_ptr(), 0);
        assert!(mimic_audio() > 0);
        let r = call(json!({"cmd": "save_state"}));
        let len = r["result"]["bytes"].as_u64().unwrap() as usize;
        assert!(len > 1000);
        let r = call(json!({"cmd": "load_state"}));
        assert_eq!(r["ok"], true, "slot 1 still holds the state: {r}");
        assert_eq!(call(json!({"cmd": "fly"}))["ok"], false);
        let _ = mimic_in(0, 3);
        with(|a| a.input[0].copy_from_slice(b"{{{"));
        let len = mimic_call() as usize;
        assert!(with(
            |a| a.out[..len].starts_with(b"{\"error\"") || a.out[..len].starts_with(b"{\"ok\"")
        ));
    }
}
