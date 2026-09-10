use anyhow::{Result, bail};
use kiri_core::{
    repo::Repository,
    sync::{SyncAction, SyncTarget},
};

fn target(head: &Option<String>) -> SyncTarget {
    SyncTarget {
        head: head.clone(),
        branch: "main".into(),
    }
}
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use tempfile::TempDir;

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
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr));
    }
    Ok(String::from_utf8(output.stdout)?)
}

struct RemoteFixture {
    _temp: TempDir,
    client: PathBuf,
    peer: PathBuf,
    remote: PathBuf,
}

fn fixture() -> Result<RemoteFixture> {
    let temp = TempDir::new()?;
    let seed = temp.path().join("seed");
    fs::create_dir(&seed)?;
    git(&seed, &["init", "--initial-branch=main", "--quiet"])?;
    fs::write(seed.join("file.txt"), "initial\n")?;
    git(&seed, &["add", "."])?;
    git(&seed, &["commit", "-m", "initial", "--quiet"])?;
    git(temp.path(), &["clone", "--bare", "seed", "remote.git"])?;
    git(temp.path(), &["clone", "remote.git", "client"])?;
    git(temp.path(), &["clone", "remote.git", "peer"])?;
    Ok(RemoteFixture {
        client: temp.path().join("client"),
        peer: temp.path().join("peer"),
        remote: temp.path().join("remote.git"),
        _temp: temp,
    })
}

fn change(root: &Path, name: &str, contents: &str) -> Result<()> {
    fs::write(root.join(name), contents)?;
    git(root, &["add", name])?;
    git(root, &["commit", "--quiet", "-m", contents])?;
    Ok(())
}

#[tokio::test]
async fn fetching_exposes_incoming_commit_and_pull_fast_forwards() -> Result<()> {
    let fixture = fixture()?;
    let repo = Repository::open(&fixture.client).await?;
    change(&fixture.peer, "file.txt", "incoming\n")?;
    git(&fixture.peer, &["push", "origin", "main"])?;
    assert_eq!(repo.status().await?.behind, 0);
    repo.sync(SyncAction::Fetch, None).await?;
    let status = repo.status().await?;
    assert_eq!(status.behind, 1);
    assert_eq!(
        fs::read_to_string(fixture.client.join("file.txt"))?,
        "initial\n"
    );
    repo.sync(SyncAction::Pull, Some(&SyncTarget::from(&status)))
        .await?;
    assert_eq!(repo.status().await?.behind, 0);
    assert_eq!(
        fs::read_to_string(fixture.client.join("file.txt"))?,
        "incoming\n"
    );
    Ok(())
}

#[tokio::test]
async fn dirty_and_diverged_pulls_leave_working_changes_intact() -> Result<()> {
    let fixture = fixture()?;
    let repo = Repository::open(&fixture.client).await?;
    change(&fixture.peer, "file.txt", "remote\n")?;
    git(&fixture.peer, &["push", "origin", "main"])?;
    repo.sync(SyncAction::Fetch, None).await?;
    fs::write(fixture.client.join("file.txt"), "local work\n")?;
    let head = repo.head().await?;
    assert!(
        repo.sync(SyncAction::Pull, Some(&target(&head)))
            .await
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(fixture.client.join("file.txt"))?,
        "local work\n"
    );
    change(&fixture.client, "file.txt", "diverged local\n")?;
    let status = repo.status().await?;
    assert_eq!((status.ahead, status.behind), (1, 1));
    assert!(
        repo.sync(SyncAction::Pull, Some(&SyncTarget::from(&status)))
            .await
            .is_err()
    );
    assert_eq!(repo.head().await?, status.head);
    Ok(())
}

#[tokio::test]
async fn push_targets_only_the_reviewed_branch_and_refuses_stale_head() -> Result<()> {
    let fixture = fixture()?;
    let repo = Repository::open(&fixture.client).await?;
    let old = repo.head().await?;
    change(&fixture.client, "new.txt", "outgoing\n")?;
    assert!(
        repo.sync(SyncAction::Push, Some(&target(&old)))
            .await
            .is_err()
    );
    assert_eq!(
        git(&fixture.remote, &["log", "-1", "--format=%s"])?.trim(),
        "initial"
    );
    let head = repo.head().await?;
    repo.sync(SyncAction::Push, Some(&target(&head))).await?;
    assert_eq!(
        git(&fixture.remote, &["rev-parse", "HEAD"])?.trim(),
        head.as_deref().unwrap_or_default()
    );
    assert_eq!(repo.status().await?.ahead, 0);
    Ok(())
}

#[tokio::test]
async fn switching_to_a_branch_at_the_same_oid_invalidates_reviewed_actions() -> Result<()> {
    let fixture = fixture()?;
    let repo = Repository::open(&fixture.client).await?;
    git(&fixture.client, &["branch", "other"])?;
    fs::write(fixture.client.join("file.txt"), "staged\n")?;
    git(&fixture.client, &["add", "file.txt"])?;
    let snapshot = repo.staged_snapshot().await?;
    let reviewed = SyncTarget::from(&repo.status().await?);
    git(&fixture.client, &["switch", "other"])?;
    assert_eq!(repo.head().await?, snapshot.head);
    assert!(repo.verify_snapshot(&snapshot).await.is_err());
    assert!(repo.sync(SyncAction::Push, Some(&reviewed)).await.is_err());
    Ok(())
}

#[tokio::test]
async fn branch_picker_and_history_use_real_refs() -> Result<()> {
    let fixture = fixture()?;
    git(&fixture.client, &["branch", "topic"])?;
    let repo = Repository::open(&fixture.client).await?;
    assert_eq!(repo.branches().await?.len(), 2);
    repo.switch_branch("topic").await?;
    assert_eq!(repo.status().await?.branch, "topic");
    assert!(repo.switch_branch("--force").await.is_err());
    let history = repo.history(20).await?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].subject, "initial");
    Ok(())
}
