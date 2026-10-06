// Browser services for the EffectCraft web app (imported by the wasm module through
// wasm-bindgen; see src/persist.rs, src/audio.rs and src/worker.rs):
//
// - storage: the Origin Private File System, or IndexedDB where OPFS can't write;
// - audio: a Web Audio output (AudioWorklet, ScriptProcessorNode fallback) fed stereo blocks;
// - workers: a pool of Web Workers, each running its own instance of the engine.
//
// Plain functions only: the Rust side owns the logic.

// ------------------------------------------------------------------ storage

const DIR = "effectcraft";
const DB = "effectcraft";
const STORE = "entries";
let backend = null;

// OPFS file names can't hold "/": keys are percent-encoded.
const enc = (k) => encodeURIComponent(k);
const dec = (n) => decodeURIComponent(n);

async function opfs() {
  if (!navigator.storage || !navigator.storage.getDirectory) throw new Error("no OPFS");
  const root = await navigator.storage.getDirectory();
  const dir = await root.getDirectoryHandle(DIR, { create: true });
  // Probe a write: some browsers expose OPFS but only write it from workers.
  const probe = await dir.getFileHandle(".probe", { create: true });
  if (!probe.createWritable) throw new Error("OPFS is read-only on this thread");
  const w = await probe.createWritable();
  await w.write(new Uint8Array([1]));
  await w.close();
  await dir.removeEntry(".probe");
  return {
    name: "opfs",
    async loadAll() {
      const out = [];
      for await (const [name, h] of dir.entries()) {
        if (h.kind !== "file" || name.startsWith(".") || name.endsWith(".crswap")) continue;
        const f = await h.getFile();
        out.push({ key: dec(name), data: new Uint8Array(await f.arrayBuffer()), modified: f.lastModified });
      }
      return out;
    },
    async put(key, data) {
      // createWritable writes a swap file and replaces the entry on close: atomic.
      const h = await dir.getFileHandle(enc(key), { create: true });
      const w = await h.createWritable();
      await w.write(data);
      await w.close();
    },
    async del(key) {
      try { await dir.removeEntry(enc(key)); } catch (e) { if (e.name !== "NotFoundError") throw e; }
    },
  };
}

function req(r) {
  return new Promise((res, rej) => { r.onsuccess = () => res(r.result); r.onerror = () => rej(r.error); });
}

async function idb() {
  if (!self.indexedDB) throw new Error("no IndexedDB");
  const open = indexedDB.open(DB, 1);
  open.onupgradeneeded = () => open.result.createObjectStore(STORE, { keyPath: "key" });
  const db = await req(open);
  const tx = (mode) => db.transaction(STORE, mode).objectStore(STORE);
  return {
    name: "indexeddb",
    async loadAll() {
      const all = await req(tx("readonly").getAll());
      return all.map((r) => ({ key: r.key, data: new Uint8Array(r.data), modified: r.modified }));
    },
    async put(key, data, modified) {
      await req(tx("readwrite").put({ key, data: data.slice().buffer, modified }));
    },
    async del(key) {
      await req(tx("readwrite").delete(key));
    },
  };
}

function memory() {
  const m = new Map();
  return {
    name: "memory",
    async loadAll() { return [...m.values()]; },
    async put(key, data, modified) { m.set(key, { key, data: data.slice(), modified }); },
    async del(key) { m.delete(key); },
  };
}

/// Open the store: OPFS, else IndexedDB, else memory (nothing persists). Resolves with the
/// backend's name. `prefer` ("opfs" | "indexeddb" | "memory") forces one (tests).
export async function storeOpen(prefer) {
  const order = { opfs: [opfs, idb, memory], indexeddb: [idb, memory], memory: [memory] }[prefer] ?? [opfs, idb, memory];
  for (const make of order) {
    try { backend = await make(); break; } catch (e) { console.info(`storage: ${make.name}: ${e.message ?? e}`); }
  }
  // Ask the browser not to evict the store under storage pressure (granted for installed apps).
  try { if (navigator.storage && navigator.storage.persist) await navigator.storage.persist(); } catch {}
  return backend.name;
}

