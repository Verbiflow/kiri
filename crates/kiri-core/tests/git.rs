use anyhow::{Result, bail};
use kiri_core::{
    diff::{LARGE_FILE_BYTES, PREVIEW_BYTES},
    model::{DiffSide, RepoPath},
    repo::Repository,
    storage::Store,
    workspace::Workspaces,
};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) -> Result<()> {
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
    Ok(())
}

fn fixture() -> Result<TempDir> {
    let temp = TempDir::new()?;
    git(temp.path(), &["init", "--quiet"])?;
    fs::write(
        temp.path().join("file.txt"),
        (1..=30).map(|i| format!("line {i}\n")).collect::<String>(),
    )?;
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "initial"])?;
    Ok(temp)
}

#[tokio::test]
async fn phased_inventory_matches_a_complete_status() -> Result<()> {
    let temp = fixture()?;
    fs::write(temp.path().join("file.txt"), "tracked change\n")?;
    fs::write(temp.path().join("new.txt"), "untracked change\n")?;
    let repo = Repository::open(temp.path()).await?;
    let mut status = repo.tracked_status().await?;
    assert_eq!(status.files.len(), 1);
    status.files.extend(repo.untracked_files().await?);
    assert_eq!(status, repo.status().await?);
    Ok(())
}

#[tokio::test]
async fn tracked_changes_under_an_ignored_directory_remain_visible_and_stage_normally() -> Result<()>
{
    let temp = fixture()?;
    fs::create_dir_all(temp.path().join("src/artifacts"))?;
    fs::write(temp.path().join("src/artifacts/tracked.ts"), "original\n")?;
    git(temp.path(), &["add", "src/artifacts/tracked.ts"])?;
    git(
        temp.path(),
        &["commit", "-qm", "track source before ignore rule"],
    )?;
    fs::write(temp.path().join(".gitignore"), "artifacts/\n")?;
    git(temp.path(), &["add", ".gitignore"])?;
    git(
        temp.path(),
        &["commit", "-qm", "ignore generated artifacts"],
    )?;
    fs::write(
        temp.path().join("src/artifacts/tracked.ts"),
        "tracked change\n",
    )?;
    fs::write(
        temp.path().join("src/artifacts/ignored.bin"),
        "must remain untracked\n",
    )?;
    let repo = Repository::open(temp.path()).await?;
    let status = repo.status().await?;
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].path.bytes(), b"src/artifacts/tracked.ts");
    let result = repo.stage(&[status.files[0].path.clone()]).await;
    assert!(
        result.is_ok(),
        "Tracked source must not be rejected by its parent's ignore rule: {result:?}"
    );
    let status = repo.status().await?;
    assert_eq!(status.files.len(), 1);
    assert!(status.files[0].staged.is_some());
    assert!(status.files[0].worktree.is_none());
    assert_eq!(
        repo.git(&["ls-files", "--", "src/artifacts/ignored.bin"])
            .await?,
        b""
    );
    Ok(())
}

