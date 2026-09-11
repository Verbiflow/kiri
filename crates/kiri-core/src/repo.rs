use crate::{
    diff::{DiffDocument, LARGE_FILE_BYTES, PREVIEW_BYTES},
    model::{ChangeKind, DiffSide, FileChange, RepoPath, RepoStatus, WorktreeStamp, terminal_text},
    process::{self, Output},
    status::parse_status,
};
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct Repository {
    root: PathBuf,
    objects: Option<std::sync::Arc<crate::review::ObjectStore>>,
    /// The repository opted into Git's builtin file system monitor. Hook-style monitors stay
    /// disabled because Kiri never runs commands named by repository configuration.
    fsmonitor: bool,
    /// In-process reader, present when gitoxide could open the working tree. Status runs here
    /// first; Git computes anything the reader declines.
    native: Option<crate::native::Native>,
}

/// Which implementation answers status reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusBackend {
    /// gitoxide in-process, falling back to Git for unsupported repository states.
    Native,
    /// `git status` subprocess only.
    Git,
}

impl Repository {
    /// A handle for `path` that has not been verified as a working tree. Git resolves the
    /// repository itself, so status reads work before `open` returns; they run with the
    /// conservative monitor setting because configuration has not been inspected yet.
    pub fn at(path: impl AsRef<Path>) -> Self {
        Self {
            root: path.as_ref().to_path_buf(),
            objects: None,
            fsmonitor: false,
            native: None,
        }
    }

    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(repo) = Self::open_native(path.clone()).await? {
            return Ok(repo);
        }
        let candidate = Self::at(&path);
        let (bytes, fsmonitor) = tokio::join!(
            candidate.git(&["rev-parse", "--show-toplevel"]),
            candidate.builtin_fsmonitor()
        );
        let root = path_from_git(
            &bytes
                .context("Open a Git working tree, not a bare repository or an ordinary folder")?,
        )?;
        Ok(Self {
            root: tokio::fs::canonicalize(root).await?,
            objects: None,
            fsmonitor,
            native: None,
        })
    }

    pub async fn discover(path: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = path.as_ref().to_path_buf();
        if native_enabled() {
            let probe = path.clone();
            match tokio::task::spawn_blocking(move || crate::native::Native::discover(&probe))
                .await?
            {
                Ok(crate::native::Discovery::Repository(found)) => {
                    return Ok(Some(Self::from_native(*found).await?));
                }
                Ok(crate::native::Discovery::NotRepository) => return Ok(None),
                Err(_) => {}
            }
        }
        let candidate = Self::at(&path);
        let mut command = candidate.command();
        command.args(["rev-parse", "--show-toplevel"]);
        let (output, fsmonitor) = tokio::join!(
            candidate.execute(command, None, 65536, Duration::from_secs(15)),
            candidate.builtin_fsmonitor()
        );
        let output = output?;
        if output.status.code() == Some(128)
            && (output.stderr.starts_with(b"fatal: not a git repository")
                || output
                    .stderr
                    .starts_with(b"fatal: this operation must be run in a work tree"))
        {
            return Ok(None);
        }
        let root = path_from_git(&checked(output)?)?;
        Ok(Some(Self {
            root: tokio::fs::canonicalize(root).await?,
            objects: None,
            fsmonitor,
            native: None,
        }))
    }

    /// Open through gitoxide without any subprocess. `None` means gitoxide could not open the
    /// path and Git should decide whether it is a repository at all.
    async fn open_native(path: PathBuf) -> Result<Option<Self>> {
        if !native_enabled() {
            return Ok(None);
        }
        match tokio::task::spawn_blocking(move || crate::native::Native::discover(&path)).await? {
            Ok(crate::native::Discovery::Repository(found)) => {
                Ok(Some(Self::from_native(*found).await?))
            }
            Ok(crate::native::Discovery::NotRepository) | Err(_) => Ok(None),
        }
    }

    async fn from_native(found: crate::native::Discovered) -> Result<Self> {
        Ok(Self {
            root: tokio::fs::canonicalize(&found.root).await?,
            objects: None,
            fsmonitor: found.builtin_fsmonitor,
            native: Some(found.native),
        })
    }

    /// Force status reads through one backend. Tests use this to compare implementations.
    pub fn with_status_backend(mut self, backend: StatusBackend) -> Self {
        if backend == StatusBackend::Git {
            self.native = None;
        }
        self
    }

    pub fn status_backend(&self) -> StatusBackend {
        if self.native.is_some() {
            StatusBackend::Native
        } else {
            StatusBackend::Git
        }
    }

    /// The in-process status, or `None` when this repository state is left to Git.
    pub async fn native_status(&self, untracked: bool) -> Result<Option<RepoStatus>> {
        let Some(native) = self.native.clone() else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || native.status(untracked)).await?
    }

    /// True only when `core.fsmonitor` is the boolean `true`, which selects Git's builtin
    /// daemon. Unset values, `false`, hook paths, and unreadable configuration all read as false.
    async fn builtin_fsmonitor(&self) -> bool {
        let mut command = self.base_command();
        command.args(["config", "--type=bool", "core.fsmonitor"]);
        match self
            .execute(command, None, 64, Duration::from_secs(5))
            .await
        {
            Ok(output) => builtin_fsmonitor_setting(output.status.code(), &output.stdout),
            Err(_) => false,
        }
    }

    pub fn uses_builtin_fsmonitor(&self) -> bool {
        self.fsmonitor
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn with_objects(&self, objects: std::sync::Arc<crate::review::ObjectStore>) -> Self {
        Self {
            root: self.root.clone(),
            objects: Some(objects),
            fsmonitor: self.fsmonitor,
            native: None,
        }
    }

    pub fn command(&self) -> Command {
        let mut command = self.base_command();
        if !self.fsmonitor {
            command.args(["-c", "core.fsmonitor=false"]);
        }
        command.args(["-c", "color.ui=false"]);
        command
    }

    fn base_command(&self) -> Command {
        let mut command = Command::new("git");
        command
            .arg("--no-pager")
            .arg("--no-optional-locks")
            .arg("-C")
            .arg(&self.root)
            .env("GIT_LITERAL_PATHSPECS", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_EXTERNAL_DIFF",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        ] {
            command.env_remove(key);
        }
        if let Some(store) = &self.objects {
            command
                .env("GIT_OBJECT_DIRECTORY", &store.objects)
                .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &store.alternates);
        }
        command
    }

    pub(crate) async fn execute(
        &self,
        command: Command,
        input: Option<&[u8]>,
        limit: usize,
        timeout: Duration,
    ) -> Result<Output> {
        process::run(command, input, limit, timeout).await
    }

    pub(crate) async fn mutate(
        &self,
        command: Command,
        input: Option<&[u8]>,
        timeout: Duration,
    ) -> Result<()> {
        let output = process::run_mutation(command, input, timeout).await?;
        if !output.status.success() {
            let diagnostic = if output.stderr_excerpt.is_empty() {
                &output.stdout_excerpt
            } else {
                &output.stderr_excerpt
            };
            bail!(
                "Git command failed ({}): {}{}",
                output.status,
                crate::model::terminal_message(String::from_utf8_lossy(diagnostic).trim()),
                if output.logs_abbreviated {
                    " (log excerpt)"
                } else {
                    ""
                }
            );
        }
        Ok(())
    }

    pub async fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        let mut command = self.command();
        command.args(args);
        checked(
            self.execute(command, None, 16 * 1024 * 1024, Duration::from_secs(15))
                .await?,
        )
    }

    pub async fn git_path(&self, name: &str) -> Result<PathBuf> {
        let output = self
            .git(&["rev-parse", "--path-format=absolute", "--git-path", name])
            .await?;
        path_from_git(&output)
    }

    pub async fn status(&self) -> Result<RepoStatus> {
        self.read_status("--untracked-files=all").await
    }

    pub async fn tracked_status(&self) -> Result<RepoStatus> {
        self.read_status("--untracked-files=no").await
    }

    async fn read_status(&self, untracked: &str) -> Result<RepoStatus> {
        let status = match self
            .native_status(untracked == "--untracked-files=all")
            .await
        {
            Ok(Some(status)) => status,
            Ok(None) | Err(_) => {
                let raw = self
                    .git(&[
                        "-c",
                        "status.renames=false",
                        "status",
                        "--porcelain=v2",
                        "-z",
                        "--branch",
                        untracked,
                        "--ignore-submodules=dirty",
                    ])
                    .await?;
                parse_status(&raw)?
            }
        };
        if status.files.len() <= 128
            && status
                .files
                .iter()
                .any(|file| file.staged == Some(ChangeKind::Added))
            && status
                .files
                .iter()
                .any(|file| file.staged == Some(ChangeKind::Deleted))
        {
            let mut command = self.command();
            command.args([
                "-c",
                "status.renames=true",
                "status",
                "--porcelain=v2",
                "-z",
                "--branch",
                untracked,
                "--ignore-submodules=dirty",
            ]);
            if let Ok(output) = self
                .execute(command, None, 16 * 1024 * 1024, Duration::from_secs(1))
                .await
                && output.status.success()
                && !output.truncated
            {
                return parse_status(&output.stdout);
            }
        }
        Ok(status)
    }

    /// One `lstat` of the working file, or `None` when it is absent. Used as part of preview
    /// cache identity so unchanged files never re-run Git and edited files never serve stale text.
    pub async fn worktree_stamp(&self, path: &RepoPath) -> Result<Option<WorktreeStamp>> {
        match tokio::fs::symlink_metadata(self.root.join(path.to_path_buf())).await {
            Ok(metadata) => Ok(Some(WorktreeStamp::from_metadata(&metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn untracked_files(&self) -> Result<Vec<FileChange>> {
        let raw = self
            .git(&["ls-files", "--others", "--exclude-standard", "-z"])
            .await?;
        raw.split(|b| *b == 0)
            .filter(|path| !path.is_empty())
            .map(|path| {
                Ok(FileChange {
                    path: RepoPath::new(path.to_vec())?,
                    original_path: None,
                    staged: None,
                    worktree: Some(ChangeKind::Untracked),
                    submodule: false,
                    head_oid: None,
                    index_oid: None,
                })
            })
            .collect()
    }

    pub async fn diff(
        &self,
        file: &FileChange,
        side: DiffSide,
        large: bool,
    ) -> Result<DiffDocument> {
        if file.conflicted() {
            return Ok(DiffDocument::notice(
                "Merge conflict. Resolve this file in your editor, then stage it.",
            ));
        }
        if file.kind(side).is_none() {
            return Ok(DiffDocument::notice("No changes on this side."));
        }
        if file.submodule {
            return Ok(DiffDocument::notice(
                "Submodule changed. Open its workspace to review the nested repository.",
            ));
        }
        if side == DiffSide::Worktree
            && let Ok(meta) =
                tokio::fs::symlink_metadata(self.root.join(file.path.to_path_buf())).await
        {
            if meta.len() > LARGE_FILE_BYTES && !large {
                return Ok(DiffDocument::notice(format!(
                    "Large file: {:.1} MiB. Press L for a bounded patch preview. Other files are ready to review.",
                    meta.len() as f64 / 1048576.0
                )));
            }
            if file.worktree == Some(ChangeKind::Untracked) && meta.file_type().is_symlink() {
                return Ok(DiffDocument::notice(
                    "New symbolic link. Stage the file to review its link target; Kiri does not follow it.",
                ));
            }
        }
        let mut command = self.command();
        command.args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--diff-algorithm=myers",
            "--no-indent-heuristic",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--unified=3",
        ]);
        let untracked = side == DiffSide::Worktree && file.worktree == Some(ChangeKind::Untracked);
        if untracked {
            command
                .args(["--no-index", "--", "/dev/null"])
                .arg(file.path.to_path_buf());
        } else {
            if side == DiffSide::Staged {
                command.arg("--cached");
            }
            command.arg("--").arg(file.path.to_path_buf());
            if let Some(original) = &file.original_path {
                command.arg(original.to_path_buf());
            }
        }
        let output = self
            .execute(
                command,
                None,
                PREVIEW_BYTES,
                Duration::from_secs(if large { 10 } else { 2 }),
            )
            .await?;
        if !(output.truncated
            || output.status.success()
            || untracked && output.status.code() == Some(1))
        {
            bail!(
                "Git diff failed: {}",
                terminal_text(&String::from_utf8_lossy(&output.stderr))
            );
        }
        Ok(DiffDocument::parse(output.stdout, output.truncated))
    }

    pub async fn stage(&self, paths: &[RepoPath]) -> Result<()> {
        if paths.is_empty() {
            bail!("Select at least one file");
        }
        let _lock = crate::storage::FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        let (tracked, untracked) = self.staging_paths(paths).await?;
        // A selection that matches nothing in the inventory is either stale
        // ignored content, which stays untouched, or a path that no longer
        // exists anywhere, which is a failed write the caller must hear about.
        // Decide before the first `add` so a mixed selection changes nothing.
        let unmatched: Vec<RepoPath> = paths
            .iter()
            .filter(|selected| {
                !tracked
                    .iter()
                    .chain(untracked.iter())
                    .any(|found| covers(selected, found))
            })
            .cloned()
            .collect();
        if !unmatched.is_empty() {
            let ignored = self.ignored_paths(&unmatched).await?;
            if let Some(missing) = unmatched
                .iter()
                .find(|selected| !ignored.iter().any(|found| covers(selected, found)))
            {
                bail!(
                    "Git: pathspec '{}' did not match any files",
                    String::from_utf8_lossy(missing.bytes())
                );
            }
        }
        for (paths, update) in [(tracked, true), (untracked, false)] {
            if paths.is_empty() {
                continue;
            }
            let mut command = self.command();
            command.arg("add");
            if update {
                command.arg("--update");
            }
            command.args(["--pathspec-from-file=-", "--pathspec-file-nul"]);
            self.mutate(
                command,
                Some(&pathspec_input(&paths)),
                Duration::from_secs(30),
            )
            .await?;
        }
        Ok(())
    }

    /// Ignored untracked files under the selected paths: content `stage` must never add.
    async fn ignored_paths(&self, paths: &[RepoPath]) -> Result<Vec<RepoPath>> {
        let mut ignored = Vec::new();
        for batch in path_batches(paths)? {
            let mut command = self.command();
            command.args([
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
            ]);
            for path in batch {
                command.arg(path.to_path_buf());
            }
            let output = checked(
                self.execute(command, None, 16 * 1024 * 1024, Duration::from_secs(15))
                    .await?,
            )?;
            for record in output
                .split(|byte| *byte == 0)
                .filter(|record| !record.is_empty())
            {
                ignored.push(RepoPath::new(record.to_vec())?);
            }
        }
        Ok(ignored)
    }

    async fn staging_paths(&self, paths: &[RepoPath]) -> Result<(Vec<RepoPath>, Vec<RepoPath>)> {
        let mut tracked = std::collections::HashSet::new();
        let mut untracked = std::collections::HashSet::new();
        for batch in path_batches(paths)? {
            let mut command = self.command();
            command.args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-t",
                "-z",
                "--",
            ]);
            for path in batch {
                command.arg(path.to_path_buf());
            }
            let output = checked(
                self.execute(command, None, 16 * 1024 * 1024, Duration::from_secs(15))
                    .await?,
            )?;
            for record in output
                .split(|byte| *byte == 0)
                .filter(|record| !record.is_empty())
            {
                if record.get(1) != Some(&b' ') {
                    bail!("Malformed staging inventory");
                }
                let path = RepoPath::new(record[2..].to_vec())?;
                match record[0] {
                    b'H' | b'S' | b'M' => {
                        tracked.insert(path);
                    }
                    b'?' => {
                        untracked.insert(path);
                    }
                    _ => bail!("Unexpected staging inventory kind"),
                }
            }
        }
        untracked.retain(|path| !tracked.contains(path));
        Ok((
            tracked.into_iter().collect(),
            untracked.into_iter().collect(),
        ))
    }

    pub async fn unstage(&self, paths: &[RepoPath]) -> Result<()> {
        if paths.is_empty() {
            bail!("Select at least one file");
        }
        let _lock = crate::storage::FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        let born = self.head().await?.is_some();
        let mut command = self.command();
        if born {
            command.args(["restore", "--staged"]);
        } else {
            command.args(["rm", "--cached", "--quiet", "--force"]);
        }
        command.args(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        self.mutate(
            command,
            Some(&pathspec_input(paths)),
            Duration::from_secs(30),
        )
        .await
    }

    pub async fn stage_hunk(
        &self,
        file: &FileChange,
        side: DiffSide,
        preview: &DiffDocument,
        hunk: usize,
    ) -> Result<()> {
        let _lock = crate::storage::FileLock::acquire(self.git_path("kiri-operation.lock").await?)?;
        let current = self.diff(file, side, false).await?;
        if current.fingerprint != preview.fingerprint {
            bail!("This file changed since the preview. Refresh and review it again.");
        }
        let patch = preview.hunk_patch(hunk)?;
        let mut command = self.command();
        command.args(["apply", "--cached", "--recount", "--whitespace=nowarn"]);
        if side == DiffSide::Staged {
            command.arg("--reverse");
        }
        command.arg("-");
        self.mutate(command, Some(&patch), Duration::from_secs(15))
            .await
    }

    pub async fn head_ref(&self) -> Result<Option<String>> {
        let mut command = self.command();
        command.args(["symbolic-ref", "--quiet", "HEAD"]);
        let output = self
            .execute(command, None, 4096, Duration::from_secs(5))
            .await?;
        if output.status.success() {
            return Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()));
        }
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        checked(output)?;
        bail!("Could not read the active branch")
    }

    pub async fn head(&self) -> Result<Option<String>> {
        let mut command = self.command();
        command.args(["rev-parse", "--verify", "--quiet", "HEAD"]);
        let output = self
            .execute(command, None, 1024, Duration::from_secs(5))
            .await?;
        if output.status.success() {
            return Ok(Some(String::from_utf8(output.stdout)?.trim().to_owned()));
        }
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        checked(output)?;
        bail!("Could not read HEAD")
    }
}

