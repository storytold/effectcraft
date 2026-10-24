// Embedded builds with the policy enabled must reject frame, layer and matte persistence before touching OPFS.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const source = await readFile(new URL('../js/host.js', import.meta.url), 'utf8');
const marker = 'const PERSISTENT_FRAME_CACHE_DISABLED = false;';
assert.ok(source.includes(marker), 'default build preserves its cache policy marker');
const disabledSource = source.replace(marker, 'const PERSISTENT_FRAME_CACHE_DISABLED = true;');
const host = await import('data:text/javascript;base64,' + Buffer.from(disabledSource).toString('base64'));
let touched = 0;
Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { storage: { getDirectory() { touched++; throw new Error('OPFS reached'); } } } });
for (const name of ['frame.ecc', 'layers/layer.ecc', 'mattes/matte.ecc']) {
  test('embedded policy refuses persistent cache read/write for ' + name, async () => {
    await assert.rejects(host.cacheRead(name), /Persistent frame cache is disabled/);
    await assert.rejects(host.cacheWrite(name, new Uint8Array([1,2,3])), /Persistent frame cache is disabled/);
    assert.equal(touched, 0);
  });
}
test('embedded policy listing cannot reopen persistent cache', async () => {
  await assert.rejects(host.cacheList(), /Persistent frame cache is disabled/);
  assert.equal(touched, 0);
});

test('ordinary build retains access to its OPFS cache backend', async () => {
  const ordinary = await import('data:text/javascript;base64,' + Buffer.from(source).toString('base64'));
  await assert.rejects(ordinary.cacheList(), /OPFS reached/);
  assert.equal(touched, 1);
});
