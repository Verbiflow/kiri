use anyhow::{Context, Result, ensure};
use futures::future::BoxFuture;
use kiri_ai::{
    analysis::{self, AnalysisOptions, AnalysisRuntime, LanguageModel},
    progress::silent,
};
use kiri_core::{model::RepoPath, repo::Repository};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Kiri Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Kiri Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

#[derive(Default)]
struct Model {
    inputs: Mutex<Vec<String>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
    fail_at: AtomicUsize,
}
struct Active<'a>(&'a AtomicUsize);
impl Drop for Active<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl LanguageModel for Model {
    fn identity(&self) -> String {
        "test-model".into()
    }
    fn complete<'a>(
        &'a self,
        _system: &'a str,
        input: &'a str,
        _schema: Value,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            let _guard = Active(&self.active);
            self.peak.fetch_max(active, Ordering::SeqCst);
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            self.inputs
                .lock()
                .map_err(|_| anyhow::anyhow!("input capture mutex poisoned"))?
                .push(input.into());
            tokio::time::sleep(Duration::from_millis(5)).await;
            if self.fail_at.load(Ordering::SeqCst) == call {
                anyhow::bail!("injected model failure");
            }
            Ok(json!({"summary":"The supplied changes update functions and data. Exact source references remain attached to this analysis node."}).to_string())
        })
    }
}

fn runtime(root: &Path, model: Arc<Model>) -> Arc<AnalysisRuntime> {
    Arc::new(AnalysisRuntime {
        model: model.clone(),
        worker: model,
        cache: Arc::new(analysis::cache::FileCache::new(
            root.join(".git/analysis-test"),
        )),
        observer: silent(),
    })
}

#[tokio::test]
async fn large_selection_is_fully_covered_parallel_recursive_and_cached() -> Result<()> {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=main"])?;
    fs::create_dir(temp.path().join("src"))?;
    for index in 0..540 {
        fs::write(
            temp.path().join(format!("src/file_{index}.rs")),
            format!("fn feature_{index}() {{}}\n"),
        )?;
    }
    let mut large = String::new();
    for index in 0..30000 {
        large.push_str(&format!("const VALUE_{index}: &str = \"東京 {index}\";\n"));
    }
    large.push_str("fn LAST_STAGED_MARKER() {}\n");
    fs::write(temp.path().join("large.rs"), &large)?;
    fs::write(temp.path().join(".env"), "SECRET_MUST_NOT_BE_SENT")?;
    git(temp.path(), &["add", "."])?;
    fs::write(
        temp.path().join("large.rs"),
        "UNSTAGED_WORK_MUST_NOT_BE_SENT",
    )?;
    let repo = Repository::open(temp.path()).await?;
    let options = AnalysisOptions {
        concurrency: 4,
        chunk_bytes: 8000,
        fan_in: 2,
        ..AnalysisOptions::default()
    };
    let prepared = Arc::new(analysis::prepare(&repo, None, options, silent()).await?);
    assert_eq!(prepared.files.len(), 542);
    assert!(prepared.input_bytes > 512 * 1024);
    assert!(prepared.batches.len() > 4);
    let large_index = prepared
        .files
        .iter()
        .position(|file| file.path.bytes() == b"large.rs")
        .context("large file")?;
    let mut pieces = Vec::new();
    for unit in &prepared.units {
        for source in &unit.sources {
            if source.file == large_index {
                pieces.push((source.start, source.end, fs::read_to_string(&unit.content)?));
            }
        }
    }
    pieces.sort_by_key(|(start, _, _)| *start);
    let mut position = 0;
    let mut restored = String::new();
    for (start, end, text) in pieces {
        assert_eq!(start, position);
        position = end;
        restored.push_str(&text);
    }
    assert_eq!(position, prepared.files[large_index].bytes);
    assert!(restored.contains("LAST_STAGED_MARKER"));
    assert!(!restored.contains("UNSTAGED_WORK_MUST_NOT_BE_SENT"));
    assert!(!restored.contains('\u{fffd}'));
    let model = Arc::new(Model::default());
    let runtime = runtime(temp.path(), model.clone());
    let result = analysis::analyze(prepared.clone(), runtime.clone()).await?;
    assert!(result.report.reduction_levels > 1);
    assert!(model.peak.load(Ordering::SeqCst) > 1);
    assert!(model.peak.load(Ordering::SeqCst) <= 4);
    {
        let inputs = model
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("input capture mutex poisoned"))?;
        for unit in &prepared.units {
            assert!(
                inputs
                    .iter()
                    .any(|input| input.contains(&format!("SOURCE {}", unit.id))),
                "source was never analyzed"
            );
        }
        assert!(
            inputs
                .iter()
                .all(|input| input.len() <= prepared.options.chunk_bytes)
        );
        assert!(
            inputs
                .iter()
                .all(|input| !input.contains("SECRET_MUST_NOT_BE_SENT")
                    && !input.contains("UNSTAGED_WORK_MUST_NOT_BE_SENT"))
        );
    }
    let calls = model.calls.load(Ordering::SeqCst);
    let cached = analysis::analyze(prepared.clone(), runtime).await?;
    assert_eq!(cached.report.model_calls, 0);
    assert!(cached.report.cache_hits > 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), calls);
    assert_eq!(cached.root_ids, result.root_ids);
    prepared.snapshot.verify(&repo).await?;
    Ok(())
}

