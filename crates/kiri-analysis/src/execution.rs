use crate::*;
use anyhow::Context;
use std::sync::atomic::{AtomicUsize, Ordering};

struct CountedModel {
    inner: Arc<dyn LanguageModel>,
    calls: Arc<AtomicUsize>,
    limit: usize,
}
impl LanguageModel for CountedModel {
    fn identity(&self) -> String {
        self.inner.identity()
    }
    fn remaining_calls(&self) -> Option<usize> {
        Some(
            self.limit
                .saturating_sub(self.calls.load(Ordering::Acquire)),
        )
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        schema: Value,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            self.calls
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |calls| {
                    (calls < self.limit).then_some(calls + 1)
                })
                .map_err(|_| {
                    anyhow::anyhow!("Model-call budget exhausted; no partial proposal was created")
                })?;
            self.inner.complete(system, input, schema).await
        })
    }
}
pub(crate) fn counted(
    runtime: Arc<AnalysisRuntime>,
    limit: usize,
) -> (Arc<AnalysisRuntime>, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let wrap = |model| -> Arc<dyn LanguageModel> {
        Arc::new(CountedModel {
            inner: model,
            calls: calls.clone(),
            limit,
        })
    };
    (
        Arc::new(AnalysisRuntime {
            model: wrap(runtime.model.clone()),
            worker: wrap(runtime.worker.clone()),
            cache: runtime.cache.clone(),
            observer: runtime.observer.clone(),
        }),
        calls,
    )
}
pub(crate) async fn smaller(
    repo: &kiri_core::repo::Repository,
    prepared: &PreparedAnalysis,
    observer: Observer,
) -> Result<Arc<PreparedAnalysis>> {
    let mut options = prepared.options.clone();
    if options.chunk_bytes <= 4096 {
        bail!(
            "The model rejected the minimum analysis context. Check its context limit and instructions; no partial proposal was created"
        );
    }
    options.chunk_bytes = (options.chunk_bytes / 2).max(4096);
    prepared.snapshot.verify(repo).await?;
    Ok(Arc::new(
        crate::corpus::rechunk(prepared, options, observer)
            .await
            .context("Could not re-chunk the captured evidence")?,
    ))
}
pub(crate) fn calls(calls: &AtomicUsize) -> usize {
    calls.load(Ordering::Acquire)
}
