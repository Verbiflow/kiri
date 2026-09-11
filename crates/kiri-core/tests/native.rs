//! Differential tests: the in-process status must equal `git status --porcelain=v2` exactly,
//! and every repository state it does not reproduce must be handed to Git.

use anyhow::{Result, bail, ensure};
use kiri_core::{
    model::{ChangeKind, RepoStatus},
    repo::{Repository, StatusBackend},
};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};
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
        .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
        .output()?;
    if !output.status.success() {
        bail!(
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write(root: &Path, path: &str, content: &str) -> Result<()> {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(full, content)?;
    Ok(())
}

fn repo_with_commit() -> Result<TempDir> {
    let temp = TempDir::new()?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=main"])?;
    write(temp.path(), "a.txt", "alpha\n")?;
    write(temp.path(), "b.txt", "bravo\n")?;
    write(temp.path(), "dir/c.txt", "charlie\n")?;
    write(temp.path(), "exec.sh", "#!/bin/sh\n")?;
    write(temp.path(), "mode.txt", "mode\n")?;
    write(temp.path(), "t.txt", "typechange\n")?;
    write(temp.path(), "t2.txt", "typechange staged\n")?;
    write(temp.path(), ".gitignore", "build/\n*.log\n")?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "initial"])?;
    Ok(temp)
}

/// Both backends must agree, and the native backend must actually have answered.
async fn assert_native_matches_git(root: &Path) -> Result<RepoStatus> {
    let repo = Repository::open(root).await?;
    ensure!(
        repo.status_backend() == StatusBackend::Native,
        "gitoxide could not open the fixture"
    );
    let native = repo
        .native_status(true)
        .await?
        .ok_or_else(|| anyhow::anyhow!("native status declined a supported repository state"))?;
    let via_git = repo
        .clone()
        .with_status_backend(StatusBackend::Git)
        .status()
        .await?;
    assert_eq!(native, via_git, "full status differs from git status");
    let native_tracked = repo
        .native_status(false)
        .await?
        .ok_or_else(|| anyhow::anyhow!("native tracked status declined"))?;
    let git_tracked = repo
        .clone()
        .with_status_backend(StatusBackend::Git)
        .tracked_status()
        .await?;
    assert_eq!(native_tracked, git_tracked, "tracked status differs");
    assert_eq!(repo.status().await?, via_git, "public status differs");
    Ok(native)
}

/// Git must answer, native must decline, and the public status must still be correct.
async fn assert_native_declines(root: &Path) -> Result<RepoStatus> {
    let repo = Repository::open(root).await?;
    ensure!(repo.status_backend() == StatusBackend::Native);
    ensure!(
        repo.native_status(true).await?.is_none(),
        "native status answered a state it must leave to git"
    );
    let via_git = repo
        .clone()
        .with_status_backend(StatusBackend::Git)
        .status()
        .await?;
    assert_eq!(repo.status().await?, via_git);
    Ok(via_git)
}

