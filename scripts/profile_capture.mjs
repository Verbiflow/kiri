import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';
import { KiriClient } from '../packages/client/dist/index.js';

const [binary, root] = process.argv.slice(2);
assert.ok(binary && root, 'Usage: node scripts/profile_capture.mjs <engine> <repository>');
const run = promisify(execFile);
const output = await mkdtemp(join(tmpdir(), 'kiri-capture-profile-'));
const trace = join(output, 'git.jsonl');
const git = async (...args) => (await run('git', ['--no-optional-locks', '-C', root, ...args], { maxBuffer: 16 * 1024 * 1024 })).stdout.trim();
const indexPath = await git('rev-parse', '--path-format=absolute', '--git-path', 'index');
const indexHash = async () => createHash('sha256').update(await readFile(indexPath)).digest('hex');
const before = { head: await git('rev-parse', 'HEAD'), index: await indexHash() };
let calls = 0;
const client = await KiriClient.connect({ binary: resolve(binary), env: { ...process.env, GIT_OPTIONAL_LOCKS: '0', GIT_TRACE2_EVENT: trace }, model: async () => { calls++; throw new Error('The capture profiler must not invoke models'); } });
try {
  const repo = await client.open(root);
  const start = performance.now();
  const prepared = await repo.prepare(null, {}, AbortSignal.timeout(120_000), 'staged');
  const elapsed = performance.now() - start;
  assert.equal(calls, 0);
  assert.deepEqual({ head: await git('rev-parse', 'HEAD'), index: await indexHash() }, before, 'The repository changed during capture');
  const events = (await readFile(trace, 'utf8')).split('\n').filter(Boolean).map((line) => JSON.parse(line));
  const commands = events.filter((event) => event.event === 'start');
  console.log(JSON.stringify({ elapsed_ms: elapsed, files: prepared.files, patch_bytes: prepared.bytes, unique_chunks: prepared.chunks, estimated_cold_calls: prepared.estimated_calls, actual_model_calls: calls, git_processes: commands.length, patch_processes: commands.filter((event) => event.argv.includes('--patch')).length, index_unchanged: true, trace }, null, 2));
  await client.request({ method: 'release', prepared: prepared.prepared });
} finally { client.dispose(); }