/// Every stored entry: [{key, data: Uint8Array, modified}].
export async function storeLoadAll() { return backend.loadAll(); }
export async function storePut(key, data, modified) { return backend.put(key, data, modified); }
export async function storeDelete(key) { return backend.del(key); }

/// Ask the browser to keep the origin's storage under pressure; resolves with whether it is.
export async function storePersist() {
  try {
    if (navigator.storage && navigator.storage.persist) return await navigator.storage.persist();
  } catch {}
  return false;
}

/// {usage, quota, persisted, backend}.
export async function storeEstimate() {
  const out = { backend: backend ? backend.name : null };
  try {
    const e = await navigator.storage.estimate();
    out.usage = e.usage; out.quota = e.quota;
    out.persisted = await navigator.storage.persisted();
  } catch {}
  return out;
}

// ------------------------------------------------------------------ audio

// One AudioContext for the page. Browsers start it suspended until the user interacts with the
// page, so the first pointer/key event resumes it ("unlock").
let ctx = null;
let node = null;
let kind = null;
// Sample frames handed to the output since `audioStart`, and the context time it started at.
let posted = 0;
let startTime = 0;
let underruns = 0;
// ScriptProcessor fallback: the queue lives on this thread.
let spQueue = [];
let spOffset = 0;
// Blocks pushed while the output node is being created.
let starting = false;
let pendingPush = [];
// Bumped by every start/stop: a start superseded while it awaited the worklet gives up.
let generation = 0;

function context() {
  if (!ctx) {
    const AC = self.AudioContext || self.webkitAudioContext;
    if (!AC) return null;
    ctx = new AC({ latencyHint: "interactive" });
  }
  return ctx;
}

function unlock() {
  const c = context();
  if (c && c.state !== "running") c.resume().catch(() => {});
}

/// Install the user-gesture unlock (called once at startup).
export function audioInstallUnlock() {
  for (const ev of ["pointerdown", "keydown", "touchend"]) {
    self.addEventListener(ev, unlock, { capture: true, passive: true });
  }
}

/// {state, sampleRate, backend, posted, played, underruns, baseLatency, outputLatency}.
export function audioState() {
  const c = ctx;
  return {
    state: c ? c.state : "none",
    sampleRate: c ? c.sampleRate : 0,
    backend: kind,
    posted,
    played: audioPlayed(),
    underruns,
    baseLatency: c ? c.baseLatency || 0 : 0,
    outputLatency: c ? c.outputLatency || 0 : 0,
  };
}

/// The output's sample rate (creates the context; 0 without Web Audio).
export function audioSampleRate() {
  const c = context();
  return c ? c.sampleRate : 0;
}

/// Start an output node; resolves when it is ready. Samples pushed before then queue up.
export async function audioStart(workletUrl) {
  audioStop();
  const c = context();
  if (!c) throw new Error("Web Audio is not available");
  unlock();
  posted = 0;
  underruns = 0;
  spQueue = [];
  spOffset = 0;
  starting = true;
  pendingPush = [];
  const gen = ++generation;
  if (c.audioWorklet && workletUrl) {
    try {
      if (!c.__ecWorklet) {
        await c.audioWorklet.addModule(workletUrl);
        c.__ecWorklet = true;
      }
      if (gen !== generation) return;
      node = new AudioWorkletNode(c, "effectcraft-output", { numberOfInputs: 0, outputChannelCount: [2] });
      node.port.onmessage = (e) => { if (e.data && e.data.underrun) underruns += e.data.underrun; };
      kind = "worklet";
    } catch (e) {
      console.warn("audio: AudioWorklet unavailable, using ScriptProcessorNode", e);
      node = null;
    }
  }
  if (!node) {
    node = c.createScriptProcessor(2048, 0, 2);
    node.onaudioprocess = (e) => {
      const l = e.outputBuffer.getChannelData(0);
      const r = e.outputBuffer.getChannelData(1);
      let i = 0;
      while (i < l.length && spQueue.length) {
        const b = spQueue[0];
        const n = Math.min(l.length - i, (b.length - spOffset) >> 1);
        for (let k = 0; k < n; k++) { l[i + k] = b[spOffset + 2 * k]; r[i + k] = b[spOffset + 2 * k + 1]; }
        i += n; spOffset += 2 * n;
        if (spOffset >= b.length) { spQueue.shift(); spOffset = 0; }
      }
      if (i < l.length) { underruns += l.length - i; l.fill(0, i); r.fill(0, i); }
    };
    kind = "scriptProcessor";
  }
  starting = false;
  if (!node) return;
  node.connect(c.destination);
  startTime = c.currentTime;
  const p = pendingPush;
  pendingPush = [];
  for (const b of p) send(b);
}

