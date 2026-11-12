// Page-side helpers of the web app (js/host.js) under Node with a fake DOM, no browser or Wasm
// build needed:
//
//   node apps/effectcraft-web/tests/page.mjs
//
// - Add Files… / Upload Folder…: a picked file that can't be read is listed with its error
//   instead of failing the whole pick silently, and cancelling picks nothing (#226).
// - A lost WebGPU device: the page says so over the canvas and offers Save Project and Reload;
//   the reload writes pending changes to browser storage first, and a failed write is reported
//   instead of reloading (#225).
// - Duplicate cancels the browser's bookmark action on the app canvas, while still delivering
//   the key to the app; other chords and HTML inputs keep their browser behavior (#2495).
// Exits non-zero on failure.
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tick = () => new Promise((r) => setTimeout(r, 0));
globalThis.self = globalThis;

class FakeElement {
  constructor(tag) {
    this.tag = tag;
    this.children = [];
    this.attributes = {};
    this.textContent = "";
  }
  setAttribute(k, v) {
    this.attributes[k] = v;
  }
  append(...c) {
    this.children.push(...c);
  }
  click() {
    this.onclick?.();
  }
  *all() {
    yield this;
    for (const c of this.children) yield* c.all();
  }
}
let picker;
globalThis.document = {
  body: new FakeElement("body"),
  createElement(tag) {
    const el = new FakeElement(tag);
    if (tag === "input") {
      picker = el;
      el.click = () => {};
    }
    return el;
  },
  getElementById(id) {
    return [...this.body.all()].find((e) => e.id === id) ?? null;
  },
};
let reloads = 0;
globalThis.location = { reload: () => reloads++ };

const host = await import(pathToFileURL(join(here, "../js/host.js")));

// #2495: prevent the browser default without swallowing the app's Duplicate key.
const canvas = new EventTarget();
const input = new EventTarget();
host.installCanvasShortcuts(canvas);
let delivered = 0;
canvas.addEventListener("keydown", () => delivered++);
const key = (target, properties) => {
  const event = new Event("keydown", { cancelable: true });
  Object.assign(event, { key: "d", ...properties });
  target.dispatchEvent(event);
  return event.defaultPrevented;
};
assert.equal(key(canvas, { ctrlKey: true }), true);
assert.equal(key(canvas, { metaKey: true, key: "D" }), true);
for (const properties of [{}, { ctrlKey: true, shiftKey: true }, { ctrlKey: true, altKey: true }, { ctrlKey: true, isComposing: true }, { ctrlKey: true, key: "r" }]) {
  assert.equal(key(canvas, properties), false);
}
// Non-Latin layouts: the physical D key types "в", and eframe still reads it as D.
assert.equal(key(canvas, { ctrlKey: true, key: "в", code: "KeyD" }), true);
// Dvorak: the physical D key types "e", which is not Duplicate.
assert.equal(key(canvas, { ctrlKey: true, key: "e", code: "KeyD" }), false);
assert.equal(delivered, 9, "all keys still reach the app");
assert.equal(key(input, { ctrlKey: true }), false, "an external HTML input keeps its shortcut");

// #226: one readable and one unreadable file.
const file = (name, read) => ({ name, webkitRelativePath: "", arrayBuffer: read });
const picked = host.browsePickFiles(false);
picker.files = [
  file("ok.png", async () => new Uint8Array([1, 2, 3]).buffer),
  file("red-corner.png", async () => {
    throw new DOMException("Generated unreadable selected file", "NotReadableError");
  }),
];
await picker.onchange();
const out = await picked;
assert.equal(out.length, 2);
assert.deepEqual([out[0].path, [...out[0].bytes]], ["ok.png", [1, 2, 3]]);
assert.deepEqual(out[1], { path: "red-corner.png", error: "Generated unreadable selected file" });
const cancelled = host.browsePickFiles(false);
picker.oncancel();
assert.deepEqual(await cancelled, [], "cancelling picks nothing");

// #225: the device-lost notice.
const calls = [];
let flushFails = false;
self.effectcraft = {
  flush: async () => {
    calls.push("flush");
    if (flushFails) throw new Error("quota exceeded");
  },
  execute: async (command, params) => {
    calls.push([command, params]);
    if (command === "file.save") throw new Error("no path: use file.saveAs {path}");
  },
};
const notice = () => document.getElementById("device-lost");
const buttons = () => [...notice().all()].filter((e) => e.tag === "button");
const text = () => [...notice().all()].find((e) => e.tag === "p").textContent;

host.showDeviceLost("GPU device lost (Destroyed): destroyed", true);
host.showDeviceLost("again", true);
assert.equal(document.body.children.length, 1, "one notice");
assert.equal([...notice().all()].find((e) => e.tag === "small").textContent, "GPU device lost (Destroyed): destroyed");
assert.match(text(), /Reload to continue/);
assert.deepEqual(
  buttons().map((b) => b.textContent),
  ["Save Project", "Reload"],
);
// Save Project: Save, or Save As for an untitled project (a download).
await buttons()[0].onclick();
assert.deepEqual(calls.splice(0), [
  ["file.save", {}],
  ["file.saveAs", { path: "/Untitled Project.ecproj" }],
]);
// Reload writes pending changes first.
await buttons()[1].onclick();
assert.deepEqual(calls.splice(0), ["flush"]);
assert.equal(reloads, 1);

// A failed write is reported instead of reloading; Reload Anyway reloads.
document.body.children = [];
flushFails = true;
host.showDeviceLost("GPU device lost (Unknown): gone", false);
assert.match(text(), /isn't kept in browser storage/);
await buttons()[1].onclick();
await tick();
assert.equal(reloads, 1, "no reload after a failed write");
assert.match(text(), /quota exceeded/);
assert.equal(buttons()[1].textContent, "Reload Anyway");
buttons()[1].onclick();
assert.equal(reloads, 2);

console.log("page: ok");
