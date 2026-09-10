use anyhow::Result;
use futures::future::BoxFuture;
use kiri_analysis::{
    AnalysisOptions, AnalysisRuntime, ContextOverflow, LanguageModel, cache::NoCache, prepare,
    progress::silent, proposal,
};
use kiri_core::repo::Repository;
use serde_json::{Value, json};
use std::{
    fs,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Model {
    calls: AtomicUsize,
    auth: bool,
}
impl LanguageModel for Model {
    fn identity(&self) -> String {
        "context-model".into()
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        _schema: Value,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.auth {
                anyhow::bail!("Authentication rejected");
            }
            if system.len() + input.len() > 6500 {
                return Err(ContextOverflow.into());
            }
            if system.contains("Keep the summary under") {
                Ok(json!({"summary":"All supplied source segments are included."}).to_string())
            } else {
                Ok(json!({"action":"finish","result":{"message":"feat: include every captured change"},"requests":[],"notes":""}).to_string())
            }
        })
    }
}
#[tokio::test]
async fn context_rejection_rechunks_complete_evidence_and_auth_does_not_retry() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let result = Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .output()?;
    anyhow::ensure!(result.status.success());
    fs::write(
        temp.path().join("file.txt"),
        "Complete source line\n".repeat(1600),
    )?;
    let repo = Repository::open(temp.path()).await?;
    repo.stage(&[kiri_core::model::RepoPath::new(b"file.txt".to_vec())?])
        .await?;
    let captured = Arc::new(
        prepare(
            &repo,
            None,
            AnalysisOptions {
                max_calls: 80,
                ..Default::default()
            },
            silent(),
        )
        .await?,
    );
    let model = Arc::new(Model {
        calls: AtomicUsize::new(0),
        auth: false,
    });
    let runtime = Arc::new(AnalysisRuntime {
        model: model.clone(),
        worker: model.clone(),
        cache: Arc::new(NoCache),
        observer: silent(),
    });
    let (draft, evidence) =
        proposal::draft_with_evidence(&repo, captured.clone(), runtime, "").await?;
    assert_eq!(captured.storage_path(), evidence.prepared.storage_path());
    assert_eq!(captured.input_bytes, evidence.prepared.input_bytes);
    assert!(std::fs::read_dir(captured.storage_path())?.count() <= 2);
    assert!(model.calls.load(Ordering::Relaxed) > 1);
    assert_eq!(
        draft.analysis.as_ref().map(|report| report.model_calls),
        Some(model.calls.load(Ordering::Relaxed))
    );
    assert!(model.calls.load(Ordering::Relaxed) <= 80);
    let denied = Arc::new(Model {
        calls: AtomicUsize::new(0),
        auth: true,
    });
    let runtime = Arc::new(AnalysisRuntime {
        model: denied.clone(),
        worker: denied.clone(),
        cache: Arc::new(NoCache),
        observer: silent(),
    });
    assert!(proposal::draft(&repo, captured, runtime, "").await.is_err());
    assert!(denied.calls.load(Ordering::Relaxed) <= 8);
    assert!(repo.head().await?.is_none());
    Ok(())
}
