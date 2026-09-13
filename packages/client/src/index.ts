import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { Socket } from 'node:net';
import { Ajv2020 } from 'ajv/dist/2020.js';
import schema from './schema.json' with { type: 'json' };
import type { Command, Request, AnalysisOptions } from './request.js';
import type { Frame, ResultValue, CommitDraft, CommitPlan } from './frame.js';
export type { Command, Request, AnalysisOptions, Inspection, Comparison, RepoPath } from './request.js';
export type CaptureScope = NonNullable<Extract<Command, { method: 'prepare' }>['scope']>;
export type { Frame, ResultValue, CommitDraft, CommitPlan, RepoStatus, Progress } from './frame.js';

export type ModelCall = Extract<Frame, { type: 'model_call' }>;
export interface ClientOptions {
  binary: string;
  env?: NodeJS.ProcessEnv;
  cacheDirectory?: string;
  model?: (call: ModelCall, signal: AbortSignal) => Promise<string>;
  progress?: (event: Extract<Frame, { type: 'progress' }>) => void;
}
export class KiriError extends Error {
  constructor(public readonly code: string, message: string) { super(message); this.name = 'KiriError'; }
}
type Pending = { resolve: (result: ResultValue) => void; reject: (error: Error) => void; controller: AbortController; mutation: boolean; cleanup: () => void };
const MAX_FRAME = 16 * 1024 * 1024;
const ajv = new Ajv2020({ strict: true });
for (const [name, maximum] of Object.entries({ uint8: 255, uint32: 4_294_967_295, uint64: Number.MAX_SAFE_INTEGER, uint: Number.MAX_SAFE_INTEGER })) {
  ajv.addFormat(name, { type: 'number', validate: (value: number) => Number.isSafeInteger(value) && value >= 0 && value <= maximum });
}
const validateFrame = ajv.compile<Frame>(schema.frame);
const validateRequest = ajv.compile<Request>(schema.request);

