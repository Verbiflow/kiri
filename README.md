<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/horizon/logo/horizontal-silver.svg">
    <img src="assets/brand/horizon/logo/horizontal-black.svg" alt="Kiri" width="240">
  </picture>
</p>

<p align="center">A fast Git TUI client with syntax-highlighted diffs, folder staging, and AI-assisted commits.</p>

Kiri opens the changed-file list first and loads the selected diff on demand. Browse a large working tree, stage a folder, and turn its changes into a reviewed commit without leaving the terminal.

## Install

Source installation from this checkout, for macOS and Linux. Requires Git and Rust 1.93 or newer. Prebuilt release archives and a package-manager installer are not published yet.

```sh
make install
~/.cargo/bin/kiri -C /path/to/repo
```

`make install-engine` installs only the headless `kiri-engine` binary. Set `INSTALL_ROOT=/your/prefix` to choose a different installation root; executables go under its `bin/` directory. Add that directory to `PATH`. Neither installation command configures an AI provider, stages files, or creates commits.

A verified macOS ARM64 source install produced a 15.8 MiB TUI/CLI and a 2.4 MiB headless engine. Release builds use thin LTO and strip symbols; this is disk footprint, not total runtime memory.

Colors are on by default. Use `--color never` for monochrome or `--color auto` to respect `NO_COLOR`.

## Review, stage, commit

| Action | Key |
| --- | --- |
| Move through files or folders | `j` / `k`, arrow keys, or click |
| Expand or collapse a folder | `Enter` |
| Stage the selected file or folder | `Space` |
| Follow it into Staged | `s` |
| Draft an AI message for the staged selection | `a` |
| Draft a message for everything staged | `A` |
| Write or edit a message yourself | `c` |
| Create the reviewed commit | `Ctrl+S` |
| See all shortcuts | `?` |

Staged items leave the Working list. They haven't been deleted. A folder commit includes its selected descendants and leaves unrelated staged work alone.

You also get split and unified diffs, hunk staging, file filtering, saved project workspaces, branch switching, history, fetch, fast-forward pull, and push. Keyboard and mouse use the same actions.

## Optional AI

Press `P` to connect Gemini, Anthropic, OpenAI, xAI, Azure OpenAI, Bedrock, or a ChatGPT subscription. Git browsing never calls a model.

Selected text patches are captured completely, independently of display limits. Large selections use parallel chunks and recursive summary reduction; the final analyst can reopen original evidence by ID and byte range. Summaries never replace source storage. Identical segments share analysis, and completed summaries are cached for retries. Limits bound requests, concurrency, calls, and output—not by silently dropping source text. Failure or cancellation never produces a partial proposal. Binary and sensitive-file exclusions are explicit.

