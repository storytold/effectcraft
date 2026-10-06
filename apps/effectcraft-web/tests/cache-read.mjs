// CPU-only OPFS boundary checks; no browser, graphics device, or large buffers.
import assert from "node:assert/strict";
import { cacheRead } from "../js/host.js";

let reads = 0;
let size = 8;
const dir = {
  async getDirectoryHandle() { return this; },
  async getFileHandle() {
    return { async getFile() {
      return { size, async arrayBuffer() { reads++; return new Uint8Array(size).buffer; } };
    } };
  },
};
Object.defineProperty(globalThis, "navigator", {
  configurable: true,
  value: { storage: { async getDirectory() { return dir; } } },
});

assert.equal((await cacheRead("original.ecc", 8)).length, 8);
assert.equal(reads, 1);
size = 9;
await assert.rejects(cacheRead("layers/original.ecc", 8), /exceeds read limit/);
assert.equal(reads, 1, "oversized files must not request their ArrayBuffer");
for (const limit of [undefined, NaN, Infinity, -1, 0, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
  await assert.rejects(cacheRead("original.ecc", limit), /invalid disk cache read limit/);
}
assert.equal(reads, 1);
size = 0;
assert.equal((await cacheRead("original.ecc", 8)).length, 0);
assert.equal(reads, 2);
console.log("OPFS cache read boundary checks passed");