function send(samples) {
  if (kind === "worklet") node.port.postMessage(samples, [samples.buffer]);
  else spQueue.push(samples);
}

/// Queue interleaved stereo samples (Float32Array) for output.
export function audioPush(samples) {
  posted += samples.length >> 1;
  if (node && !starting) send(samples);
  else if (starting) pendingPush.push(samples);
}

/// Sample frames played since `audioStart`, by the context clock (AudioContext.currentTime).
export function audioPlayed() {
  if (!ctx || !node || starting) return 0;
  return Math.max(0, Math.round((ctx.currentTime - startTime) * ctx.sampleRate));
}

/// Output latency in sample frames (what's handed over but not audible yet comes on top).
export function audioOutputLatency() {
  if (!ctx) return 0;
  return Math.round(((ctx.outputLatency || 0) + (ctx.baseLatency || 0)) * ctx.sampleRate);
}

export function audioStop() {
  generation++;
  starting = false;
  pendingPush = [];
  if (node) {
    try { node.disconnect(); } catch {}
    if (kind === "worklet") node.port.postMessage("stop");
  }
  node = null;
  spQueue = [];
}

// ------------------------------------------------------------------ workers

// Idle workers are reused; each keeps the files it was sent (path → size) so a file goes to a
// worker once.
const idle = [];
const running = new Map(); // job id → {worker, done}
// Job workers open a WebGPU device of their own (unless `?nogpuworkers`): their jobs' frames
// render on it. `jobGpu`: what the last started worker reported; `jobStats`: the last jobs'
// {kind, gpu, passes, readbacks, ms}.
let jobWorkerGpu = true;
let jobGpu = null;
const jobStats = [];

export function setJobWorkerGpu(on) {
  jobWorkerGpu = !!on;
}

function spawn(base) {
  const worker = new Worker(new URL("worker.js", base), { type: "module", name: "effectcraft-worker" });
  const w = { worker, files: new Map(), ready: null, gpu: null };
  w.ready = new Promise((res, rej) => {
    worker.onmessage = (e) => {
      if (e.data && e.data.type === "gpu") {
        w.gpu = e.data.gpu ? { adapter: e.data.gpu } : { cpu: e.data.error || "no GPU" };
        jobGpu = w.gpu;
      }
      if (e.data && e.data.type === "ready") res();
    };
    worker.onerror = (e) => rej(new Error(e.message || "worker failed to start"));
  });
  worker.postMessage({ type: "init", module: self.effectcraftModule ?? null, base, gpu: jobWorkerGpu });
  return w;
}

