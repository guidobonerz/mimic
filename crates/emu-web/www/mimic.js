// mimic in the browser: loads mimic.wasm (crates/emu-web) and drives it
// through its plain exports (see crates/emu-web/src/abi.rs). Nothing is
// bundled: ROMs and images come from the user's files (kept in IndexedDB
// on request, only in this browser).
"use strict";

const $ = (id) => document.getElementById(id);
const enc = new TextEncoder(), dec = new TextDecoder();
let wasm = null;

// ---- the exports ----------------------------------------------------------

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

// ---- status ----------------------------------------------------------------

function say(text, error = false) {
  const s = $("status");
  s.textContent = text;
  s.classList.toggle("error", error);
}

// ---- files (IndexedDB keeps them across visits) ------------------------------

const DB = "mimic-files";
function openDb() {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore("files");
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}
async function dbDo(mode, f) {
  try {
    const db = await openDb();
    return await new Promise((resolve, reject) => {
      const tx = db.transaction("files", mode);
      const req = f(tx.objectStore("files"));
      tx.oncomplete = () => resolve(req && req.result);
      tx.onerror = () => reject(tx.error);
    });
  } catch (e) { return undefined; } // private window, blocked storage: no persistence
}
const keepFiles = () => $("keep").checked;

async function addFile(path, bytes, persist = true) {
  call("add_file", { path }, bytes);
  if (persist && keepFiles()) await dbDo("readwrite", (s) => s.put(bytes, path));
}

async function restoreFiles() {
  const keys = await dbDo("readonly", (s) => s.getAllKeys());
  if (!keys) return;
  for (const k of keys) {
    const v = await dbDo("readonly", (s) => s.get(k));
    if (v) call("add_file", { path: k }, new Uint8Array(v));
  }
}

async function addFileList(list, relative) {
  for (const f of list) {
    const path = relative && f.webkitRelativePath ? f.webkitRelativePath : f.name;
    await addFile(path, new Uint8Array(await f.arrayBuffer()));
  }
  showFiles();
}

function showFiles() {
  const files = call("files");
  $("files").replaceChildren(...files.map((f) => {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.textContent = f.path;
    const size = document.createElement("small");
    size.textContent = `${f.kind ?? "rom/file"} · ${(f.size / 1024).toFixed(f.size < 10240 ? 1 : 0)} KB`;
    const del = document.createElement("button");
    del.textContent = "×";
    del.title = "Remove";
    del.onclick = async () => {
      call("remove_file", { path: f.path });
      await dbDo("readwrite", (s) => s.delete(f.path));
      showFiles();
    };
    li.append(name, size, del);
    return li;
  }));
  showSlots();
}

// ---- machines ---------------------------------------------------------------

let machines = [];
function showMachines() {
  machines = call("machines");
  $("machine").replaceChildren(...machines.map((m) => new Option(`${m.name} (${m.id})`, m.id)));
  try { $("machine").value = localStorage.getItem("mimic-machine") || "c64"; } catch (e) { /* no storage */ }
  showModels();
}
function showModels() {
  const m = machines.find((x) => x.id === $("machine").value);
  if (!m) return;
  $("model").replaceChildren(new Option("default model", ""), ...m.models.map((x) => new Option(x, x)));
  const needed = m.roms.filter((r) => !r.optional).map((r) => `${r.role} (${r.size} bytes)`);
  $("romneeds").textContent = needed.length
    ? `ROMs needed: ${needed.join(", ")} — add the files or a ROM folder.`
    : "No system ROM needed.";
}

function parseOptions(text) {
  const opts = {};
  for (const w of text.split(/\s+/).filter(Boolean)) {
    const [k, ...v] = w.split("=");
    if (k && v.length) opts[k] = v.join("=");
  }
  return opts;
}

let info = null, running = false, paused = false;