#[tokio::test]
async fn worktree_and_index_changes_match_git_exactly() -> Result<()> {
    let temp = repo_with_commit()?;
    let root = temp.path();
    write(root, "a.txt", "alpha changed\n")?;
    write(root, "b.txt", "bravo staged\n")?;
    git(root, &["add", "b.txt"])?;
    write(root, "b.txt", "bravo staged then edited\n")?;
    fs::remove_file(root.join("dir/c.txt"))?;
    git(root, &["rm", "--cached", "--quiet", "exec.sh"])?;
    write(root, "new.txt", "new staged\n")?;
    git(root, &["add", "new.txt"])?;
    write(root, "untracked.txt", "loose\n")?;
    write(root, "deep/x/y.txt", "nested untracked\n")?;
    write(root, "build/out.o", "ignored\n")?;
    write(root, "trace.log", "ignored too\n")?;
    fs::create_dir_all(root.join("empty"))?;
    let mut perms = fs::metadata(root.join("mode.txt"))?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(root.join("mode.txt"), perms)?;
    std::os::unix::fs::symlink("a.txt", root.join("link"))?;
    std::os::unix::fs::symlink("b.txt", root.join("slink"))?;
    git(root, &["add", "slink"])?;
    fs::remove_file(root.join("t.txt"))?;
    std::os::unix::fs::symlink("a.txt", root.join("t.txt"))?;
    fs::remove_file(root.join("t2.txt"))?;
    std::os::unix::fs::symlink("b.txt", root.join("t2.txt"))?;
    git(root, &["add", "t2.txt"])?;
    write(root, "with\nnewline.txt", "newline in name\n")?;
    let status = assert_native_matches_git(root).await?;
    let kinds: Vec<(String, Option<ChangeKind>, Option<ChangeKind>)> = status
        .files
        .iter()
        .map(|file| (file.path.display(), file.staged, file.worktree))
        .collect();
    assert!(kinds.contains(&("a.txt".into(), None, Some(ChangeKind::Modified))));
    assert!(kinds.contains(&(
        "b.txt".into(),
        Some(ChangeKind::Modified),
        Some(ChangeKind::Modified)
    )));
    assert!(kinds.contains(&("dir/c.txt".into(), None, Some(ChangeKind::Deleted))));
    assert!(kinds.contains(&("exec.sh".into(), Some(ChangeKind::Deleted), None)));
    assert!(kinds.contains(&("exec.sh".into(), None, Some(ChangeKind::Untracked))));
    assert!(kinds.contains(&("new.txt".into(), Some(ChangeKind::Added), None)));
    assert!(kinds.contains(&("mode.txt".into(), None, Some(ChangeKind::Modified))));
    assert!(kinds.contains(&("t.txt".into(), None, Some(ChangeKind::TypeChanged))));
    assert!(kinds.contains(&("t2.txt".into(), Some(ChangeKind::TypeChanged), None)));
    assert!(kinds.contains(&("slink".into(), Some(ChangeKind::Added), None)));
    assert!(kinds.contains(&("deep/x/y.txt".into(), None, Some(ChangeKind::Untracked))));
    assert!(
        !kinds
            .iter()
            .any(|(path, _, _)| path.starts_with("build/") || path.ends_with(".log"))
    );
    assert!(
        status
            .files
            .iter()
            .all(|file| file.path.display() != "empty")
    );
    assert_eq!(status.branch, "main");
    assert!(status.head.is_some() && status.upstream.is_none());
    Ok(())
}

#[tokio::test]
async fn unborn_branch_reports_additions_without_head() -> Result<()> {
    let temp = TempDir::new()?;
    git(temp.path(), &["init", "--quiet", "--initial-branch=trunk"])?;
    write(temp.path(), "staged.txt", "one\n")?;
    write(temp.path(), "loose.txt", "two\n")?;
    git(temp.path(), &["add", "staged.txt"])?;
    let status = assert_native_matches_git(temp.path()).await?;
    assert_eq!(status.branch, "trunk");
    assert_eq!(status.head, None);
    assert_eq!(status.files[0].staged, Some(ChangeKind::Added));
    assert_eq!(status.files[0].head_oid, None);
    assert!(status.files[0].index_oid.is_some());
    Ok(())
}

#[tokio::test]
async fn clean_and_detached_repositories_match() -> Result<()> {
    let temp = repo_with_commit()?;
    let status = assert_native_matches_git(temp.path()).await?;
    assert!(status.files.is_empty());
    git(temp.path(), &["checkout", "--quiet", "--detach"])?;
    write(temp.path(), "a.txt", "detached edit\n")?;
    let status = assert_native_matches_git(temp.path()).await?;
    assert_eq!(status.branch, "(detached)");
    assert_eq!(status.files.len(), 1);
    Ok(())
}

