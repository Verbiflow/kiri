import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { KiriClient } from '../packages/client/dist/index.js';

const run = promisify(execFile);
const [binary, path, count = '20', mode] = process.argv.slice(2);
assert.ok(mode === undefined || mode === '--capture', 'Only --capture is supported after the run count');
assert.ok(binary && path, 'Provide a release engine and a disposable benchmark repository');
const runs = Number(count);
assert.ok(Number.isInteger(runs) && runs >= 5 && runs <= 200);
const output = await mkdtemp(join(tmpdir(), 'kiri-service-benchmark-'));
const trace = join(output, 'git.jsonl');
const env = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', GIT_OPTIONAL_LOCKS: '0' };
const git = async (...args) => (await run('git', ['--no-optional-locks', ...args], { cwd: path, env, maxBuffer: 16 * 1024 * 1024 })).stdout;
const before = { head: await git('rev-parse', 'HEAD'), index: await git('diff', '--cached', '--raw'), working: await git('status', '--porcelain=v2', '-z') };
const start = performance.now();
const client = await KiriClient.connect({ binary: resolve(binary), env: { ...env, GIT_TRACE2_EVENT: trace } });
const report = { runs, engine_handshake_ms: performance.now() - start, operations: {} };
const starts = async () => (await readFile(trace, 'utf8').catch(() => '')).split('\n').filter(Boolean).map((line) => JSON.parse(line)).filter((event) => event.event === 'start').length;
async function measure(name, operation) {
  const calls = await starts(); const values = [];
  for (let i = 0; i < runs; i++) { const at = performance.now(); await operation(); values.push(performance.now() - at); }
  const sorted = [...values].sort((a, b) => a - b);
  report.operations[name] = { p50_ms: sorted[Math.floor(runs / 2)], p95_ms: sorted[Math.ceil(runs * 0.95) - 1], git_processes: await starts() - calls, samples_ms: values };
}
try {
  const repo = await client.open(path);
  const inventory = await repo.status(true);
  report.files = inventory.status.files.length;
  const selected = inventory.status.files[0]; assert.ok(selected);
  await measure('fresh_status', () => repo.status(true));
  await repo.status(true);
  await measure('cached_status', () => repo.status(false));
  await repo.preview(selected.path, 'worktree');
  await measure('cached_preview', () => repo.preview(selected.path, 'worktree'));
  const compare = () => client.request({ method: 'compare', repo: repo.id, path: selected.path, comparison: { kind: 'head_to_worktree' } });
  assert.equal((await compare()).kind, 'compared');
  await measure('cached_compare', compare);
  await measure('native_git_status', () => git('-c', 'status.renames=false', 'status', '--porcelain=v2', '--branch', '--untracked-files=all', '-z'));
  report.operations.native_git_status.git_processes = runs;
  if (mode === '--capture') {
    const calls = await starts(); const at = performance.now();
    const capture = await repo.prepare(null);
    report.capture = { elapsed_ms: performance.now() - at, git_processes: await starts() - calls, files: capture.files, bytes: capture.bytes, chunks: capture.chunks, estimated_model_calls: capture.estimated_calls };
    await client.request({ method: 'release', prepared: capture.prepared });
  }
  assert.deepEqual({ head: await git('rev-parse', 'HEAD'), index: await git('diff', '--cached', '--raw'), working: await git('status', '--porcelain=v2', '-z') }, before);
  console.log(JSON.stringify({ ...report, trace, scope: 'Read-only repository service, no rendering or model calls; Git tracing enabled for process counts.' }, null, 2));
} finally { client.dispose(); }