#[tokio::test]
async fn folder_scope_preserves_unrelated_index_entries_and_includes_renames() -> Result<()> {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=main"])?;
    fs::create_dir(temp.path().join("src"))?;
    fs::create_dir(temp.path().join("src-other"))?;
    fs::write(temp.path().join("src/a.rs"), "fn old() {}\n")?;
    fs::write(temp.path().join("src-other/b.rs"), "fn unrelated() {}\n")?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "initial"])?;
    git(temp.path(), &["mv", "src/a.rs", "src/renamed.rs"])?;
    fs::write(
        temp.path().join("src-other/b.rs"),
        "fn separately_staged() {}\n",
    )?;
    git(temp.path(), &["add", "."])?;
    let repo = Repository::open(temp.path()).await?;
    let prepared = analysis::prepare(
        &repo,
        Some(&[RepoPath::new(b"src".to_vec())?]),
        AnalysisOptions::default(),
        silent(),
    )
    .await?;
    assert_eq!(prepared.files.len(), 2);
    assert!(
        prepared
            .files
            .iter()
            .all(|file| file.path.bytes().starts_with(b"src/"))
    );
    let draft = kiri_ai::workflow::CommitDraft {
        repository: repo.root().to_path_buf(),
        snapshot: prepared.snapshot,
        message: "refactor: rename source".into(),
        warnings: Vec::new(),
        paths: Some(prepared.files.into_iter().map(|file| file.path).collect()),
        analysis: None,
    };
    let path = temp.path().join(".git/scoped-draft.json");
    fs::write(&path, serde_json::to_vec(&draft)?)?;
    assert!(
        git(
            temp.path(),
            &["diff", "--cached", "--name-only", "--no-renames"]
        )?
        .contains("src-other/b.rs")
    );
    draft.snapshot.verify(&repo).await?;
    Ok(())
}

#[tokio::test]
async fn failed_analysis_reuses_completed_chunks_on_retry() -> Result<()> {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=main"])?;
    let content: String = (0..12000)
        .map(|i| format!("different_line_{i}\n"))
        .collect();
    fs::write(temp.path().join("data.txt"), content)?;
    git(temp.path(), &["add", "."])?;
    let repo = Repository::open(temp.path()).await?;
    let prepared = Arc::new(
        analysis::prepare(
            &repo,
            None,
            AnalysisOptions {
                concurrency: 2,
                chunk_bytes: 8000,
                fan_in: 2,
                ..AnalysisOptions::default()
            },
            silent(),
        )
        .await?,
    );
    let model = Arc::new(Model::default());
    model.fail_at.store(6, Ordering::SeqCst);
    let runtime = runtime(temp.path(), model.clone());
    assert!(
        analysis::analyze(prepared.clone(), runtime.clone())
            .await
            .is_err()
    );
    model.fail_at.store(0, Ordering::SeqCst);
    let result = analysis::analyze(prepared.clone(), runtime).await?;
    assert!(result.report.cache_hits > 0);
    prepared.snapshot.verify(&repo).await?;
    Ok(())
}
