# Kiri

Kiri is a Rust Git client with a Ratatui interface and non-interactive CLI.

## Structure

- `crates/kiri-core`: Git subprocess boundary, byte-safe paths, patches, folder trees, index operations and workspace storage. Syntax grammars are behind the opt-in `syntax` feature; wire schema derives use `schema`.
- `crates/kiri-analysis`: provider-neutral evidence capture, parallel analysis, hybrid source inspection, reviewed drafts and commit plans. Model and cache implementations are injected. No Rig, credentials, TUI or global settings.
- `crates/kiri-ai`: standalone Kiri provider settings, authentication and Rig transports. Thin workflow adapters construct an analysis runtime; they do not implement chunking or reduction.
- `crates/kiri-service`: shared repository reads, preview cache, admission-ordered writes, read-task cancellation and the `kiri-engine` sidecar. It has no TUI or provider SDK dependency.
- `crates/kiri-tui`: view state, input, syntax presentation and rendering. Uses the shared repository service and task group. Render only visible rows; no filesystem, network or Git calls during rendering.
- `crates/kiri-cli`: command parsing and application composition.
- `packages/client`: `@kiri/client`, with TypeScript generated from the Rust protocol schema and runtime validation. Never hand-edit generated request.ts, frame.ts or schema.json.

## Verification

- `make check`: formatting, strict Clippy, and Rust workspace tests.
- `make sdk-check`: build the headless sidecar, regenerate schema/types and drive the real Node → Rust → host-model → scoped-commit path in disposable repositories.
- `cargo check -p kiri-core --no-default-features` and `cargo tree -p kiri-service --no-default-features`: verify headless dependency boundaries.
- `scripts/benchmark_clients.py --kiri <release-binary> --lumen <release-binary> --gitui <release-binary>` compares real PTY inventory/patch visibility on identical disposable fixtures. Inspect saved screens and Git guards; never interpret missing patch markers as a performance win.
- `make build`: release binary at `target/release/kiri`. Requires Rust 1.93 or newer. Release builds strip symbols without changing the optimization level.
- `make install` and `make install-engine`: source installation into `INSTALL_ROOT` (defaults to `~/.cargo`), executables under `bin/`. Verify installs in a fresh temporary prefix before changing a user's PATH or installed tools.
- `cargo test -p kiri-cli --test commands hybrid_generated_folder_plan_inspects_sources_then_creates_exact_commits`: scripted-model hybrid inspection → generated folder plan → real CLI commits, including incomplete-plan rejection and unrelated-change preservation. This proves mechanics, not live-model semantic quality.
- `make smoke`: exercise the actual binary in a PTY with forced colors, auto colors, and no colors. Checks actual ANSI output, file/folder staging and unstaging, and commit creation in disposable repositories.
- Add `--svg /tmp/kiri-screen.svg` to the PTY verifier to capture actual terminal text and colors for visual inspection. It always sets NO_COLOR=1 deliberately; --color always must override it.
- `python3 scripts/verify_tui.py --repo /path/to/repo --snapshot /tmp/kiri-screen.txt`: read-only TUI checks against a real repository with an isolated configuration directory.
- `target/release/kiri --repo /path/to/repo bench --runs 10`: read-only real-repository benchmark.
- Never stage or commit in a user's repository during tests. Use temporary repositories with isolated Git config and identity supplied through environment variables.

## Invariants