function start(config) {
  stopAudioQueue();
  try {
    info = call("start", { config });
  } catch (e) {
    say(e.message, true);
    return;
  }
  try { localStorage.setItem("mimic-machine", config.machine); } catch (e) { /* no storage */ }
  if ($("machine").value !== config.machine) { $("machine").value = config.machine; showModels(); }
  running = true;
  paused = false;
  for (const id of ["pause", "reset", "stop", "save", "load"]) $(id).disabled = false;
  $("pause").textContent = "Pause";
  showSlots();
  showCheats();
  if (info.cheats && info.cheats.length) say(`cheats loaded: ${info.cheats.join(", ")}`);
  showPanel(info.controls);
  wantsMouse = !!info.mouse;
  wantsPointer = !!info.pointer;
  say(`${info.machine} running`);
  $("screen").focus();
  last = performance.now();
  acc = 0;
}

function startFromForm() {
  const options = parseOptions($("options").value);
  if ($("stub").checked) options.roms = "stub";
  start({ machine: $("machine").value, model: $("model").value, options, crt: crtValue() });
}

function crtValue() { return $("crt").value; }

function stop() {
  call("stop");
  running = false;
  info = null;
  for (const id of ["pause", "reset", "stop", "save", "load"]) $(id).disabled = true;
  $("mediabox").hidden = true;
  $("cheatbox").hidden = true;
  $("panel").replaceChildren();
  say("stopped");
}

// ---- drives and slots -------------------------------------------------------

function showSlots() {
  if (!running) return;
  info = call("info");
  const files = call("files");
  $("mediabox").hidden = info.slots.length === 0;
  $("slots").replaceChildren(...info.slots.map((s) => {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.textContent = `${s.name} (${s.kind})`;
    name.style.wordBreak = "normal";
    const pick = document.createElement("select");
    pick.append(new Option("insert…", ""));
    for (const f of files) {
      if (!f.kind || f.kind === s.kind || (s.kind === "tape" && f.kind === "program") || s.kind === "cartridge") {
        pick.append(new Option(f.path, f.path));
      }
    }
    pick.onchange = () => {
      if (!pick.value) return;
      try {
        const r = call("insert", { slot: s.id, path: pick.value });
        say(`${pick.value} → ${s.name}${r.cheats ? ` (cheats: ${r.cheats})` : ""}`);
        showCheats();
      }
      catch (e) { say(e.message, true); }
      pick.value = "";
    };
    const eject = document.createElement("button");
    eject.textContent = "Eject";
    eject.onclick = () => { try { call("eject", { slot: s.id }); say(`${s.name} ejected`); } catch (e) { say(e.message, true); } };
    li.append(name, pick, eject);
    return li;
  }));
}

// A dropped file goes into the first slot of its kind.
async function dropIntoMachine(list) {
  await addFileList(list, false);
  if (!running) return;
  const files = call("files");
  for (const f of list) {
    const kind = files.find((x) => x.path === f.name)?.kind;
    const slot = info.slots.find((s) => s.kind === kind) || (kind === "program" && info.slots.find((s) => s.kind === "tape"));
    if (!slot) continue;
    try { call("insert", { slot: slot.id, path: f.name }); say(`${f.name} → ${slot.name}`); }
    catch (e) { say(e.message, true); }
  }
}

// ---- cheats ----------------------------------------------------------------

const hex = (v, digits) => "$" + v.toString(16).toUpperCase().padStart(digits, "0");
function showCheats() {
  if (!running) return;
  $("cheatbox").hidden = false;
  const list = call("cheats");
  $("cheats").replaceChildren(...list.map((c) => {
    const li = document.createElement("li");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = c.enabled;
    box.onchange = () => { call("cheat_toggle", { id: c.id, enabled: box.checked }); };
    const text = document.createElement("span");
    text.textContent = `${c.label ?? "cheat"} · ${hex(c.addr, 4)}=${c.value}`;
    const del = document.createElement("button");
    del.textContent = "×";
    del.title = "Remove";
    del.onclick = () => { call("cheat_remove", { id: c.id }); showCheats(); };
    li.append(box, text, del);
    return li;
  }));
}