/// `KIRI_STATUS_BACKEND=git` disables the in-process reader for a session.
fn native_enabled() -> bool {
    std::env::var_os("KIRI_STATUS_BACKEND").is_none_or(|value| value != "git")
}

pub(crate) fn builtin_fsmonitor_setting(code: Option<i32>, stdout: &[u8]) -> bool {
    code == Some(0) && stdout.trim_ascii() == b"true"
}

/// Argument batches under Git's command-line budget; every batch holds at least one path.
fn path_batches(paths: &[RepoPath]) -> Result<Vec<&[RepoPath]>> {
    let mut batches = Vec::new();
    let mut start = 0;
    while start < paths.len() {
        let mut end = start;
        let mut bytes = 0;
        while end < paths.len() && bytes + paths[end].bytes().len() < 48 * 1024 {
            bytes += paths[end].bytes().len() + 1;
            end += 1;
        }
        if end == start {
            bail!("Selected path exceeds the staging argument budget");
        }
        batches.push(&paths[start..end]);
        start = end;
    }
    Ok(batches)
}

/// Whether a selected path names `found` itself or a folder containing it.
fn covers(selected: &RepoPath, found: &RepoPath) -> bool {
    let prefix = selected
        .bytes()
        .strip_suffix(b"/")
        .unwrap_or(selected.bytes());
    let candidate = found.bytes();
    candidate == prefix
        || (candidate.len() > prefix.len()
            && candidate.starts_with(prefix)
            && candidate[prefix.len()] == b'/')
}

