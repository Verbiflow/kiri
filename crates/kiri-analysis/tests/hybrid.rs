use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures::future::BoxFuture;
use kiri_analysis::{
    AnalysisOptions, AnalysisResult, AnalysisRuntime, Inspection, LanguageModel, cache::NoCache,
    inspect, prepare, progress::silent, proposal,
};
use kiri_core::{model::RepoPath, repo::Repository};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn git(root: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()?;
    anyhow::ensure!(output.status.success(), "Git fixture failed");
    Ok(())
}
fn empty_result() -> AnalysisResult {
    AnalysisResult {
        context: String::new(),
        report: Default::default(),
        nodes: Vec::new(),
        root_ids: Vec::new(),
    }
}

#[tokio::test]
async fn evidence_roundtrips_exact_patch_bytes_including_headers_and_invalid_utf8() -> Result<()> {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "-q"])?;
    let mut bytes = "Unicode λ and ordinary text\n".repeat(1000).into_bytes();
    bytes.extend_from_slice(b"\xff final byte\n");
    fs::write(temp.path().join("sample.txt"), bytes)?;
    git(temp.path(), &["add", "sample.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let prepared = prepare(
        &repo,
        None,
        AnalysisOptions {
            chunk_bytes: 8000,
            ..Default::default()
        },
        silent(),
    )
    .await?;
    let raw = tempfile::NamedTempFile::new()?;
    let base = repo.snapshot_base(prepared.snapshot.staged()?).await?;
    repo.snapshot_patch(
        &base,
        &prepared.evidence.tree,
        &RepoPath::new(b"sample.txt".to_vec())?,
        raw.reopen()?,
    )
    .await?;
    let mut units: Vec<_> = prepared
        .units
        .iter()
        .flat_map(|unit| unit.sources.iter().map(move |source| (source.start, unit)))
        .collect();
    units.sort_by_key(|(start, _)| *start);
    let result = empty_result();
    let mut rebuilt = Vec::new();
    for (_, unit) in units {
        let mut offset = 0;
        loop {
            let page = inspect(
                &prepared,
                &result,
                &Inspection::Source {
                    id: unit.id.clone(),
                    offset,
                    limit: 197,
                },
            )
            .await?;
            rebuilt.extend(if page.encoding == "base64" {
                STANDARD.decode(page.data)?
            } else {
                page.data.into_bytes()
            });
            let Some(next) = page.next_offset else { break };
            offset = next;
        }
    }
    assert_eq!(rebuilt, fs::read(raw.path())?);
    assert!(
        inspect(
            &prepared,
            &result,
            &Inspection::Source {
                id: "../../outside".into(),
                offset: 0,
                limit: 100
            }
        )
        .await
        .is_err()
    );
    Ok(())
}

struct FinalizingAnalyst {
    source: String,
    inspections: AtomicUsize,
    finishes: AtomicUsize,
}
impl LanguageModel for FinalizingAnalyst {
    fn identity(&self) -> String {
        "finalizing-analyst".into()
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        _input: &'a str,
        schema: Value,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if system.contains("Keep the summary under") {
                return Ok(json!({"summary":"All evidence was analyzed."}).to_string());
            }
            let may_inspect = schema
                .pointer("/properties/action/enum")
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().any(|value| value == "inspect"));
            if may_inspect {
                self.inspections.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"action":"inspect","result":null,"requests":[{"kind":"source","id":self.source,"offset":0,"limit":100}],"notes":"Verified the original patch."}).to_string())
            } else {
                self.finishes.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"action":"finish","result":{"message":"fix: finalize the reviewed changes"},"requests":[],"notes":""}).to_string())
            }
        })
    }
}

