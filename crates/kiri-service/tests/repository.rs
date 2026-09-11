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

fn git(temp: &tempfile::TempDir, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(temp.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Kiri Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Kiri Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn init(temp: &tempfile::TempDir) -> Result<()> {
    for args in [
        ["init", "-q"].as_slice(),
        &["add", "."],
        &["commit", "-qm", "Fixture"],
    ] {
        git(temp, args)?;
    }
    Ok(())
}

#[tokio::test]
async fn unchanged_status_keeps_its_revision_and_cached_previews() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("file.txt"), "one\n")?;
    init(&temp)?;
    fs::write(temp.path().join("file.txt"), "two\n")?;
    let service = Service::default();
    let repo = service.open(temp.path()).await?;
    let first = repo.status(true).await?;
    let file = first.status.files[0].clone();
    assert!(file.index_oid.is_some() && file.head_oid.is_some());
    let preview = repo.preview(&file, DiffSide::Worktree, false).await?;
    let again = repo.status(true).await?;
    assert_eq!(again.revision, first.revision);
    assert!(Arc::ptr_eq(&again.status, &first.status));
    let reused = repo.preview(&file, DiffSide::Worktree, false).await?;
    assert!(Arc::ptr_eq(&preview, &reused));
    Ok(())
}

#[tokio::test]
async fn an_edit_with_identical_status_still_refreshes_the_preview() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("file.txt"), "one\n")?;
    init(&temp)?;
    fs::write(temp.path().join("file.txt"), "two\n")?;
    let service = Service::default();
    let repo = service.open(temp.path()).await?;
    let file = repo.status(true).await?.status.files[0].clone();
    let before = repo.preview(&file, DiffSide::Worktree, false).await?;
    assert!(String::from_utf8_lossy(&before.raw).contains("+two"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(temp.path().join("file.txt"), "three\n")?;
    let status = repo.status(true).await?;
    let after = repo
        .preview(&status.status.files[0], DiffSide::Worktree, false)
        .await?;
    assert!(String::from_utf8_lossy(&after.raw).contains("+three"));
    assert!(!Arc::ptr_eq(&before, &after));
    Ok(())
}

#[tokio::test]
async fn restaging_between_polls_changes_the_staged_identity() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("file.txt"), "one\n")?;
    init(&temp)?;
    fs::write(temp.path().join("file.txt"), "two\n")?;
    git(&temp, &["add", "file.txt"])?;
    let service = Service::default();
    let repo = service.open(temp.path()).await?;
    let first = repo.status(true).await?;
    let staged = repo
        .preview(&first.status.files[0], DiffSide::Staged, false)
        .await?;
    assert!(String::from_utf8_lossy(&staged.raw).contains("+two"));
    fs::write(temp.path().join("file.txt"), "three\n")?;
    git(&temp, &["add", "file.txt"])?;
    let second = repo.status(true).await?;
    assert_ne!(second.revision, first.revision);
    assert_eq!(second.status.files[0].staged, first.status.files[0].staged);
    assert_ne!(
        second.status.files[0].index_oid,
        first.status.files[0].index_oid
    );
    let restaged = repo
        .preview(&second.status.files[0], DiffSide::Staged, false)
        .await?;
    assert!(String::from_utf8_lossy(&restaged.raw).contains("+three"));
    Ok(())
}

#[tokio::test]
async fn overlapped_open_reports_tracked_changes_before_untracked_files() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("tracked.txt"), "one\n")?;
    fs::create_dir(temp.path().join("nested"))?;
    init(&temp)?;
    fs::write(temp.path().join("tracked.txt"), "two\n")?;
    fs::write(temp.path().join("nested").join("new.txt"), "new\n")?;
    let service = Service::default();
    let partial = std::sync::Mutex::new(None);
    let (repo, snapshot) = service
        .open_with_inventory(temp.path().join("nested"), |entry, status| {
            *partial.lock().unwrap_or_else(|e| e.into_inner()) =
                Some((entry.root().to_path_buf(), status));
        })
        .await?;
    assert_eq!(repo.root(), fs::canonicalize(temp.path())?);
    assert_eq!(snapshot.status.files.len(), 2);
    assert_eq!(snapshot.status.files[0].path.bytes(), b"tracked.txt");
    assert_eq!(snapshot.status.files[1].path.bytes(), b"nested/new.txt");
    assert_eq!(*snapshot.status, *repo.status(true).await?.status);
    // The phased callback only exists on the Git path; when it fires it carries tracked changes.
    if let Some((root, tracked)) = partial.into_inner().unwrap_or_else(|e| e.into_inner()) {
        assert_eq!(root, repo.root());
        assert_eq!(tracked.files.len(), 1);
        assert_eq!(tracked.files[0].path.bytes(), b"tracked.txt");
    }
    let plain = tempfile::tempdir()?;
    assert!(
        service
            .open_with_inventory(plain.path(), |_, _| {})
            .await
            .is_err()
    );
    Ok(())
}