pub(crate) fn pathspec_input(paths: &[RepoPath]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for path in paths {
        bytes.extend_from_slice(path.bytes());
        bytes.push(0);
    }
    bytes
}

pub(crate) fn checked(output: Output) -> Result<Vec<u8>> {
    if output.truncated {
        bail!("Git output exceeded its safety limit. Narrow the selection.");
    }
    if !output.status.success() {
        bail!(
            "Git: {}",
            terminal_text(String::from_utf8_lossy(&output.stderr).trim())
        );
    }
    Ok(output.stdout)
}

fn path_from_git(bytes: &[u8]) -> Result<PathBuf> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if bytes.is_empty() {
        bail!("Git returned an empty path");
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(OsString::from(std::str::from_utf8(bytes)?)))
    }
}

#[cfg(test)]
mod tests {
    use super::builtin_fsmonitor_setting;

    #[test]
    fn only_the_boolean_true_selects_the_builtin_monitor() {
        assert!(builtin_fsmonitor_setting(Some(0), b"true\n"));
        assert!(!builtin_fsmonitor_setting(Some(0), b"false\n"));
        assert!(!builtin_fsmonitor_setting(Some(1), b""));
        assert!(!builtin_fsmonitor_setting(Some(128), b""));
        assert!(!builtin_fsmonitor_setting(
            Some(0),
            b".git/hooks/fsmonitor-watchman\n"
        ));
    }
}