function showSearch(r) {
  $("searchcount").textContent = `${r.candidates} candidate(s)`;
  const res = call("search_results", { max: 30 });
  $("searchresults").replaceChildren(...res.results.map((x) => {
    const li = document.createElement("li");
    const t = document.createElement("span");
    t.textContent = `${hex(x.addr, 4)}  ${x.value}`;
    const b = document.createElement("button");
    b.textContent = "Freeze";
    b.onclick = () => { call("search_freeze", { addr: x.addr }); showCheats(); };
    li.append(t, b);
    return li;
  }));
}

function wireCheats() {
  $("cheatimport").onclick = () => {
    try {
      const r = call("cheats_import", { format: $("cheatformat").value, text: $("cheattext").value });
      say(`imported ${r.freezes} freeze(s), ${r.patches} patch(es)`);
      showCheats();
    } catch (e) { say(e.message, true); }
  };
  $("cheatexport").onclick = () => download(`${info.machine}-cheats.toml`, enc.encode(call("cheats_export")));
  $("searchstart").onclick = () => { try { showSearch(call("search_start", { size: Number($("searchsize").value) })); } catch (e) { say(e.message, true); } };
  $("searchnext").onclick = () => {
    const v = $("searchvalue").value.trim();
    const value = v === "" ? undefined : Number(v.replace(/^\$/, "0x"));
    try { showSearch(call("search_next", { op: $("searchop").value, value })); } catch (e) { say(e.message, true); }
  };
}

// ---- console panel ---------------------------------------------------------

function showPanel(controls) {
  $("panel").replaceChildren(...controls.map((c) => {
    if (c.kind === "button") {
      const b = document.createElement("button");
      b.textContent = c.name;
      b.classList.toggle("on", c.value > 0);
      const set = (v) => showPanel(call("control", { id: c.id, value: v }));
      b.onpointerdown = () => set(1);
      b.onpointerup = b.onpointerleave = () => { if (b.classList.contains("on") || c.value > 0) set(0); };
      return b;
    }
    const wrap = document.createElement("span");
    wrap.textContent = c.name + " ";
    c.positions.forEach((p, i) => {
      const b = document.createElement("button");
      b.textContent = p;
      b.classList.toggle("on", c.value === i);
      b.onclick = () => showPanel(call("control", { id: c.id, value: i }));
      wrap.append(b);
    });
    return wrap;
  }));
}

// ---- picture ----------------------------------------------------------------

const canvas = $("screen"), ctx = canvas.getContext("2d");
let image = null;
function paint() {
  const w = wasm.mimic_frame_width(), h = wasm.mimic_frame_height();
  if (!w || !h) return;
  if (!image || image.width !== w || image.height !== h) {
    canvas.width = w;
    canvas.height = h;
    image = new ImageData(w, h);
    fit();
  }
  image.data.set(new Uint8ClampedArray(mem(), wasm.mimic_frame_ptr(), w * h * 4));
  ctx.putImageData(image, 0, 0);
}
function fit() {
  const aspect = info && info.aspect ? info.aspect : 1;
  const s = Number($("scale").value);
  canvas.style.width = `${Math.round(canvas.width * aspect * s)}px`;
  canvas.style.aspectRatio = `${canvas.width * aspect} / ${canvas.height}`;
}

// ---- sound: an AudioWorklet plays what the frames produced -------------------