- TUI repository mutations, including hunk staging, sync, branch switching and applying a commit series, go through RepositoryService. Manual draft capture joins that admission queue too. UI selection generations and presentation caches remain client-local; do not bypass the queue through repository() for writes.
- No repository-wide patch or numstat on startup. Status first, selected-file diff second. All reads are bounded and cancellable.
- Git paths remain raw bytes. Display strings must never be fed back into Git as file identities. Disable pathspec magic and external diff/textconv execution.
- Folder actions select exact descendant file indices from the tree, including collapsed descendants but respecting the active filter and staged/working tab. Resolve current eligibility with Git's tagged index/untracked inventory in bounded path batches: tracked updates use add --update, while new files use add without forcing ignored content. Ignore rules must not hide tracked edits or deletions; stale ignored untracked paths must not be added. Send mutation pathspecs as NUL-delimited stdin. Preserve unrelated index entries and all working files. Refresh observed status after failed jobs too: nonzero Git exits can follow partial index updates.
- Compute tree structure and folder counts when inventory/filter changes, not during rendering. Expand the initial file's ancestors; unrelated folders begin collapsed.
- Syntax uses lazy, per-language Tree-sitter configurations. Show the patch before background tokenization; cancel abandoned work, reject stale results, and reuse colors for unchanged patch fingerprints. No file-wide or repository-wide parsing in the render loop.
- Interactive colors default to always, including when NO_COLOR is inherited. Explicit --color auto honors NO_COLOR; --color never requests monochrome. The t key toggles colors. Test the bare launch command's ANSI output as well as explicit modes, not only terminal text or TestBackend buffers.
- AI is opt-in. Never send repository contents to a provider during browsing. Commit drafts require exact staged-snapshot validation before commit. Plans contain known change IDs, never executable commands.
- Working-tree analysis captures use private Git objects and a private index, not temporary staging in the real index. ReviewSnapshot distinguishes staged and worktree guards. Check selected file fingerprints, HEAD, active branch and index state before reviewed mutations; working-file reads cooperate with cancellation.
- `node scripts/benchmark_service.mjs <release-engine> <disposable-repo> 20` measures fresh/cached service paths and Git process counts through Trace2. Cached speed is not a claim about fresh status or TUI startup.
- Sidecar frames use a four-byte big-endian length and validated JSON, with a protocol version plus canonical schema-digest handshake and increasing request IDs. Model callbacks never contain credentials. Disconnects never replay uncertain mutations. Prepared evidence has explicit leases; cancellation waits for its task before reclaiming published evidence.
- Capture patches in bounded path batches using Git's NUL-delimited raw metadata to associate patch spans with byte-exact names. Preserve spans in shared spool files; do not create a file or subprocess per evidence chunk. Re-chunk captured spans without re-running Git. Local preparation progress and estimates must not be presented as model calls already made.
- `node scripts/profile_capture.mjs <release-engine> <repository>` profiles staged capture only, verifies the index digest and HEAD, and forbids model callbacks. `scripts/diagnose_capture.py <repository>` reports staged blob metadata without reading patches.
- Evidence pages expose total bytes and continuation offsets. Preserve exact original patch bytes, including headers and non-UTF-8 data; encode non-UTF-8 pages losslessly rather than replacing bytes. Summaries are compressed views, not replacements for source evidence.
- AI analysis reads complete patches from immutable Git trees, separately from bounded UI previews. Process every selected text segment, deduplicate identical segments with explicit source references, and reduce summaries in bounded parallel levels. Keep cache identity tied to input, prompt, and provider/model. Failed or cancelled work must never become a partial commit proposal.
- Git mutation completion is independent of retained log size: use `run_mutation`/`Repository::mutate`, not the bounded read runner, for real index/ref writes. Keep hooks enabled. Verify commit HEAD transitions and return OutcomeUnknown rather than claiming nothing committed after unobserved completion. The verbose-commit and noisy-hook CLI regressions must stay green.
- AnalysisMode is Fast (default) or Deep. Both retain complete evidence coverage. Optional inspections must leave one model request for a finish-only final schema, including when the global call budget is nearly exhausted. Keep provider reasoning parameters in adapters and include effective mode in model cache identity.
- Rig handles standard model protocols through the bounded Kiri HTTP transport; Bedrock also supports its native API and AWS CLI profile path. Retry only transient provider failures, honor Retry-After, sanitize errors, and never retry Git writes.
- In Staged, a drafts the selected file/folder and A drafts all staged changes. Scoped drafts carry exact paths through commit_selection; unrelated index entries remain staged. Preserve the last staged selection when switching tabs. Large AI requests require an explicit TUI cost review.
- scripts/demo.py --run /absolute/path/to/kiri creates an isolated public-safe repository for screenshots. Never publish screenshots of private repositories or credential settings.
- Never bypass Git hooks, discard working changes, force push, or automatically retry a commit.
- Credentials are separate from settings, saved with owner-only permissions. Do not log prompts, provider bodies, tokens, or keys.
- Workspace roots are canonical Git worktree roots. Registry updates are locked and atomic, and opening an existing workspace is idempotent. Never fsync workspace state on the UI thread.
- Incoming/outgoing counts describe cached remote refs until an explicit fetch. Pull is fast-forward-only with no automatic stash. Push targets one reviewed branch and never forces.
- Snapshot guards check both the commit ID and active branch. An external branch switch can keep the same commit ID. Kiri's operation lock coordinates Kiri processes, not arbitrary external Git writers.
- Set `KIRI_REDUCED_MOTION=1` to suppress completion feedback. Keyboard navigation never animates.
- Settings and credentials default to `~/.config/kiri`; `KIRI_CONFIG_DIR` overrides this for tests and portable setups. API keys are never command-line arguments. ChatGPT sign-in is `kiri provider login codex`; other providers use `kiri provider connect` or the TUI's P picker.
