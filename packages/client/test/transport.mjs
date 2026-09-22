import assert from 'node:assert/strict';
import { mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';
import { KiriClient } from '../dist/index.js';

async function fixture(mode, run) {
  const root = await mkdtemp(join(tmpdir(), 'kiri-transport-'));
  let client;
  try {
    const binary = join(root, 'engine');
    await writeFile(binary, `#!${process.execPath}
let buffered = Buffer.alloc(0);
process.stdin.on('data', chunk => {
  buffered = Buffer.concat([buffered, chunk]);
  while (buffered.length >= 4 && buffered.length >= 4 + buffered.readUInt32BE()) {
    const size = buffered.readUInt32BE();
    const { id, command } = JSON.parse(buffered.subarray(4, 4 + size));
    buffered = buffered.subarray(4 + size);
    if (command.method === 'hello') {
      const body = Buffer.from(JSON.stringify({ type: 'reply', id, result: { kind: 'hello', version: command.version, capabilities: [] } }));
      const header = Buffer.alloc(4); header.writeUInt32BE(body.length);
      process.stdout.write(Buffer.concat([header, body]));
    } else if (process.env.FIXTURE_MODE === 'eof') process.stdout.end();
  }
});
process.stdin.on('end', () => process.exit(0));
`, { mode: 0o755 });
    client = await KiriClient.connect({ binary, env: { ...process.env, FIXTURE_MODE: mode } });
    await run(client);
  } finally {
    client?.dispose();
    await rm(root, { recursive: true, force: true });
  }
}

test('response EOF rejects pending reads even while the child remains alive', { timeout: 3000 }, async () => {
  await fixture('eof', async client => {
    await assert.rejects(client.discover('/'), error => error.code === 'disconnected');
  });
});

test('a silent read times out, then an unacknowledged cancel closes the transport without replaying a write', { timeout: 3000 }, async t => {
  const timeout = AbortSignal.timeout.bind(AbortSignal);
  t.mock.method(AbortSignal, 'timeout', ms => timeout(ms === 30_000 || ms === 5000 ? 50 : ms));
  await fixture('silent', async client => {
    const write = assert.rejects(client.request({ method: 'stage', repo: 1, paths: [[97]], side: 'worktree' }), error => error.code === 'outcome_unknown');
    await assert.rejects(client.discover('/'), error => error.code === 'timeout');
    await write;
    await assert.rejects(client.discover('/'), error => error.code === 'disconnected');
  });
});
