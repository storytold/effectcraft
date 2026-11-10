// The IndexedDB storage backend of the web app (js/host.js) under Node with a fake IndexedDB, no
// browser or Wasm build needed:
//
//   node apps/effectcraft-web/tests/storage.mjs
//
// A write is acknowledged only once its transaction has committed, not when its request
// succeeded: a transaction that aborts afterwards rolls the write back, and the write must be
// reported as failed so the flush keeps it for a retry (#1242). Exits non-zero on failure.
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tick = () => new Promise((r) => setTimeout(r, 0));
globalThis.self = globalThis;

// A small IndexedDB: requests succeed (or fail) one task after they're made, and a readwrite
// transaction commits once its requests are done, unless it was aborted (which rolls back its
// writes). `fault` makes the next request do something else.
let fault = null; // "abortAfterSuccess" | "requestError" | "throw"
const databases = new Map();

class Request {
  constructor(tx) {
    this.transaction = tx;
    this.result = undefined;
    this.error = null;
  }
}

class Store {
  constructor(tx, rows) {
    this.tx = tx;
    this.rows = rows;
  }
  request(apply) {
    const tx = this.tx;
    if (tx.finished) throw new DOMException("the transaction has finished", "TransactionInactiveError");
    const f = fault;
    fault = null;
    if (f === "throw") throw new DOMException("the value can't be cloned", "DataCloneError");
    const r = new Request(tx);
    tx.pending++;
    setTimeout(() => {
      tx.pending--;
      if (tx.finished) return;
      if (f === "requestError") {
        r.error = new DOMException("the key already exists", "ConstraintError");
        const e = { target: r, defaultPrevented: false, preventDefault() { this.defaultPrevented = true; } };
        r.onerror?.(e);
        tx.onerror?.(e);
        if (!e.defaultPrevented) tx.fail(r.error);
        return;
      }
      r.result = apply(tx.staged);
      r.onsuccess?.({ target: r });
      if (f === "abortAfterSuccess") tx.abort();
      tx.maybeCommit();
    }, 0);
    return r;
  }
  put(v) {
    return this.request((rows) => { rows.set(v[this.keyPath], structuredClone(v)); return v[this.keyPath]; });
  }
  delete(k) {
    return this.request((rows) => { rows.delete(k); });
  }
  get(k) {
    return this.request((rows) => rows.get(k));
  }
  getAll() {
    return this.request((rows) => [...rows.values()]);
  }
}

class Transaction {
  constructor(db, name) {
    this.db = db;
    this.name = name;
    this.staged = new Map(db.stores.get(name));
    this.pending = 0;
    this.finished = false;
    this.error = null;
    setTimeout(() => this.maybeCommit(), 0);
  }
  objectStore(name) {
    const s = new Store(this, this.staged);
    s.keyPath = this.db.keyPaths.get(name);
    return s;
  }
  maybeCommit() {
    if (this.finished || this.pending > 0) return;
    this.finished = true;
    this.db.stores.set(this.name, this.staged);
    setTimeout(() => this.oncomplete?.(), 0);
  }
  fail(error) {
    this.error = error;
    this.abort();
  }
  abort() {
    if (this.finished) throw new DOMException("the transaction has finished", "InvalidStateError");
    this.finished = true;
    setTimeout(() => this.onabort?.(), 0);
  }
}

class Database {
  constructor() {
    this.stores = new Map();
    this.keyPaths = new Map();
  }
  createObjectStore(name, { keyPath }) {
    this.stores.set(name, new Map());
    this.keyPaths.set(name, keyPath);
  }
  transaction(name) {
    if (!this.stores.has(name)) throw new DOMException(`no store ${name}`, "NotFoundError");
    return new Transaction(this, name);
  }
}

globalThis.indexedDB = {
  open(name) {
    const r = new Request(null);
    setTimeout(() => {
      let db = databases.get(name);
      if (!db) {
        db = new Database();
        databases.set(name, db);
        r.result = db;
        r.onupgradeneeded?.();
      }
      r.result = db;
      r.onsuccess?.({ target: r });
    }, 0);
    return r;
  },
};

const host = await import(pathToFileURL(join(here, "../js/host.js")));
assert.equal(await host.storeOpen("indexeddb"), "indexeddb");
const stored = (key) => databases.get("effectcraft").stores.get("entries").get(key);
const bytes = (s) => new TextEncoder().encode(s);
const text = (row) => row && new TextDecoder().decode(new Uint8Array(row.data));

// A healthy write resolves once committed, and the store has it then.
await host.storePut("project.json", bytes("v1"), 1);
assert.equal(text(stored("project.json")), "v1", "a resolved put is committed");

// The put's request succeeds, then the transaction aborts: the put fails, the old value stays.
fault = "abortAfterSuccess";
await assert.rejects(host.storePut("project.json", bytes("v2"), 2), { name: "AbortError" }, "an aborted put is reported");
assert.equal(text(stored("project.json")), "v1", "the aborted put was rolled back");

// The same for a delete: it fails and the entry stays.
fault = "abortAfterSuccess";
await assert.rejects(host.storeDelete("project.json"), { name: "AbortError" }, "an aborted delete is reported");
assert.equal(text(stored("project.json")), "v1", "the aborted delete was rolled back");

// A failed request aborts its transaction with the request's error.
fault = "requestError";
await assert.rejects(host.storePut("project.json", bytes("v3"), 3), { name: "ConstraintError" });
assert.equal(text(stored("project.json")), "v1");

// A request that throws at once (a value that can't be stored) fails the write.
fault = "throw";
await assert.rejects(host.storePut("project.json", bytes("v4"), 4), { name: "DataCloneError" });
assert.equal(text(stored("project.json")), "v1");

// The retry after a failure commits the latest value, and a later delete removes it.
await host.storePut("project.json", bytes("v5"), 5);
assert.equal(text(stored("project.json")), "v5");
await host.storeDelete("project.json");
assert.equal(stored("project.json"), undefined, "a resolved delete is committed");
const all = await host.storeLoadAll();
assert.deepEqual(all, [], "nothing else was written");

await tick();
console.log("storage: ok");