let audio = null, worklet = null;
const WORKLET = `
class Feed extends AudioWorkletProcessor {
  constructor() { super(); this.q = []; this.off = 0; this.port.onmessage = (e) => {
    if (e.data === "clear") { this.q = []; this.off = 0; return; }
    this.q.push(e.data); let n = 0; for (const b of this.q) n += b.length;
    while (n > 48000 && this.q.length > 1) { n -= this.q.shift().length; this.off = 0; } }; }
  process(_, outputs) {
    const [l, r] = outputs[0];
    for (let i = 0; i < l.length; i++) {
      const b = this.q[0];
      if (!b) { l[i] = r[i] = 0; continue; }
      l[i] = b[this.off]; r[i] = b[this.off + 1]; this.off += 2;
      if (this.off >= b.length) { this.q.shift(); this.off = 0; }
    }
    return true;
  }
}
registerProcessor("feed", Feed);`;
async function startAudio() {
  if (audio || !$("sound").checked) return;
  try {
    audio = new AudioContext({ sampleRate: 48000 });
    const url = URL.createObjectURL(new Blob([WORKLET], { type: "text/javascript" }));
    await audio.audioWorklet.addModule(url);
    worklet = new AudioWorkletNode(audio, "feed", { outputChannelCount: [2] });
    worklet.connect(audio.destination);
  } catch (e) {
    audio = null;
    say("no sound in this browser: " + e.message, true);
  }
}
function feedAudio() {
  const n = wasm.mimic_audio();
  if (!n || !worklet || !$("sound").checked || audio.state !== "running") return;
  worklet.port.postMessage(new Float32Array(mem(), wasm.mimic_audio_ptr(), n).slice());
}
function stopAudioQueue() { if (worklet) worklet.port.postMessage("clear"); }

// ---- input --------------------------------------------------------------------

// Keys the browser would act on itself (scrolling, find, back) stay with the
// machine while the picture has the focus.
canvas.addEventListener("keydown", (e) => {
  if (!running || e.repeat) { if (running) e.preventDefault(); return; }
  call("key", { code: e.code, down: true });
  e.preventDefault();
});
canvas.addEventListener("keyup", (e) => {
  if (!running) return;
  call("key", { code: e.code, down: false });
  e.preventDefault();
});
canvas.addEventListener("blur", () => {
  // Release what is held, so no key sticks.
  for (const code of held) call("key", { code, down: false });
  held.clear();
});
const held = new Set();
canvas.addEventListener("keydown", (e) => held.add(e.code));
canvas.addEventListener("keyup", (e) => held.delete(e.code));

// Standard gamepad mapping (W3C): buttons 0–3 face, 4/5 shoulders, 8/9
// select/start, 12–15 d-pad; axes 0/1 left stick.
const PAD_BUTTONS = { 0: "South", 1: "East", 2: "West", 3: "North", 4: "LeftShoulder", 5: "RightShoulder", 8: "Select", 9: "Start", 12: "DPadUp", 13: "DPadDown", 14: "DPadLeft", 15: "DPadRight" };
const padState = new Map();
function pollPads() {
  if (!navigator.getGamepads) return;
  for (const pad of navigator.getGamepads()) {
    if (!pad) continue;
    const prev = padState.get(pad.index) || { buttons: [], axes: [] };
    const now = { buttons: pad.buttons.map((b) => b.pressed), axes: pad.axes.slice(0, 2).map((a) => Math.round(a * 20) / 20) };
    now.buttons.forEach((down, i) => {
      if (PAD_BUTTONS[i] && down !== !!prev.buttons[i]) call("button", { pad: pad.index, button: PAD_BUTTONS[i], down });
    });
    ["LeftX", "LeftY"].forEach((axis, i) => {
      if (now.axes[i] !== undefined && now.axes[i] !== prev.axes[i]) call("axis", { pad: pad.index, axis, value: now.axes[i] });
    });
    padState.set(pad.index, now);
  }
}