The standalone client connects models through [Rig](https://github.com/0xPlaygrounds/rig). Embedded hosts supply their own model callbacks, credentials, and cache identity. The TUI asks before a large batch of model calls.

Messages remain editable. Commit plans are proposals, not executable commands. Kiri keeps normal Git hooks and checks that the reviewed index and branch haven't changed before committing.

## Scripts work too

```sh
kiri status --json
kiri stage src/
kiri draft src/ --concurrency 8 --json
kiri plan --json
```

Use `kiri --help` for commit approval, provider setup, workspaces, and the read-only benchmark.

## Fast and Deep analysis

The TUI's analysis review offers **Fast** (default) and **Deep**, with `f` / `d` shortcuts and mouse controls. Mako exposes the same two modes beside Generate. CLI examples: `kiri draft --mode fast` and `kiri plan --mode deep`.

Both modes capture and process the complete selected text; neither changes preview limits, Git hooks, or reviewed-commit safeguards. Fast directly synthesizes the completed analysis without an optional inspection loop. Deep permits additional source inspection, reserves a final request, and switches that last request to a finish-only schema. Reaching the optional inspection allowance is not a reason to discard completed analysis.

The standalone Gemini adapter maps Fast/Deep to low/high thinking levels for supported Gemini 3 text models and explicit thinking budgets for Gemini 2.5. Other standalone adapters retain their provider reasoning defaults. Mako requests low/high reasoning through its provider adapters. Large inputs still cost real model time: these modes do not promise instant inference or reduce coverage to achieve a latency target.

Commit log limits are distinct from read-result limits. Verbose Git or hook output is drained with bounded retained excerpts; it cannot turn an already-created commit into a false failure. Kiri observes the resulting HEAD and distinguishes rejected commands from uncertain outcomes. Do not automatically replay a mutation whose outcome is uncertain.

## Multiple commits and hunks

`kiri plan <staged-folder> --json` proposes several commits for a selected staged folder; omit the path for all staged changes. The TUI can review the groups and edit their messages. Applying a plan requires explicit approval and preserves unrelated staged entries and working edits.

These are different concepts:

- An **analysis chunk** is a model-request scheduling unit. It does not define a commit.
- A **diff hunk** is a contiguous changed region inside one file. Manual hunk staging exists.
- A **commit group** is a reviewed set of related changes with a message. Current generated plans assign whole files, not individual hunks.

Automatic within-file splitting, moving changes between groups in the UI, and Mako's multi-commit review/apply flow are not implemented. The sidecar can generate and inspect plans but does not yet expose an apply-plan command. Large plans currently coarsen their commit units into folders to bound the output catalog; they retain complete source evidence but lose grouping flexibility. Hook failures can leave an already-created prefix of a commit series; Kiri stops, reports it, and does not silently roll back or replay commits.

## Embed the engine

The TUI is one client of a reusable engine. Mako uses the same engine as its normal Git backend while retaining its renderer, model connections, and encrypted credential storage.

| Module | Responsibility |
| --- | --- |
| `kiri-core` | Byte-safe Git operations, immutable captures, reviewed mutations |
| `kiri-analysis` | Provider-neutral evidence graph, caching, inspection, proposals |
| `kiri-service` | Shared reads, ordered writes, cancellation, persistent sidecar |
| `kiri-ai` | Standalone provider settings, credentials, and Rig adapters |
| `kiri-tui` / `kiri-cli` | Terminal presentation and command entry points |
| `packages/client` | Generated, runtime-validated TypeScript protocol and Node client |

`kiri-core` has no default syntax feature. The TUI enables `syntax`; headless consumers do not inherit its Tree-sitter grammars or a provider SDK.

Build `kiri-engine` with `cargo build --locked --release -p kiri-service`. Build and validate the Node package with `make sdk-check`; the package is consumed locally or as an `npm pack` artifact, not assumed to be published to npm.

```ts
import { KiriClient } from '@kiri/client';

const client = await KiriClient.connect({ binary: '/path/to/kiri-engine' });
const repo = await client.open('/path/to/repository');
const inventory = await repo.status();
const selected = inventory.status.files[0];
if (selected) await repo.preview(selected.path, 'worktree');
await repo.close();
client.dispose();
```

For analysis, supply `model(call, signal)` to `connect`. Call `repo.prepare(paths, options, signal, scope)`, then `repo.draft(prepared, modelHandle, instructions, signal, cacheIdentity)`. Scope is `staged`, `worktree`, or `auto` (staged if present, otherwise working-tree changes). The returned draft carries the reviewed snapshot; only pass it to `repo.commit(draft)` after user approval. Working-tree capture uses private Git objects and a private index, then verifies selected file fingerprints before staging and committing. Unrelated working changes remain outside a scoped commit.

The typed `client.request` also exposes discovery, comparisons, history, manifest/source inspection, plans, release, write barriers, and push. A prepared lease owns one successful proposal and its retained graph. The manifest exposes root IDs and the exact source units used after any context re-chunking; inspect nodes to traverse their children and source IDs to read original bytes. Release the lease when finished. Frames are length-prefixed and bounded; the handshake checks the protocol version and canonical schema digest. Lost mutation responses are reported as unknown outcomes and never replayed automatically. `cacheDirectory` enables persistent successful-summary reuse without placing model credentials in the sidecar.

## Measurements

A local macOS ARM64 PTY run compared identical repositories and the same selected patch. Warm median launch-to-visible-patch latency:

| Changed files | Kiri | GitUI 0.28 | Lumen 2.32 |
| --- | ---: | ---: | ---: |
| 100 | 72 ms | 18 ms | 579 ms |
| 5,000 | 226 ms | 143 ms | 6,589 ms |

Kiri was substantially faster than Lumen here; **GitUI was still faster than Kiri**. This is a small local sample, not a universal ranking. The first launch is recorded separately, client order rotates, and repository state is checked after each run.

On the 5,000-file fixture, 20 repeated Node → sidecar calls measured cached status at **3.9 ms p50 / 21.3 ms p95** and the cached `Preview` RPC at **0.20 / 0.38 ms**, with **zero Git subprocesses** in either cached phase. Fresh status remained Git-bound: 263 ms p50 versus 219 ms for the native Git subprocess baseline. We have not established a live-model quality or cost advantage.

Mako uses the separate `Compare` RPC. It now shares the same coalesced-cache implementation, with tests covering write and refresh invalidation. A later 20-call cached probe measured **0.22 ms p50 / 0.47 ms p95**, with zero Git subprocesses. That run's fresh reads were much slower for both Kiri and native Git; it is not evidence of a fresh-status improvement.

The original per-file capture path took 822 ms and 121 Git processes for just 100 files. It has been replaced with bounded path batches and shared spool files: chunks reference byte ranges rather than individual files, and context recovery reuses those captured bytes. A reported 9,099-file, 170.8 MiB staged selection then prepared in **3.17 seconds**, using **21 patch processes**, with the index unchanged and zero model calls. Its cold-cache estimate fell from 11,545 to 359 requests. This measures preparation, not inference latency or model quality.

The default candidate request budget is 1 MiB **of bytes, not tokens**, with room reserved for prompts. Native context-limit errors trigger smaller lossless batches. This is not a claim that every model has the same context window; model-call limits remain enforced, and large selections still require explicit TUI approval.

A saved 5,000-file status trace spent 206 of 231 ms in Git's index refresh, so eliminating UI overhead alone will not reach GitUI parity. Any replacement status backend needs differential correctness tests, especially for same-size edits, restored timestamps, symlinks, and index races.

[Samples, binary digests, and limitations](benchmarks/local-baseline.json). Reproduce with `scripts/benchmark_clients.py` and `node scripts/benchmark_service.mjs <engine> <disposable-repository>`. Append `20 --capture` to include local evidence preparation without model calls.

## Development

```sh
make check
make smoke
```

The workspace separates Git operations, AI analysis, terminal rendering, and CLI commands. Smoke tests drive a real PTY and mutate only disposable repositories. Bug reports with a reproduction and terminal dimensions are especially useful.

Early preview. Commit plans split at file boundaries; very large plans use folder units. Merge, rebase, and conflict editing stay in Git for now.

MIT licensed. The [Horizon identity](assets/brand/horizon/README.md) includes SVG marks and desktop icons.
