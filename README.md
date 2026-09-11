<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/horizon/logo/horizontal-silver.svg">
    <img src="assets/brand/horizon/logo/horizontal-black.svg" alt="Kiri" width="240">
  </picture>
</p>

<p align="center">A Git TUI for people who read the diff before they commit.</p>

<p align="center">
  <img src="assets/screenshots/review.png" alt="Kiri showing a Rust diff in split view, with the changed-file tree on the left" width="960">
</p>

Kiri is a terminal Git client written in Rust. It opens on the changed-file tree, syntax-highlights the diff you pick, and stages a file, a folder, or a single hunk. Commit messages are yours to write, or press one key and Kiri drafts one from the staged patch using your own Claude, OpenAI, Gemini, or xAI account. Nothing is sent anywhere until you ask.

If you have used lazygit or gitui, the layout will feel familiar. The difference is the review flow: Kiri shows exactly which paths a commit will contain before it runs `git commit`.

## Install

Requires Git and Rust 1.93 or newer. macOS and Linux. No prebuilt binaries yet.

```sh
git clone https://github.com/Verbiflow/kiri
cd kiri && make install
kiri -C /path/to/repo
```

`make install` puts `kiri` in `~/.cargo/bin`. Set `INSTALL_ROOT` to change that.

## Keys

| | |
| --- | --- |
| `j` `k` | Move through files, folders, and diff rows |
| `Enter` | Open a folder or diff. `Esc` goes back |
| `Space` | Stage or unstage the file or folder |
| `H` | Stage or unstage the selected hunk. `[` `]` jump between hunks |
| `s` `u` | Show staged or working changes |
| `/` | Fuzzy filter files and folders |
| `v` | Split or unified diff |
| `c` | Write a commit message |
| `a` `A` | AI message for the selection, or for everything staged |
| `b` | Ask AI to split the staged changes into several commits |
| `Ctrl+S` | Create the reviewed commit |
| `f` `d` `U` | Fetch, fast-forward pull, push |
| `B` `l` | Branches, history |
| `?` | Everything else |

Mouse works too. `Ctrl+P` opens a command palette.

<p align="center">
  <img src="assets/screenshots/folder.png" alt="Folder summary showing which files a staging action will touch" width="49%">
  <img src="assets/screenshots/commit.png" alt="Commit review listing the exact paths that will be committed" width="49%">
</p>

## AI only when you ask

Press `P` and connect Gemini, Anthropic, OpenAI, xAI, Azure OpenAI, Bedrock, or a ChatGPT subscription. API keys are stored on your machine. There is no Kiri account and no telemetry.

A draft is a proposal. Edit it, or throw it away. Before committing, Kiri checks that the index and branch still match what you reviewed, then runs the normal Git hooks. Big selections are summarized in chunks, and Kiri asks before making a large batch of model calls.

`b` in the TUI, or `kiri plan` from a shell, groups the staged changes into several commits split at file boundaries. Each commit is applied only after you approve it.

<p align="center">
  <img src="assets/screenshots/providers.png" alt="AI provider picker" width="720">
</p>

## Scripting

Every TUI action has a subcommand, most with `--json` output.

```sh
kiri status --json
kiri stage src/
kiri draft src/ --json
kiri plan --json
```

`kiri --help` lists the rest, including provider setup and a read-only benchmark.

## Embedding

The TUI is one client of a headless engine. `kiri-engine` speaks a length-prefixed, schema-checked protocol over stdio, and [`packages/client`](packages/client) is a generated TypeScript client for it.

```ts
import { KiriClient } from '@kiri/client';

const client = await KiriClient.connect({ binary: '/path/to/kiri-engine' });
const repo = await client.open('/path/to/repository');
const { status } = await repo.status();
await repo.close();
```

Build the engine with `make install-engine` and validate the client package with `make sdk-check`.

## Speed

Warm medians on macOS ARM64, same disposable repository and selection for each client, GitUI 0.28.1 from Homebrew:

| | Kiri | GitUI |
| --- | ---: | ---: |
| Launch to visible patch, 100 changed files | 30 ms | 17 ms |
| Launch to visible patch, 5,000 changed files | 94 ms | 86 ms |
| Next file after a keypress, 100 files | 2 ms | 2 ms |
| Next file after a keypress, 5,000 files | 2 ms | 8 ms |
| Git processes during 6 s idle | 1 | 0 |

Status is computed in-process with gitoxide on a thread pool and is checked against `git status` output by a differential test suite; repository states the mapping does not reproduce exactly, such as conflicts and submodules, are computed by Git itself. Patches are cached by the blob IDs in that status plus one `lstat`, the next files are read ahead, and an unchanged repository never re-runs a diff. What remains on launch is the first `git diff`. Samples and method are in [`benchmarks/local-baseline.json`](benchmarks/local-baseline.json); reproduce with `scripts/benchmark_clients.py --kiri target/release/kiri --gitui $(which gitui)`.

## Development

```sh
make check                      # fmt, clippy, tests
make smoke                      # drives the TUI in a real PTY
python3 scripts/screenshots.py  # regenerates the images above
```

Crates: `kiri-core` (Git), `kiri-analysis` (evidence graph and proposals), `kiri-ai` (providers), `kiri-service` (engine), `kiri-tui`, `kiri-cli`.

Early preview. Merge, rebase, and conflict editing stay in Git for now. MIT licensed.
