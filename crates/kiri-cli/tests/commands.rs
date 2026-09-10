use anyhow::{Context, Result, bail};
use kiri_ai::workflow::{CommitDraft, CommitGroup, CommitPlan, PlanFile};
use kiri_analysis::{
    AnalysisOptions, AnalysisRuntime, Inspection, LanguageModel, PreparedAnalysis, cache::NoCache,
    progress::silent, proposal,
};
use kiri_core::{model::RepoPath, repo::Repository};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};
use tempfile::TempDir;

struct InspectingPlanner {
    requests: Vec<Inspection>,
    rounds: AtomicUsize,
    summaries: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    omit_unit: bool,
}
impl LanguageModel for InspectingPlanner {
    fn identity(&self) -> String {
        format!("scripted-plan-{}", self.omit_unit)
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        _schema: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            anyhow::ensure!(
                !input.contains("UNRELATED_STAGED_EVIDENCE"),
                "Unselected content reached the model"
            );
            if system.contains("Keep the summary under") {
                self.summaries.fetch_add(1, Ordering::SeqCst);
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(active, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                return Ok(serde_json::json!({"summary":"The supplied files changed; inspect original evidence for the precise behavior."}).to_string());
            }
            if self.rounds.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(serde_json::json!({"action":"inspect","result":null,"requests":self.requests,"notes":"Read the behavior omitted from summaries."}).to_string());
            }
            anyhow::ensure!(
                input.contains("API_TRUTH_923") && input.contains("DOC_TRUTH_184"),
                "Original source facts were not reopened"
            );
            #[derive(serde::Deserialize)]
            struct Unit {
                id: String,
                path: String,
            }
            let catalog = input
                .split("COMMIT UNITS\n")
                .nth(1)
                .and_then(|rest| rest.lines().next())
                .context("Commit unit catalog")?;
            let units: Vec<Unit> = serde_json::from_str(catalog)?;
            let api: Vec<_> = units
                .iter()
                .filter(|unit| unit.path.contains("api"))
                .map(|unit| unit.id.clone())
                .collect();
            let mut docs: Vec<_> = units
                .iter()
                .filter(|unit| unit.path.contains("docs"))
                .map(|unit| unit.id.clone())
                .collect();
            if self.omit_unit {
                docs.pop();
            }
            Ok(serde_json::json!({"action":"finish","result":{"groups":[{"message":"feat: implement the API behavior","reason":"Keep the API and its tests together","files":api},{"message":"docs: explain the new workflow","reason":"Keep documentation and its checks together","files":docs}]},"requests":[],"notes":""}).to_string())
        })
    }
}
fn inspecting_planner(
    prepared: &PreparedAnalysis,
    omit_unit: bool,
) -> Result<Arc<InspectingPlanner>> {
    let mut requests = Vec::new();
    for path in [b"feature/api.rs".as_slice(), b"feature/docs.md".as_slice()] {
        let file = prepared
            .files
            .iter()
            .position(|file| file.path.bytes() == path)
            .context("Selected source file")?;
        let unit = prepared
            .units
            .iter()
            .filter(|unit| unit.sources.iter().any(|source| source.file == file))
            .max_by_key(|unit| {
                unit.sources
                    .iter()
                    .filter(|source| source.file == file)
                    .map(|source| source.end)
                    .max()
            })
            .context("Source tail")?;
        requests.push(Inspection::Source {
            id: unit.id.clone(),
            offset: unit.bytes.saturating_sub(200),
            limit: 200,
        });
    }
    Ok(Arc::new(InspectingPlanner {
        requests,
        rounds: AtomicUsize::new(0),
        summaries: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        omit_unit,
    }))
}

