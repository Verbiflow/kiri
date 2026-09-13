import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm, readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { KiriClient, KiriError } from '../dist/index.js';

const binary = process.env.KIRI_ENGINE_BINARY ?? fileURLToPath(new URL('../../../target/debug/kiri-engine', import.meta.url));
const pathBytes = (path) => [...Buffer.from(path)];
const environment = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1', GIT_AUTHOR_NAME: 'SDK Test', GIT_AUTHOR_EMAIL: 'sdk@example.invalid', GIT_COMMITTER_NAME: 'SDK Test', GIT_COMMITTER_EMAIL: 'sdk@example.invalid' };

test('typed sidecar stages, analyzes through host callbacks, and commits only the reviewed selection', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kiri-sdk-'));
  let client;
  const git = (...args) => execFileSync('git', args, { cwd: root, env: environment, encoding: 'utf8' });
  try {
    git('init', '-q'); await mkdir(join(root, 'folder'));
    await writeFile(join(root, 'folder', 'a.rs'), 'fn before() {}\n');
    await writeFile(join(root, 'sibling.rs'), 'fn original() {}\n');
    git('add', '.'); git('commit', '-qm', 'Initial');
    await writeFile(join(root, 'folder', 'a.rs'), 'fn after() {}\n');
    await writeFile(join(root, 'sibling.rs'), 'fn unrelated() {}\n');
    let calls = 0;
    const progress = [];
    client = await KiriClient.connect({ binary, env: environment, progress: (event) => progress.push(event), model: async (call) => {
      calls += 1;
      assert.ok(call.input.includes('fn after()'));
      assert.ok(!call.input.includes('fn unrelated()'));
      return JSON.stringify({ action: 'finish', result: { message: 'refactor: update the selected function' } });
    } });
    const repo = await client.open(root);
    assert.equal((await repo.status()).status.files.length, 2);
    await repo.stage([pathBytes('sibling.rs')]);
    const stage = repo.stage([pathBytes('folder/a.rs')]);
    const ready = repo.prepare([pathBytes('folder')]);
    await stage;
    const evidence = await ready;
    assert.equal(evidence.files, 1);
    const draft = await repo.draft(evidence.prepared, 'host:test-model');
    assert.equal(calls, 1);
    assert.ok(progress.some((event) => event.event.phase === 'reading'));
    await writeFile(join(root, 'folder', 'a.rs'), 'fn later_unstaged_edit() {}\n');
    const oid = await repo.commit(draft);
    assert.equal(oid, git('rev-parse', 'HEAD').trim());
    assert.equal(git('diff', '--cached', '--name-only').trim(), 'sibling.rs');
    assert.equal(git('show', 'HEAD:folder/a.rs'), 'fn after() {}\n');
    assert.equal(await readFile(join(root, 'folder', 'a.rs'), 'utf8'), 'fn later_unstaged_edit() {}\n');
    await assert.rejects(repo.commit(draft), KiriError);
    await client.request({ method: 'release', prepared: evidence.prepared });
    await repo.close();
  } finally { client?.dispose(); await rm(root, { recursive: true, force: true }); }
});

test('typed sidecar plans and applies an exact staged selection', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kiri-sdk-plan-'));
  let client;
  const git = (...args) => execFileSync('git', args, { cwd: root, env: environment, encoding: 'utf8' });
  try {
    git('init', '-q'); await mkdir(join(root, 'selected'));
    await writeFile(join(root, 'selected', 'a.txt'), 'first\n');
    await writeFile(join(root, 'selected', 'b.txt'), 'second\n');
    await writeFile(join(root, 'unrelated.txt'), 'later\n');
    git('add', '.');
    client = await KiriClient.connect({ binary, env: environment, model: async (call) => {
      const required = call.schema.properties.result.properties.assignments.required;
      assert.deepEqual(required, ['u0', 'u1']);
      return JSON.stringify({ action: 'finish', result: { commits: [
        { message: 'feat: add the first selected change', reason: 'The first file is independently useful.' },
        { message: 'feat: add the second selected change', reason: 'The second file is independently useful.' },
      ], assignments: { u0: 0, u1: 1 } }, requests: [], notes: '' });
    } });
    const repo = await client.open(root);
    const evidence = await repo.prepare([pathBytes('selected')], {}, undefined, 'staged');
    const plan = await repo.plan(evidence.prepared, 'host:test-model');
    assert.deepEqual(plan.files.map((file) => Buffer.from(file.path).toString()), ['selected/a.txt', 'selected/b.txt']);
    await client.request({ method: 'release', prepared: evidence.prepared });
    const commits = await repo.applyPlan(plan);
    assert.equal(commits.length, 2);
    assert.equal(git('show', '--format=', '--name-only', commits[0]).trim(), 'selected/a.txt');
    assert.equal(git('show', '--format=', '--name-only', commits[1]).trim(), 'selected/b.txt');
    assert.equal(git('diff', '--cached', '--name-only').trim(), 'unrelated.txt');
  } finally { client?.dispose(); await rm(root, { recursive: true, force: true }); }
});

