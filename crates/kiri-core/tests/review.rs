use anyhow::Result;
use kiri_core::{repo::Repository, review::CaptureScope};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    process::Command,
};

#[tokio::test]
async fn batched_patches_preserve_exact_bytes_and_literal_names() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let output = Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .output()?;
    anyhow::ensure!(output.status.success());
    let mut paths = Vec::new();
    for (name, content) in [
        ("normal.txt", b"literal\ndiff --git not-a-header\n".to_vec()),
        ("with spaces.txt", b"spaces\n".to_vec()),
        ("line\nbreak.txt", b"newline filename\n".to_vec()),
        ("binary.dat", vec![0, 255, 0, 17]),
        (
            "minified.txt",
            format!("{}TAIL\n", "x".repeat(200000)).into_bytes(),
        ),
    ] {
        fs::write(temp.path().join(name), content)?;
        paths.push(kiri_core::model::RepoPath::new(name.as_bytes().to_vec())?);
    }
    let repo = Repository::open(temp.path()).await?;
    repo.stage(&paths).await?;
    let captured = repo.capture_changes(CaptureScope::Staged).await?;
    let batch = tempfile::NamedTempFile::new()?;
    let locations = captured.evidence.patches(&paths, batch.reopen()?).await?;
    assert_eq!(locations.len(), paths.len());
    for location in locations {
        let single = tempfile::NamedTempFile::new()?;
        captured
            .evidence
            .patch(&location.path, single.reopen()?)
            .await?;
        let mut reader = batch.reopen()?;
        reader.seek(SeekFrom::Start(location.offset))?;
        let mut bytes = Vec::new();
        reader.take(location.bytes).read_to_end(&mut bytes)?;
        assert_eq!(bytes, fs::read(single.path())?, "{}", location.path);
    }
    captured.snapshot.verify(&repo).await?;
    Ok(())
}

#[tokio::test]
async fn working_capture_is_private_complete_and_detects_later_edits() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let output = Command::new("git")
        .args(["init", "-q"])
        .current_dir(temp.path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    anyhow::ensure!(output.status.success(), "Fixture Git init");
    fs::write(temp.path().join("file.txt"), "Complete working evidence\n")?;
    let repo = Repository::open(temp.path()).await?;
    let before = repo.git(&["count-objects", "-v"]).await?;
    let captured = repo.capture_changes(CaptureScope::Auto).await?;
    assert_eq!(before, repo.git(&["count-objects", "-v"]).await?);
    assert!(!temp.path().join(".git/index").exists());
    let changes = captured.evidence.changes().await?;
    assert_eq!(changes.len(), 1);
    let patch = tempfile::NamedTempFile::new()?;
    captured
        .evidence
        .patch(&changes[0].0, patch.reopen()?)
        .await?;
    assert!(fs::read_to_string(patch.path())?.contains("Complete working evidence"));
    captured.snapshot.verify(&repo).await?;
    fs::write(temp.path().join("file.txt"), "Later working edit\n")?;
    assert!(captured.snapshot.verify(&repo).await.is_err());
    assert!(repo.head().await?.is_none());
    Ok(())
}