#[tokio::test]
async fn hybrid_generated_folder_plan_inspects_sources_then_creates_exact_commits() -> Result<()> {
    let temp = fixture()?;
    fs::create_dir(temp.path().join("feature"))?;
    let api = format!(
        "{}API_TRUTH_923\n",
        (0..500)
            .map(|i| format!("API implementation detail {i}\n"))
            .collect::<String>()
    );
    let docs = format!(
        "{}DOC_TRUTH_184\n",
        (0..500)
            .map(|i| format!("Documentation detail {i}\n"))
            .collect::<String>()
    );
    for (path, content) in [
        ("feature/api.rs", api.as_str()),
        ("feature/api_test.rs", "API behavior tests\n"),
        ("feature/docs.md", docs.as_str()),
        ("feature/docs_test.rs", "Documentation checks\n"),
        ("outside.txt", "UNRELATED_STAGED_EVIDENCE\n"),
    ] {
        fs::write(temp.path().join(path), content)?;
    }
    git(temp.path(), &["add", "feature", "outside.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let prepared = Arc::new(
        kiri_analysis::prepare(
            &repo,
            Some(&[RepoPath::new(b"feature".to_vec())?]),
            AnalysisOptions {
                mode: kiri_analysis::AnalysisMode::Deep,
                chunk_bytes: 8000,
                concurrency: 4,
                max_calls: 100,
                ..Default::default()
            },
            silent(),
        )
        .await?,
    );
    let model = inspecting_planner(&prepared, false)?;
    let runtime = Arc::new(AnalysisRuntime {
        model: model.clone(),
        worker: model.clone(),
        cache: Arc::new(NoCache),
        observer: silent(),
    });
    let head = git(temp.path(), &["rev-parse", "HEAD"])?;
    let plan = proposal::plan(&repo, prepared.clone(), runtime, "").await?;
    assert_eq!(plan.groups.len(), 2);
    assert_eq!(plan.files.len(), 4);
    assert_eq!(model.rounds.load(Ordering::SeqCst), 2);
    assert!(model.summaries.load(Ordering::SeqCst) > 1);
    assert!((2..=4).contains(&model.peak.load(Ordering::SeqCst)));
    assert_eq!(git(temp.path(), &["rev-parse", "HEAD"])?, head);
    let invalid = inspecting_planner(&prepared, true)?;
    let runtime = Arc::new(AnalysisRuntime {
        model: invalid.clone(),
        worker: invalid,
        cache: Arc::new(NoCache),
        observer: silent(),
    });
    let error = match proposal::plan(&repo, prepared, runtime, "").await {
        Ok(_) => bail!("An incomplete generated plan was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("omitted"));
    assert_eq!(git(temp.path(), &["rev-parse", "HEAD"])?, head);
    let saved = temp.path().join(".git/hybrid-plan.json");
    fs::write(&saved, serde_json::to_vec(&plan)?)?;
    fs::write(
        temp.path().join("feature/api.rs"),
        "Later unstaged API edit\n",
    )?;
    fs::write(temp.path().join("outside.txt"), "Later outside edit\n")?;
    let output = kiri(
        temp.path(),
        &[
            "apply",
            saved.to_str().context("Plan filename")?,
            "--yes",
            "--json",
        ],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git(
            temp.path(),
            &["rev-list", "--count", &format!("{}..HEAD", head.trim())]
        )?
        .trim(),
        "2"
    );
    assert_eq!(
        git(
            temp.path(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD~1"]
        )?
        .lines()
        .collect::<Vec<_>>(),
        ["feature/api.rs", "feature/api_test.rs"]
    );
    assert_eq!(
        git(
            temp.path(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]
        )?
        .lines()
        .collect::<Vec<_>>(),
        ["feature/docs.md", "feature/docs_test.rs"]
    );
    assert_eq!(git(temp.path(), &["show", "HEAD:feature/api.rs"])?, api);
    assert_eq!(
        git(temp.path(), &["diff", "--cached", "--name-only"])?.trim(),
        "outside.txt"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("feature/api.rs"))?,
        "Later unstaged API edit\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("outside.txt"))?,
        "Later outside edit\n"
    );
    let completed = git(temp.path(), &["rev-parse", "HEAD"])?;
    assert!(
        !kiri(
            temp.path(),
            &["apply", saved.to_str().context("Plan filename")?, "--yes"]
        )?
        .status
        .success()
    );
    assert_eq!(git(temp.path(), &["rev-parse", "HEAD"])?, completed);
    Ok(())
}

fn isolated(command: &mut Command) -> &mut Command {
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Kiri Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Kiri Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    let output = isolated(&mut command).output()?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn fixture() -> Result<TempDir> {
    let temp = TempDir::new()?;
    git(temp.path(), &["init", "--quiet"])?;
    fs::write(temp.path().join("first.txt"), "old first\n")?;
    fs::write(temp.path().join("second.txt"), "old second\n")?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "initial"])?;
    Ok(temp)
}

fn kiri(root: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kiri"));
    command
        .arg("--repo")
        .arg(root)
        .args(args)
        .env("KIRI_CONFIG_DIR", root.join(".git/kiri-test-state"));
    Ok(isolated(&mut command).output()?)
}

#[test]
fn noninteractive_mode_does_not_require_a_terminal_or_provider() -> Result<()> {
    let temp = fixture()?;
    let result = kiri(temp.path(), &["status", "--json"])?;
    assert!(result.status.success());
    let status: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    assert!(
        status["files"]
            .as_array()
            .context("files array")?
            .is_empty()
    );
    let result = kiri(temp.path(), &[])?;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("TUI needs a terminal"));
    Ok(())
}

#[test]
fn verbose_large_commit_reports_the_commit_that_was_created() -> Result<()> {
    let temp = fixture()?;
    fs::create_dir(temp.path().join("bulk"))?;
    for index in 0..512 {
        fs::write(
            temp.path()
                .join("bulk")
                .join(format!("{index:04}-{}.txt", "long-name".repeat(15))),
            format!("staged {index}\n"),
        )?;
    }
    git(temp.path(), &["add", "bulk"])?;
    let before = git(temp.path(), &["rev-parse", "HEAD"])?;
    let result = kiri(
        temp.path(),
        &[
            "commit",
            "--message",
            "test: commit all reviewed files",
            "--yes",
            "--json",
        ],
    )?;
    let after = git(temp.path(), &["rev-parse", "HEAD"])?;
    assert_ne!(before, after, "The fixture must create a real commit");
    assert!(
        result.status.success(),
        "A commit exists, but Kiri reported failure: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        git(
            temp.path(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]
        )?
        .lines()
        .count(),
        512
    );
    assert!(git(temp.path(), &["diff", "--cached", "--name-only"])?.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn noisy_hooks_do_not_hide_success_or_turn_rejection_into_success() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for (name, code, succeeds) in [
        ("commit-msg", 0, true),
        ("commit-msg", 1, false),
        ("post-commit", 7, true),
    ] {
        let temp = fixture()?;
        fs::write(temp.path().join("first.txt"), "Reviewed hook fixture\n")?;
        git(temp.path(), &["add", "first.txt"])?;
        let hooks = temp.path().join(".git/noisy-hooks");
        fs::create_dir(&hooks)?;
        let hook = hooks.join(name);
        fs::write(
            &hook,
            format!(
                "#!/bin/sh\ni=0\nwhile [ $i -lt 12000 ]; do printf 'hook progress output\\n'; printf 'hook diagnostic output\\n' >&2; i=$((i+1)); done\nexit {code}\n"
            ),
        )?;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
        let before = git(temp.path(), &["rev-parse", "HEAD"])?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiri"));
        command
            .arg("--repo")
            .arg(temp.path())
            .args([
                "commit",
                "--message",
                "test: observe the real hook outcome",
                "--yes",
                "--json",
            ])
            .env("KIRI_CONFIG_DIR", temp.path().join(".git/kiri-test-state"))
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "core.hooksPath")
            .env("GIT_CONFIG_VALUE_0", &hooks);
        let output = isolated(&mut command).output()?;
        let after = git(temp.path(), &["rev-parse", "HEAD"])?;
        assert_eq!(
            output.status.success(),
            succeeds,
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(after != before, succeeds);
        if succeeds {
            let result: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            assert_eq!(result["commit"].as_str(), Some(after.trim()));
        } else {
            assert_eq!(
                git(temp.path(), &["diff", "--cached", "--name-only"])?.trim(),
                "first.txt"
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn commit_uses_reviewed_index_not_new_working_contents() -> Result<()> {
    let temp = fixture()?;
    fs::write(temp.path().join("first.txt"), "staged\n")?;
    git(temp.path(), &["add", "first.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let draft = CommitDraft {
        repository: repo.root().to_path_buf(),
        snapshot: repo.staged_snapshot().await?.into(),
        message: "fix: commit reviewed content".into(),
        warnings: Vec::new(),
        paths: None,
        analysis: None,
    };
    fs::write(temp.path().join("first.txt"), "later unstaged edit\n")?;
    let saved = temp.path().join(".git/draft.json");
    fs::write(&saved, serde_json::to_vec(&draft)?)?;
    let saved = saved.to_str().context("fixture path")?;
    assert!(
        !kiri(temp.path(), &["commit", "--draft", saved])?
            .status
            .success()
    );
    let output = kiri(temp.path(), &["commit", "--draft", saved, "--yes"])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(git(temp.path(), &["show", "HEAD:first.txt"])?, "staged\n");
    assert_eq!(
        fs::read_to_string(temp.path().join("first.txt"))?,
        "later unstaged edit\n"
    );
    assert!(
        !kiri(temp.path(), &["commit", "--draft", saved, "--yes"])?
            .status
            .success()
    );
    Ok(())
}

#[tokio::test]
async fn scoped_commit_preserves_other_staged_files_and_unstaged_edits() -> Result<()> {
    let temp = fixture()?;
    fs::create_dir(temp.path().join("src"))?;
    git(temp.path(), &["mv", "first.txt", "src/renamed.txt"])?;
    fs::write(temp.path().join("second.txt"), "separate staged change\n")?;
    git(temp.path(), &["add", "second.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let draft = CommitDraft {
        repository: repo.root().to_path_buf(),
        snapshot: repo.staged_snapshot().await?.into(),
        message: "refactor: move the first file".into(),
        warnings: Vec::new(),
        paths: Some(vec![
            RepoPath::new(b"first.txt".to_vec())?,
            RepoPath::new(b"src/renamed.txt".to_vec())?,
        ]),
        analysis: None,
    };
    fs::write(temp.path().join("src/renamed.txt"), "later unstaged edit\n")?;
    let saved = temp.path().join(".git/folder-draft.json");
    fs::write(&saved, serde_json::to_vec(&draft)?)?;
    let result = kiri(
        temp.path(),
        &[
            "commit",
            "--draft",
            saved.to_str().context("draft path")?,
            "--yes",
        ],
    )?;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        git(temp.path(), &["diff", "--cached", "--name-only"])?.trim(),
        "second.txt"
    );
    assert_eq!(
        git(temp.path(), &["show", "HEAD:src/renamed.txt"])?,
        "old first\n"
    );
    assert!(git(temp.path(), &["show", "HEAD:first.txt"]).is_err());
    assert_eq!(
        fs::read_to_string(temp.path().join("src/renamed.txt"))?,
        "later unstaged edit\n"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("second.txt"))?,
        "separate staged change\n"
    );
    Ok(())
}

#[tokio::test]
async fn stale_draft_cannot_commit_newly_staged_changes() -> Result<()> {
    let temp = fixture()?;
    fs::write(temp.path().join("first.txt"), "first staged\n")?;
    git(temp.path(), &["add", "first.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let draft = CommitDraft {
        repository: repo.root().to_path_buf(),
        snapshot: repo.staged_snapshot().await?.into(),
        message: "fix: stale draft".into(),
        warnings: Vec::new(),
        paths: None,
        analysis: None,
    };
    fs::write(temp.path().join("second.txt"), "second staged\n")?;
    git(temp.path(), &["add", "second.txt"])?;
    let saved = temp.path().join(".git/draft.json");
    fs::write(&saved, serde_json::to_vec(&draft)?)?;
    let result = kiri(
        temp.path(),
        &[
            "commit",
            "--draft",
            saved.to_str().context("path")?,
            "--yes",
        ],
    )?;
    assert!(!result.status.success());
    assert_eq!(
        git(temp.path(), &["log", "-1", "--format=%s"])?.trim(),
        "initial"
    );
    assert_eq!(
        git(temp.path(), &["diff", "--cached", "--name-only"])?
            .lines()
            .count(),
        2
    );
    Ok(())
}

async fn staged_plan(temp: &TempDir) -> Result<CommitPlan> {
    for name in ["first.txt", "second.txt"] {
        fs::write(temp.path().join(name), format!("new {name}\n"))?;
    }
    git(temp.path(), &["add", "first.txt", "second.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let files: Vec<_> = ["first.txt", "second.txt"]
        .iter()
        .map(|name| {
            let path = RepoPath::new(name.as_bytes().to_vec())?;
            Ok(PlanFile {
                id: path.id(),
                path,
            })
        })
        .collect::<Result<_>>()?;
    let groups = files
        .iter()
        .enumerate()
        .map(|(i, file)| CommitGroup {
            message: if i == 0 {
                "fix: first change".into()
            } else {
                "fix: second change".into()
            },
            reason: "independent change".into(),
            files: vec![file.id.clone()],
        })
        .collect();
    Ok(CommitPlan {
        repository: repo.root().to_path_buf(),
        snapshot: repo.staged_snapshot().await?,
        files,
        groups,
        warnings: Vec::new(),
    })
}

#[tokio::test]
async fn chunked_commits_are_exact_and_preserve_unstaged_edits() -> Result<()> {
    let temp = fixture()?;
    let plan = staged_plan(&temp).await?;
    fs::write(temp.path().join("first.txt"), "later unstaged\n")?;
    let saved = temp.path().join(".git/plan.json");
    fs::write(&saved, serde_json::to_vec(&plan)?)?;
    let output = kiri(
        temp.path(),
        &["apply", saved.to_str().context("path")?, "--yes", "--json"],
    )?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        git(
            temp.path(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD~1"]
        )?
        .trim(),
        "first.txt"
    );
    assert_eq!(
        git(
            temp.path(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]
        )?
        .trim(),
        "second.txt"
    );
    assert!(git(temp.path(), &["diff", "--cached", "--name-only"])?.is_empty());
    assert_eq!(
        fs::read_to_string(temp.path().join("first.txt"))?,
        "later unstaged\n"
    );
    assert!(
        !kiri(
            temp.path(),
            &["apply", saved.to_str().context("path")?, "--yes"]
        )?
        .status
        .success()
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn hook_failure_stops_a_plan_without_losing_the_remaining_index() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temp = fixture()?;
    let plan = staged_plan(&temp).await?;
    let saved = temp.path().join(".git/plan.json");
    fs::write(&saved, serde_json::to_vec(&plan)?)?;
    let hooks = temp.path().join(".git/test-hooks");
    fs::create_dir(&hooks)?;
    let hook = hooks.join("commit-msg");
    fs::write(
        &hook,
        "#!/bin/sh\ncase \"$(cat \"$1\")\" in *second*) exit 1 ;; esac\n",
    )?;
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755))?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_kiri"));
    command
        .arg("--repo")
        .arg(temp.path())
        .arg("apply")
        .arg(&saved)
        .arg("--yes")
        .env("KIRI_CONFIG_DIR", temp.path().join(".git/kiri-test-state"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_0", hooks);
    let output = isolated(&mut command).output()?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("1 commits completed"));
    assert_eq!(
        git(temp.path(), &["log", "-1", "--format=%s"])?.trim(),
        "fix: first change"
    );
    assert_eq!(
        git(temp.path(), &["diff", "--cached", "--name-only"])?.trim(),
        "second.txt"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("second.txt"))?,
        "new second.txt\n"
    );
    Ok(())
}
