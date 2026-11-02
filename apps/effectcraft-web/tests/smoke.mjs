// Browser smoke test of the EffectCraft web app over the Chrome DevTools Protocol.
// No npm dependencies (Node ≥ 22: global WebSocket and fetch).
//
//   cargo xtask web --serve 8765 &            # build + serve <target>/web/dist
//   node apps/effectcraft-web/tests/smoke.mjs --url http://127.0.0.1:8765/ --out /tmp/ec-web
//
// Steps: load (timing), the GPU path (WebGPU → GPU compositor, viewer frames on the GPU), demo
// comp in the viewer (screenshot, viewer pixels), `render.frame` through `window.effectcraft`,
// Render Queue GIF and PNG-sequence (.zip) exports (downloads), a background Render Queue job in
// a Web Worker while the page stays responsive (event-loop gaps measured), audio (the
// AudioContext starts on a user gesture; preview playback feeds it), File ▸ Save As (download),
// re-opening the saved project through `effectcraft.addFile`, persistence across a reload (the
// session, imported media, settings, a project saved to browser storage, Open Recent) and an
// offline reload through the service worker; M13.24: GPU frame workers, the storage manager and
// the disk cache; M13.30: GPU job workers (Render Queue frames and an analysis's input frames on
// the worker's WebGPU device, in passes) and layer buffers in the disk cache. Prints a JSON
// report; exits non-zero on failure.
// `--headed` shows the browser.
import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync, readdirSync, statSync, mkdtempSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

const arg = (k, d) => {
  const i = process.argv.indexOf(`--${k}`);
  return i > 0 ? process.argv[i + 1] : d;
};
const url = arg("url", "http://127.0.0.1:8765/");
const out = arg("out", join(tmpdir(), "effectcraft-web-smoke"));
const chrome = arg("chrome", process.platform === "darwin" ? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" : "google-chrome");
const port = Number(arg("port", "9334"));
const headless = !process.argv.includes("--headed");
mkdirSync(out, { recursive: true });
const downloads = join(out, "downloads");
mkdirSync(downloads, { recursive: true });

const profile = mkdtempSync(join(tmpdir(), "ec-chrome-"));
const proc = spawn(chrome, [
  ...(headless ? ["--headless=new"] : []),
  `--remote-debugging-port=${port}`,
  `--user-data-dir=${profile}`,
  "--no-first-run",
  "--no-default-browser-check",
  "--enable-unsafe-webgpu",
  "--window-size=1600,1000",
  "about:blank",
], { stdio: "ignore" });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let ws;
let nextId = 1;
const waiting = new Map();
const logs = [];

async function connect() {
  for (let i = 0; i < 100; i++) {
    try {
      const list = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const page = list.find((t) => t.type === "page");
      if (page) {
        ws = new WebSocket(page.webSocketDebuggerUrl);
        await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
        ws.onmessage = (m) => {
          const msg = JSON.parse(m.data);
          if (msg.id && waiting.has(msg.id)) {
            waiting.get(msg.id)(msg);
            waiting.delete(msg.id);
          } else if (msg.method === "Runtime.consoleAPICalled") {
            logs.push(msg.params.args.map((a) => a.value ?? a.description ?? "").join(" "));
          } else if (msg.method === "Runtime.exceptionThrown") {
            logs.push("EXCEPTION " + JSON.stringify(msg.params.exceptionDetails.exception?.description ?? msg.params.exceptionDetails.text));
          }
        };
        return;
      }
    } catch {}
    await sleep(100);
  }
  throw new Error("Chrome did not start");
}

function send(method, params = {}) {
  const id = nextId++;
  ws.send(JSON.stringify({ id, method, params }));
  return new Promise((res, rej) => waiting.set(id, (m) => (m.error ? rej(new Error(`${method}: ${m.error.message}`)) : res(m.result))));
}

async function js(expr, timeout = 300000) {
  // (promise rejections carry plain strings: wrap them so the message survives)
  const wrapped = `(async () => { try { return await (${expr}); } catch (e) { throw new Error(String(e && e.message || e)); } })()`;
  const r = await send("Runtime.evaluate", { expression: wrapped, awaitPromise: true, returnByValue: true, timeout });
  if (r.exceptionDetails) throw new Error(`${expr}: ${r.exceptionDetails.exception?.description ?? r.exceptionDetails.text}`);
  return r.result.value;
}

const check = (cond, msg) => {
  if (!cond) throw new Error(`check failed: ${msg}`);
};

// Wait until the app runs (after a navigation or reload).
async function ready(ms = 180000) {
  await until("!!(window.effectcraftLoad && (window.effectcraftLoad.readyMs || window.effectcraftLoad.error))", ms);
  const load = await js("window.effectcraftLoad");
  if (load.error) throw new Error(load.error);
  return load;
}

async function idle() {
  await until(`effectcraft.execute("renderQueue.list", {}).then(r => !r.rendering)`, 300000, 200);
}

async function shot(name) {
  const r = await send("Page.captureScreenshot", { format: "png" });
  const p = join(out, `${name}.png`);
  writeFileSync(p, Buffer.from(r.data, "base64"));
  return p;
}

async function until(expr, ms = 60000, step = 200) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if (await js(expr)) return Date.now() - t0;
    await sleep(step);
  }
  throw new Error(`timeout waiting for ${expr}`);
}