#[tokio::test]
async fn upstream_counts_and_missing_tracking_refs_match() -> Result<()> {
    let origin = repo_with_commit()?;
    let temp = TempDir::new()?;
    let clone = temp.path().join("clone");
    git(
        temp.path(),
        &[
            "clone",
            "--quiet",
            origin.path().to_str().unwrap_or_default(),
            "clone",
        ],
    )?;
    write(origin.path(), "remote-one.txt", "1\n")?;
    git(origin.path(), &["add", "."])?;
    git(origin.path(), &["commit", "--quiet", "-m", "remote one"])?;
    write(origin.path(), "remote-two.txt", "2\n")?;
    git(origin.path(), &["add", "."])?;
    git(origin.path(), &["commit", "--quiet", "-m", "remote two"])?;
    write(&clone, "local.txt", "local\n")?;
    git(&clone, &["add", "."])?;
    git(&clone, &["commit", "--quiet", "-m", "local"])?;
    git(&clone, &["fetch", "--quiet"])?;
    write(&clone, "a.txt", "edit\n")?;
    let status = assert_native_matches_git(&clone).await?;
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((status.ahead, status.behind), (1, 2));
    git(&clone, &["update-ref", "-d", "refs/remotes/origin/main"])?;
    let status = assert_native_matches_git(&clone).await?;
    assert_eq!((status.ahead, status.behind), (0, 0));
    git(&clone, &["checkout", "--quiet", "-b", "topic"])?;
    let status = assert_native_matches_git(&clone).await?;
    assert_eq!(status.branch, "topic");
    assert_eq!(status.upstream, None);
    Ok(())
}

#[tokio::test]
async fn line_ending_conversion_agrees_with_git() -> Result<()> {
    // Git reports CRLF rewrites of a committed LF file as modified under every combination of
    // `text` attributes and `core.autocrlf`; the in-process filter pipeline must agree exactly.
    for (attributes, autocrlf) in [
        (None, None),
        (None, Some("true")),
        (None, Some("input")),
        (Some("*.txt text\n"), None),
        (Some("*.txt text\n"), Some("true")),
        (Some("* text=auto\n"), None),
        (Some("*.txt text eol=crlf\n"), None),
        (Some("*.txt -text\n"), Some("true")),
    ] {
        let temp = repo_with_commit()?;
        let root = temp.path();
        if let Some(attributes) = attributes {
            write(root, ".gitattributes", attributes)?;
            git(root, &["add", ".gitattributes"])?;
            git(root, &["commit", "--quiet", "-m", "attributes"])?;
        }
        if let Some(value) = autocrlf {
            git(root, &["config", "core.autocrlf", value])?;
        }
        write(root, "a.txt", "alpha\r\n")?;
        write(root, "b.txt", "bravo\r\nmore\r\n")?;
        let status = assert_native_matches_git(root).await?;
        assert!(status.files.iter().any(|f| f.path.display() == "b.txt"));
    }
    Ok(())
}

#[tokio::test]
async fn nested_repository_is_listed_as_a_directory() -> Result<()> {
    let temp = repo_with_commit()?;
    let nested = temp.path().join("nested");
    fs::create_dir_all(&nested)?;
    git(&nested, &["init", "--quiet"])?;
    write(&nested, "inner.txt", "inner\n")?;
    let status = assert_native_matches_git(temp.path()).await?;
    let paths: Vec<String> = status.files.iter().map(|f| f.path.display()).collect();
    assert_eq!(paths, vec!["nested/"]);
    Ok(())
}