export class KiriClient {
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly pending = new Map<number, Pending>();
  private next = 1;
  private header = Buffer.alloc(4);
  private headerBytes = 0;
  private body: Buffer | undefined;
  private bodyBytes = 0;
  private stopped = false;
  private writes: Promise<void> = Promise.resolve();
  private readonly options: ClientOptions;
  private constructor(options: ClientOptions) {
    this.options = options;
    this.child = spawn(options.binary, options.cacheDirectory ? ['--cache-dir', options.cacheDirectory] : [], { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true, env: options.env });
    this.child.stdin.on('error', () => this.fail('Kiri transport disconnected'));
    this.child.stdout.on('error', () => this.fail('Kiri transport disconnected'));
    this.child.stderr.resume();
    this.child.stdout.on('data', (bytes: Buffer) => this.consume(bytes));
    this.child.once('error', () => this.fail('Could not start the Kiri engine'));
    this.child.once('exit', () => this.fail('Kiri engine disconnected'));
    this.child.unref();
    for (const stream of [this.child.stdin, this.child.stdout, this.child.stderr]) if (stream instanceof Socket) stream.unref();
  }
  static async connect(options: ClientOptions): Promise<KiriClient> {
    const client = new KiriClient(options);
    try {
      const reply = await client.request({ method: 'hello', version: schema.version, schema_hash: schema.schema_hash }, AbortSignal.timeout(10_000));
      if (reply.kind !== 'hello' || reply.version !== schema.version) throw new KiriError('protocol_mismatch', 'Incompatible Kiri engine');
      return client;
    } catch (error) { client.dispose(); throw error; }
  }
  async open(path: string): Promise<KiriRepository> {
    const reply = await this.request({ method: 'open', path });
    if (reply.kind === 'not_repository') throw new KiriError('not_repository', 'This folder is not a Git working tree');
    if (reply.kind !== 'opened') throw new KiriError('protocol', 'Expected repository handle');
    return new KiriRepository(this, reply.repo, reply.root);
  }
  async discover(path: string): Promise<KiriRepository | null> {
    const reply = await this.request({ method: 'open', path });
    if (reply.kind === 'not_repository') return null;
    if (reply.kind !== 'opened') throw new KiriError('protocol', 'Expected repository discovery');
    return new KiriRepository(this, reply.repo, reply.root);
  }
  request(command: Command, signal?: AbortSignal): Promise<ResultValue> {
    if (this.stopped) return Promise.reject(new KiriError('disconnected', 'Kiri engine is closed'));
    if (signal?.aborted) return Promise.reject(new KiriError('cancelled', 'Request cancelled'));
    if (this.pending.size >= 128 && command.method !== 'model_result' && command.method !== 'cancel') return Promise.reject(new KiriError('busy', 'Too many pending Kiri requests'));
    const id = this.next++;
    const request: Request = { id, command };
    if (!validateRequest(request)) return Promise.reject(new KiriError('invalid_request', 'Invalid Kiri request'));
    const mutation = command.method === 'stage' || command.method === 'commit' || command.method === 'apply_plan' || command.method === 'stage_all' || command.method === 'commit_message' || command.method === 'push';
    return new Promise((resolve, reject) => {
      const controller = new AbortController();
      const abort = () => {
        if (mutation) return;
        controller.abort(); this.pending.delete(id); signal?.removeEventListener('abort', abort);
        void this.request({ method: 'cancel', request: id }).catch(() => undefined);
        reject(new KiriError('cancelled', 'Read or analysis cancelled'));
      };
      signal?.addEventListener('abort', abort, { once: true });
      if (this.child.stdout instanceof Socket) this.child.stdout.ref();
      this.pending.set(id, { resolve, reject, controller, mutation, cleanup: () => signal?.removeEventListener('abort', abort) });
      const bytes = Buffer.from(JSON.stringify(request));
      if (bytes.length > MAX_FRAME) { this.settle(id, new KiriError('limit', 'Request exceeds the frame budget')); return; }
      const header = Buffer.alloc(4); header.writeUInt32BE(bytes.length);
      this.writes = this.writes.then(() => new Promise<void>((done, failed) => {
        this.child.stdin.write(header);
        this.child.stdin.write(bytes, (error) => error ? failed(error) : done());
      })).catch(() => this.fail('Kiri transport write failed'));
    });
  }
  private consume(chunk: Buffer): void {
    try {
      let offset = 0;
      while (offset < chunk.length) {
        if (!this.body) {
          const count = Math.min(4 - this.headerBytes, chunk.length - offset);
          chunk.copy(this.header, this.headerBytes, offset, offset + count);
          this.headerBytes += count; offset += count;
          if (this.headerBytes < 4) continue;
          const length = this.header.readUInt32BE();
          if (length === 0 || length > MAX_FRAME) throw new Error('Invalid frame length');
          this.body = Buffer.allocUnsafe(length); this.bodyBytes = 0;
        }
        const count = Math.min(this.body.length - this.bodyBytes, chunk.length - offset);
        chunk.copy(this.body, this.bodyBytes, offset, offset + count);
        this.bodyBytes += count; offset += count;
        if (this.bodyBytes === this.body.length) {
          const value: unknown = JSON.parse(this.body.toString('utf8'));
          if (!validateFrame(value)) throw new Error('Invalid response schema');
          this.body = undefined; this.headerBytes = 0; this.bodyBytes = 0;
          this.dispatch(value);
        }
      }
    } catch { this.fail('Invalid Kiri protocol response'); }
  }
  private dispatch(frame: Frame): void {
    if (frame.type === 'reply') { this.settle(frame.id, frame.result); return; }
    if (frame.type === 'error') { this.settle(frame.id, new KiriError(frame.code, frame.message)); return; }
    if (frame.type === 'progress') { this.options.progress?.(frame); return; }
    const pending = this.pending.get(frame.request);
    const model = this.options.model;
    if (!pending || !model) {
      void this.request({ method: 'model_result', call: frame.call, result: { kind: 'error', message: 'No active host model handler' } }).catch(() => undefined);
      return;
    }
    void model(frame, pending.controller.signal).then(
      (text) => this.request({ method: 'model_result', call: frame.call, result: { kind: 'text', text } }),
      (error: unknown) => this.request({ method: 'model_result', call: frame.call, result: { kind: 'error', code: error instanceof KiriError ? error.code : 'request', message: error instanceof KiriError ? error.message : 'Host model request failed' } }),
    ).catch(() => undefined);
  }
  private settle(id: number, result: ResultValue | Error): void {
    const pending = this.pending.get(id); if (!pending) return;
    this.pending.delete(id); pending.cleanup(); pending.controller.abort();
    if (this.pending.size === 0 && this.child.stdout instanceof Socket) this.child.stdout.unref();
    if (result instanceof Error) pending.reject(result); else pending.resolve(result);
  }
  private fail(message: string): void {
    if (this.stopped) return; this.stopped = true;
    for (const [id, request] of this.pending) this.settle(id, new KiriError(request.mutation ? 'outcome_unknown' : 'disconnected', request.mutation ? `${message}; inspect repository state before retrying the mutation` : message));
    this.child.stdin.destroy();
  }
  dispose(): void { this.fail('Kiri client closed'); this.child.stdin.end(); }
}