#[tokio::test]
async fn optional_inspections_reserve_a_finish_only_call() -> Result<()> {
    for (mode, max_calls, expected_inspections) in [
        (kiri_analysis::AnalysisMode::Deep, 10, 2),
        (kiri_analysis::AnalysisMode::Deep, 1, 0),
        (kiri_analysis::AnalysisMode::Deep, 2, 1),
        (kiri_analysis::AnalysisMode::Fast, 10, 0),
    ] {
        let temp = tempfile::tempdir()?;
        git(temp.path(), &["init", "-q"])?;
        fs::write(temp.path().join("file.txt"), "Complete selected evidence\n")?;
        git(temp.path(), &["add", "file.txt"])?;
        let repo = Repository::open(temp.path()).await?;
        let prepared = Arc::new(
            prepare(
                &repo,
                None,
                AnalysisOptions {
                    mode,
                    inspection_rounds: 2,
                    max_calls,
                    ..Default::default()
                },
                silent(),
            )
            .await?,
        );
        let model = Arc::new(FinalizingAnalyst {
            source: prepared.units[0].id.clone(),
            inspections: AtomicUsize::new(0),
            finishes: AtomicUsize::new(0),
        });
        let runtime = Arc::new(AnalysisRuntime {
            model: model.clone(),
            worker: model.clone(),
            cache: Arc::new(NoCache),
            observer: silent(),
        });
        let draft = proposal::draft(&repo, prepared, runtime, "").await?;
        assert_eq!(draft.message, "fix: finalize the reviewed changes");
        assert_eq!(
            model.inspections.load(Ordering::SeqCst),
            expected_inspections
        );
        assert_eq!(model.finishes.load(Ordering::SeqCst), 1);
        assert!(repo.head().await?.is_none());
    }
    Ok(())
}

struct Analyst {
    source: String,
    offset: usize,
    rounds: AtomicUsize,
}
impl LanguageModel for Analyst {
    fn identity(&self) -> String {
        "hybrid-fixture".into()
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        _schema: Value,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if system.contains("Keep the summary under") {
                return Ok(json!({"summary":"A text file was updated."}).to_string());
            }
            if self.rounds.fetch_add(1, Ordering::SeqCst) == 0 {
                return Ok(json!({"action":"inspect","requests":[{"kind":"source","id":self.source,"offset":self.offset,"limit":200}],"notes":"Check the original tail."}).to_string());
            }
            anyhow::ensure!(
                input.contains("TAIL_EVIDENCE_7182"),
                "Original evidence did not reach the analyst"
            );
            Ok(json!({"action":"finish","result":{"message":"fix: retain the complete document tail"}}).to_string())
        })
    }
}
#[tokio::test]
async fn analyst_reopens_original_evidence_before_final_synthesis() -> Result<()> {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "-q"])?;
    fs::write(
        temp.path().join("text.txt"),
        format!(
            "{}TAIL_EVIDENCE_7182\n",
            "Some changing content\n".repeat(1500)
        ),
    )?;
    git(temp.path(), &["add", "text.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let prepared = Arc::new(
        prepare(
            &repo,
            None,
            AnalysisOptions {
                mode: kiri_analysis::AnalysisMode::Deep,
                chunk_bytes: 8000,
                ..Default::default()
            },
            silent(),
        )
        .await?,
    );
    let last = prepared
        .units
        .iter()
        .max_by_key(|unit| unit.sources[0].end)
        .context("Evidence tail")?;
    let model = Arc::new(Analyst {
        source: last.id.clone(),
        offset: last.bytes.saturating_sub(150),
        rounds: AtomicUsize::new(0),
    });
    let runtime = Arc::new(AnalysisRuntime {
        model: model.clone(),
        worker: model.clone(),
        cache: Arc::new(NoCache),
        observer: silent(),
    });
    let draft = proposal::draft(&repo, prepared, runtime, "").await?;
    assert!(draft.message.contains("document tail"));
    assert_eq!(model.rounds.load(Ordering::SeqCst), 2);
    assert_eq!(draft.paths.as_ref().map(Vec::len), Some(1));
    assert!(repo.head().await?.is_none());
    Ok(())
}
