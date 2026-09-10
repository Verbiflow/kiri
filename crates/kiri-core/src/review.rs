use crate::{
    commit::StagedSnapshot,
    model::{RepoPath, digest},
    repo::{Repository, checked},
};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureScope {
    Staged,
    Worktree,
    #[default]
    Auto,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileStamp {
    pub path: RepoPath,
    pub mode: u32,
    pub digest: Option<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeSnapshot {
    pub head: Option<String>,
    pub head_ref: Option<String>,
    pub index_digest: Option<String>,
    pub files: Vec<FileStamp>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewSnapshot {
    Staged { snapshot: StagedSnapshot },
    Worktree { snapshot: WorktreeSnapshot },
}
impl From<StagedSnapshot> for ReviewSnapshot {
    fn from(snapshot: StagedSnapshot) -> Self {
        Self::Staged { snapshot }
    }
}
impl ReviewSnapshot {
    pub fn source(&self) -> &'static str {
        match self {
            Self::Staged { .. } => "staged",
            Self::Worktree { .. } => "working-tree",
        }
    }
    pub fn head(&self) -> Option<&str> {
        match self {
            Self::Staged { snapshot } => snapshot.head.as_deref(),
            Self::Worktree { snapshot } => snapshot.head.as_deref(),
        }
    }
    pub fn staged(&self) -> Result<&StagedSnapshot> {
        match self {
            Self::Staged { snapshot } => Ok(snapshot),
            Self::Worktree { .. } => {
                bail!("Stage the selected changes before creating a multi-commit plan")
            }
        }
    }
    pub async fn verify(&self, repo: &Repository) -> Result<()> {
        match self {
            Self::Staged { snapshot } => repo.verify_snapshot(snapshot).await,
            Self::Worktree { snapshot } => snapshot.verify(repo).await,
        }
    }
    pub async fn commit(
        &self,
        repo: &Repository,
        message: &str,
        paths: Option<&[RepoPath]>,
    ) -> Result<String> {
        match self {
            Self::Staged { snapshot } => repo.commit_selection(snapshot, message, paths).await,
            Self::Worktree { snapshot } => {
                snapshot.verify(repo).await?;
                let selected = paths.map(<[RepoPath]>::to_vec).unwrap_or_else(|| {
                    snapshot
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect()
                });
                let allowed: std::collections::HashSet<_> =
                    snapshot.files.iter().map(|file| &file.path).collect();
                ensure!(
                    !selected.is_empty() && selected.iter().all(|path| allowed.contains(path)),
                    "Commit selection is outside the reviewed capture"
                );
                repo.stage(&selected).await?;
                let staged = repo.staged_snapshot().await?;
                ensure!(
                    staged.head == snapshot.head && staged.head_ref == snapshot.head_ref,
                    "Branch changed while staging reviewed files"
                );
                let selected_paths: std::collections::HashSet<_> = selected.iter().collect();
                let status = repo.status().await?;
                ensure!(status.files.iter().all(|file| !selected_paths.contains(&file.path) || file.worktree.is_none()), "Staged files no longer match the reviewed working files; review the index before retrying");
                snapshot.verify_files(repo).await?;
                repo.commit_selection(&staged, message, Some(&selected))
                    .await
            }
        }
    }
}

#[derive(Default)]
struct ReadCancellation(Arc<AtomicBool>);
impl Drop for ReadCancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
pub(crate) struct ObjectStore {
    pub directory: tempfile::TempDir,
    pub objects: PathBuf,
    pub alternates: PathBuf,
}
#[derive(Clone, Debug)]
pub struct EvidenceTree {
    repo: Repository,
    pub base: String,
    pub tree: String,
}
impl EvidenceTree {
    pub async fn staged(repo: &Repository, snapshot: &StagedSnapshot) -> Result<Self> {
        Ok(Self {
            repo: repo.clone(),
            base: repo.snapshot_base(snapshot).await?,
            tree: snapshot.tree.clone(),
        })
    }
    pub async fn changes(&self) -> Result<Vec<(RepoPath, char)>> {
        self.repo.snapshot_changes(&self.base, &self.tree).await
    }
    pub async fn patches(
        &self,
        paths: &[RepoPath],
        destination: File,
    ) -> Result<Vec<crate::PatchLocation>> {
        self.repo
            .snapshot_patches(&self.base, &self.tree, paths, destination)
            .await
    }
    pub async fn patch(&self, path: &RepoPath, destination: File) -> Result<()> {
        self.repo
            .snapshot_patch(&self.base, &self.tree, path, destination)
            .await
    }
}
pub struct CapturedChanges {
    pub snapshot: ReviewSnapshot,
    pub evidence: EvidenceTree,
}

impl Repository {
    pub async fn capture_changes(&self, scope: CaptureScope) -> Result<CapturedChanges> {
        let status = self.status().await?;
        if matches!(scope, CaptureScope::Staged)
            || matches!(scope, CaptureScope::Auto)
                && status.files.iter().any(|file| file.staged.is_some())
        {
            let snapshot = self.staged_snapshot().await?;
            return Ok(CapturedChanges {
                evidence: EvidenceTree::staged(self, &snapshot).await?,
                snapshot: snapshot.into(),
            });
        }
        ensure!(!status.files.is_empty(), "There are no changes to describe");
        ensure!(
            !status.files.iter().any(|file| file.conflicted()),
            "Resolve merge conflicts before drafting a commit"
        );
        let mut snapshot = WorktreeSnapshot {
            head: self.head().await?,
            head_ref: self.head_ref().await?,
            index_digest: index_digest(self).await?,
            files: Vec::new(),
        };
        let cancellation = ReadCancellation::default();
        let directory = tempfile::tempdir()?;
        let objects = directory.path().join("objects");
        std::fs::create_dir(&objects)?;
        let store = Arc::new(ObjectStore {
            directory,
            objects,
            alternates: self.git_path("objects").await?,
        });
        let private = self.with_objects(store.clone());
        let index = store.directory.path().join("index");
        let mut command = private.command();
        command
            .arg("read-tree")
            .arg(snapshot.head.as_deref().unwrap_or("--empty"))
            .env("GIT_INDEX_FILE", &index);
        checked(
            private
                .execute(command, None, 65536, Duration::from_secs(30))
                .await?,
        )?;
        let mut hashed_names = Vec::new();
        let mut stamps = Vec::new();
        let mut copies = Vec::new();
        let mut files = status.files;
        let originals: Vec<_> = files
            .iter()
            .filter_map(|file| file.original_path.clone())
            .collect();
        for path in originals {
            if !files.iter().any(|file| file.path == path) {
                files.push(crate::model::FileChange {
                    path,
                    original_path: None,
                    staged: None,
                    worktree: Some(crate::model::ChangeKind::Deleted),
                    submodule: false,
                });
            }
        }
        for (number, file) in files.iter().enumerate() {
            if file.submodule {
                let sub = Repository::open(self.root().join(file.path.to_path_buf())).await?;
                stamps.push(FileStamp {
                    path: file.path.clone(),
                    mode: 0o160000,
                    digest: sub.head().await?,
                });
                continue;
            }
            let name = format!("file-{number}");
            let destination = store.directory.path().join(&name);
            let root = self.root().to_path_buf();
            let path = file.path.clone();
            let cancelled = cancellation.0.clone();
            let stamp = tokio::task::spawn_blocking(move || {
                stamp_file(&root, &path, Some(&destination), &cancelled)
            })
            .await??;
            if stamp.mode != 0 {
                hashed_names.extend_from_slice(name.as_bytes());
                hashed_names.push(b'\n');
                copies.push(stamps.len());
            }
            stamps.push(stamp);
        }
        let mut hashes = vec![None; stamps.len()];
        if !copies.is_empty() {
            let mut command = private.command();
            command
                .arg("-C")
                .arg(store.directory.path())
                .arg("--git-dir")
                .arg(self.git_path("").await?)
                .args(["hash-object", "-w", "--no-filters", "--stdin-paths"]);
            let output = checked(
                private
                    .execute(
                        command,
                        Some(&hashed_names),
                        16 * 1024 * 1024,
                        Duration::from_secs(120),
                    )
                    .await?,
            )?;
            let text = String::from_utf8(output)?;
            let ids: Vec<_> = text.lines().collect();
            ensure!(
                ids.len() == copies.len(),
                "Incomplete worktree object capture"
            );
            for (index, id) in copies.into_iter().zip(ids) {
                hashes[index] = Some(id.to_owned());
            }
        }
        let empty = String::from_utf8(
            private
                .git(&["hash-object", "-w", "-t", "tree", "--stdin"])
                .await?,
        )?
        .trim()
        .to_owned();
        let mut entries = Vec::new();
        for (number, stamp) in stamps.iter().enumerate() {
            let oid = if stamp.mode == 0 {
                "0".repeat(empty.len())
            } else if stamp.mode == 0o160000 {
                stamp.digest.clone().context("Submodule has no commit")?
            } else {
                hashes[number].clone().context("Missing captured object")?
            };
            entries.extend_from_slice(format!("{:o} {oid}\t", stamp.mode).as_bytes());
            entries.extend_from_slice(stamp.path.bytes());
            entries.push(0);
        }
        let mut command = private.command();
        command
            .args(["update-index", "-z", "--index-info"])
            .env("GIT_INDEX_FILE", &index);
        checked(
            private
                .execute(command, Some(&entries), 65536, Duration::from_secs(30))
                .await?,
        )?;
        let mut command = private.command();
        command.arg("write-tree").env("GIT_INDEX_FILE", &index);
        let tree = String::from_utf8(checked(
            private
                .execute(command, None, 1024, Duration::from_secs(30))
                .await?,
        )?)?
        .trim()
        .to_owned();
        snapshot.files = stamps;
        snapshot.verify(self).await?;
        let base = snapshot.head.clone().unwrap_or(empty);
        Ok(CapturedChanges {
            snapshot: ReviewSnapshot::Worktree { snapshot },
            evidence: EvidenceTree {
                repo: private,
                base,
                tree,
            },
        })
    }
}
impl WorktreeSnapshot {
    pub async fn verify(&self, repo: &Repository) -> Result<()> {
        ensure!(
            repo.head().await? == self.head && repo.head_ref().await? == self.head_ref,
            "HEAD or branch changed since review"
        );
        ensure!(
            index_digest(repo).await? == self.index_digest,
            "Staging changed since review"
        );
        self.verify_files(repo).await
    }
    async fn verify_files(&self, repo: &Repository) -> Result<()> {
        let cancellation = ReadCancellation::default();
        for expected in &self.files {
            let actual = if expected.mode == 0o160000 {
                let sub = Repository::open(repo.root().join(expected.path.to_path_buf())).await?;
                FileStamp {
                    path: expected.path.clone(),
                    mode: expected.mode,
                    digest: sub.head().await?,
                }
            } else {
                let root = repo.root().to_path_buf();
                let path = expected.path.clone();
                let cancelled = cancellation.0.clone();
                tokio::task::spawn_blocking(move || stamp_file(&root, &path, None, &cancelled))
                    .await??
            };
            ensure!(
                &actual == expected,
                "A selected working file changed since review: {}",
                expected.path
            );
        }
        Ok(())
    }
}
async fn index_digest(repo: &Repository) -> Result<Option<String>> {
    match tokio::fs::read(repo.git_path("index").await?).await {
        Ok(bytes) => Ok(Some(digest(&bytes))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn stamp_file(
    root: &Path,
    path: &RepoPath,
    destination: Option<&Path>,
    cancelled: &AtomicBool,
) -> Result<FileStamp> {
    ensure!(
        !cancelled.load(Ordering::Acquire),
        "Working-tree read cancelled"
    );
    use crate::worktree_file::{self, Entry};
    use sha2::{Digest, Sha256};
    use std::{io::Read, os::unix::fs::PermissionsExt};
    let entry = worktree_file::open(root, path)?;
    let mut output = destination
        .map(|path| {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)
        })
        .transpose()?;
    let (mode, hash) = match entry {
        Entry::Missing => {
            return Ok(FileStamp {
                path: path.clone(),
                mode: 0,
                digest: None,
            });
        }
        Entry::Link(target) => {
            if let Some(output) = &mut output {
                output.write_all(&target)?;
            }
            (0o120000, digest(&target))
        }
        Entry::File(mut file) => {
            let before = file.metadata()?;
            let mut hasher = Sha256::new();
            let mut buffer = [0; 65536];
            loop {
                ensure!(
                    !cancelled.load(Ordering::Acquire),
                    "Working-tree read cancelled"
                );
                let size = file.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                hasher.update(&buffer[..size]);
                if let Some(output) = &mut output {
                    output.write_all(&buffer[..size])?;
                }
            }
            let after = file.metadata()?;
            ensure!(
                before.len() == after.len() && before.modified()? == after.modified()?,
                "File changed during capture"
            );
            (
                if before.permissions().mode() & 0o111 == 0 {
                    0o100644
                } else {
                    0o100755
                },
                format!("{:x}", hasher.finalize()),
            )
        }
    };
    Ok(FileStamp {
        path: path.clone(),
        mode,
        digest: Some(hash),
    })
}
#[cfg(not(unix))]
fn stamp_file(
    _root: &Path,
    _path: &RepoPath,
    _destination: Option<&Path>,
    _cancelled: &AtomicBool,
) -> Result<FileStamp> {
    bail!("Worktree capture requires the platform capability filesystem backend")
}
