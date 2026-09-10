use super::*;
use crate::highlight::{SyntaxState, highlight_patch};
use anyhow::Result;
use kiri_core::{
    diff::DiffDocument,
    model::{ChangeKind, FileChange},
    syntax::Language,
};
use std::sync::atomic::AtomicUsize;

fn runtime() -> Runtime {
    let (tx, rx) = mpsc::unbounded_channel();
    Runtime {
        app: App::new(
            Vec::new(),
            PathBuf::from("/project"),
            DiffSide::Worktree,
            Settings::default(),
        ),
        store: Store::at(PathBuf::from("/unused-test-store")),
        service: kiri_service::Service::default(),
        tx,
        rx,
        pending_message: None,
        reads: kiri_service::jobs::TaskGroup::default(),
        highlight_task: None,
        job: None,
        pending_analysis: None,
        diff_request: 0,
        job_request: 0,
        refreshed: Instant::now() - Duration::from_secs(6),
    }
}

fn patch(runtime: &mut Runtime) -> Result<(RepoPath, DiffView)> {
    let path = RepoPath::new(b"main.rs".to_vec())?;
    runtime.app.current_mut().set_status(RepoStatus {
        files: vec![FileChange {
            path: path.clone(),
            original_path: None,
            staged: None,
            worktree: Some(ChangeKind::Modified),
            submodule: false,
        }],
        ..RepoStatus::default()
    });
    Ok((
        path,
        DiffView::new(DiffDocument::parse(
            b"@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n".to_vec(),
            false,
        )),
    ))
}

#[tokio::test]
async fn automatic_refresh_never_restarts_an_inflight_scan() {
    let mut runtime = runtime();
    runtime
        .reads
        .replace(ReadSlot::Status(0), std::future::pending());
    assert!(!runtime.refresh_due());
    runtime.reads.cancel(&ReadSlot::Status(0));
    assert!(runtime.refresh_due());
}

#[tokio::test]
async fn failed_jobs_refresh_observed_git_state_without_dismissing_the_error() -> Result<()> {
    let mut runtime = runtime();
    patch(&mut runtime)?;
    runtime.app.current_mut().scan = FileScan::Complete;
    let revision = runtime.app.current().revision;
    runtime.tx.send(Message::Job {
        request: 0,
        workspace: 0,
        result: Err(anyhow::anyhow!(
            "A mutation can fail after changing part of the index"
        )),
    })?;
    runtime.drain();
    assert!(matches!(runtime.app.modal, Modal::Error(_)));
    assert_eq!(runtime.app.current().revision, revision + 1);
    assert!(matches!(runtime.app.current().scan, FileScan::Pending));
    assert!(runtime.reads.contains(&ReadSlot::Status(0)));
    Ok(())
}

#[tokio::test]
async fn text_is_ready_before_highlighting_and_stale_colors_are_ignored() -> Result<()> {
    let mut runtime = runtime();
    let (path, diff) = patch(&mut runtime)?;
    let fingerprint = diff.document.fingerprint.clone();
    runtime.show_diff(0, path.clone(), DiffSide::Worktree, Arc::new(diff), true);
    assert!(
        matches!(&runtime.app.current().diff, Load::Ready(view) if matches!(view.syntax, SyntaxState::Loading(_)))
    );
    assert!(runtime.highlight_task.is_some());
    runtime.diff_request += 1;
    runtime.tx.send(Message::Highlighted {
        request: 0,
        workspace: 0,
        path,
        side: DiffSide::Worktree,
        view: Arc::new(DiffView::new(DiffDocument::notice("stale result"))),
    })?;
    runtime.drain();
    assert!(
        matches!(&runtime.app.current().diff, Load::Ready(view) if view.document.fingerprint == fingerprint)
    );
    Ok(())
}

#[tokio::test]
async fn unchanged_patch_reuses_colors_instead_of_flickering_on_refresh() -> Result<()> {
    let mut runtime = runtime();
    let (path, plain) = patch(&mut runtime)?;
    let tokens = Arc::new(highlight_patch(
        &plain,
        Language::Rust,
        &AtomicUsize::new(0),
    )?);
    let mut colored = plain.clone();
    colored.syntax = SyntaxState::Ready {
        language: Language::Rust,
        tokens: tokens.clone(),
    };
    runtime.show_diff(0, path.clone(), DiffSide::Worktree, Arc::new(colored), true);
    runtime.show_diff(0, path, DiffSide::Worktree, Arc::new(plain), false);
    assert!(runtime.highlight_task.is_none());
    let Load::Ready(view) = &runtime.app.current().diff else {
        anyhow::bail!("missing diff")
    };
    let SyntaxState::Ready { tokens: reused, .. } = &view.syntax else {
        anyhow::bail!("lost highlighting")
    };
    assert!(Arc::ptr_eq(&tokens, reused));
    Ok(())
}