// Mouse: pointer lock while a mouse-like device is plugged in. A light pen
// or light gun aims with the pointer over the picture (as a fraction of it).
let wantsMouse = false, wantsPointer = false, mouseButtons = 0;
function framePos(e) {
  const r = canvas.getBoundingClientRect();
  const fx = (e.clientX - r.left) / r.width, fy = (e.clientY - r.top) / r.height;
  return fx >= 0 && fy >= 0 && fx < 1 && fy < 1 ? { fx, fy } : {};
}
let penButtons = 0;
canvas.addEventListener("pointermove", (e) => { if (wantsPointer && running) call("pointer", { ...framePos(e), buttons: penButtons }); });
canvas.addEventListener("pointerleave", () => { if (wantsPointer && running) call("pointer", { buttons: 0 }); });
canvas.addEventListener("pointerdown", (e) => {
  if (!wantsPointer || !running) return;
  penButtons |= e.button === 2 ? 2 : 1;
  call("pointer", { ...framePos(e), buttons: penButtons });
});
canvas.addEventListener("pointerup", (e) => {
  if (!wantsPointer || !running) return;
  penButtons &= e.button === 2 ? ~2 : ~1;
  call("pointer", { ...framePos(e), buttons: penButtons });
});
canvas.addEventListener("click", () => {
  startAudio().then(() => audio && audio.resume());
  if (wantsMouse && document.pointerLockElement !== canvas) canvas.requestPointerLock();
});
document.addEventListener("mousemove", (e) => {
  if (document.pointerLockElement === canvas && running) call("mouse", { dx: e.movementX, dy: e.movementY, buttons: mouseButtons });
});
canvas.addEventListener("mousedown", (e) => {
  if (document.pointerLockElement !== canvas) return;
  mouseButtons |= e.button === 2 ? 2 : 1;
  call("mouse", { dx: 0, dy: 0, buttons: mouseButtons });
});
canvas.addEventListener("mouseup", (e) => {
  if (document.pointerLockElement !== canvas) return;
  mouseButtons &= e.button === 2 ? ~2 : ~1;
  call("mouse", { dx: 0, dy: 0, buttons: mouseButtons });
});
canvas.addEventListener("contextmenu", (e) => { if (document.pointerLockElement === canvas) e.preventDefault(); });

// ---- the loop: frames at the machine's rate, painted per animation frame ----

let last = 0, acc = 0, fpsCount = 0, fpsAt = 0;
function loop(t) {
  requestAnimationFrame(loop);
  if (!running || paused) { last = t; return; }
  pollPads();
  const period = 1000 / (info.fps || 50);
  acc += Math.min(t - last, 100);
  last = t;
  let ran = 0;
  while (acc >= period && ran < 3) {
    if (wasm.mimic_run_frame()) { say("the machine stopped with a fault", true); running = false; return; }
    feedAudio();
    acc -= period;
    ran++;
    fpsCount++;
  }
  if (ran) paint();
  if (t - fpsAt > 1000) {
    say(`${info.machine} · ${fpsCount} fps${paused ? " · paused" : ""}`);
    fpsCount = 0;
    fpsAt = t;
  }
}

// ---- state, profiles --------------------------------------------------------

function download(name, bytes) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([bytes]));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

// A start profile's command line (from profile_args) as a start config;
// image arguments name uploaded files by their base name.
function configFromArgs(args) {
  const cfg = { machine: "", model: "", options: {}, media: [], crt: crtValue() };
  const files = call("files").map((f) => f.path);
  const file = (name) => files.find((p) => p === name || p.endsWith("/" + name.split(/[\\/]/).pop())) || name;
  for (let i = 0; i < args.length; i++) {
    const a = args[i], v = args[i + 1];
    switch (a) {
      case "--machine": cfg.machine = v; i++; break;
      case "--model": cfg.model = v; i++; break;
      case "--option": { const [k, ...r] = v.split("="); cfg.options[k] = r.join("="); i++; break; }
      case "--disk": cfg.media.push({ file: file(v), kind: "disk" }); i++; break;
      case "--tape": cfg.media.push({ file: file(v), kind: "tape" }); i++; break;
      case "--cart": cfg.media.push({ file: file(v), kind: "cartridge" }); i++; break;
      case "--prg": cfg.media.push({ file: file(v), kind: "program" }); i++; break;
      case "--crt": cfg.crt = v; i++; break;
      default: if (v && !v.startsWith("--")) i++;
    }
  }
  return cfg;
}