async function waitDownload(pred, ms = 120000) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    const f = readdirSync(downloads).find((n) => pred(n) && !n.endsWith(".crdownload"));
    if (f) return { name: f, bytes: statSync(join(downloads, f)).size };
    await sleep(200);
  }
  throw new Error("no download");
}

const report = { url, steps: {} };
let failed = false;
try {
  await connect();
  await send("Page.enable");
  await send("Runtime.enable");
  await send("Browser.setDownloadBehavior", { behavior: "allow", downloadPath: downloads }).catch(() => {});
  await send("Emulation.setDeviceMetricsOverride", { width: 1600, height: 1000, deviceScaleFactor: 1, mobile: false });
  const t0 = Date.now();
  await send("Page.navigate", { url });
  report.load = await ready();
  report.load.wallMs = Date.now() - t0;
  report.info = await js("effectcraft.info()");
  // Storage: OPFS where the browser writes it (Chrome does), else IndexedDB.
  check(["opfs", "indexeddb"].includes(report.info.storage?.backend), `storage backend ${JSON.stringify(report.info.storage)}`);

  // The demo comp in the viewer: wait for the first viewer frame (renderMs > 0).
  const ms = await until("effectcraft.inspect().then(i => i.renderMs > 0)", 120000, 500);
  await sleep(1500);
  const insp = await js("effectcraft.inspect()");
  report.steps.viewer = { waitMs: ms, renderMs: insp.renderMs, activeComp: insp.activeComp, window: insp.window, screenshot: await shot("01-demo") };

  // GPU: with WebGPU, eframe runs on it and the compositor renders viewer frames on the GPU
  // (no readback); without it, WebGL2 / CPU.
  const info = await js("effectcraft.info()");
  report.steps.gpu = { webgpu: info.webgpu, backend: info.backend, ...info.gpu };
  if (info.webgpu && info.backend === "BrowserWebGpu") {
    check(info.gpu && info.gpu.compositor, "GPU compositor on WebGPU");
    check(info.gpu.viewerOnGpu, "viewer frames on the GPU");
  }

  // Comp pixels through the API.
  const f = await js("effectcraft.renderFrame({max_side: 480}).then(r => ({w: r.width, h: r.height, pngBytes: atob(r.png).length}))");
  report.steps.renderFrame = f;

  // Viewer frames render in frame workers (project replicas fed by diffs): with the CPU
  // renderer, scrubbing (and an edit, which travels as a diff) keeps the page's event loop free.
  await js(`effectcraft.execute("render.backend", {backend: "cpu"})`);
  report.steps.frameWorkers = await js(`(async () => {
    const before = effectcraft.info().frameWorkers;
    let maxGapMs = 0, last = performance.now();
    const timer = setInterval(() => { const now = performance.now(); maxGapMs = Math.max(maxGapMs, now - last); last = now; }, 10);
    await effectcraft.execute("layer.newSolid", {name: "Frame Worker Solid", color: "#3366cc", width: 120, height: 80});
    for (let k = 0; k < 12; k++) {
      await effectcraft.execute("time.set", {time: k / 4});
      await new Promise((r) => setTimeout(r, 150));
    }
    const t0 = performance.now();
    while (effectcraft.info().frameWorkers.rendered < before.rendered + 3 && performance.now() - t0 < 120000) await new Promise((r) => setTimeout(r, 100));
    clearInterval(timer);
    await effectcraft.execute("edit.undo", {});
    return {before, after: effectcraft.info().frameWorkers, maxGapMs};
  })()`);
  const fwk = report.steps.frameWorkers;
  await js(`effectcraft.execute("render.backend", {backend: "gpu"})`);
  check(fwk.after.alive > 0, `frame workers alive: ${JSON.stringify(fwk.after)}`);
  check(fwk.after.rendered >= fwk.before.rendered + 3, `frames rendered in workers: ${JSON.stringify(fwk)}`);
  check(fwk.after.syncs >= 2, `project synced as diffs: ${JSON.stringify(fwk.after)}`);
  check(fwk.maxGapMs < 400, `event loop blocked ${fwk.maxGapMs} ms while scrubbing`);

  // GPU effects in the browser (M13.24): each frame worker opens its own WebGPU device; frames
  // with effects go to the workers, which render them on the GPU in passes (awaiting the
  // device's readbacks), while the page's event loop stays free.
  const gw = await js(`(async () => {
    const t0 = performance.now();
    while (effectcraft.info().frameWorkers.gpu.some((g) => g === null) && performance.now() - t0 < 30000) await new Promise((r) => setTimeout(r, 200));
    return {gpu: effectcraft.info().frameWorkers.gpu, workerWebGpu: effectcraft.info().webgpu};
  })()`);
  report.steps.gpuWorkers = gw;
  if (gw.gpu.some((g) => typeof g === "string")) {
    report.steps.gpuWorkers.effects = await js(`(async () => {
      const before = effectcraft.info().frameWorkers;
      let maxGapMs = 0, last = performance.now();
      const timer = setInterval(() => { const now = performance.now(); maxGapMs = Math.max(maxGapMs, now - last); last = now; }, 10);
      await effectcraft.execute("layer.newSolid", {name: "GPU Blur Solid", color: "#cc3366", width: 300, height: 200});
      await effectcraft.execute("effect.apply", {effect: "Gaussian Blur"});
      await effectcraft.execute("prop.set", {path: "effects/#1/blurriness", value: 25});
      await effectcraft.execute("effect.apply", {effect: "Glow"});
      for (let k = 0; k < 6; k++) {
        await effectcraft.execute("time.set", {time: 0.3 + k / 5});
        await new Promise((r) => setTimeout(r, 200));
      }
      const t0 = performance.now();
      while (effectcraft.info().frameWorkers.gpuFrames < before.gpuFrames + 3 && performance.now() - t0 < 120000) await new Promise((r) => setTimeout(r, 100));
      clearInterval(timer);
      const after = effectcraft.info().frameWorkers;
      // The worker's GPU frame against the page's CPU render of the same frame.
      let parity = null;
      for (let i = 0; i < 100 && !parity; i++) {
        parity = await effectcraft.workerFrameCheck({scale: 0.5}).catch(() => null);
        if (!parity) await new Promise((r) => setTimeout(r, 100));
      }
      after.parity = parity;
      await effectcraft.execute("edit.undo", {});
      await effectcraft.execute("edit.undo", {});
      await effectcraft.execute("edit.undo", {});
      await effectcraft.execute("edit.undo", {});
      return {before, after, maxGapMs};
    })()`);
    const ge = report.steps.gpuWorkers.effects;
    report.steps.gpuWorkers.screenshot = await shot("01b-gpu-effects");
    check(ge.after.gpuFrames >= ge.before.gpuFrames + 3, `GPU effect frames rendered in workers: ${JSON.stringify(ge)}`);
    check(ge.after.lastPasses >= 2, `frames rendered in passes (deferred readbacks): ${ge.after.lastPasses}`);
    const pa = ge.after.parity;
    check(pa && pa.meanDiff < 0.5 && pa.over4 < 0.005, `GPU worker frame vs the CPU: ${JSON.stringify(pa)}`);
    check(ge.maxGapMs < 400, `event loop blocked ${ge.maxGapMs} ms while GPU workers rendered`);
  } else {
    report.steps.gpuWorkers.note = "no WebGPU in workers here: frame workers render on their CPU";
  }

  // The storage manager (Settings ▸ Disk ▸ Browser Storage) and its commands.
  await js(`effectcraft.execute("storage.persist", {})`);
  const st = await js(`(async () => { await new Promise((r) => setTimeout(r, 1200)); return effectcraft.execute("storage.info", {}); })()`);
  report.steps.storage = st;
  check(st.available && st.quota > 0 && st.usage > 0, `storage.info: ${JSON.stringify(st)}`);
  check(st.diskCache && st.diskCache.enabled && st.diskCache.writes > 0, `disk cache in OPFS: ${JSON.stringify(st.diskCache)}`);
  await js(`effectcraft.execute("prefs.open", {page: "disk"})`);
  await sleep(800);
  report.steps.storage.screenshot = await shot("01c-storage-manager");
  const ids = await js(`effectcraft.request("ui.elements", {prefix: "settings.storage."}).then((l) => l.map((e) => e.id))`);
  report.steps.storage.elements = ids;
  check(ids.includes("settings.storage.usage") && ids.includes("settings.storage.clear.diskCache"), `storage manager widgets: ${ids}`);
  await js(`effectcraft.request("ui.click", {id: "settings.cancel"})`);
  await sleep(300);

  // Render Queue: a short, small GIF → download. `renderQueue.render` waits by default; the
  // render still runs in a worker and the reply comes when it ends (the page keeps running).
  const rqT = Date.now();
  // (`ui.menu.invoke`: `engine.execute`'s strict parameter check does not know the spread
  // Render Settings / Output Module keys)
  await js(`effectcraft.request("ui.menu.invoke", {id: "renderQueue.add", params: {format: "gif", output: "smoke_[compName].gif", resolution: 0.25, timeSpan: "custom", start: 0, end: 1}})`);
  const waited = await js(`(async () => {
    let maxGapMs = 0, last = performance.now();
    const timer = setInterval(() => { const now = performance.now(); maxGapMs = Math.max(maxGapMs, now - last); last = now; }, 10);
    const r = await effectcraft.execute("renderQueue.render", {});
    clearInterval(timer);
    return {rendering: r.rendering, status: r.items.map((i) => i.status), maxGapMs};
  })()`);
  const gif = await waitDownload((n) => n.endsWith(".gif"));
  report.steps.gifExport = { ...gif, ms: Date.now() - rqT, waited };
  check(!waited.rendering && waited.status.every((s) => s === "Done"), `wait:true replies when done: ${JSON.stringify(waited)}`);
  check(waited.maxGapMs < 400, `event loop blocked ${waited.maxGapMs} ms during a waiting render`);
  await idle();

  // An image sequence arrives as one .zip.
  await js(`effectcraft.execute("renderQueue.setRender", {index: 1, render: false})`);
  await js(`effectcraft.request("ui.menu.invoke", {id: "renderQueue.add", params: {format: "png", output: "seq_[####].png", resolution: 0.125, timeSpan: "custom", start: 0, end: 0.2}})`);
  await js(`effectcraft.execute("renderQueue.render", {})`);
  report.steps.pngSequence = await waitDownload((n) => n.endsWith(".zip"));
  await idle();

  // A background render (`wait: false`, what the Render button does) runs in a Web Worker: the
  // page keeps its event loop. Measure the longest gap between 10 ms timers while it renders.
  await js(`effectcraft.execute("renderQueue.setRender", {index: 2, render: false})`);
  await js(`effectcraft.request("ui.menu.invoke", {id: "renderQueue.add", params: {format: "gif", output: "bg_[compName].gif", resolution: 0.5, timeSpan: "custom", start: 0, end: 4}})`);
  report.steps.backgroundRender = await js(`(async () => {
    const t0 = performance.now();
    const r = await effectcraft.execute("renderQueue.render", {wait: false});
    const callMs = performance.now() - t0;
    let maxGapMs = 0, last = performance.now(), ticks = 0, progress = 0;
    for (;;) {
      await new Promise((res) => setTimeout(res, 10));
      const now = performance.now();
      maxGapMs = Math.max(maxGapMs, now - last);
      last = now;
      if (++ticks % 5) continue;
      const l = await effectcraft.execute("renderQueue.list", {});
      if (l.progress) progress = Math.max(progress, l.progress.done);
      if (!l.rendering) break;
    }
    return {startedRendering: r.rendering, callMs, renderMs: performance.now() - t0, maxGapMs, ticks, progressFrames: progress, workers: effectcraft.info().workers};
  })()`);
  const bg = report.steps.backgroundRender;
  bg.download = await waitDownload((n) => n.startsWith("bg_") && n.endsWith(".gif"));
  check(bg.startedRendering, "render started in the background");
  check(bg.maxGapMs < 400, `event loop blocked ${bg.maxGapMs} ms during the render`);
  check(bg.progressFrames > 0, "progress reported while rendering");

  // GPU in the job workers (M13.30): a Render Queue job and a Warp Stabilizer analysis whose
  // frames have GPU effects render on the job worker's own WebGPU device, in passes (the job
  // awaits the device's readbacks between them), while the page's event loop stays free.
  if (report.steps.gpuWorkers.effects) {
    await js(`effectcraft.execute("renderQueue.setRender", {index: 3, render: false})`);
    report.steps.gpuJobs = await js(`(async () => {
      let maxGapMs = 0, last = performance.now();
      const timer = setInterval(() => { const now = performance.now(); maxGapMs = Math.max(maxGapMs, now - last); last = now; }, 10);
      const gaps = {};
      // (a phase ends once the timer has run: a stall at its end counts in it, not the next)
      const phase = async (n) => { await new Promise((res) => setTimeout(res, 30)); gaps[n] = maxGapMs; maxGapMs = 0; };
      await effectcraft.execute("layer.newSolid", {name: "GPU Job Solid", color: "#3366cc", width: 320, height: 180});
      await effectcraft.execute("effect.apply", {effect: "Fractal Noise"});
      await effectcraft.execute("prop.addKey", {path: "effects/#1/transform/offset", time: 0, value: [0.5, 0.5]});
      await effectcraft.execute("prop.addKey", {path: "effects/#1/transform/offset", time: 1, value: [0.65, 0.58]});
      await effectcraft.execute("effect.apply", {effect: "Gaussian Blur"});
      await effectcraft.execute("prop.set", {path: "effects/#2/blurriness", value: 12});
      // (A resolution no earlier render used: Backend Auto warms this comp and scale up afresh,
      // GPU first.)
      await effectcraft.request("ui.menu.invoke", {id: "renderQueue.add", params: {format: "png", output: "gpujob_[####].png", resolution: 0.2, timeSpan: "custom", start: 0, end: 0.4}});
      const t0 = performance.now();
      const r = await effectcraft.execute("renderQueue.render", {});
      const renderMs = performance.now() - t0;
      await phase("render");
      // Applying Warp Stabilizer starts its analysis in a job worker (as in After Effects).
      await effectcraft.execute("effect.apply", {effect: "Warp Stabilizer"});
      const t1 = performance.now();
      let warp;
      do {
        await new Promise((res) => setTimeout(res, 200));
        warp = await effectcraft.execute("warp.status", {});
      } while (!warp.analyzed && performance.now() - t1 < 180000);
      const warpMs = performance.now() - t1;
      await phase("warp");
      // Content-Aware Fill (a job worker too): its input is the layer's source, so the noise and
      // blur go into a precomp (rendered on the worker's GPU); cut a hole, fill half a second.
      // Editing on the page after the analysis (M13.32: the first warp.status with the
      // analysis used to solve the stabilization plan on the page, ≈ 5 s; now the job sends
      // it): every command stays a few UI frames.
      await effectcraft.execute("effect.remove", {effect: "Warp Stabilizer"});
      await effectcraft.execute("layer.precompose", {name: "GPU Fill Source", mode: "move", open: false});
      await effectcraft.execute("layer.addMask", {rect: [140, 70, 40, 40], mode: "subtract"});
      const wa = await effectcraft.execute("comp.info", {}).then((c) => c.workArea).catch(() => null);
      await effectcraft.execute("comp.workArea", {start: 0, end: 0.5});
      await phase("prepareFill");
      const t2 = performance.now();
      const fill = await effectcraft.execute("contentFill.generate", {method: "edgeBlend", wait: true}).catch((e) => ({error: String(e)}));
      const fillMs = performance.now() - t2;
      const fillLayers = await effectcraft.execute("comp.info", {}).then((c) => c.layers.map((l) => l.name).filter((n) => n.startsWith("Fill ")));
      await phase("fill");
      clearInterval(timer);
      maxGapMs = Math.max(gaps.render, gaps.warp, gaps.fill);
      // Back to the project as it was.
      const c = await effectcraft.execute("comp.info", {});
      const ids = c.layers.filter((l) => l.name === "GPU Fill Source" || l.name.startsWith("Fill ")).map((l) => l.id);
      await effectcraft.execute("edit.clear", {layers: ids});
      if (wa) await effectcraft.execute("comp.workArea", {start: wa[0], end: wa[1]});
      const workers = effectcraft.info().workers;
      return {status: r.items.map((i) => i.status), renderMs, warp, warpMs, fill, fillMs, fillLayers, workArea: wa, workers, maxGapMs, gaps};
    })()`);
    const gj = report.steps.gpuJobs;
    gj.download = await waitDownload((n) => n.startsWith("gpujob"));
    const jobs = gj.workers.jobs || [];
    const rj = jobs.filter((j) => j.kind === "Render").pop();
    const wj = jobs.filter((j) => j.kind === "Warp").pop();
    check(gj.workers.gpu && gj.workers.gpu.adapter, `job workers have a GPU: ${JSON.stringify(gj.workers)}`);
    check(gj.status.every((st) => st === "Done"), `GPU job render: ${JSON.stringify(gj.status)}`);
    check(rj && rj.gpu && rj.readbacks >= 3 && rj.passes >= 6, `Render Queue frames on the job worker's GPU, in passes: ${JSON.stringify(rj)}`);
    check(gj.warp.analyzed, `Warp Stabilizer analysed in a job worker: ${JSON.stringify(gj.warp)}`);
    check(wj && wj.gpu && wj.readbacks > 0, `Warp Stabilizer input frames on the job worker's GPU: ${JSON.stringify(wj)}`);
    const fj = jobs.filter((j) => j.kind === "ContentFill").pop();
    check(!gj.fill.error && gj.fillLayers.length > 0, `Content-Aware Fill in a job worker: ${JSON.stringify(gj.fill)} ${JSON.stringify(gj.fillLayers)}`);
    check(fj && fj.gpu && fj.readbacks > 0, `Content-Aware Fill frames on the job worker's GPU: ${JSON.stringify(fj)}`);
    check(gj.maxGapMs < 400, `event loop blocked ${gj.maxGapMs} ms during the GPU jobs: ${JSON.stringify(gj.gaps)}`);
    check(gj.gaps.prepareFill < 50, `event loop blocked ${gj.gaps.prepareFill} ms while editing after the analysis (pre-compose, mask): ${JSON.stringify(gj.gaps)}`);
  } else {
    report.steps.gpuJobs = { note: "no WebGPU in workers here: job workers render on their CPU" };
  }

  // Audio: the AudioContext starts on a user gesture.
  report.steps.audio = { before: (await js("effectcraft.info()")).audio };
  check(report.steps.audio.before.state !== "running", "audio context not running before a gesture");
  // (a click in the window's bottom-right corner, the status bar: modifier keys alone don't
  // count as user activation)
  await send("Input.dispatchMouseEvent", { type: "mousePressed", x: 1590, y: 995, button: "left", clickCount: 1 });
  await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: 1590, y: 995, button: "left", clickCount: 1 });
  await until("effectcraft.info().audio.state === 'running'", 10000);
  report.steps.audio.afterGesture = (await js("effectcraft.info()")).audio;
  // A 2 s tone in the comp; preview playback feeds the output and the meters.
  await js(`(async () => {
    const rate = 48000, n = 2 * rate, b = new ArrayBuffer(44 + n * 4), v = new DataView(b);
    const w = (o, t) => [...t].forEach((c, i) => v.setUint8(o + i, c.charCodeAt(0)));
    w(0, "RIFF"); v.setUint32(4, 36 + n * 4, true); w(8, "WAVE"); w(12, "fmt "); v.setUint32(16, 16, true);
    v.setUint16(20, 1, true); v.setUint16(22, 2, true); v.setUint32(24, rate, true); v.setUint32(28, rate * 4, true);
    v.setUint16(32, 4, true); v.setUint16(34, 16, true); w(36, "data"); v.setUint32(40, n * 4, true);
    for (let i = 0; i < n; i++) { const x = Math.round(Math.sin(2 * Math.PI * 440 * i / rate) * 0.5 * 32767); v.setInt16(44 + 4 * i, x, true); v.setInt16(46 + 4 * i, x, true); }
    return effectcraft.addFile(new File([b], "tone.wav"));
  })()`);
  await until(`effectcraft.execute("layer.addItem", {item: "tone.wav", time: 0}).then(() => true, () => false)`, 20000, 300);
  // Media Browser: browser storage (where tone.wav now is) and its actions.
  const mb = await js(`effectcraft.execute("mediaBrowser.go", {})`);
  report.steps.mediaBrowser = { path: mb.path, entries: mb.entries.map((e) => e.name), places: mb.places, actions: mb.actions.map((a) => a.id) };
  check(mb.path === "/Browser Storage" && mb.entries.some((e) => e.name === "tone.wav" && e.kind === "audio"), `Media Browser: ${JSON.stringify(report.steps.mediaBrowser)}`);
  check(mb.actions.some((a) => a.id === "addFiles"), "Media Browser: Add Files…");
  const mbImport = await js(`effectcraft.execute("mediaBrowser.import", {paths: [${JSON.stringify(mb.entries.find((e) => e.name === "tone.wav")?.path ?? "/tone.wav")}]})`);
  check(!mbImport.pending && mbImport.items && mbImport.items.length === 1, `Media Browser import: ${JSON.stringify(mbImport)}`);
  await js(`effectcraft.request("ui.playback", {action: "stop"})`);
  await js(`effectcraft.execute("time.set", {time: 0})`);
  await js(`effectcraft.request("ui.playback", {action: "play"})`);
  await sleep(1500);
  const pb = await js(`effectcraft.request("ui.playback", {action: "status"})`);
  const au = (await js("effectcraft.info()")).audio;
  await js(`effectcraft.request("ui.playback", {action: "stop"})`);
  report.steps.audio.playing = { playback: pb, output: au };
  check(pb.audio, "audio preview running");
  check(au.posted > 0.5 * au.sampleRate, `audio fed (${au.posted} frames)`);
  check(au.played > 0, "audio clock advancing");
  check(Math.max(...(pb.levelsDb || [-99])) > -20, `meters (${JSON.stringify(pb.levelsDb)})`);

  // Save As → .ecproj download; reopen it.
  await js(`effectcraft.execute("file.saveAs", {path: "/smoke.ecproj"})`);
  const proj = await waitDownload((n) => n.endsWith(".ecproj"));
  report.steps.save = proj;
  await js(`effectcraft.execute("file.newProject", {})`);
  const opened = await js(`(async () => { const d = effectcraft.readFile("/smoke.ecproj"); const p = await effectcraft.addFile(new File([d], "smoke-copy.ecproj")); await new Promise(r => setTimeout(r, 800)); return {path: p, files: effectcraft.files()}; })()`);
  const comps = await js(`effectcraft.inspect().then(i => i.activeComp)`);
  if (!comps) throw new Error("reopened project has no active comp");
  report.steps.reopen = { ...opened, activeComp: comps };

  // Files the page holds (browser storage, no std::fs): a script opens one next to the project,
  // and the footage check finds the imported media instead of marking it missing.
  const hf = await js(`(async () => {
    const p = await effectcraft.addFile(new File(["from the page"], "script-data.txt"));
    await effectcraft.saveToBrowser("/host-files.ecproj");
    const run = await effectcraft.execute("script.run", {code: "var f = new File(" + JSON.stringify(p) + "); writeLn(f.open('r') ? f.read() : f.error);"});
    return {output: run.output, footage: await effectcraft.execute("footage.check", {wait: true})};
  })()`);
  report.steps.hostFiles = hf;
  check(hf.output === "from the page", `a script opens a file the page holds (${hf.output})`);
  check(hf.footage.checked > 0 && hf.footage.missing === 0, `stored footage isn't missing (${JSON.stringify(hf.footage)})`);
  report.steps.final = { screenshot: await shot("02-after") };

  // Persistence: the session, imported media, settings and a project saved to browser storage
  // survive a reload; File ▸ Open Recent reopens the project.
  await js(`effectcraft.execute("layer.newSolid", {name: "Persisted Solid", color: "#22aa66"})`);
  await js(`effectcraft.execute("prefs.set", {key: "general.recentItems", value: 7})`);
  const saved = await js(`effectcraft.saveToBrowser("/persist-test.ecproj")`);
  await js(`effectcraft.execute("layer.newSolid", {name: "Unsaved Solid"})`);
  // A layer slow enough on the CPU for its buffer to go to the disk cache too (M13.30).
  await js(`(async () => {
    await effectcraft.execute("effect.apply", {effect: "Fractal Noise"});
    await effectcraft.execute("prop.set", {path: "effects/#1/complexity", value: 10});
    await effectcraft.execute("effect.apply", {effect: "Gaussian Blur"});
    await effectcraft.execute("prop.set", {path: "effects/#2/blurriness", value: 60});
  })()`);
  // Disk cache: frames the workers render now are stored in the Origin Private File System
  // (the CPU renderer sends every frame to the workers).
  await js(`effectcraft.execute("render.backend", {backend: "cpu"})`);
  const diskTimes = [0.4, 0.8, 1.2];
  const stored = await js(`(async () => {
    const w0 = (await effectcraft.execute("cache.diskStats", {})).writes;
    for (const t of ${JSON.stringify(diskTimes)}) {
      await effectcraft.execute("time.set", {time: t});
      await new Promise((r) => setTimeout(r, 300));
    }
    const t0 = performance.now();
    let st;
    do {
      await new Promise((r) => setTimeout(r, 200));
      st = await effectcraft.execute("cache.diskStats", {});
    } while (st.writes < w0 + ${diskTimes.length} && performance.now() - t0 < 60000);
    return {writesBefore: w0, after: st};
  })()`);
  check(stored.after.writes >= stored.writesBefore + diskTimes.length, `frames written to the disk cache: ${JSON.stringify(stored)}`);
  check(stored.after.layers > 0 && stored.after.layerWrites > 0, `layer buffers written to the disk cache: ${JSON.stringify(stored.after)}`);
  await sleep(1500); // the session snapshot is taken at most every second
  await js("effectcraft.flush()");
  const before = await js("effectcraft.listStored()");
  const tr = Date.now();
  await send("Page.reload", {});
  await sleep(500);
  const load2 = await ready();
  const after = await js("effectcraft.listStored()");
  const info2 = await js("effectcraft.info()");
  const layers = await js(`effectcraft.execute("comp.info", {}).then(c => c.layers.map(l => l.name))`);
  const recentItems = await js(`effectcraft.execute("prefs.get", {key: "general.recentItems"})`);
  const rec = await js(`effectcraft.execute("file.recoveryInfo", {})`);
  const paths = after.files.map((f) => f.path);
  report.steps.persistence = { saved, reloadMs: Date.now() - tr, load: load2, restored: info2.restored, storage: info2.storage, layers, recentItems, recent: rec.recentProjects, stored: paths, usage: after.usage, filesBefore: before.files.length };
  check(info2.restored, "session restored after reload");
  check(layers.includes("Persisted Solid") && layers.includes("Unsaved Solid"), `layers after reload: ${layers}`);
  check(paths.includes("/tone.wav") && paths.includes("/persist-test.ecproj"), `stored files: ${paths}`);
  check(recentItems === 7, `setting after reload: ${recentItems}`);
  check(rec.recentProjects[0] === "/persist-test.ecproj", `recent projects: ${rec.recentProjects}`);
  // The disk cache survived the reload: the same frames come from it, not from a render.
  report.steps.diskCache = await js(`(async () => {
    const t0 = performance.now();
    let st;
    do {
      await new Promise((r) => setTimeout(r, 200));
      st = await effectcraft.execute("cache.diskStats", {});
    } while (!st.loaded && performance.now() - t0 < 20000);
    const listed = st.entries;
    for (const t of ${JSON.stringify(diskTimes)}) {
      await effectcraft.execute("time.set", {time: t});
      await new Promise((r) => setTimeout(r, 300));
    }
    const t1 = performance.now();
    while (effectcraft.info().frameWorkers.diskHits < ${diskTimes.length} && performance.now() - t1 < 30000) await new Promise((r) => setTimeout(r, 200));
    return {listed, stats: await effectcraft.execute("cache.diskStats", {}), frameWorkers: effectcraft.info().frameWorkers};
  })()`);
  const dcr = report.steps.diskCache;
  dcr.stored = stored;
  // Layer buffers from the disk cache (M13.30): after a purge (the workers' memory is empty), a
  // frame not seen yet renders and reports its layer misses; the page then names the ones on
  // disk, and the worker of the next frame reads them before rendering: its lookups hit.
  dcr.layers = await js(`(async () => {
    const before = await effectcraft.execute("cache.diskStats", {});
    const hits = [];
    for (const t of [2.05, 2.15, 2.25]) {
      await effectcraft.execute("edit.purge", {what: "memory"});
      await effectcraft.execute("time.set", {time: t});
      const r0 = effectcraft.info().frameWorkers.rendered, t0 = performance.now();
      while (effectcraft.info().frameWorkers.rendered <= r0 && performance.now() - t0 < 30000) await new Promise((r) => setTimeout(r, 100));
      hits.push((await effectcraft.execute("cache.diskStats", {})).layerHits);
    }
    return {before, after: await effectcraft.execute("cache.diskStats", {}), hits};
  })()`);
  check(dcr.layers.before.layers > 0, `layer entries after reload: ${JSON.stringify(dcr.layers.before)}`);
  check(dcr.layers.after.layerHits > dcr.layers.before.layerHits, `layer buffers served from the disk cache: ${JSON.stringify(dcr.layers)}`);
  check(dcr.listed >= diskTimes.length, `disk cache entries after reload: ${JSON.stringify(dcr)}`);
  check(dcr.frameWorkers.diskHits >= diskTimes.length, `cache hits after reload: ${JSON.stringify(dcr.frameWorkers)}`);
  const cleared = await js(`effectcraft.execute("storage.clear", {what: "diskCache"})`);
  const afterClear = await js(`effectcraft.execute("cache.diskStats", {})`);
  dcr.cleared = { cleared, afterClear };
  check(cleared.entries >= diskTimes.length && afterClear.entries === 0, `storage.clear diskCache: ${JSON.stringify(dcr.cleared)}`);
  await js(`effectcraft.execute("file.openRecent", {index: 0})`);
  const reopened = await js(`effectcraft.execute("comp.info", {}).then(c => c.layers.map(l => l.name))`);
  check(reopened.includes("Persisted Solid") && !reopened.includes("Unsaved Solid"), `Open Recent: ${reopened}`);
  // The imported tone is still in the project and decodes from storage after the reload.
  const tone = await js(`effectcraft.execute("layer.addItem", {item: "tone.wav", time: 0}).then(() => "ok", (e) => String(e))`);
  check(tone === "ok", `tone.wav after reload: ${tone}`);
  report.steps.persistence.openRecent = reopened;

  // PWA: manifest, service worker in control, and an offline reload.
  const pwa = await js(`(async () => {
    const m = await (await fetch("manifest.webmanifest")).json();
    const reg = await navigator.serviceWorker.getRegistration();
    const keys = await caches.keys();
    const cached = keys.length ? (await (await caches.open(keys[0])).keys()).length : 0;
    return {manifest: m.name, icons: m.icons.length, display: m.display, sw: !!(reg && reg.active), controlled: !!navigator.serviceWorker.controller, caches: keys, cached};
  })()`);
  check(pwa.sw && pwa.cached > 4, `service worker: ${JSON.stringify(pwa)}`);
  await send("Network.enable", {});
  await send("Network.emulateNetworkConditions", { offline: true, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
  await send("Page.reload", {});
  await sleep(500);
  const offline = await ready();
  const info3 = await js("effectcraft.info()");
  await send("Network.emulateNetworkConditions", { offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1 });
  report.steps.pwa = { ...pwa, offlineLoad: offline, offlineControlled: info3.serviceWorker, crossOriginIsolated: info3.crossOriginIsolated };
  check(info3.serviceWorker, "offline reload served by the service worker");
  check(info3.crossOriginIsolated, "cross-origin isolated under the service worker");
  report.steps.final2 = { screenshot: await shot("03-offline") };
} catch (e) {
  failed = true;
  report.error = String(e && e.stack || e);
  try { report.errorShot = await shot("error"); } catch {}
}
report.console = logs.slice(-40);
writeFileSync(join(out, "report.json"), JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
try { ws.close(); } catch {}
proc.kill();
process.exit(failed ? 1 : 0);
