// Clean-room scalar property comparison. Raw host observations remain gitignored.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
// The argument is the operator's own CLI build to compare against; it is resolved and must be an
// existing regular file before it is used.
const cli = path.resolve(process.argv[2] ?? ''); // NOSONAR
// Reject anything that is not an existing regular file before the checker uses it.
if (!process.argv[2] || !fs.statSync(cli, { throwIfNoEntry: false })?.isFile()) { // NOSONAR
  throw new Error('Usage: node examples/check-ae-ease.mjs <aesync/server/dist/cli.js>');
}
const tolerance = 1e-7;
function run(command, args) {
  // A developer-run comparison script; spawnSync never uses a shell and every command is either
  // a fixed binary (`cargo`, `process.execPath`) or the CLI script path validated above.
  const result = spawnSync(command, args, { cwd: root, windowsHide: true, encoding: 'utf8', maxBuffer: 8 * 1024 * 1024 }); // NOSONAR
  if (result.status !== 0) throw new Error(result.stderr || result.stdout || 'Process failed');
  return JSON.parse(result.stdout.replace(/^\uFEFF/, '').trim());
}
const ec = run('cargo', ['run', '--quiet', '-p', 'effectcraft-keyframe', '--example', 'ae_ease_samples']);
const ae = run(process.execPath, [cli, 'run', '-f', path.join(root, 'examples', 'ae-ease-oracle.jsx'), '--raw']);
const local = path.join(root, 'plan', 'aftereffects', 'ref', 'ease');
fs.mkdirSync(local, { recursive: true });
fs.writeFileSync(path.join(local, 'ae-observations.json'), JSON.stringify(ae, null, 2) + '\n');
fs.writeFileSync(path.join(local, 'effectcraft-samples.json'), JSON.stringify(ec, null, 2) + '\n');
if (ae.cases.length !== ec.cases.length) throw new Error('Case count mismatch');
const scores = ae.cases.map(reference => {
  const candidate = ec.cases.find(c => c.name === reference.name);
  if (!candidate || reference.values.length !== candidate.values.length) throw new Error('Missing or truncated case: ' + reference.name);
  for (const key of ['from', 'to', 'outSpeed', 'outInfluence', 'inSpeed', 'inInfluence']) {
    if (!Number.isFinite(reference[key]) || Math.abs(reference[key] - candidate[key]) > 1e-9) throw new Error('Case metadata mismatch: ' + reference.name + '/' + key);
  }
  for (const key of ['outSpeed', 'outInfluence', 'inSpeed', 'inInfluence']) {
    if (!Number.isFinite(reference.applied[key]) || Math.abs(reference.applied[key] - reference[key]) > 1e-9) throw new Error('AE changed requested ease: ' + reference.name + '/' + key);
  }
  if (reference.applied.outAuto || reference.applied.inAuto || reference.applied.outContinuous || reference.applied.inContinuous) throw new Error('Unexpected automatic ease');
  const errors = reference.values.map((value, i) => {
    if (!Number.isFinite(value) || !Number.isFinite(candidate.values[i]) || reference.times[i] !== candidate.times[i]) throw new Error('Invalid or mismatched sample: ' + reference.name);
    return Math.abs(value - candidate.values[i]);
  });
  const maxAbsoluteError = Math.max(...errors);
  return { name: reference.name, samples: errors.length, maxAbsoluteError, pass: maxAbsoluteError <= tolerance };
});
const report = { generatedAt: new Date().toISOString(), aeVersion: ae.aeVersion, property: 'Rotation (scalar)', tolerance, cases: scores, pass: scores.every(c => c.pass) };
fs.mkdirSync(path.join(root, 'docs', 'fidelity'), { recursive: true });
fs.writeFileSync(path.join(root, 'docs', 'fidelity', 'keyframe-ease.json'), JSON.stringify(report, null, 2) + '\n');
console.log(JSON.stringify(report, null, 2));
if (!report.pass) process.exitCode = 1;