let profileText = "";
async function loadProfiles(file) {
  profileText = await file.text();
  const names = [...profileText.matchAll(/^  ([A-Za-z0-9_.-]+):\s*$/gm)].map((m) => m[1]);
  const sel = $("profile");
  sel.replaceChildren(new Option("start profile…", ""), ...names.map((n) => new Option(n, n)));
  sel.hidden = false;
}

// ---- wiring ------------------------------------------------------------------

function wire() {
  wireCheats();
  $("machine").onchange = showModels;
  $("start").onclick = () => { startAudio(); startFromForm(); };
  $("stop").onclick = stop;
  $("pause").onclick = () => {
    paused = !paused;
    $("pause").textContent = paused ? "Continue" : "Pause";
    $("pause").classList.toggle("on", paused);
    if (paused) stopAudioQueue();
  };
  $("reset").onclick = (e) => { call("reset", { hard: e.shiftKey }); stopAudioQueue(); };
  $("scale").onchange = fit;
  $("crt").onchange = () => { if (running) say("the monitor applies from the next start"); };
  $("addfiles").onclick = () => $("fileinput").click();
  $("addfolder").onclick = () => $("folderinput").click();
  $("fileinput").onchange = (e) => addFileList(e.target.files, false);
  $("folderinput").onchange = (e) => addFileList(e.target.files, true);
  $("save").onclick = () => {
    const r = call("save_state");
    download(`${info.machine}.mimicstate`, new Uint8Array(mem(), wasm.mimic_bytes_ptr(), r.bytes).slice());
  };
  $("load").onclick = () => $("stateinput").click();
  $("stateinput").onchange = async (e) => {
    const f = e.target.files[0];
    if (!f) return;
    try { call("load_state", {}, new Uint8Array(await f.arrayBuffer())); paint(); say("state loaded"); }
    catch (err) { say(err.message, true); }
    e.target.value = "";
  };
  $("profiles").onclick = () => $("profileinput").click();
  $("profileinput").onchange = (e) => e.target.files[0] && loadProfiles(e.target.files[0]);
  $("profile").onchange = () => {
    const name = $("profile").value;
    if (!name) return;
    try { start(configFromArgs(call("profile_args", { yaml: profileText, name }))); }
    catch (err) { say(err.message, true); }
  };
  const drop = $("drop");
  for (const [el, intoMachine] of [[drop, false], [canvas, true]]) {
    el.addEventListener("dragover", (e) => { e.preventDefault(); drop.classList.add("over"); });
    el.addEventListener("dragleave", () => drop.classList.remove("over"));
    el.addEventListener("drop", (e) => {
      e.preventDefault();
      drop.classList.remove("over");
      const list = [...e.dataTransfer.files];
      if (intoMachine) dropIntoMachine(list); else addFileList(list, false);
    });
  }
}

(async function main() {
  try {
    const r = await fetch("mimic.wasm");
    const { instance } = typeof WebAssembly.instantiateStreaming === "function"
      ? await WebAssembly.instantiateStreaming(r, {})
      : await WebAssembly.instantiate(await r.arrayBuffer(), {});
    wasm = instance.exports;
  } catch (e) {
    say("could not load mimic.wasm: " + e.message + " (build it with tools/build-web.sh and serve the folder over http)", true);
    return;
  }
  wire();
  await restoreFiles();
  showMachines();
  showFiles();
  say("choose a machine, add its ROMs (or use the built-in test ROMs) and press Start");
  requestAnimationFrame(loop);
  // ?machine=c64&model=ntsc&options=roms=stub,ram=8k&crt=raw&start=1 starts at once.
  const q = new URLSearchParams(location.search);
  if (q.get("machine")) {
    $("machine").value = q.get("machine");
    showModels();
    if (q.get("model")) $("model").value = q.get("model");
    if (q.get("options")) $("options").value = q.get("options").split(",").join(" ");
    if (q.get("crt")) $("crt").value = q.get("crt");
    if (q.get("start") === "1") startFromForm();
  }
})();
