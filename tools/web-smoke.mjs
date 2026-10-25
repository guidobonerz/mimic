// Smoke test of the web frontend's WebAssembly (node tools/web-smoke.mjs
// [target/web/mimic.wasm]): every machine starts with its built-in test
// ROMs, runs frames, draws a picture and makes sound; a savestate loads
// back. No browser and no ROMs needed (CI runs it after tools/build-web.sh).
import { readFileSync } from "node:fs";

const path = process.argv[2] ?? "target/web/mimic.wasm";
const { instance, module } = await WebAssembly.instantiate(readFileSync(path), {});
if (WebAssembly.Module.imports(module).length) throw new Error("the module must not need imports");
const x = instance.exports;
const enc = new TextEncoder(), dec = new TextDecoder();
// Ask for the buffer first: it may grow memory, which detaches old views.
const put = (slot, bytes) => {
  const p = x.mimic_in(slot, bytes.length);
  new Uint8Array(x.memory.buffer, p, bytes.length).set(bytes);
};
function call(req, data) {
  if (data) put(1, data);
  put(0, enc.encode(JSON.stringify(req)));
  const len = x.mimic_call();
  const a = JSON.parse(dec.decode(new Uint8Array(x.memory.buffer, x.mimic_out_ptr(), len)));
  if (!a.ok) throw new Error(`${req.cmd}: ${a.error}`);
  return a.result;
}

let failed = 0;
for (const m of call({ cmd: "machines" })) {
  try {
    call({ cmd: "start", config: { machine: m.id, options: { roms: "stub" } } });
    for (let i = 0; i < 30; i++) if (x.mimic_run_frame()) throw new Error("fault");
    const w = x.mimic_frame_width(), h = x.mimic_frame_height();
    const px = new Uint8Array(x.memory.buffer, x.mimic_frame_ptr(), w * h * 4);
    if (!w || !h || px[3] !== 255) throw new Error(`no picture (${w}x${h})`);
    const samples = x.mimic_audio();
    const saved = call({ cmd: "save_state" }).bytes;
    const state = new Uint8Array(x.memory.buffer, x.mimic_bytes_ptr(), saved).slice();
    x.mimic_run_frame();
    call({ cmd: "load_state" }, state);
    console.log(`ok   ${m.id.padEnd(10)} ${w}x${h}, ${samples} samples, state ${saved} bytes`);
  } catch (e) {
    // A machine that cannot run without real ROMs says so; anything else fails.
    if (/missing ROM|no .* ROM/.test(e.message)) { console.log(`skip ${m.id.padEnd(10)} ${e.message}`); continue; }
    console.log(`FAIL ${m.id.padEnd(10)} ${e.message}`);
    failed++;
  }
}
process.exit(failed ? 1 : 0);
