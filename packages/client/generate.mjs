import { execFileSync } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { compile } from 'json-schema-to-typescript';

const binary = process.env.KIRI_ENGINE_BINARY ?? fileURLToPath(new URL('../../target/debug/kiri-engine', import.meta.url));
const schema = JSON.parse(execFileSync(binary, ['--schema'], { encoding: 'utf8', maxBuffer: 4 * 1024 * 1024 }));
const directory = new URL('./src/', import.meta.url);
await mkdir(directory, { recursive: true });
await writeFile(new URL('schema.json', directory), JSON.stringify(schema));
for (const name of ['request', 'frame']) {
  const types = await compile(schema[name], name === 'request' ? 'Request' : 'Frame', { bannerComment: '', unknownAny: true, style: { singleQuote: true } });
  await writeFile(new URL(`${name}.ts`, directory), types);
}
