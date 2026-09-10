use crate::{model::ChangeKind, repo::Repository, storage::FileLock};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    Fetch,
    Pull,
    Push,
}

impl SyncAction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fetch => "Fetch",
            Self::Pull => "Pull",
            Self::Push => "Push",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: String,
    pub oid: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub oid: String,
    pub short_oid: String,
    pub author: String,
    pub age: String,
    pub subject: String,
    pub refs: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SyncTarget {
    pub head: Option<String>,
    pub branch: String,
}

impl From<&crate::model::RepoStatus> for SyncTarget {
    fn from(status: &crate::model::RepoStatus) -> Self {
        Self {
            head: status.head.clone(),
            branch: status.branch.clone(),
        }
    }
}

impl Repository {
    pub async fn sync(&self, action: SyncAction, expected: Option<&SyncTarget>) -> Result<()> {
        let _lock = FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        let status = self.status().await?;
        if action != SyncAction::Fetch && expected != Some(&SyncTarget::from(&status)) {
            bail!(
                "HEAD or branch changed since you chose this action. Refresh and review the branch again."
            );
        }
        let mut command = self.command();
        match action {
            SyncAction::Fetch => {
                command.args(["fetch", "--all", "--no-recurse-submodules"]);
            }
            SyncAction::Pull => {
                self.require_idle().await?;
                if status.upstream.is_none() {
                    bail!("This branch has no upstream. Set one with Git before pulling.");
                }
                if status.files.iter().any(|f| {
                    f.staged.is_some() || f.worktree.is_some_and(|k| k != ChangeKind::Untracked)
                }) {
                    bail!(
                        "Commit or stash tracked changes before pulling. Kiri will not create a hidden stash or discard edits."
                    );
                }
                if status.ahead > 0 && status.behind > 0 {
                    bail!(
                        "Branches have diverged. Choose a merge or rebase in Git; Kiri will not rewrite history automatically."
                    );
                }
                command.args([
                    "pull",
                    "--ff-only",
                    "--no-rebase",
                    "--no-autostash",
                    "--no-recurse-submodules",
                ]);
            }
            SyncAction::Push => {
                let head = status.head.context("Create a commit before pushing")?;
                if status.branch == "(detached)" {
                    bail!("Create or switch to a branch before pushing");
                }
                let branch_ref = format!("refs/heads/{}", status.branch);
                let upstream = self
                    .git(&[
                        "for-each-ref",
                        "--format=%(upstream:remotename)%00%(upstream:remoteref)",
                        &branch_ref,
                    ])
                    .await?;
                let mut fields = upstream.splitn(2, |b| *b == 0);
                let remote =
                    std::str::from_utf8(fields.next().context("Missing upstream remote")?)?.trim();
                let destination =
                    std::str::from_utf8(fields.next().context("Missing upstream branch")?)?.trim();
                if remote.is_empty() || !destination.starts_with("refs/heads/") {
                    bail!(
                        "This branch has no upstream. Publish it with `git push -u <remote> <branch>` first."
                    );
                }
                command.args([
                    "push",
                    "--porcelain",
                    "--no-follow-tags",
                    "--",
                    remote,
                    &format!("{head}:{destination}"),
                ]);
            }
        }
        self.mutate(command, None, Duration::from_secs(120)).await
            .with_context(|| format!("{} did not finish successfully. Check Git status before retrying. For authentication, sign in with your usual Git credential helper or SSH setup.", action.label()))?;
        Ok(())
    }

    pub async fn branches(&self) -> Result<Vec<Branch>> {
        let bytes = self
            .git(&[
                "for-each-ref",
                "--sort=-committerdate",
                "--format=%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(objectname:short)%00",
                "refs/heads/",
            ])
            .await?;
        let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
        let mut branches = Vec::new();
        for row in fields.chunks_exact(4) {
            branches.push(Branch {
                name: std::str::from_utf8(row[0])?
                    .trim_start_matches('\n')
                    .to_owned(),
                current: row[1] == b"*",
                upstream: String::from_utf8_lossy(row[2]).into_owned(),
                oid: String::from_utf8_lossy(row[3]).into_owned(),
            });
        }
        Ok(branches)
    }

    pub async fn switch_branch(&self, name: &str) -> Result<()> {
        let _lock = FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        self.require_idle().await?;
        if !self
            .branches()
            .await?
            .iter()
            .any(|branch| branch.name == name)
        {
            bail!("Select an existing local branch");
        }
        if self
            .status()
            .await?
            .files
            .iter()
            .any(|f| f.staged.is_some() || f.worktree.is_some_and(|k| k != ChangeKind::Untracked))
        {
            bail!(
                "Commit or stash tracked changes before switching branches. Working files were not changed."
            );
        }
        let mut command = self.command();
        command.args(["switch", "--no-guess", "--", name]);
        self.mutate(command, None, Duration::from_secs(30)).await
    }

    pub async fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        if self.head().await?.is_none() {
            return Ok(Vec::new());
        }
        let raw = self
            .git(&[
                "log",
                &format!("-{}", limit.clamp(1, 500)),
                "-z",
                "--date-order",
                "--format=%H%x00%h%x00%an%x00%ar%x00%s%x00%D",
                "HEAD",
                "--",
            ])
            .await?;
        let fields: Vec<_> = raw.split(|b| *b == 0).collect();
        Ok(fields
            .chunks_exact(6)
            .map(|row| HistoryEntry {
                oid: String::from_utf8_lossy(row[0]).into_owned(),
                short_oid: String::from_utf8_lossy(row[1]).into_owned(),
                author: String::from_utf8_lossy(row[2]).into_owned(),
                age: String::from_utf8_lossy(row[3]).into_owned(),
                subject: String::from_utf8_lossy(row[4]).into_owned(),
                refs: String::from_utf8_lossy(row[5]).into_owned(),
            })
            .collect())
    }

    async fn require_idle(&self) -> Result<()> {
        for state in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
        ] {
            if tokio::fs::try_exists(self.git_path(state).await?).await? {
                bail!(
                    "Finish the current merge, rebase, or cherry-pick before changing branches or pulling."
                );
            }
        }
        Ok(())
    }
}
