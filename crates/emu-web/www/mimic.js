// mimic in the browser: the page. The emulation runs in a Web Worker
// (mimic-worker.js, which loads mimic.wasm from crates/emu-web and drives
// it through its plain exports, see crates/emu-web/src/abi.rs); the page
// sends it commands and input and draws the pictures it gets back. Nothing
// is bundled: ROMs and images come from the user's files (kept in
// IndexedDB on request, only in this browser).
"use strict";

const $ = (id) => document.getElementById(id);
const enc = new TextEncoder();

// ---- the worker: every command is a message, every answer a promise --------

const worker = new Worker("mimic-worker.js");
let callId = 0;
const pending = new Map();
let latestFrame = null, newFrame = false;

function call(cmd, args = {}, data = null) {
  return new Promise((resolve, reject) => {
    const id = ++callId;
    pending.set(id, { resolve, reject });
    const msg = { type: "call", id, cmd, args };
    if (data) {
      // The bytes move to the worker (their buffer is not used again here).
      msg.bytes = data.buffer.byteLength === data.byteLength ? data.buffer : data.slice().buffer;
      worker.postMessage(msg, [msg.bytes]);
    } else {
      worker.postMessage(msg);
    }
  });
}
// A command whose answer nobody waits for (input events): errors reach the status line.
function send(cmd, args = {}) { call(cmd, args).catch((e) => say(e.message, true)); }

let workerReady = null;
worker.onmessage = (e) => {
  const m = e.data;
  switch (m.type) {
    case "reply": {
      const p = pending.get(m.id);
      pending.delete(m.id);
      if (!p) break;
      if (m.ok) p.resolve(m.bytes ? { ...m.result, data: new Uint8Array(m.bytes) } : m.result);
      else p.reject(new Error(m.error));
      break;
    }
    case "frame":
      latestFrame = { w: m.w, h: m.h, rgba: new Uint8Array(m.rgba), params: m.params, aspect: m.aspect, zoom: m.zoom };
      newFrame = true;
      break;
    case "stats":
      if (running) say(`${info.machine} · ${m.fps} fps${paused ? " · paused" : ""}`);
      break;
    case "fault":
      say("the machine stopped with a fault", true);
      running = false;
      break;
    case "ready":
      workerReady.resolve(m.version);
      break;
    case "loadError":
      workerReady.reject(new Error(m.error));
      break;
  }
};

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
  if (persist && keepFiles()) await dbDo("readwrite", (s) => s.put(bytes, path));
  await call("add_file", { path }, bytes);
}

async function restoreFiles() {
  const keys = await dbDo("readonly", (s) => s.getAllKeys());
  if (!keys) return;
  for (const k of keys) {
    const v = await dbDo("readonly", (s) => s.get(k));
    if (v) await call("add_file", { path: k }, new Uint8Array(v));
  }
}

async function addFileList(list, relative) {
  for (const f of list) {
    const path = relative && f.webkitRelativePath ? f.webkitRelativePath : f.name;
    await addFile(path, new Uint8Array(await f.arrayBuffer()));
  }
  await showFiles();
}