test('working-tree proposals capture privately and commit reviewed content on an unborn branch', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kiri-sdk-working-'));
  const git = (...args) => execFileSync('git', args, { cwd: root, env: environment, encoding: 'utf8' });
  let client;
  try {
    git('init', '-q'); await writeFile(join(root, 'file.txt'), 'Working evidence\n');
    await writeFile(join(root, 'sibling.txt'), 'Unrelated initial content\n');
    const objects = git('count-objects', '-v');
    client = await KiriClient.connect({ binary, env: environment, model: async (call) => {
      assert.ok(call.input.includes('Working evidence'));
      return JSON.stringify({ action: 'finish', result: { message: 'feat: capture the initial content' } });
    } });
    const repo = await client.open(root);
    const evidence = await repo.prepare([pathBytes('file.txt')]);
    const draft = await repo.draft(evidence.prepared, 'host:fixture');
    assert.equal(draft.snapshot.source, 'worktree');
    assert.equal(git('diff', '--cached', '--name-only'), '');
    assert.equal(git('count-objects', '-v'), objects);
    await writeFile(join(root, 'sibling.txt'), 'Unrelated later edit\n');
    await writeFile(join(root, 'file.txt'), 'Unreviewed later edit\n');
    await assert.rejects(repo.commit(draft), /changed since review/);
    await writeFile(join(root, 'file.txt'), 'Working evidence\n');
    await repo.commit(draft);
    assert.equal(git('show', 'HEAD:file.txt'), 'Working evidence\n');
    assert.equal(git('status', '--porcelain').trim(), '?? sibling.txt');
    assert.equal(await readFile(join(root, 'sibling.txt'), 'utf8'), 'Unrelated later edit\n');
  } finally { client?.dispose(); await rm(root, { recursive: true, force: true }); }
});

test('published graph inspection follows re-chunked sources and ends with its lease', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kiri-sdk-graph-'));
  let client; let rejected = 0;
  try {
    execFileSync('git', ['init', '-q'], { cwd: root, env: environment });
    await writeFile(join(root, 'file.txt'), Array.from({ length: 1200 }, (_, i) => `Complete unique source line ${i}\n`).join(''));
    client = await KiriClient.connect({ binary, env: environment, model: async (call) => {
      if (call.system.length + call.input.length > 6500) { rejected++; throw new KiriError('context', 'Fixture context limit'); }
      return JSON.stringify(call.system.includes('Keep the summary under') ? { summary: 'The supplied source segments changed.' } : { action: 'finish', result: { message: 'feat: retain inspectable evidence' }, requests: [], notes: '' });
    } });
    const repo = await client.open(root); await repo.stage([pathBytes('file.txt')]);
    const prepared = await repo.prepare(null, { chunk_bytes: 64000, max_calls: 100 });
    const original = await client.request({ method: 'manifest', prepared: prepared.prepared });
    assert.equal(original.kind, 'manifest');
    await repo.draft(prepared.prepared, 'bounded-model');
    const manifest = await client.request({ method: 'manifest', prepared: prepared.prepared });
    assert.equal(manifest.kind, 'manifest'); assert.ok(rejected > 0); assert.ok(manifest.roots.length);
    assert.notDeepEqual(manifest.units.map((unit) => unit.id), original.units.map((unit) => unit.id));
    const sources = new Set(manifest.units.map((unit) => unit.id)); const reached = new Set(); const pending = [...manifest.roots];
    while (pending.length) {
      const id = pending.pop();
      if (sources.has(id)) {
        const source = await client.request({ method: 'inspect', prepared: prepared.prepared, request: { kind: 'source', id, offset: 0, limit: 128000 } });
        assert.equal(source.kind, 'evidence'); assert.ok(source.page.total_bytes > 0); reached.add(id);
      } else {
        const node = await client.request({ method: 'inspect', prepared: prepared.prepared, request: { kind: 'node', id, offset: 0, limit: 128000 } });
        assert.equal(node.kind, 'evidence'); assert.equal(node.page.next_offset, null);
        pending.push(...JSON.parse(node.page.data).children);
      }
    }
    assert.deepEqual(reached, sources);
    await client.request({ method: 'release', prepared: prepared.prepared });
    await assert.rejects(client.request({ method: 'manifest', prepared: prepared.prepared }), /Unknown evidence/);
  } finally { client?.dispose(); await rm(root, { recursive: true, force: true }); }
});