export class KiriRepository {
  constructor(private readonly client: KiriClient, public readonly id: number, public readonly root: string) {}
  async status(fresh = false) {
    const reply = await this.client.request({ method: 'status', repo: this.id, fresh });
    if (reply.kind !== 'status') throw new KiriError('protocol', 'Expected status');
    return reply;
  }
  /** Poll cheaply: when the engine still reports `since`, the reply carries no file list. */
  async statusSince(since: number, fresh = true) {
    const reply = await this.client.request({ method: 'status', repo: this.id, fresh, since });
    if (reply.kind !== 'status' && reply.kind !== 'unchanged') throw new KiriError('protocol', 'Expected status');
    return reply;
  }
  async preview(path: number[], side: 'worktree' | 'staged', signal?: AbortSignal) {
    const reply = await this.client.request({ method: 'preview', repo: this.id, path, side, large: false }, signal);
    if (reply.kind !== 'preview') throw new KiriError('protocol', 'Expected preview');
    return reply;
  }
  async stage(paths: number[][], staged = true): Promise<void> {
    await this.client.request({ method: 'stage', repo: this.id, paths, side: staged ? 'worktree' : 'staged' });
  }
  async prepare(paths: number[][] | null, options: AnalysisOptions = {}, signal?: AbortSignal, scope: CaptureScope = 'auto') {
    const reply = await this.client.request({ method: 'prepare', repo: this.id, paths, options, scope }, signal);
    if (reply.kind !== 'prepared') throw new KiriError('protocol', 'Expected prepared evidence');
    return reply;
  }
  async draft(prepared: number, model: string, instructions = '', signal?: AbortSignal, cacheIdentity?: string): Promise<CommitDraft> {
    const reply = await this.client.request({ method: 'propose', prepared, model, cache_identity: cacheIdentity, instructions, kind: 'draft' }, signal);
    if (reply.kind !== 'draft') throw new KiriError('protocol', 'Expected draft');
    return reply.draft;
  }
  async plan(prepared: number, model: string, instructions = '', signal?: AbortSignal, cacheIdentity?: string): Promise<CommitPlan> {
    const reply = await this.client.request({ method: 'propose', prepared, model, cache_identity: cacheIdentity, instructions, kind: 'plan' }, signal);
    if (reply.kind !== 'plan') throw new KiriError('protocol', 'Expected commit plan');
    return reply.plan;
  }
  async commit(draft: CommitDraft): Promise<string> {
    const reply = await this.client.request({ method: 'commit', repo: this.id, draft });
    if (reply.kind !== 'committed') throw new KiriError('protocol', 'Expected commit receipt');
    return reply.oid;
  }
  async applyPlan(plan: CommitPlan): Promise<string[]> {
    const reply = await this.client.request({ method: 'apply_plan', repo: this.id, plan });
    if (reply.kind !== 'applied') throw new KiriError('protocol', 'Expected applied commit plan');
    return reply.commits;
  }
  async close(): Promise<void> { await this.client.request({ method: 'close', repo: this.id }); }
}
