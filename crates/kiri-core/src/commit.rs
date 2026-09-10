use crate::{
    model::digest,
    repo::{Repository, checked},
    storage::FileLock,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{io::Write, time::Duration};
use tempfile::NamedTempFile;

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedSnapshot {
    pub head: Option<String>,
    pub head_ref: Option<String>,
    pub tree: String,
    pub index_digest: String,
}

impl Repository {
    async fn frozen_index(&self) -> Result<(NamedTempFile, String)> {
        let index_path = self.git_path("index").await?;
        let metadata = tokio::fs::metadata(&index_path)
            .await
            .context("Stage some changes before drafting a commit")?;
        if metadata.len() > 64 * 1024 * 1024 {
            bail!("Index exceeds the supported 64 MiB limit");
        }
        let bytes = tokio::fs::read(&index_path).await?;
        let mut frozen =
            NamedTempFile::new_in(index_path.parent().context("Index has no parent")?)?;
        frozen.write_all(&bytes)?;
        Ok((frozen, digest(&bytes)))
    }

    pub async fn staged_snapshot(&self) -> Result<StagedSnapshot> {
        let (head, head_ref) = tokio::try_join!(self.head(), self.head_ref())?;
        let (index, index_digest) = self.frozen_index().await?;
        let mut command = self.command();
        command
            .args(["write-tree"])
            .env("GIT_INDEX_FILE", index.path());
        let tree = String::from_utf8(checked(
            self.execute(command, None, 1024, Duration::from_secs(10))
                .await?,
        )?)?
        .trim()
        .to_owned();
        let mut command = self.command();
        command
            .args([
                "diff",
                "--cached",
                "--quiet",
                "--no-ext-diff",
                "--no-textconv",
            ])
            .env("GIT_INDEX_FILE", index.path());
        let output = self
            .execute(command, None, 1024, Duration::from_secs(15))
            .await?;
        if output.status.success() {
            bail!("Nothing is staged. Stage files or hunks first.");
        }
        if output.status.code() != Some(1) {
            checked(output)?;
        }
        let snapshot = StagedSnapshot {
            head,
            head_ref,
            tree,
            index_digest,
        };
        self.verify_snapshot(&snapshot).await?;
        Ok(snapshot)
    }

    pub async fn verify_snapshot(&self, snapshot: &StagedSnapshot) -> Result<()> {
        let object_id =
            |s: &str| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit());
        if !object_id(&snapshot.tree)
            || snapshot.head.as_ref().is_some_and(|s| !object_id(s))
            || snapshot.index_digest.len() != 64
            || !snapshot.index_digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            bail!("Invalid staged snapshot identifiers");
        }
        let (head, head_ref) = tokio::try_join!(self.head(), self.head_ref())?;
        if head != snapshot.head || head_ref != snapshot.head_ref {
            bail!("HEAD or branch changed since this draft. Generate a fresh draft.");
        }
        let bytes = tokio::fs::read(self.git_path("index").await?).await?;
        if digest(&bytes) != snapshot.index_digest {
            bail!(
                "Staged changes changed since this draft. Review the index and generate a fresh draft."
            );
        }
        Ok(())
    }

    pub async fn commit_snapshot(
        &self,
        snapshot: &StagedSnapshot,
        message: &str,
    ) -> Result<String> {
        self.commit_selection(snapshot, message, None).await
    }

    pub async fn commit_selection(
        &self,
        snapshot: &StagedSnapshot,
        message: &str,
        paths: Option<&[crate::model::RepoPath]>,
    ) -> Result<String> {
        let message = validate_message(message)?;
        let _lock = FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        for state in [
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-merge",
            "rebase-apply",
        ] {
            if tokio::fs::try_exists(self.git_path(state).await?).await? {
                bail!(
                    "Finish the current Git merge, rebase, or cherry-pick with Git before creating a Kiri commit."
                );
            }
        }
        self.verify_snapshot(snapshot).await?;
        let (index, fingerprint) = self.frozen_index().await?;
        if fingerprint != snapshot.index_digest {
            bail!("The index changed while preparing the commit. Nothing was committed.");
        }
        let mut command = self.command();
        command
            .args(["write-tree"])
            .env("GIT_INDEX_FILE", index.path());
        let tree = String::from_utf8(checked(
            self.execute(command, None, 1024, Duration::from_secs(10))
                .await?,
        )?)?;
        if tree.trim() != snapshot.tree {
            bail!("The draft does not match the staged tree. Generate a fresh draft.");
        }
        if let Some(paths) = paths {
            if paths.is_empty() {
                bail!("A commit group must contain at least one file");
            }
            let mut command = self.command();
            command
                .arg("read-tree")
                .arg(snapshot.head.as_deref().unwrap_or("--empty"))
                .env("GIT_INDEX_FILE", index.path());
            checked(
                self.execute(command, None, 1024, Duration::from_secs(10))
                    .await?,
            )?;
            let mut command = self.command();
            command
                .args([
                    "restore",
                    "--staged",
                    "--source",
                    &snapshot.tree,
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ])
                .env("GIT_INDEX_FILE", index.path());
            checked(
                self.execute(
                    command,
                    Some(&crate::repo::pathspec_input(paths)),
                    65536,
                    Duration::from_secs(15),
                )
                .await?,
            )?;
        }
        self.verify_snapshot(snapshot).await?;
        let mut tree_command = self.command();
        tree_command
            .arg("write-tree")
            .env("GIT_INDEX_FILE", index.path());
        let expected_tree = String::from_utf8(checked(
            self.execute(tree_command, None, 1024, Duration::from_secs(10))
                .await?,
        )?)?
        .trim()
        .to_owned();
        let mut command = self.command();
        command
            .args(["commit", "--file=-"])
            .env("GIT_INDEX_FILE", index.path());
        let result = self
            .mutate(command, Some(message.as_bytes()), Duration::from_secs(120))
            .await;
        self.observe_commit(snapshot, &expected_tree, &message, result)
            .await
    }

    async fn observe_commit(
        &self,
        snapshot: &StagedSnapshot,
        expected_tree: &str,
        message: &str,
        result: Result<()>,
    ) -> Result<String> {
        let after = self
            .head()
            .await
            .map_err(|_| crate::process::MutationCompletionUnknown)?;
        let branch = self
            .head_ref()
            .await
            .map_err(|_| crate::process::MutationCompletionUnknown)?;
        if let Some(oid) = &after
            && after != snapshot.head
            && branch == snapshot.head_ref
        {
            let metadata = self
                .git(&["show", "-s", "--format=%P%x00%T%x00%B", oid])
                .await
                .map_err(|_| crate::process::MutationCompletionUnknown)?;
            let mut fields = metadata.splitn(3, |byte| *byte == 0);
            let parents = fields.next().unwrap_or_default();
            let tree = fields.next().unwrap_or_default();
            let body = fields.next().unwrap_or_default();
            let expected_parent = snapshot.head.as_deref().unwrap_or("").as_bytes();
            if parents == expected_parent
                && (result.is_ok()
                    || tree == expected_tree.as_bytes()
                        && String::from_utf8_lossy(body).trim() == message)
            {
                return Ok(oid.clone());
            }
        }
        if after != snapshot.head
            || result.is_ok()
            || result.as_ref().is_err_and(|error| {
                error
                    .downcast_ref::<crate::process::MutationCompletionUnknown>()
                    .is_some()
            })
        {
            return Err(
                anyhow::Error::new(crate::process::MutationCompletionUnknown).context(format!(
                    "Commit outcome needs review. Observed HEAD: {}. Do not retry automatically.",
                    after.as_deref().unwrap_or("unborn")
                )),
            );
        }
        result.context(
            "Git rejected the commit; no new HEAD was observed. Hooks may have changed files.",
        )?;
        Err(crate::process::MutationCompletionUnknown.into())
    }
}

pub fn validate_message(message: &str) -> Result<String> {
    let message = message.trim();
    if message.is_empty()
        || message.len() > 16384
        || message
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        bail!("Use a non-empty commit message without control characters, at most 16 KiB");
    }
    Ok(message.to_owned())
}