async function showFiles() {
  const files = await call("files");
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
      await call("remove_file", { path: f.path });
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
async function showMachines() {
  machines = await call("machines");
  $("machine").replaceChildren(...machines.map((m) => new Option(`${m.name} (${m.id})`, m.id)));
  try { $("machine").value = localStorage.getItem("mimic-machine") || "c64"; } catch (e) { /* no storage */ }
  showModels();
  loadProfiles(null); // the built-in profiles, before any profile file
}
function showModels() {
  const m = machines.find((x) => x.id === $("machine").value);
  if (!m) return;
  $("model").replaceChildren(new Option("default model", ""), ...m.models.map((x) => new Option(x, x)));
  $("model").hidden = !m.models.length;
  $("videoformat").replaceChildren(new Option("default video", ""), ...m.videoFormats.map((x) => new Option(x.toUpperCase(), x)));
  $("videoformat").hidden = !m.videoFormats.length;
  const needed = m.roms.filter((r) => !r.optional).map((r) => `${r.role} (${r.size} bytes)`);
  const optional = m.roms.filter((r) => r.optional).map((r) => r.role);
  $("romneeds").textContent = (needed.length
    ? `ROMs needed: ${needed.join(", ")} — add the files or a ROM folder.`
    : "No system ROM needed.")
    + (optional.length ? ` Optional: ${optional.join(", ")} (without a drive DOS ROM the drive is virtual: no fast loaders, no disk-change detection).` : "");
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

async function start(config) {
  stopAudioQueue();
  try {
    info = await call("start", { config });
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
}

function startFromForm() {
  const options = parseOptions($("options").value);
  if ($("stub").checked) options.roms = "stub";
  start({ machine: $("machine").value, model: $("model").value, videoFormat: $("videoformat").value, options, crt: crtValue() });
}

function crtValue() { return $("crt").value; }

async function stop() {
  await call("stop");
  running = false;
  info = null;
  for (const id of ["pause", "reset", "stop", "save", "load"]) $(id).disabled = true;
  $("mediabox").hidden = true;
  $("cheatbox").hidden = true;
  $("panel").replaceChildren();
  say("stopped");
}

// ---- drives and slots -------------------------------------------------------

async function showSlots() {
  if (!running) return;
  info = await call("info");
  $("mediabox").hidden = info.slots.length === 0;
  // One line per drive or slot, as in the desktop's media panel: its name,
  // the image in it, and one button that mounts (green, a file dialog) or
  // ejects (red).
  $("slots").replaceChildren(...info.slots.map((s) => {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.textContent = s.name;
    name.style.wordBreak = "normal";
    name.style.flex = "0 0 auto";
    const file = document.createElement("small");
    file.textContent = s.inserted ? (s.file ?? "(inserted)") : "empty";
    file.title = file.textContent;
    Object.assign(file.style, { flex: "1", minWidth: "0", overflow: "hidden", textOverflow: "ellipsis" });
    const toggle = document.createElement("button");
    toggle.className = s.inserted ? "eject" : "mount";
    toggle.textContent = s.inserted ? "Eject" : "Mount";
    toggle.onclick = () => (s.inserted ? ejectSlot(s) : mountSlot(s));
    li.append(name, file, toggle);
    return li;
  }));
}

async function ejectSlot(s) {
  try {
    await call("eject", { slot: s.id });
    say(`${s.name} ejected`);
  }
  catch (e) { say(e.message, true); }
  showSlots();
}

// The file dialog for a slot: the chosen image is kept with the other files
// (and in the browser's storage) and goes into the slot.
function mountSlot(s) {
  const input = document.createElement("input");
  input.type = "file";
  input.onchange = async () => {
    const f = input.files[0];
    if (!f) return;
    await addFileList([f], false);
    try {
      const r = await call("insert", { slot: s.id, path: f.name });
      say(`${f.name} → ${s.name}${r.cheats ? ` (cheats: ${r.cheats})` : ""}`);
      showCheats();
    }
    catch (e) { say(e.message, true); }
    showSlots();
  };
  input.click();
}

// A dropped file goes into the first slot of its kind.
async function dropIntoMachine(list) {
  await addFileList(list, false);
  if (!running) return;
  const files = await call("files");
  for (const f of list) {
    const kind = files.find((x) => x.path === f.name)?.kind;
    const slot = info.slots.find((s) => s.kind === kind) || (kind === "program" && info.slots.find((s) => s.kind === "tape"));
    if (!slot) continue;
    try { await call("insert", { slot: slot.id, path: f.name }); say(`${f.name} → ${slot.name}`); }
    catch (e) { say(e.message, true); }
  }
  showSlots();
}

// ---- cheats ----------------------------------------------------------------

const hex = (v, digits) => "$" + v.toString(16).toUpperCase().padStart(digits, "0");
async function showCheats() {
  if (!running) return;
  $("cheatbox").hidden = false;
  const list = await call("cheats");
  $("cheats").replaceChildren(...list.map((c) => {
    const li = document.createElement("li");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = c.enabled;
    box.onchange = () => send("cheat_toggle", { id: c.id, enabled: box.checked });
    const text = document.createElement("span");
    text.textContent = `${c.label ?? "cheat"} · ${hex(c.addr, 4)}=${c.value}`;
    const del = document.createElement("button");
    del.textContent = "×";
    del.title = "Remove";
    del.onclick = async () => { await call("cheat_remove", { id: c.id }); showCheats(); };
    li.append(box, text, del);
    return li;
  }));
}

async function showSearch(r) {
  $("searchcount").textContent = `${r.candidates} candidate(s)`;
  const res = await call("search_results", { max: 30 });
  $("searchresults").replaceChildren(...res.results.map((x) => {
    const li = document.createElement("li");
    const t = document.createElement("span");
    t.textContent = `${hex(x.addr, 4)}  ${x.value}`;
    const b = document.createElement("button");
    b.textContent = "Freeze";
    b.onclick = async () => { await call("search_freeze", { addr: x.addr }); showCheats(); };
    li.append(t, b);
    return li;
  }));
}

function wireCheats() {
  $("cheatimport").onclick = async () => {
    try {
      const r = await call("cheats_import", { format: $("cheatformat").value, text: $("cheattext").value });
      say(`imported ${r.freezes} freeze(s), ${r.patches} patch(es)`);
      showCheats();
    } catch (e) { say(e.message, true); }
  };
  $("cheatexport").onclick = async () => download(`${info.machine}-cheats.toml`, enc.encode(await call("cheats_export")));
  $("searchstart").onclick = async () => { try { showSearch(await call("search_start", { size: Number($("searchsize").value) })); } catch (e) { say(e.message, true); } };
  $("searchnext").onclick = async () => {
    const v = $("searchvalue").value.trim();
    const value = v === "" ? undefined : Number(v.replace(/^\$/, "0x"));
    try { showSearch(await call("search_next", { op: $("searchop").value, value })); } catch (e) { say(e.message, true); }
  };
}

// ---- console panel ---------------------------------------------------------

function showPanel(controls) {
  $("panel").replaceChildren(...controls.map((c) => {
    if (c.kind === "button") {
      const b = document.createElement("button");
      b.textContent = c.name;
      b.classList.toggle("on", c.value > 0);
      const set = async (v) => showPanel(await call("control", { id: c.id, value: v }));
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
      b.onclick = async () => showPanel(await call("control", { id: c.id, value: i }));
      wrap.append(b);
    });
    return wrap;
  }));
}

// ---- picture ----------------------------------------------------------------

const canvas = $("screen");
// WebGL2 draws the picture: raw pixels through a pass-through shader, a
// monitor through the CRT stage (the GLSL port of video-crt's crt.wgsl,
// same model as its CPU path; the signal stage runs in the wasm). Without
// WebGL2 the 2D canvas shows the wasm's own CPU rendering.
const gl = canvas.getContext("webgl2", { alpha: false, antialias: false, preserveDrawingBuffer: false });
const ctx = gl ? null : canvas.getContext("2d");
let image = null, shape = "";

const VERTEX = `#version 300 es
void main() {
  // Full-screen triangle.
  vec2 uv = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(uv * 2.0 - 1.0, 0.0, 1.0);
}`;
const RAW_FRAGMENT = `#version 300 es
precision highp float;
uniform sampler2D source;
uniform vec2 out_size;
out vec4 color;
void main() {
  // Rows top-down, as the picture is stored.
  vec2 p = vec2(gl_FragCoord.x, out_size.y - gl_FragCoord.y);
  color = texelFetch(source, ivec2(floor(p)), 0);
}`;
// params: see video_crt::gpu::params (20 floats as 5 vec4):
// [0] src_size.xy out_size.zw, [1] tint.xyz scanlines.w,
// [2] gamma.x persistence.y origin.zw, [3] mask_kind.x mask_strength.y
// mask_pitch.z bloom.w, [4] curvature.x vignette.y.
const CRT_FRAGMENT = `#version 300 es
precision highp float;
uniform sampler2D source;
uniform sampler2D previous;
uniform vec4 u[5];
out vec4 color;

vec3 fetch(int x, int y) {
  ivec2 m = ivec2(u[0].xy) - ivec2(1, 1);
  return texelFetch(source, clamp(ivec2(x, y), ivec2(0, 0), m), 0).rgb;
}

// Glow around a source pixel: centre 0.2, +-1 0.15, +-2 0.1, lines above
// and below 0.15 (GLOW in cpu.rs).
vec3 glow(int x, int y) {
  return 0.2 * fetch(x, y)
    + 0.15 * (fetch(x - 1, y) + fetch(x + 1, y))
    + 0.1 * (fetch(x - 2, y) + fetch(x + 2, y))
    + 0.15 * (fetch(x, y - 1) + fetch(x, y + 1));
}

// Per-channel mask factors (gain included) of the screen pixel centred at p.
vec3 mask(vec2 p) {
  float s = clamp(u[3].y, 0.0, 1.0);
  int kind = int(u[3].x + 0.5);
  if (kind == 0 || s == 0.0) return vec3(1.0);
  float pitch = max(u[3].z, 1.0);
  float shift = 0.0;
  float gap = 1.0;
  if (kind == 1) {
    float row = floor(p.y / (pitch * 0.5));
    if (row - 2.0 * floor(row * 0.5) >= 1.0) shift = 0.5;
  } else if (kind == 3) {
    float h = pitch * 2.0;
    float row = floor(p.y / h);
    if (row - 2.0 * floor(row * 0.5) >= 1.0) shift = 0.5;
    if (p.y / h - row > 0.875) gap = 1.0 - s;
  }
  float uu = p.x / pitch + shift;
  float t = uu - floor(uu);
  vec3 centre = (vec3(0.0, 1.0, 2.0) + 0.5) / 3.0;
  vec3 d0 = abs(vec3(t) - centre);
  vec3 d = min(d0, vec3(1.0) - d0) * 3.0;
  vec3 w = clamp(vec3(1.0) - d, vec3(0.0), vec3(1.0));
  vec3 m = vec3(1.0) - s * (vec3(1.0) - w);
  return m * (gap / (1.0 - s / 3.0));
}

void main() {
  vec2 src_size = u[0].xy, out_size = u[0].zw;
  // Rows top-down, as in the WGSL version.
  vec2 screen = vec2(gl_FragCoord.x, out_size.y - gl_FragCoord.y) - u[2].zw;
  vec2 p = screen;
  vec2 c = p * 2.0 / out_size - 1.0;
  vec3 lit = vec3(0.0);
  bool visible = true;
  float k = clamp(u[4].x, 0.0, 0.25);
  if (k > 0.0) {
    c = c * ((1.0 + k * dot(c, c)) / (1.0 + k));
    if (abs(c.x) > 1.0 || abs(c.y) > 1.0) visible = false;
    p = (c + 1.0) * 0.5 * out_size;
  }
  if (visible) {
    float scale_y = out_size.y / src_size.y;
    float ly = p.y / scale_y;
    int sy = int(floor(ly));
    // Centre-aligned linear resampling along the line.
    float pos = max(p.x * src_size.x / out_size.x - 0.5, 0.0);
    int x0 = int(floor(pos));
    float t = pos - floor(pos);
    vec3 col = mix(fetch(x0, sy), fetch(x0 + 1, sy), t);
    // 0 at the beam centre, 1 in the gap between lines.
    float offset = 0.0;
    if (scale_y > 1.0) offset = abs(fract(ly) * 2.0 - 1.0);
    float luma = dot(col, vec3(0.299, 0.587, 0.114));
    float beam = 0.5 + 0.5 * luma;
    float falloff = min(offset / max(beam, 0.01), 1.0);
    lit = col * (1.0 - u[1].w * falloff * falloff);
    float bloom = clamp(u[3].w, 0.0, 1.0);
    if (bloom > 0.0) {
      vec3 g = mix(glow(x0, sy), glow(x0 + 1, sy), t);
      lit = lit + bloom * g * g;
    }
    lit = lit * u[1].xyz * mask(screen);
    float vignette = clamp(u[4].y, 0.0, 1.0);
    if (vignette > 0.0) lit = lit * max(1.0 - vignette * 0.5 * dot(c, c), 0.0);
  }
  vec3 o = pow(clamp(lit, vec3(0.0), vec3(1.0)), vec3(1.0 / max(u[2].x, 0.01)));
  float keep = clamp(u[2].y, 0.0, 1.0);
  if (keep > 0.0) {
    // The last output, copied from the canvas in its own orientation.
    vec3 prev = texelFetch(previous, ivec2(gl_FragCoord.xy), 0).rgb;
    o = max(o, prev * keep);
  }
  color = vec4(o, 1.0);
}`;

let glState = null;
function compile(type, src) {
  const sh = gl.createShader(type);
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(sh));
  return sh;
}
function program(fragment) {
  const p = gl.createProgram();
  gl.attachShader(p, compile(gl.VERTEX_SHADER, VERTEX));
  gl.attachShader(p, compile(gl.FRAGMENT_SHADER, fragment));
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p));
  return p;
}
function texture() {
  const t = gl.createTexture();
  gl.bindTexture(gl.TEXTURE_2D, t);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  return t;
}
function initGl() {
  const raw = program(RAW_FRAGMENT), crt = program(CRT_FRAGMENT);
  glState = {
    raw, crt,
    rawSource: gl.getUniformLocation(raw, "source"),
    rawOut: gl.getUniformLocation(raw, "out_size"),
    crtSource: gl.getUniformLocation(crt, "source"),
    crtPrevious: gl.getUniformLocation(crt, "previous"),
    crtU: gl.getUniformLocation(crt, "u"),
    source: texture(), previous: texture(),
    sourceSize: "", previousSize: "",
  };
  gl.bindVertexArray(gl.createVertexArray());
  gl.disable(gl.DEPTH_TEST);
  gl.disable(gl.BLEND);
}