#[tokio::test]
async fn folder_staging_respects_fresh_ignore_rules_without_hiding_tracked_changes() -> Result<()> {
    for folder_scope in [false, true] {
        let temp = fixture()?;
        fs::create_dir_all(temp.path().join("src/artifacts"))?;
        for name in ["tracked.ts", "deleted.ts"] {
            fs::write(temp.path().join("src/artifacts").join(name), "original\n")?;
        }
        fs::write(temp.path().join("sibling.txt"), "original sibling\n")?;
        git(temp.path(), &["add", "."])?;
        git(temp.path(), &["commit", "-qm", "tracked source fixture"])?;
        fs::write(
            temp.path().join(".gitignore"),
            "artifacts/\n*.log\n!src/keep.log\n",
        )?;
        git(temp.path(), &["add", ".gitignore"])?;
        git(temp.path(), &["commit", "-qm", "ignore policy fixture"])?;
        fs::write(
            temp.path().join("src/artifacts/tracked.ts"),
            "tracked edit\n",
        )?;
        fs::remove_file(temp.path().join("src/artifacts/deleted.ts"))?;
        for name in [
            "new.ts",
            "keep.log",
            "cache.log",
            "became-ignored.tmp",
            "literal[*]\n.ts",
            "artifacts/untracked.ts",
        ] {
            fs::write(temp.path().join("src").join(name), "new content\n")?;
        }
        fs::write(temp.path().join("sibling.txt"), "staged sibling\n")?;
        git(temp.path(), &["add", "sibling.txt"])?;
        fs::write(temp.path().join("sibling.txt"), "later working sibling\n")?;
        let repo = Repository::open(temp.path()).await?;
        let stale: Vec<_> = repo
            .status()
            .await?
            .files
            .into_iter()
            .filter(|file| file.path.bytes().starts_with(b"src/") && file.worktree.is_some())
            .map(|file| file.path)
            .collect();
        assert!(
            stale
                .iter()
                .any(|path| path.bytes() == b"src/became-ignored.tmp")
        );
        fs::write(temp.path().join(".git/info/exclude"), "*.tmp\n")?;
        let paths = if folder_scope {
            vec![RepoPath::new(b"src".to_vec())?]
        } else {
            stale
        };
        repo.stage(&paths).await?;
        let status = repo.status().await?;
        let staged: std::collections::BTreeSet<_> = status
            .files
            .iter()
            .filter(|file| file.staged.is_some())
            .map(|file| file.path.bytes().to_vec())
            .collect();
        assert_eq!(
            staged,
            [
                b"src/artifacts/tracked.ts".to_vec(),
                b"src/artifacts/deleted.ts".to_vec(),
                b"src/new.ts".to_vec(),
                b"src/keep.log".to_vec(),
                b"src/literal[*]\n.ts".to_vec(),
                b"sibling.txt".to_vec()
            ]
            .into_iter()
            .collect()
        );
        assert!(
            !status
                .files
                .iter()
                .any(|file| file.path.bytes().starts_with(b"src/") && file.worktree.is_some())
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("sibling.txt"))?,
            "later working sibling\n"
        );
        assert_eq!(
            repo.git(&["show", ":sibling.txt"]).await?,
            b"staged sibling\n"
        );
        let index = fs::read(temp.path().join(".git/index"))?;
        repo.stage(&[RepoPath::new(b"src/became-ignored.tmp".to_vec())?])
            .await?;
        assert_eq!(fs::read(temp.path().join(".git/index"))?, index);
    }
    Ok(())
}

#[tokio::test]
async fn stages_one_hunk_without_touching_the_worktree() -> Result<()> {
    let temp = fixture()?;
    let repo = Repository::open(temp.path()).await?;
    let original = fs::read_to_string(temp.path().join("file.txt"))?;
    let changed = original
        .replace("line 2\n", "changed 2\n")
        .replace("line 29\n", "changed 29\n");
    fs::write(temp.path().join("file.txt"), &changed)?;
    let status = repo.status().await?;
    let doc = repo
        .diff(&status.files[0], DiffSide::Worktree, false)
        .await?;
    assert_eq!(doc.hunks.len(), 2);
    repo.stage_hunk(&status.files[0], DiffSide::Worktree, &doc, 0)
        .await?;
    let status = repo.status().await?;
    let staged = repo.diff(&status.files[0], DiffSide::Staged, false).await?;
    assert!(String::from_utf8_lossy(&staged.raw).contains("changed 2"));
    assert!(!String::from_utf8_lossy(&staged.raw).contains("changed 29"));
    assert_eq!(fs::read_to_string(temp.path().join("file.txt"))?, changed);
    assert!(
        repo.stage_hunk(&status.files[0], DiffSide::Worktree, &doc, 1)
            .await
            .is_err()
    );
    repo.stage_hunk(&status.files[0], DiffSide::Staged, &staged, 0)
        .await?;
    assert!(repo.status().await?.files[0].staged.is_none());
    Ok(())
}

#[tokio::test]
async fn literal_paths_and_unborn_unstage_preserve_files() -> Result<()> {
    let temp = TempDir::new()?;
    git(temp.path(), &["init", "--quiet"])?;
    fs::write(temp.path().join(":(glob)*"), "literal\n")?;
    fs::write(temp.path().join("other"), "other\n")?;
    let repo = Repository::open(temp.path()).await?;
    let path = RepoPath::new(b":(glob)*".to_vec())?;
    repo.stage(std::slice::from_ref(&path)).await?;
    assert_eq!(
        repo.status()
            .await?
            .files
            .iter()
            .filter(|f| f.staged.is_some())
            .count(),
        1
    );
    fs::write(temp.path().join(":(glob)*"), "edited after staging\n")?;
    repo.unstage(&[path]).await?;
    assert_eq!(
        fs::read_to_string(temp.path().join(":(glob)*"))?,
        "edited after staging\n"
    );
    assert!(
        repo.status()
            .await?
            .files
            .iter()
            .all(|f| f.staged.is_none())
    );
    assert!(temp.path().join(":(glob)*").exists());
    Ok(())
}

