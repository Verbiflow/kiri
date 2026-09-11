use std::{
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<u8>", into = "Vec<u8>")]
pub struct RepoPath(Vec<u8>);

impl RepoPath {
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        let value = Self(bytes);
        let path = value.to_path_buf();
        if value.0.is_empty()
            || value.0.contains(&0)
            || path.is_absolute()
            || path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || path
                .components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
        {
            bail!("Expected a relative repository file path");
        }
        Ok(value)
    }

    pub fn from_path(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::new(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(not(unix))]
        {
            Self::new(
                path.to_str()
                    .ok_or_else(|| anyhow::anyhow!("Path is not UTF-8"))?
                    .as_bytes()
                    .to_vec(),
            )
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_path_buf(&self) -> PathBuf {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(OsString::from_vec(self.0.clone()))
        }
        #[cfg(not(unix))]
        {
            PathBuf::from(OsString::from(
                String::from_utf8_lossy(&self.0).into_owned(),
            ))
        }
    }

    pub fn display(&self) -> String {
        terminal_text(&String::from_utf8_lossy(&self.0))
    }
    pub fn id(&self) -> String {
        digest(&self.0)
    }
}

impl TryFrom<Vec<u8>> for RepoPath {
    type Error = anyhow::Error;
    fn try_from(bytes: Vec<u8>) -> Result<Self> {
        Self::new(bytes)
    }
}

impl From<RepoPath> for Vec<u8> {
    fn from(path: RepoPath) -> Self {
        path.0
    }
}

impl fmt::Display for RepoPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    #[default]
    Worktree,
    Staged,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Unmerged,
    Untracked,
}

impl ChangeKind {
    pub fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Copied => 'C',
            Self::TypeChanged => 'T',
            Self::Unmerged => 'U',
            Self::Untracked => '?',
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: RepoPath,
    pub original_path: Option<RepoPath>,
    pub staged: Option<ChangeKind>,
    pub worktree: Option<ChangeKind>,
    pub submodule: bool,
    /// Blob recorded in HEAD for this path, when Git reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    /// Blob recorded in the index for this path, when Git reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_oid: Option<String>,
}

impl FileChange {
    pub fn kind(&self, side: DiffSide) -> Option<ChangeKind> {
        match side {
            DiffSide::Staged => self.staged,
            DiffSide::Worktree => self.worktree,
        }
    }
    pub fn conflicted(&self) -> bool {
        self.staged == Some(ChangeKind::Unmerged) || self.worktree == Some(ChangeKind::Unmerged)
    }
    /// Exact identity of the content a patch for `side` depends on, excluding the working file.
    pub fn content_identity(&self, side: DiffSide) -> ContentIdentity {
        match side {
            DiffSide::Staged => ContentIdentity {
                base: self.head_oid.clone(),
                target: self.index_oid.clone(),
            },
            DiffSide::Worktree => ContentIdentity {
                base: self.index_oid.clone(),
                target: None,
            },
        }
    }
}

/// Git object identities a patch was computed from. Two patches with equal identities and an
/// equal [`WorktreeStamp`] are byte-identical, so they can share one cached document.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ContentIdentity {
    pub base: Option<String>,
    pub target: Option<String>,
}

/// Cheap fingerprint of a working file taken with one `lstat`. Any edit that Git would notice
/// changes at least one of these fields, so previews keyed by a stamp never serve stale content.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorktreeStamp {
    pub size: u64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
    pub inode: u64,
    pub mode: u32,
}

impl WorktreeStamp {
    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Self {
                size: metadata.len(),
                modified: (metadata.mtime(), metadata.mtime_nsec()),
                changed: (metadata.ctime(), metadata.ctime_nsec()),
                inode: metadata.ino(),
                mode: metadata.mode(),
            }
        }
        #[cfg(not(unix))]
        {
            let seconds = |time: std::io::Result<std::time::SystemTime>| {
                time.ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
                    .unwrap_or((0, 0))
            };
            Self {
                size: metadata.len(),
                modified: seconds(metadata.modified()),
                changed: seconds(metadata.created()),
                inode: 0,
                mode: u32::from(metadata.permissions().readonly()),
            }
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RepoStatus {
    pub branch: String,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub files: Vec<FileChange>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn terminal_message(text: &str) -> String {
    text.split('\n')
        .map(terminal_text)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn terminal_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\t' {
            result.push_str("    ");
        } else if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            result.extend(c.escape_default());
        } else {
            result.push(c);
        }
    }
    result
}