/// Run a job in a worker. `files`: [[path, Uint8Array]] the job reads; `onMessage(type, json,
/// path, bytes)` receives "reply" (json), "file" (path, bytes: a rendered file to download),
/// "store" (path, bytes: a file to keep in browser storage) and "error" (json = message).
export function workerRun(id, json, files, base, onMessage) {
  const w = idle.pop() ?? spawn(base);
  running.set(id, w);
  w.ready.then(() => {
    w.worker.onmessage = (e) => {
      const m = e.data;
      if (m.type === "reply") {
        onMessage("reply", m.json, null, null);
        if (m.done) {
          running.delete(id);
          idle.push(w);
        }
      } else if (m.type === "file") {
        onMessage("file", null, m.path, new Uint8Array(m.bytes));
      } else if (m.type === "store") {
        onMessage("store", null, m.path, new Uint8Array(m.bytes));
      } else if (m.type === "stats") {
        jobStats.push({ kind: m.kind, gpu: m.gpu, passes: m.passes, readbacks: m.readbacks, ms: m.ms });
        if (jobStats.length > 16) jobStats.shift();
      }
    };
    w.worker.onerror = (e) => {
      running.delete(id);
      onMessage("error", String(e.message || "worker error"), null, null);
      w.worker.terminate();
    };
    for (const [path, bytes] of files) {
      if (w.files.get(path) === bytes.length) continue;
      w.files.set(path, bytes.length);
      w.worker.postMessage({ type: "file", path, bytes });
    }
    w.worker.postMessage({ type: "job", json });
  }, (e) => {
    running.delete(id);
    onMessage("error", String(e.message || e), null, null);
  });
}

/// Stop a job: its worker is terminated (it can't be interrupted mid-job).
export function workerCancel(id) {
  const w = running.get(id);
  if (!w) return;
  running.delete(id);
  w.worker.terminate();
}

/// {running, idle, gpu, jobs: [{kind, gpu, passes, readbacks, ms}]}.
export function workerStats() {
  return { running: running.size, idle: idle.length, gpu: jobGpu, jobs: jobStats.slice() };
}

// ------------------------------------------------------------------ frame workers

// Long-lived workers that render viewer frames from a project replica (src/frames.rs). Unlike job
// workers they are never terminated between requests; the page sends one frame at a time each.
const frameWorkers = [];

/// Start a frame worker; returns its index. `onMessage(index, json, bytes, error)` receives each
/// `frameReply` (JSON, pixels as a Uint8Array) or, if the worker dies, the error. `gpu`: the
/// worker opens its own WebGPU device (when the browser has WebGPU in workers) and renders
/// GPU effects on it.
export function frameWorkerStart(base, onMessage, gpu) {
  const index = frameWorkers.length;
  const worker = new Worker(new URL("worker.js", base), { type: "module", name: `effectcraft-frames-${index}` });
  worker.onmessage = (e) => {
    const m = e.data;
    if (m && m.type === "frameReply") onMessage(index, m.json, m.bytes ? new Uint8Array(m.bytes) : null, undefined);
  };
  worker.onerror = (e) => {
    onMessage(index, undefined, undefined, String(e.message || "frame worker failed"));
    worker.terminate();
  };
  worker.postMessage({ type: "init", module: self.effectcraftModule ?? null, base, frames: true, gpu: !!gpu });
  frameWorkers.push(worker);
  return index;
}

// ------------------------------------------------------------------ disk cache (OPFS)

// The disk cache (src/diskcache.rs): one file per frame under
// effectcraft-cache/v1/frames/<32 hex>.ecc and one per layer buffer under
// effectcraft-cache/v1/layers/<32 hex>.ecc in the Origin Private File System (entry names with a
// `layers/` prefix are layer buffers). Frame workers write them with synchronous access handles
// (a temporary file moved into place, so a reader never sees a partial entry; every entry also
// ends in a checksum) and read the layer buffers they prefetch; the page lists, reads and
// deletes.
const CACHE_ROOT = "effectcraft-cache";
const CACHE_KINDS = ["frames", "layers"];
const cacheDirHandles = {};

async function cacheDir(kind = "frames") {
  if (cacheDirHandles[kind]) return cacheDirHandles[kind];
  if (!navigator.storage || !navigator.storage.getDirectory) throw new Error("no Origin Private File System");
  let d = await navigator.storage.getDirectory();
  for (const n of [CACHE_ROOT, "v1", kind]) d = await d.getDirectoryHandle(n, { create: true });
  cacheDirHandles[kind] = d;
  return d;
}

