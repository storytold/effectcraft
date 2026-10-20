// Job-worker plumbing of the web app (js/host.js, web/worker.js) under Node with a fake Worker,
// no browser or Wasm build needed:
//
//   node apps/effectcraft-web/tests/workers.mjs
//
// - A reused worker is sent a file again when it was replaced, even by one of the same size
//   (#218), and not when it is unchanged.
// - A worker whose start fails fails its job and is terminated, instead of the job waiting for
//   "ready" forever (#216): host.js on the worker's error, worker.js re-raising the failed start.
// - A `new Worker` that throws (a sandboxed iframe's opaque origin, #1586) fails the job, and a
//   frame worker is reported dead, instead of the exception escaping into startup.
// Exits non-zero on failure.
import assert from "node:assert/strict";
import { copyFileSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tick = () => new Promise((r) => setTimeout(r, 0));
globalThis.self = globalThis;

// A Worker that starts (or fails to) on "init" and finishes each job at once.
let failStart = false;
const workers = [];
class FakeWorker {
  constructor() {
    this.posted = [];
    this.terminated = false;
    workers.push(this);
  }
  postMessage(m) {
    this.posted.push(m);
    setTimeout(() => {
      if (m.type === "init") {
        if (failStart) this.onerror?.({ message: "worker failed to start" });
        else this.onmessage?.({ data: { type: "ready" } });
      } else if (m.type === "job") {
        this.onmessage?.({ data: { type: "reply", json: "{}", done: true } });
      }
    });
  }
  terminate() {
    this.terminated = true;
  }
}
globalThis.Worker = FakeWorker;

const host = await import(pathToFileURL(join(here, "../js/host.js")));
const run = (id, files) =>
  new Promise((resolve) => host.workerRun(id, "{}", files, "http://localhost/", (kind, json) => kind !== "file" && resolve({ kind, json })));
const sent = (w) => w.posted.filter((m) => m.type === "file").map((m) => m.bytes[0]);

// #218: equal-length replacement.
const red = new Uint8Array(512).fill(1);
const blue = new Uint8Array(512).fill(2);
assert.equal((await run(1, [["/collision.png", red, 1]])).kind, "reply");
assert.equal((await run(2, [["/collision.png", blue, 2]])).kind, "reply");
assert.equal((await run(3, [["/collision.png", blue, 2]])).kind, "reply");
assert.equal(workers.length, 1, "the worker is reused");
assert.deepEqual(sent(workers[0]), [1, 2], "the replacement is sent once, the unchanged file not again");

// #216: a worker that fails to start.
failStart = true;
const slow = run(4, []); // the idle worker takes this one
const failed = await run(5, []);
await slow;
assert.equal(failed.kind, "error");
assert.ok(workers[1].terminated, "the failed worker is terminated");
assert.equal(host.workerStats().running, 0);

// A `new Worker` that throws (a sandboxed iframe's opaque origin): the job fails, and a frame
// worker is reported dead, instead of the exception escaping into startup.
failStart = false;
const RealWorker = globalThis.Worker;
globalThis.Worker = class {
  constructor() {
    throw new Error("SecurityError: cannot be accessed from origin 'null'");
  }
};
// (Take the idle workers out of play first, so that a new one has to be built.)
for (let n = host.workerStats().idle, id = 100; n > 0; n--, id++) {
  host.workerRun(id, "{}", [], "http://localhost/", () => {});
  host.workerCancel(id);
}
assert.equal(host.workerStats().idle, 0);
const threw = await run(6, []);
assert.equal(threw.kind, "error");
assert.match(threw.json, /SecurityError/);
assert.equal(host.workerStats().running, 0);
let frameCall = null;
const frameDone = new Promise((resolve) => {
  frameCall = host.frameWorkerStart("http://localhost/", (index, json, bytes, error) => resolve({ index, error }), false);
});
const dead = await frameDone;
assert.equal(dead.index, frameCall, "the dead worker keeps its index");
assert.match(dead.error, /SecurityError/);
host.frameWorkerPost(frameCall, { type: "frame" }); // no worker behind it: ignored
globalThis.Worker = RealWorker;

// #216, worker side: a rejected start is re-raised where the page's `onerror` sees it.
const dir = mkdtempSync(join(tmpdir(), "ec-worker-"));
copyFileSync(join(here, "../web/worker.js"), join(dir, "worker.js"));
writeFileSync(
  join(dir, "effectcraft_web.js"),
  `export default async function init() {}
export function workerInit() {}
export async function workerJobInit() { throw new Error("no device"); }
export async function workerFrameInit() {}`,
);
const posted = [];
globalThis.postMessage = (m) => posted.push(m);
// A browser's Worker `error` event only sees thrown errors, not unhandled rejections (Node would
// turn one into an uncaught exception unless it is listened to).
const unhandled = [];
process.on("unhandledRejection", (e) => unhandled.push(e));
const raised = new Promise((resolve) => {
  process.once("uncaughtException", resolve);
  setTimeout(() => resolve(new Error("nothing was raised")), 1000);
});
await import(pathToFileURL(join(dir, "worker.js")));
self.onmessage({ data: { type: "init", base: "http://localhost/" } });
const err = await raised;
await tick();
// (So a failed assertion below fails the run.)
process.removeAllListeners("unhandledRejection");
process.removeAllListeners("uncaughtException");
assert.deepEqual(unhandled, [], "the failed start is handled, not left as an unhandled rejection");
assert.equal(err.message, "no device");
assert.ok(!posted.some((m) => m.type === "ready"), "no ready after a failed start");

console.log("workers: ok");