test('cancel propagates into an in-flight host model without writing Git', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kiri-sdk-cancel-'));
  let client;
  try {
    execFileSync('git', ['init', '-q'], { cwd: root, env: environment });
    await writeFile(join(root, 'file.txt'), 'Evidence\n');
    let started; const ready = new Promise((resolve) => { started = resolve; });
    let stopped; const aborted = new Promise((resolve) => { stopped = resolve; });
    client = await KiriClient.connect({ binary, env: environment, model: async (_call, signal) => {
      started(); await new Promise((_resolve, reject) => signal.addEventListener('abort', () => { stopped(); reject(new Error('cancelled')); }, { once: true }));
      return '';
    } });
    const repo = await client.open(root); await repo.stage([pathBytes('file.txt')]);
    const evidence = await repo.prepare(null);
    const controller = new AbortController();
    const pending = repo.draft(evidence.prepared, 'host:test', '', controller.signal);
    const rejected = assert.rejects(pending, (error) => error instanceof KiriError && error.code === 'cancelled');
    await ready; controller.abort(); await rejected; await aborted;
    assert.throws(() => execFileSync('git', ['rev-parse', '--verify', 'HEAD'], { cwd: root, env: environment, stdio: 'pipe' }));
  } finally { client?.dispose(); await rm(root, { recursive: true, force: true }); }
});

test('status polling reports unchanged revisions without resending the inventory', async () => {
  const root = await mkdtemp(join(tmpdir(), 'kiri-sdk-revision-'));
  const git = (...args) => execFileSync('git', args, { cwd: root, env: environment, encoding: 'utf8' });
  let client;
  try {
    git('init', '-q'); await writeFile(join(root, 'a.txt'), 'one\n'); git('add', '.'); git('commit', '-qm', 'Initial');
    await writeFile(join(root, 'a.txt'), 'two\n');
    client = await KiriClient.connect({ binary, env: environment });
    const repo = await client.open(root);
    const first = await repo.status(true);
    assert.equal(first.status.files.length, 1);
    assert.equal(typeof first.status.files[0].index_oid, 'string');
    const unchanged = await repo.statusSince(first.revision);
    assert.equal(unchanged.kind, 'unchanged');
    assert.equal(unchanged.revision, first.revision);
    await writeFile(join(root, 'b.txt'), 'new\n');
    const changed = await repo.statusSince(first.revision);
    assert.equal(changed.kind, 'status');
    assert.ok(changed.revision > first.revision);
    assert.equal(changed.status.files.length, 2);
    const preview = await repo.preview([...Buffer.from('a.txt')], 'worktree');
    assert.ok(Buffer.from(preview.patch).toString().includes('+two'));
    await writeFile(join(root, 'a.txt'), 'three\n');
    await repo.status(true);
    const edited = await repo.preview([...Buffer.from('a.txt')], 'worktree');
    assert.ok(Buffer.from(edited.patch).toString().includes('+three'));
    await repo.close();
  } finally { client?.dispose(); await rm(root, { recursive: true, force: true }); }
});
