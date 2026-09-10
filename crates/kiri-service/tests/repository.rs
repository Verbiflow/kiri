use anyhow::Result;
use kiri_core::{
    model::{DiffSide, RepoPath},
    workbench::{Comparison, Preview},
};
use kiri_service::Service;
use std::{fs, process::Command, sync::Arc};

#[tokio::test]
async fn repository_handles_share_reads_and_order_admitted_mutations() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let output = Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    anyhow::ensure!(output.status.success(), "Fixture init failed");
    fs::write(temp.path().join("file.txt"), "Selected content\n")?;
    fs::write(temp.path().join("sibling.txt"), "Unrelated content\n")?;
    let service = Service::default();
    let one = service.open(temp.path()).await?;
    let two = service.open(temp.path().join(".")).await?;
    assert!(Arc::ptr_eq(&one, &two));
    service.close(one.root()).await;
    let reopened = service.open(temp.path()).await?;
    assert!(Arc::ptr_eq(&one, &reopened));
    let (first, second) = tokio::join!(one.status(true), two.status(true));
    let (first, second) = (first?, second?);
    assert!(Arc::ptr_eq(&first.status, &second.status));
    let path = RepoPath::new(b"file.txt".to_vec())?;
    let stage = one.stage(vec![path.clone()], DiffSide::Worktree);
    let unstage = two.stage(vec![path.clone()], DiffSide::Staged);
    let restage = one.stage(vec![path.clone()], DiffSide::Worktree);
    let capture = two.capture();
    let (a, b, c) = tokio::join!(stage, unstage, restage);
    a?;
    b?;
    c?;
    let snapshot = capture.await?;
    let base = one.repository().snapshot_base(&snapshot).await?;
    let changed = one
        .repository()
        .snapshot_changes(&base, &snapshot.tree)
        .await?;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].0, path);
    drop(one.stage(
        vec![RepoPath::new(b"sibling.txt".to_vec())?],
        DiffSide::Worktree,
    ));
    let admitted = two.capture().await?;
    assert_eq!(
        two.repository()
            .snapshot_changes(&base, &admitted.tree)
            .await?
            .len(),
        2
    );
    let status = one.status(false).await?;
    let file = status
        .status
        .files
        .iter()
        .find(|file| file.path == path)
        .ok_or_else(|| anyhow::anyhow!("Selected file"))?;
    let (left, right) = tokio::join!(
        one.preview(file, DiffSide::Staged, false),
        two.preview(file, DiffSide::Staged, false)
    );
    assert!(Arc::ptr_eq(&left?, &right?));
    fs::write(
        temp.path().join("file.txt"),
        "Selected content\nReviewed hunk\n",
    )?;
    let status = one.status(true).await?;
    let file = status
        .status
        .files
        .iter()
        .find(|file| file.path == path)
        .ok_or_else(|| anyhow::anyhow!("Working file"))?
        .clone();
    let preview = one.preview(&file, DiffSide::Worktree, false).await?;
    let (left, right) = tokio::join!(
        one.compare(&path, &Comparison::IndexToWorktree),
        two.compare(&path, &Comparison::IndexToWorktree)
    );
    let before_write = left?;
    assert!(Arc::ptr_eq(&before_write, &right?));
    let stage_hunk = one.stage_hunk(file.clone(), DiffSide::Worktree, preview.clone(), 0);
    let draft = two.manual_draft(Some(vec![path.clone()]));
    drop(stage_hunk);
    let draft = draft.await?;
    let snapshot = draft.snapshot.staged()?;
    assert_eq!(
        one.repository()
            .git(&["show", &format!("{}:file.txt", snapshot.tree)])
            .await?,
        b"Selected content\nReviewed hunk\n"
    );
    let after_write = two.compare(&path, &Comparison::IndexToWorktree).await?;
    assert!(!Arc::ptr_eq(&before_write, &after_write));
    match after_write.as_ref() {
        Preview::Files { before, after } => {
            assert_eq!(before, after);
            assert_eq!(after.as_deref(), Some("Selected content\nReviewed hunk\n"));
        }
        _ => anyhow::bail!("Expected full-text comparison"),
    }
    fs::write(temp.path().join("file.txt"), "Changed after review\n")?;
    one.status(true).await?;
    let external_edit = two.compare(&path, &Comparison::IndexToWorktree).await?;
    assert!(!Arc::ptr_eq(&after_write, &external_edit));
    match external_edit.as_ref() {
        Preview::Files { after, .. } => {
            assert_eq!(after.as_deref(), Some("Changed after review\n"))
        }
        _ => anyhow::bail!("Expected refreshed full-text comparison"),
    }
    let stale = one.stage_hunk(file, DiffSide::Worktree, preview, 0);
    let next = two.manual_draft(Some(vec![path]));
    assert!(stale.await.is_err());
    next.await?.snapshot.verify(one.repository()).await?;
    assert_eq!(
        fs::read_to_string(temp.path().join("sibling.txt"))?,
        "Unrelated content\n"
    );
    Ok(())
}
