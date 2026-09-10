use crate::{
    model::{ChangeKind, RepoPath},
    repo::{Repository, checked},
    worktree_file::{self, Entry},
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, io::Read, time::Duration};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Comparison {
    HeadToIndex,
    IndexToWorktree,
    HeadToWorktree,
    Commit { oid: String },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Preview {
    Files {
        before: Option<String>,
        after: Option<String>,
    },
    Binary,
    Patch {
        patch: String,
        limited: bool,
    },
    Unavailable {
        reason: String,
    },
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitEntry {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitFile {
    pub path: RepoPath,
    pub kind: ChangeKind,
}
enum Content {
    Missing,
    Bytes(Vec<u8>),
    Large(u64),
}
const INLINE: usize = 64 * 1024;

impl Repository {
    pub async fn preview_comparison(
        &self,
        path: &RepoPath,
        comparison: &Comparison,
    ) -> Result<Preview> {
        let (before, after) = match comparison {
            Comparison::HeadToIndex => (Some("HEAD".to_owned()), Some(String::new())),
            Comparison::IndexToWorktree => (Some(String::new()), None),
            Comparison::HeadToWorktree => (Some("HEAD".to_owned()), None),
            Comparison::Commit { oid } => {
                validate_oid(oid)?;
                (Some(format!("{oid}^")), Some(oid.clone()))
            }
        };
        let read = |version: Option<String>| async move {
            if let Some(version) = version {
                self.blob_content(&version, path).await
            } else {
                let root = self.root().to_path_buf();
                let path = path.clone();
                tokio::task::spawn_blocking(move || match worktree_file::open(&root, &path)? {
                    Entry::Missing => Ok(Content::Missing),
                    Entry::Link(bytes) => Ok(Content::Bytes(bytes)),
                    Entry::File(file) => {
                        let size = file.metadata()?.len();
                        if size > INLINE as u64 {
                            return Ok(Content::Large(size));
                        }
                        let mut bytes = Vec::new();
                        file.take(INLINE as u64 + 1).read_to_end(&mut bytes)?;
                        Ok(if bytes.len() > INLINE {
                            Content::Large(bytes.len() as u64)
                        } else {
                            Content::Bytes(bytes)
                        })
                    }
                })
                .await?
            }
        };
        let (old, new) = tokio::try_join!(read(before), read(after))?;
        if [&old, &new]
            .iter()
            .any(|content| matches!(content, Content::Large(size) if *size > 32 * 1024 * 1024))
        {
            return Ok(Preview::Unavailable { reason: "The file exceeds the interactive source budget. Staging and analysis still use complete content.".into() });
        }
        if [&old, &new].iter().any(|content| matches!(content, Content::Bytes(bytes) if bytes.contains(&0) || std::str::from_utf8(bytes).is_err())) { return Ok(Preview::Binary); }
        let small = [&old, &new].iter().all(|content| match content {
            Content::Missing => true,
            Content::Bytes(bytes) => bytes.iter().filter(|&&byte| byte == b'\n').count() <= 2000,
            Content::Large(_) => false,
        });
        if small {
            let text = |content| -> Result<Option<String>> {
                match content {
                    Content::Missing => Ok(None),
                    Content::Bytes(bytes) => Ok(Some(String::from_utf8(bytes)?)),
                    Content::Large(_) => bail!("Large content cannot be rendered inline"),
                }
            };
            return Ok(Preview::Files {
                before: text(old)?,
                after: text(new)?,
            });
        }
        let mut command = self.command();
        let flags = [
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--unified=3",
        ];
        match comparison {
            Comparison::Commit { oid } => {
                command
                    .args(["show", "--format=", "--root"])
                    .args(flags)
                    .arg(oid)
                    .arg("--")
                    .arg(path.to_path_buf());
            }
            Comparison::HeadToIndex => {
                command
                    .args(["diff", "--cached"])
                    .args(flags)
                    .arg("--")
                    .arg(path.to_path_buf());
            }
            Comparison::IndexToWorktree | Comparison::HeadToWorktree => {
                command.arg("diff").args(flags);
                if matches!(old, Content::Missing) {
                    command
                        .args(["--no-index", "--", "/dev/null"])
                        .arg(path.to_path_buf());
                } else {
                    if matches!(comparison, Comparison::HeadToWorktree) {
                        command.arg("HEAD");
                    }
                    command.arg("--").arg(path.to_path_buf());
                }
            }
        }
        let output = self
            .execute(command, None, 128 * 1024, Duration::from_secs(8))
            .await?;
        ensure!(
            output.truncated || output.status.success() || output.status.code() == Some(1),
            "Git preview failed"
        );
        let text = String::from_utf8_lossy(&output.stdout);
        let mut end = text.len();
        let mut lines = 0;
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                lines += 1;
                if lines == 1000 {
                    end = index + 1;
                    break;
                }
            }
        }
        Ok(Preview::Patch {
            patch: text[..end].to_owned(),
            limited: output.truncated || end < text.len(),
        })
    }
    async fn blob_content(&self, revision: &str, path: &RepoPath) -> Result<Content> {
        let mut object = OsString::from(revision);
        object.push(":");
        object.push(path.to_path_buf());
        let mut command = self.command();
        command.args(["cat-file", "-s"]).arg(&object);
        let output = self
            .execute(command, None, 128, Duration::from_secs(5))
            .await?;
        if output.status.code() == Some(128) {
            return Ok(Content::Missing);
        }
        let size: u64 = String::from_utf8(checked(output)?)?.trim().parse()?;
        if size > INLINE as u64 {
            return Ok(Content::Large(size));
        }
        let mut command = self.command();
        command.arg("show").arg(object);
        let output = self
            .execute(command, None, INLINE, Duration::from_secs(5))
            .await?;
        if output.truncated {
            return Ok(Content::Large(INLINE as u64 + 1));
        }
        Ok(Content::Bytes(checked(output)?))
    }
    pub async fn workbench_log(&self, limit: usize) -> Result<Vec<CommitEntry>> {
        if self.head().await?.is_none() {
            return Ok(Vec::new());
        }
        let bytes = self
            .git(&[
                "log",
                &format!("--max-count={}", limit.clamp(1, 200)),
                "--format=%H%x00%h%x00%an%x00%aI%x00%s%x00",
            ])
            .await?;
        let fields: Vec<_> = bytes.split(|&b| b == 0).collect();
        let mut entries = Vec::new();
        for row in fields.chunks(5) {
            if row.len() < 5 {
                break;
            }
            let text = |index| String::from_utf8_lossy(row[index]).trim().to_owned();
            entries.push(CommitEntry {
                oid: text(0),
                short_oid: text(1),
                author: text(2),
                date: text(3),
                subject: text(4),
            });
        }
        Ok(entries)
    }
    pub async fn commit_files(&self, oid: &str) -> Result<Vec<CommitFile>> {
        validate_oid(oid)?;
        let bytes = self
            .git(&[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-status",
                "--no-renames",
                "-r",
                "-z",
                oid,
            ])
            .await?;
        let fields: Vec<_> = bytes.split(|&b| b == 0).collect();
        let mut files = Vec::new();
        for pair in fields.chunks(2) {
            if pair.len() < 2 || pair[0].is_empty() {
                break;
            }
            files.push(CommitFile {
                path: RepoPath::new(pair[1].to_vec())?,
                kind: match pair[0] {
                    b"A" => ChangeKind::Added,
                    b"D" => ChangeKind::Deleted,
                    b"T" => ChangeKind::TypeChanged,
                    _ => ChangeKind::Modified,
                },
            });
        }
        Ok(files)
    }
    pub async fn list_files(&self) -> Result<Vec<RepoPath>> {
        let (tracked, untracked) = tokio::try_join!(
            self.git(&["ls-files", "-z"]),
            self.git(&["ls-files", "--others", "--exclude-standard", "-z"])
        )?;
        tracked
            .split(|&b| b == 0)
            .chain(untracked.split(|&b| b == 0))
            .filter(|path| !path.is_empty())
            .map(|path| RepoPath::new(path.to_vec()))
            .collect()
    }
    pub async fn search(
        &self,
        term: &str,
        case_sensitive: bool,
        whole_word: bool,
        regex: bool,
    ) -> Result<Vec<String>> {
        ensure!(term.len() <= 4096, "Search query exceeds the input budget");
        let mut command = self.command();
        command.args(["grep", "--no-color", "-n", "-I", "--untracked"]);
        if !case_sensitive {
            command.arg("-i");
        }
        if whole_word {
            command.arg("-w");
        }
        command
            .arg(if regex { "-E" } else { "-F" })
            .arg("-e")
            .arg(term)
            .arg("--");
        let output = self
            .execute(command, None, 16 * 1024 * 1024, Duration::from_secs(15))
            .await?;
        if output.status.code() == Some(1) {
            return Ok(Vec::new());
        }
        Ok(String::from_utf8_lossy(&checked(output)?)
            .lines()
            .map(str::to_owned)
            .collect())
    }
    pub async fn operation(&self) -> Result<Option<String>> {
        let directory = self.git_path("").await?;
        for (path, label) in [
            ("MERGE_HEAD", "merge"),
            ("CHERRY_PICK_HEAD", "cherry-pick"),
            ("REVERT_HEAD", "revert"),
            ("rebase-merge", "rebase"),
            ("rebase-apply", "rebase"),
        ] {
            if tokio::fs::try_exists(directory.join(path)).await? {
                return Ok(Some(label.into()));
            }
        }
        Ok(None)
    }
    pub async fn publish(&self, expected_branch: &str) -> Result<()> {
        let _lock = crate::storage::FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        let status = self.status().await?;
        ensure!(
            status.branch == expected_branch
                && status.branch != "(detached)"
                && status.head.is_some(),
            "The branch changed before pushing"
        );
        let branch = format!("refs/heads/{}", status.branch);
        let upstream = self
            .git(&[
                "for-each-ref",
                "--format=%(upstream:remotename)%00%(upstream:remoteref)",
                &branch,
            ])
            .await?;
        let fields: Vec<_> = upstream.split(|&byte| byte == 0).collect();
        let remote = String::from_utf8_lossy(fields.first().copied().unwrap_or_default())
            .trim()
            .to_owned();
        let destination = String::from_utf8_lossy(fields.get(1).copied().unwrap_or_default())
            .trim()
            .to_owned();
        let mut command = self.command();
        command.args(["push", "--porcelain", "--no-follow-tags"]);
        if remote.is_empty() {
            command
                .args(["--set-upstream", "--", "origin"])
                .arg(format!("{branch}:{branch}"));
        } else {
            ensure!(
                destination.starts_with("refs/heads/"),
                "Invalid upstream branch"
            );
            command
                .arg("--")
                .arg(remote)
                .arg(format!("{branch}:{destination}"));
        }
        self.mutate(command, None, Duration::from_secs(120)).await
    }
    pub async fn commit_message(&self, message: &str, amend: bool) -> Result<String> {
        let message = crate::commit::validate_message(message)?;
        if !amend {
            let status = self.status().await?;
            if !status.files.iter().any(|file| file.staged.is_some()) {
                self.stage(
                    &status
                        .files
                        .iter()
                        .map(|file| file.path.clone())
                        .collect::<Vec<_>>(),
                )
                .await?;
            }
            return self
                .commit_snapshot(&self.staged_snapshot().await?, &message)
                .await;
        }
        let _lock = crate::storage::FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        let before = self.head().await?;
        let branch = self.head_ref().await?;
        let mut command = self.command();
        command.args(["commit", "--amend", "--file=-"]);
        let result = self
            .mutate(command, Some(message.as_bytes()), Duration::from_secs(120))
            .await;
        let after = self
            .head()
            .await
            .map_err(|_| crate::process::MutationCompletionUnknown)?;
        let same_branch = self
            .head_ref()
            .await
            .map_err(|_| crate::process::MutationCompletionUnknown)?
            == branch;
        if result.is_ok()
            && same_branch
            && let Some(oid) = after
        {
            return Ok(oid);
        }
        if after != before || !same_branch {
            return Err(crate::process::MutationCompletionUnknown.into());
        }
        result?;
        Err(crate::process::MutationCompletionUnknown.into())
    }
}
fn validate_oid(oid: &str) -> Result<()> {
    ensure!(
        (7..=64).contains(&oid.len()) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "Invalid commit reference"
    );
    Ok(())
}