/// The folder and file name of an entry name (`layers/<hex>.ecc` or `<hex>.ecc`).
async function entryDir(name) {
  if (name.startsWith("layers/")) return [await cacheDir("layers"), name.slice(7)];
  return [await cacheDir("frames"), name];
}

/// Every cached entry: [{name, size, modified}] (layer buffers named `layers/…`). Temporaries
/// older than ten minutes (a writer that died) are removed.
export async function cacheList() {
  const out = [];
  for (const kind of CACHE_KINDS) {
    const dir = await cacheDir(kind);
    const prefix = kind === "frames" ? "" : `${kind}/`;
    const stale = [];
    for await (const [name, h] of dir.entries()) {
      if (h.kind !== "file") continue;
      const f = await h.getFile();
      if (name.includes(".tmp")) {
        if (Date.now() - f.lastModified > 600000) stale.push(name);
        continue;
      }
      out.push({ name: prefix + name, size: f.size, modified: f.lastModified });
    }
    for (const n of stale) { try { await dir.removeEntry(n); } catch {} }
  }
  return out;
}

/// An entry's bytes.
export async function cacheRead(name, maxBytes) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) throw new Error("invalid disk cache read limit");
  const [dir, file] = await entryDir(name);
  const f = await (await dir.getFileHandle(file)).getFile();
  if (f.size > maxBytes) throw new Error("disk cache entry exceeds read limit");
  return new Uint8Array(await f.arrayBuffer());
}

/// Write an entry (in a worker: a synchronous access handle; elsewhere a writable stream);
/// resolves with its size.
export async function cacheWrite(fullName, bytes) {
  const [dir, name] = await entryDir(fullName);
  const canMove = typeof FileSystemHandle !== "undefined" && "move" in FileSystemHandle.prototype;
  const tmp = canMove ? `${name}.tmp.${Math.random().toString(36).slice(2)}` : name;
  const fh = await dir.getFileHandle(tmp, { create: true });
  if (fh.createSyncAccessHandle) {
    const h = await fh.createSyncAccessHandle();
    try {
      h.truncate(0);
      h.write(bytes, { at: 0 });
      h.flush();
    } finally {
      h.close();
    }
  } else {
    const w = await fh.createWritable();
    await w.write(bytes);
    await w.close();
  }
  if (canMove) {
    try { await fh.move(dir, name); } catch (e) { try { await dir.removeEntry(tmp); } catch {} throw e; }
  }
  return bytes.length;
}

/// Delete entries (names); missing ones are ignored.
export async function cacheDelete(names) {
  for (const n of names) {
    try {
      const [dir, file] = await entryDir(n);
      await dir.removeEntry(file);
    } catch {}
  }
}

/// Delete the whole cache.
export async function cacheClear() {
  for (const k of CACHE_KINDS) delete cacheDirHandles[k];
  try {
    const root = await navigator.storage.getDirectory();
    await root.removeEntry(CACHE_ROOT, { recursive: true });
  } catch (e) {
    if (e.name !== "NotFoundError") throw e;
  }
}

/// Whether this thread has WebGPU (`navigator.gpu`).
export function hasWebGpu() {
  return !!(self.navigator && self.navigator.gpu);
}

/// Post a message ({type: "frame", json} or {type: "file", path, bytes}) to frame worker `index`.
export function frameWorkerPost(index, msg, transfer) {
  const w = frameWorkers[index];
  if (w) w.postMessage(msg, transfer ?? []);
}

// ------------------------------------------------------------------ media browser

// Folders opened with the File System Access API (src/browse.rs): name → {handle, files: Map of
// "name/sub/file" → FileSystemFileHandle}. Their handles are remembered in IndexedDB so the
// folders come back on the next visit (once the browser grants access again).
const folders = new Map();
const HANDLES_DB = "effectcraft-handles";
const MAX_ENTRIES = 20000;

async function handlesDb() {
  const open = indexedDB.open(HANDLES_DB, 1);
  open.onupgradeneeded = () => open.result.createObjectStore("folders", { keyPath: "name" });
  return req(open);
}