function paint() {
  const f = latestFrame;
  if (!f) return;
  newFrame = false;
  const { w, h } = f;
  const params = glState ? f.params : null;
  const cw = params ? Math.round(params[2]) : w, ch = params ? Math.round(params[3]) : h;
  if (canvas.width !== cw || canvas.height !== ch) {
    canvas.width = cw;
    canvas.height = ch;
    image = null;
  }
  // Raw pixels stay sharp; a CRT picture is smoothed when scaled.
  canvas.style.imageRendering = params ? "auto" : "pixelated";
  // The pixel aspect can change while the machine runs (resolution,
  // pixel clock, the monitor): the displayed size follows every frame.
  const now = `${cw}x${ch}:${f.aspect}:${f.zoom}`;
  if (now !== shape) {
    shape = now;
    fit();
  }
  const pixels = f.rgba;
  if (!glState) {
    if (!image) image = new ImageData(w, h);
    image.data.set(pixels);
    ctx.putImageData(image, 0, 0);
    return;
  }
  const g = glState;
  gl.activeTexture(gl.TEXTURE0);
  gl.bindTexture(gl.TEXTURE_2D, g.source);
  const size = `${w}x${h}`;
  if (g.sourceSize !== size) {
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, w, h, 0, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
    g.sourceSize = size;
  } else {
    gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, w, h, gl.RGBA, gl.UNSIGNED_BYTE, pixels);
  }
  gl.viewport(0, 0, cw, ch);
  if (!params) {
    gl.useProgram(g.raw);
    gl.uniform1i(g.rawSource, 0);
    gl.uniform2f(g.rawOut, cw, ch);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
    return;
  }
  // Persistence reads the last output: kept as a copy of the canvas.
  gl.activeTexture(gl.TEXTURE1);
  gl.bindTexture(gl.TEXTURE_2D, g.previous);
  const osize = `${cw}x${ch}`;
  if (g.previousSize !== osize) {
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA8, cw, ch, 0, gl.RGBA, gl.UNSIGNED_BYTE, null);
    g.previousSize = osize;
  }
  gl.useProgram(g.crt);
  gl.uniform1i(g.crtSource, 0);
  gl.uniform1i(g.crtPrevious, 1);
  gl.uniform4fv(g.crtU, params);
  gl.drawArrays(gl.TRIANGLES, 0, 3);
  if (params[9] > 0) gl.copyTexSubImage2D(gl.TEXTURE_2D, 0, 0, 0, 0, 0, cw, ch);
}
function fit() {
  const aspect = latestFrame ? latestFrame.aspect || 1 : 1;
  const zoom = latestFrame ? latestFrame.zoom || 1 : 1;
  const s = Number($("scale").value);
  canvas.style.width = `${Math.round(canvas.width * aspect * zoom * s)}px`;
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
    // The worker feeds the worklet directly.
    worker.postMessage({ type: "audio", port: worklet.port }, [worklet.port]);
    worker.postMessage({ type: "sound", on: $("sound").checked });
  } catch (e) {
    audio = null;
    say("no sound in this browser: " + e.message, true);
  }
}
function stopAudioQueue() { worker.postMessage({ type: "audioClear" }); }

