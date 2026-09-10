use crate::{
    analysis::{self, AnalysisRuntime, PreparedAnalysis},
    config::Settings,
    provider::AiClient,
};
use anyhow::Result;
pub use kiri_analysis::proposal::{CommitDraft, CommitGroup, CommitPlan, PlanFile};
use kiri_core::{model::RepoPath, repo::Repository};
use std::sync::Arc;

pub async fn prepare(
    repo: &Repository,
    client: &AiClient,
    paths: Option<&[RepoPath]>,
) -> Result<Arc<PreparedAnalysis>> {
    let options = Settings::load(client.store())?.analysis;
    Ok(Arc::new(
        analysis::prepare(repo, paths, options, client.observer()).await?,
    ))
}
fn runtime(prepared: &PreparedAnalysis, client: &AiClient) -> Result<Arc<AnalysisRuntime>> {
    let client = client.clone().with_mode(prepared.options.mode);
    let worker = if let Some(model) = &prepared.options.worker_model {
        client.with_model(model)?
    } else {
        client.clone()
    };
    Ok(Arc::new(AnalysisRuntime {
        model: Arc::new(client.clone()),
        worker: Arc::new(worker),
        cache: Arc::new(analysis::cache::FileCache::new(
            client.store().path("analysis-cache"),
        )),
        observer: client.observer(),
    }))
}
pub async fn draft(
    repo: &Repository,
    client: &AiClient,
    paths: Option<&[RepoPath]>,
) -> Result<CommitDraft> {
    draft_prepared(repo, client, prepare(repo, client, paths).await?).await
}
pub async fn draft_prepared(
    repo: &Repository,
    client: &AiClient,
    prepared: Arc<PreparedAnalysis>,
) -> Result<CommitDraft> {
    let runtime = runtime(&prepared, client)?;
    kiri_analysis::proposal::draft(repo, prepared, runtime, "").await
}
pub async fn plan(repo: &Repository, client: &AiClient) -> Result<CommitPlan> {
    plan_prepared(repo, client, prepare(repo, client, None).await?).await
}
pub async fn plan_prepared(
    repo: &Repository,
    client: &AiClient,
    prepared: Arc<PreparedAnalysis>,
) -> Result<CommitPlan> {
    let runtime = runtime(&prepared, client)?;
    kiri_analysis::proposal::plan(repo, prepared, runtime, "").await
}