async function enumerate(dir, prefix, out, files, depth) {
  for await (const [name, h] of dir.entries()) {
    if (name.startsWith(".") || out.length >= MAX_ENTRIES) continue;
    const rel = `${prefix}/${name}`;
    if (h.kind === "directory") {
      out.push({ path: rel, dir: true, size: 0, modified: 0 });
      if (depth < 8) await enumerate(h, rel, out, files, depth + 1);
    } else {
      const f = await h.getFile();
      out.push({ path: rel, dir: false, size: f.size, modified: Math.floor(f.lastModified / 1000) });
      files.set(rel, h);
    }
  }
}

async function openFolder(handle, remember) {
  const name = handle.name;
  const entries = [];
  const files = new Map();
  await enumerate(handle, name, entries, files, 0);
  folders.set(name, { handle, files });
  if (remember) {
    try {
      const db = await handlesDb();
      await req(db.transaction("folders", "readwrite").objectStore("folders").put({ name, handle }));
    } catch (e) { console.info("media browser: cannot remember the folder", e); }
  }
  return { name, entries };
}

/// Whether the browser can open folders (File System Access API).
export function browseCanPickFolder() {
  return typeof self.showDirectoryPicker === "function";
}

/// Ask for a folder; resolves with {name, entries: [{path, dir, size, modified}]}.
export async function browsePickFolder() {
  const handle = await self.showDirectoryPicker({ id: "effectcraft-media", mode: "read" });
  return openFolder(handle, true);
}

/// Folders opened on earlier visits: [{name, granted, entries?}] (entries when access is granted).
export async function browseRestore() {
  if (!self.indexedDB || !browseCanPickFolder()) return [];
  let all = [];
  try {
    const db = await handlesDb();
    all = await req(db.transaction("folders", "readonly").objectStore("folders").getAll());
  } catch { return []; }
  const out = [];
  for (const { name, handle } of all) {
    let granted = false;
    try { granted = (await handle.queryPermission({ mode: "read" })) === "granted"; } catch {}
    if (granted) {
      try { out.push({ granted, ...(await openFolder(handle, false)) }); continue; } catch {}
    }
    out.push({ name, granted: false });
  }
  return out;
}

/// Ask for access to a remembered folder again (needs a user gesture); resolves like
/// browsePickFolder.
export async function browseReconnect(name) {
  const db = await handlesDb();
  const rec = await req(db.transaction("folders", "readonly").objectStore("folders").get(name));
  if (!rec) throw new Error(`${name} is not remembered`);
  if ((await rec.handle.requestPermission({ mode: "read" })) !== "granted") throw new Error(`access to ${name} was not granted`);
  return openFolder(rec.handle, false);
}

/// Forget a remembered folder.
export async function browseForget(name) {
  folders.delete(name);
  try {
    const db = await handlesDb();
    await req(db.transaction("folders", "readwrite").objectStore("folders").delete(name));
  } catch {}
}

/// A file of an opened folder ("/Folders/name/sub/file"): its bytes.
export async function browseRead(path) {
  const rel = path.replace(/^\/Folders\//, "");
  const folder = folders.get(rel.split("/")[0]);
  const h = folder && folder.files.get(rel);
  if (!h) throw new Error(`${path}: its folder is not open (reconnect it)`);
  return new Uint8Array(await (await h.getFile()).arrayBuffer());
}

/// The fallback where folders can't be opened: pick files (or a whole folder, `directory`) with a
/// plain file input; resolves with [{path, bytes}] (path relative to the picked folder).
export function browsePickFiles(directory) {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    if (directory) input.webkitdirectory = true;
    input.onchange = async () => {
      try {
        const out = [];
        for (const f of input.files) {
          out.push({ path: (directory && f.webkitRelativePath) || f.name, bytes: new Uint8Array(await f.arrayBuffer()) });
        }
        resolve(out);
      } catch (e) { reject(e); }
    };
    input.click();
  });
}