// ---- input --------------------------------------------------------------------

// Keys the browser would act on itself (scrolling, find, back) stay with the
// machine while the picture has the focus.
canvas.addEventListener("keydown", (e) => {
  if (!running || e.repeat) { if (running) e.preventDefault(); return; }
  send("key", { code: e.code, down: true });
  e.preventDefault();
});
canvas.addEventListener("keyup", (e) => {
  if (!running) return;
  send("key", { code: e.code, down: false });
  e.preventDefault();
});
canvas.addEventListener("blur", () => {
  // Release what is held, so no key sticks.
  for (const code of held) send("key", { code, down: false });
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
      if (PAD_BUTTONS[i] && down !== !!prev.buttons[i]) send("button", { pad: pad.index, button: PAD_BUTTONS[i], down });
    });
    ["LeftX", "LeftY"].forEach((axis, i) => {
      if (now.axes[i] !== undefined && now.axes[i] !== prev.axes[i]) send("axis", { pad: pad.index, axis, value: now.axes[i] });
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
canvas.addEventListener("pointermove", (e) => { if (wantsPointer && running) send("pointer", { ...framePos(e), buttons: penButtons }); });
canvas.addEventListener("pointerleave", () => { if (wantsPointer && running) send("pointer", { buttons: 0 }); });
canvas.addEventListener("pointerdown", (e) => {
  if (!wantsPointer || !running) return;
  penButtons |= e.button === 2 ? 2 : 1;
  send("pointer", { ...framePos(e), buttons: penButtons });
});
canvas.addEventListener("pointerup", (e) => {
  if (!wantsPointer || !running) return;
  penButtons &= e.button === 2 ? ~2 : ~1;
  send("pointer", { ...framePos(e), buttons: penButtons });
});
canvas.addEventListener("click", () => {
  startAudio().then(() => audio && audio.resume());
  if (wantsMouse && document.pointerLockElement !== canvas) canvas.requestPointerLock();
});
document.addEventListener("mousemove", (e) => {
  if (document.pointerLockElement === canvas && running) send("mouse", { dx: e.movementX, dy: e.movementY, buttons: mouseButtons });
});
canvas.addEventListener("mousedown", (e) => {
  if (document.pointerLockElement !== canvas) return;
  mouseButtons |= e.button === 2 ? 2 : 1;
  send("mouse", { dx: 0, dy: 0, buttons: mouseButtons });
});
canvas.addEventListener("mouseup", (e) => {
  if (document.pointerLockElement !== canvas) return;
  mouseButtons &= e.button === 2 ? ~2 : ~1;
  send("mouse", { dx: 0, dy: 0, buttons: mouseButtons });
});
canvas.addEventListener("contextmenu", (e) => { if (document.pointerLockElement === canvas) e.preventDefault(); });

// ---- the loop: frames at the machine's rate, painted per animation frame ----

function loop() {
  requestAnimationFrame(loop);
  if (newFrame) paint();
  if (running && !paused) pollPads();
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
async function configFromArgs(args) {
  const cfg = { machine: "", model: "", videoFormat: "", options: {}, media: [], crt: crtValue() };
  const files = (await call("files")).map((f) => f.path);
  const file = (name) => files.find((p) => p === name || p.endsWith("/" + name.split(/[\\/]/).pop())) || name;
  for (let i = 0; i < args.length; i++) {
    const a = args[i], v = args[i + 1];
    switch (a) {
      case "--machine": cfg.machine = v; i++; break;
      case "--model": cfg.model = v; i++; break;
      case "--videoformat": cfg.videoFormat = v === "auto" ? "" : v; i++; break;
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
  profileText = file ? await file.text() : "";
  // The file's profiles and the built-in ones (the Cogitatron 16).
  const names = await call("profile_names", { yaml: profileText });
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
    worker.postMessage({ type: "pause", paused });
    if (paused) { stopAudioQueue(); say(`${info.machine} · paused`); }
  };
  $("reset").onclick = (e) => { send("reset", { hard: e.shiftKey }); stopAudioQueue(); };
  $("scale").onchange = fit;
  $("sound").onchange = () => worker.postMessage({ type: "sound", on: $("sound").checked });
  $("crt").onchange = async () => {
    if (!running) return;
    try { await call("crt", { name: crtValue() }); }
    catch (e) { say(e.message, true); }
  };
  $("addfiles").onclick = () => $("fileinput").click();
  $("addfolder").onclick = () => $("folderinput").click();
  $("fileinput").onchange = (e) => addFileList(e.target.files, false);
  $("folderinput").onchange = (e) => addFileList(e.target.files, true);
  $("save").onclick = async () => {
    const r = await call("save_state");
    download(`${info.machine}.mimicstate`, r.data);
  };
  $("load").onclick = () => $("stateinput").click();
  $("stateinput").onchange = async (e) => {
    const f = e.target.files[0];
    if (!f) return;
    try { await call("load_state", {}, new Uint8Array(await f.arrayBuffer())); say("state loaded"); }
    catch (err) { say(err.message, true); }
    e.target.value = "";
  };
  $("profiles").onclick = () => $("profileinput").click();
  $("profileinput").onchange = (e) => e.target.files[0] && loadProfiles(e.target.files[0]);
  $("profile").onchange = async () => {
    const name = $("profile").value;
    if (!name) return;
    try { start(await configFromArgs(await call("profile_args", { yaml: profileText, name }))); }
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
  let version;
  try {
    version = await new Promise((resolve, reject) => { workerReady = { resolve, reject }; });
  } catch (e) {
    say("could not load mimic.wasm: " + e.message + " (build it with tools/build-web.sh and serve the folder over http)", true);
    return;
  }
  wire();
  if (version) $("version").textContent = `mimic ${version}`;
  if (gl) {
    try { initGl(); await call("gpu", { on: true }); }
    catch (e) { glState = null; say("WebGL failed, drawing on the CPU: " + e.message, true); }
  }
  await restoreFiles();
  await showMachines();
  await showFiles();
  say("choose a machine, add its ROMs (or use the built-in test ROMs) and press Start");
  requestAnimationFrame(loop);
  // ?machine=c64&videoformat=ntsc&options=roms=stub,ram=8k&crt=raw&start=1 starts at once.
  const q = new URLSearchParams(location.search);
  if (q.get("machine")) {
    $("machine").value = q.get("machine");
    showModels();
    if (q.get("model")) $("model").value = q.get("model");
    if (q.get("videoformat")) $("videoformat").value = q.get("videoformat");
    if (q.get("options")) $("options").value = q.get("options").split(",").join(" ");
    if (q.get("crt")) $("crt").value = q.get("crt");
    if (q.get("start") === "1") startFromForm();
  }
})();