#[tokio::test]
async fn staged_renames_use_gits_rename_detection_on_both_backends() -> Result<()> {
    let temp = repo_with_commit()?;
    git(temp.path(), &["mv", "a.txt", "renamed.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let raw = repo
        .native_status(true)
        .await?
        .ok_or_else(|| anyhow::anyhow!("native declined"))?;
    let kinds: Vec<_> = raw
        .files
        .iter()
        .map(|f| (f.path.display(), f.staged))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("a.txt".to_owned(), Some(ChangeKind::Deleted)),
            ("renamed.txt".to_owned(), Some(ChangeKind::Added))
        ]
    );
    let status = repo.status().await?;
    assert_eq!(
        status,
        repo.clone()
            .with_status_backend(StatusBackend::Git)
            .status()
            .await?
    );
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].staged, Some(ChangeKind::Renamed));
    assert_eq!(
        status.files[0].original_path.as_ref().map(|p| p.display()),
        Some("a.txt".into())
    );
    Ok(())
}

#[tokio::test]
async fn conflicts_are_left_to_git() -> Result<()> {
    let temp = repo_with_commit()?;
    let root = temp.path();
    git(root, &["checkout", "--quiet", "-b", "other"])?;
    write(root, "a.txt", "theirs\n")?;
    git(root, &["commit", "--quiet", "-am", "theirs"])?;
    git(root, &["checkout", "--quiet", "main"])?;
    write(root, "a.txt", "ours\n")?;
    git(root, &["commit", "--quiet", "-am", "ours"])?;
    let _ = git(root, &["merge", "other"]);
    let status = assert_native_declines(root).await?;
    assert!(status.files.iter().any(|file| file.conflicted()));
    Ok(())
}

#[tokio::test]
async fn submodules_sparse_entries_and_intent_to_add_are_left_to_git() -> Result<()> {
    let library = repo_with_commit()?;
    let temp = repo_with_commit()?;
    let root = temp.path();
    git(
        root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            library.path().to_str().unwrap_or_default(),
            "vendor/lib",
        ],
    )?;
    let status = assert_native_declines(root).await?;
    assert!(status.files.iter().any(|file| file.submodule));

    let temp = repo_with_commit()?;
    git(temp.path(), &["update-index", "--skip-worktree", "a.txt"])?;
    write(temp.path(), "a.txt", "hidden edit\n")?;
    assert_native_declines(temp.path()).await?;

    let temp = repo_with_commit()?;
    git(
        temp.path(),
        &["update-index", "--assume-unchanged", "a.txt"],
    )?;
    write(temp.path(), "a.txt", "ignored edit\n")?;
    assert_native_declines(temp.path()).await?;

    let temp = repo_with_commit()?;
    write(temp.path(), "intent.txt", "later\n")?;
    git(temp.path(), &["add", "-N", "intent.txt"])?;
    assert_native_declines(temp.path()).await?;
    Ok(())
}

#[tokio::test]
async fn backend_switch_and_discovery_agree_with_git() -> Result<()> {
    let temp = repo_with_commit()?;
    let nested = temp.path().join("dir");
    let repo = Repository::open(&nested).await?;
    assert_eq!(repo.root(), fs::canonicalize(temp.path())?);
    assert_eq!(repo.status_backend(), StatusBackend::Native);
    assert!(Repository::discover(&nested).await?.is_some());
    let plain = TempDir::new()?;
    assert!(Repository::discover(plain.path()).await?.is_none());
    assert!(Repository::open(plain.path()).await.is_err());
    let bare = TempDir::new()?;
    git(bare.path(), &["init", "--quiet", "--bare"])?;
    assert!(Repository::discover(bare.path()).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn decomposed_unicode_names_match_git() -> Result<()> {
    // macOS stores names in NFD; Git re-composes them when core.precomposeUnicode is set.
    let temp = repo_with_commit()?;
    let root = temp.path();
    write(root, "caf\u{0065}\u{0301}.txt", "decomposed\n")?;
    write(root, "na\u{00ef}ve.txt", "composed\n")?;
    let status = assert_native_matches_git(root).await?;
    assert_eq!(status.files.len(), 2);
    git(root, &["add", "."])?;
    assert_native_matches_git(root).await?;
    Ok(())
}