#[tokio::test]
async fn oversized_file_does_not_block_ordinary_diffs() -> Result<()> {
    let temp = fixture()?;
    fs::write(
        temp.path().join("huge.txt"),
        vec![b'a'; LARGE_FILE_BYTES as usize + 1],
    )?;
    fs::write(temp.path().join("file.txt"), "small change\n")?;
    let repo = Repository::open(temp.path()).await?;
    let status = repo.status().await?;
    let large = status
        .files
        .iter()
        .find(|f| f.path.bytes() == b"huge.txt")
        .ok_or_else(|| anyhow::anyhow!("large file missing"))?;
    let start = Instant::now();
    let diff = repo.diff(large, DiffSide::Worktree, false).await?;
    assert!(diff.notice.is_some());
    assert!(start.elapsed() < Duration::from_secs(1));
    let expanded = repo.diff(large, DiffSide::Worktree, true).await?;
    assert!(expanded.truncated);
    assert!(expanded.raw.len() <= PREVIEW_BYTES);
    let small = repo
        .diff(&status.files[0], DiffSide::Worktree, false)
        .await?;
    assert!(!small.lines.is_empty());
    Ok(())
}

#[tokio::test]
async fn staging_a_folder_preserves_siblings_and_existing_index_entries() -> Result<()> {
    let temp = fixture()?;
    fs::create_dir(temp.path().join("src"))?;
    fs::create_dir(temp.path().join("src-other"))?;
    for name in [
        "src/edit.txt",
        "src/delete.txt",
        "src-other/unrelated.txt",
        "already-staged.txt",
    ] {
        fs::write(temp.path().join(name), "original\n")?;
    }
    git(temp.path(), &["add", "."])?;
    git(temp.path(), &["commit", "--quiet", "-m", "folders"])?;
    fs::write(temp.path().join("src/edit.txt"), "edited\n")?;
    fs::write(temp.path().join("src/new file.txt"), "new\n")?;
    fs::remove_file(temp.path().join("src/delete.txt"))?;
    fs::write(temp.path().join("src-other/unrelated.txt"), "unrelated\n")?;
    fs::write(temp.path().join("already-staged.txt"), "staged earlier\n")?;
    git(temp.path(), &["add", "already-staged.txt"])?;
    let repo = Repository::open(temp.path()).await?;
    let status = repo.status().await?;
    let included: Vec<_> = status
        .files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.worktree.is_some())
        .map(|(i, _)| i)
        .collect();
    let tree = kiri_core::tree::ChangeTree::build(&status.files, &included);
    let folder = tree
        .nodes
        .iter()
        .position(|node| node.is_folder() && node.path(&status.files).bytes() == b"src")
        .ok_or_else(|| anyhow::anyhow!("folder missing"))?;
    let paths: Vec<_> = tree
        .members(folder)
        .iter()
        .map(|&i| status.files[i].path.clone())
        .collect();
    assert_eq!(paths.len(), 3);
    repo.stage(&paths).await?;
    let status = repo.status().await?;
    assert_eq!(
        status.files.iter().filter(|f| f.staged.is_some()).count(),
        4
    );
    assert!(
        status
            .files
            .iter()
            .any(|f| f.path.bytes() == b"src-other/unrelated.txt" && f.staged.is_none())
    );
    repo.unstage(&paths).await?;
    let staged: Vec<_> = repo
        .status()
        .await?
        .files
        .into_iter()
        .filter(|f| f.staged.is_some())
        .collect();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].path.bytes(), b"already-staged.txt");
    assert_eq!(
        fs::read_to_string(temp.path().join("src/edit.txt"))?,
        "edited\n"
    );
    assert!(temp.path().join("src/new file.txt").exists());
    assert!(!temp.path().join("src/delete.txt").exists());
    Ok(())
}

#[tokio::test]
async fn workspace_registration_is_idempotent_and_canonical() -> Result<()> {
    let temp = fixture()?;
    let config = TempDir::new()?;
    fs::create_dir(temp.path().join("nested"))?;
    let store = Store::at(config.path());
    Workspaces::add(&store, temp.path(), None).await?;
    Workspaces::add(&store, &temp.path().join("nested"), None).await?;
    assert_eq!(Workspaces::load(&store)?.entries.len(), 1);
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn filenames_need_not_be_utf8() -> Result<()> {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let temp = fixture()?;
    let name = b"strange\xff\nname".to_vec();
    fs::write(
        temp.path().join(OsString::from_vec(name.clone())),
        "content\n",
    )?;
    let repo = Repository::open(temp.path()).await?;
    let status = repo.status().await?;
    assert_eq!(status.files[0].path.bytes(), name);
    repo.stage(&[status.files[0].path.clone()]).await?;
    assert!(repo.status().await?.files[0].staged.is_some());
    Ok(())
}
