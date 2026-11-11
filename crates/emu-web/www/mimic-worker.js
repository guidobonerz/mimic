// The emulation thread: owns mimic.wasm (crates/emu-web, exports in
// src/abi.rs), runs the frames at the machine's rate, feeds the sound to
// the page's AudioWorklet through a port handed over by the page, and
// answers the page's commands. The page (mimic.js) only draws the pictures
// it is sent and collects the input.
"use strict";

const enc = new TextEncoder(), dec = new TextDecoder();
let wasm = null;

function mem() { return wasm.memory.buffer; }

function putInput(slot, bytes) {
  const p = wasm.mimic_in(slot, bytes.length);
  new Uint8Array(mem(), p, bytes.length).set(bytes);
}

function call(cmd, args = {}, data = null) {
  if (data) putInput(1, data);
  putInput(0, enc.encode(JSON.stringify({ cmd, ...args })));
  const len = wasm.mimic_call();
  const answer = JSON.parse(dec.decode(new Uint8Array(mem(), wasm.mimic_out_ptr(), len)));
  if (!answer.ok) throw new Error(answer.error);
  return answer.result;
}

// ---- the loop: frames at the machine's rate, pictures to the page -----------

let running = false, paused = false, period = 20, next = 0, fpsCount = 0, fpsAt = 0;
let audioPort = null, soundOn = true;

function feedAudio() {
  const n = wasm.mimic_audio();
  if (!n || !audioPort || !soundOn) return;
  const samples = new Float32Array(mem(), wasm.mimic_audio_ptr(), n).slice();
  audioPort.postMessage(samples, [samples.buffer]);
}

function postFrame() {
  const w = wasm.mimic_frame_width(), h = wasm.mimic_frame_height();
  if (!w || !h) return;
  const pp = wasm.mimic_crt_params_ptr();
  const params = pp ? new Float32Array(mem(), pp, 20).slice() : null;
  const rgba = new Uint8Array(mem(), wasm.mimic_frame_ptr(), w * h * 4).slice();
  postMessage({ type: "frame", w, h, rgba: rgba.buffer, params, aspect: wasm.mimic_frame_aspect(), zoom: wasm.mimic_frame_zoom() }, [rgba.buffer]);
}

function tick() {
  const now = performance.now();
  if (!running || paused) {
    next = now;
    setTimeout(tick, 50);
    return;
  }
  if (next < now - 100) next = now; // a stall: no catching up beyond a few frames
  let ran = 0;
  while (now >= next && ran < 3) {
    if (wasm.mimic_run_frame()) {
      running = false;
      postMessage({ type: "fault" });
      return;
    }
    feedAudio();
    next += period;
    ran++;
    fpsCount++;
  }
  if (ran) postFrame();
  if (now - fpsAt > 1000) {
    postMessage({ type: "stats", fps: fpsCount });
    fpsCount = 0;
    fpsAt = now;
  }
  setTimeout(tick, Math.max(0, next - performance.now()));
}

// ---- messages from the page ------------------------------------------------

onmessage = (e) => {
  const m = e.data;
  switch (m.type) {
    case "call": {
      let reply;
      try {
        const result = call(m.cmd, m.args, m.bytes ? new Uint8Array(m.bytes) : null);
        reply = { type: "reply", id: m.id, ok: true, result };
        if (m.cmd === "start") {
          period = 1000 / (result.fps || 50);
          running = true;
          paused = false;
          next = performance.now();
          postFrame();
        } else if (m.cmd === "stop") {
          running = false;
        } else if (m.cmd === "save_state") {
          const bytes = new Uint8Array(mem(), wasm.mimic_bytes_ptr(), result.bytes).slice();
          reply.bytes = bytes.buffer;
          postMessage(reply, [bytes.buffer]);
          return;
        } else if (m.cmd === "load_state" || m.cmd === "crt" || m.cmd === "overlay") {
          postFrame();
        }
      } catch (err) {
        reply = { type: "reply", id: m.id, ok: false, error: err.message };
      }
      postMessage(reply);
      break;
    }
    case "pause":
      paused = m.paused;
      break;
    case "audio":
      audioPort = m.port;
      break;
    case "sound":
      soundOn = m.on;
      break;
    case "audioClear":
      if (audioPort) audioPort.postMessage("clear");
      break;
  }
};

(async function main() {
  try {
    const r = await fetch("mimic.wasm");
    const { instance } = typeof WebAssembly.instantiateStreaming === "function"
      ? await WebAssembly.instantiateStreaming(r, {})
      : await WebAssembly.instantiate(await r.arrayBuffer(), {});
    wasm = instance.exports;
  } catch (e) {
    postMessage({ type: "loadError", error: e.message });
    return;
  }
  let version = "";
  try { version = call("version"); } catch (e) { /* older wasm */ }
  postMessage({ type: "ready", version });
  setTimeout(tick, 0);
})();
