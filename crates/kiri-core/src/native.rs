//! In-process repository reads through gitoxide.
//!
//! Status is the one read that bounds launch time: Git hashes every stat-dirty file on a single
//! thread, while gitoxide compares the index to the worktree on a thread pool and never pays for
//! a process. The mapping below produces exactly what [`crate::status::parse_status`] produces
//! from `git status --porcelain=v2`, and it refuses (returns `None`) whenever the repository is
//! in a state whose porcelain rendering it does not reproduce byte for byte. Callers then run
//! Git itself, so accuracy never depends on this module being complete.

use crate::model::{ChangeKind, FileChange, RepoPath, RepoStatus};
use anyhow::{Context, Result};
use gix::{
    bstr::{BString, ByteSlice},
    index::entry::{Flags, Mode, Stage},
    status::{
        Item, UntrackedFiles, index_worktree,
        plumbing::index_as_worktree::{Change, EntryStatus},
        tree_index::TrackRenames,
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// A repository opened in-process. Cheap to clone; every read creates a thread-local view.
#[derive(Clone)]
pub struct Native {
    repo: gix::ThreadSafeRepository,
}

impl std::fmt::Debug for Native {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Native")
            .field("git_dir", &self.repo.git_dir())
            .finish()
    }
}

pub struct Discovered {
    pub native: Native,
    /// The working tree root as gitoxide resolved it, before canonicalization.
    pub root: PathBuf,
    /// `core.fsmonitor` is the boolean `true`, selecting Git's builtin daemon.
    pub builtin_fsmonitor: bool,
}

pub enum Discovery {
    Repository(Box<Discovered>),
    NotRepository,
}

impl Native {
    /// Locate the working tree containing `path`. Bare repositories count as not found because
    /// Kiri only operates on working trees.
    pub fn discover(path: &Path) -> Result<Discovery> {
        let repo = match gix::ThreadSafeRepository::discover(path) {
            Ok(repo) => repo,
            Err(gix::discover::Error::Discover(_)) => return Ok(Discovery::NotRepository),
            Err(error) => return Err(error.into()),
        };
        let local = repo.to_thread_local();
        let Some(root) = local.workdir().map(Path::to_path_buf) else {
            return Ok(Discovery::NotRepository);
        };
        let builtin_fsmonitor = matches!(
            local.config_snapshot().try_boolean("core.fsmonitor"),
            Ok(Some(true))
        );
        Ok(Discovery::Repository(Box::new(Discovered {
            native: Self { repo },
            root,
            builtin_fsmonitor,
        })))
    }

    /// Compute the repository status, or `None` when Git must compute it instead. `None` is
    /// returned for conflicts, submodules, sparse checkouts, assume-unchanged entries, and
    /// intent-to-add entries, where porcelain output has rules this mapping does not implement.
    pub fn status(&self, untracked: bool) -> Result<Option<RepoStatus>> {
        let repo = self.repo.to_thread_local();
        let index = repo.index_or_empty()?;
        let unsupported = index.entries().iter().any(|entry| {
            entry.stage() != Stage::Unconflicted
                || entry.mode.is_submodule()
                || entry
                    .flags
                    .intersects(Flags::SKIP_WORKTREE | Flags::INTENT_TO_ADD | Flags::ASSUME_VALID)
        });
        if unsupported {
            return Ok(None);
        }
        let mut status = RepoStatus::default();
        if !self.branch_headers(&repo, &mut status)? {
            return Ok(None);
        }
        let platform = repo
            .status(gix::progress::Discard)?
            .untracked_files(if untracked {
                UntrackedFiles::Files
            } else {
                UntrackedFiles::None
            })
            .index_worktree_rewrites(None)
            .index_worktree_submodules(None)
            .tree_index_track_renames(TrackRenames::Disabled);
        let mut tracked: BTreeMap<Vec<u8>, FileChange> = BTreeMap::new();
        let mut untracked_paths: BTreeSet<Vec<u8>> = BTreeSet::new();
        for item in platform.into_iter(Vec::<BString>::new())? {
            match item? {
                Item::TreeIndex(change) => {
                    use gix::diff::index::Change as Tree;
                    let (location, staged, head_oid, index_oid) = match change {
                        Tree::Addition { location, id, .. } => {
                            (location, ChangeKind::Added, None, Some(hex(&id)))
                        }
                        Tree::Deletion { location, id, .. } => {
                            (location, ChangeKind::Deleted, Some(hex(&id)), None)
                        }
                        Tree::Modification {
                            location,
                            previous_entry_mode,
                            previous_id,
                            entry_mode,
                            id,
                            ..
                        } => {
                            let kind = if type_of(previous_entry_mode) != type_of(entry_mode) {
                                ChangeKind::TypeChanged
                            } else {
                                ChangeKind::Modified
                            };
                            (location, kind, Some(hex(&previous_id)), Some(hex(&id)))
                        }
                        Tree::Rewrite { .. } => return Ok(None),
                    };
                    let location: &[u8] = location.as_ref();
                    let file = entry(&mut tracked, location)?;
                    file.staged = Some(staged);
                    file.head_oid = head_oid;
                    file.index_oid = index_oid;
                }
                Item::IndexWorktree(index_worktree::Item::Modification {
                    entry: index_entry,
                    rela_path,
                    status,
                    ..
                }) => {
                    let kind = match status {
                        EntryStatus::Conflict { .. } | EntryStatus::IntentToAdd => return Ok(None),
                        EntryStatus::NeedsUpdate(_) => continue,
                        EntryStatus::Change(Change::Removed) => ChangeKind::Deleted,
                        EntryStatus::Change(Change::Type { .. }) => ChangeKind::TypeChanged,
                        EntryStatus::Change(Change::Modification { .. }) => ChangeKind::Modified,
                        EntryStatus::Change(Change::SubmoduleModification(_)) => return Ok(None),
                    };
                    let file = entry(&mut tracked, rela_path.as_bytes())?;
                    file.worktree = Some(kind);
                    if file.staged.is_none() {
                        // Unchanged in the index, so HEAD and the index hold the same blob.
                        let id = index_entry.id.to_string();
                        file.head_oid = Some(id.clone());
                        file.index_oid = Some(id);
                    }
                }
                Item::IndexWorktree(index_worktree::Item::DirectoryContents { entry, .. }) => {
                    if entry.status != gix::dir::entry::Status::Untracked {
                        continue;
                    }
                    let mut path = entry.rela_path.to_vec();
                    if entry.disk_kind == Some(gix::dir::entry::Kind::Repository) {
                        path.push(b'/');
                    }
                    untracked_paths.insert(path);
                }
                Item::IndexWorktree(index_worktree::Item::Rewrite { .. }) => return Ok(None),
            }
        }
        status.files = tracked.into_values().collect();
        for path in untracked_paths {
            status.files.push(FileChange {
                path: RepoPath::new(path)?,
                original_path: None,
                staged: None,
                worktree: Some(ChangeKind::Untracked),
                submodule: false,
                head_oid: None,
                index_oid: None,
            });
        }
        Ok(Some(status))
    }

    /// Fill the `# branch.*` headers. Returns `false` when HEAD points at something porcelain
    /// output would describe differently from this mapping.
    fn branch_headers(&self, repo: &gix::Repository, status: &mut RepoStatus) -> Result<bool> {
        let head = repo.head()?;
        let name = head.referent_name().map(|name| name.to_owned());
        status.branch = match &name {
            Some(name) => name.shorten().to_string(),
            None => "(detached)".to_owned(),
        };
        status.head = match head.kind {
            gix::head::Kind::Unborn(_) => None,
            _ => Some(repo.head_id()?.to_string()),
        };
        let Some(name) = name else {
            return Ok(true);
        };
        let Some(tracking) =
            repo.branch_remote_tracking_ref_name(name.as_ref(), gix::remote::Direction::Fetch)
        else {
            return Ok(true);
        };
        let tracking = tracking?;
        status.upstream = Some(tracking.shorten().to_string());
        let (Some(head_id), Some(upstream)) = (
            status.head.as_deref(),
            repo.try_find_reference(tracking.as_ref())?,
        ) else {
            return Ok(true);
        };
        let upstream_id = upstream.into_fully_peeled_id()?.detach();
        let head_id = gix::ObjectId::from_hex(head_id.as_bytes())?;
        status.ahead = count(repo, head_id, upstream_id)?;
        status.behind = count(repo, upstream_id, head_id)?;
        Ok(true)
    }
}

fn count(repo: &gix::Repository, tip: gix::ObjectId, hidden: gix::ObjectId) -> Result<usize> {
    let mut total = 0;
    for commit in repo.rev_walk([tip]).with_hidden([hidden]).all()? {
        commit?;
        total += 1;
    }
    Ok(total)
}

fn hex(id: &gix::hash::oid) -> String {
    id.to_hex().to_string()
}

fn type_of(mode: Mode) -> u8 {
    if mode.is_submodule() {
        3
    } else if mode.contains(Mode::SYMLINK) {
        2
    } else if mode.contains(Mode::DIR) {
        4
    } else {
        1
    }
}

fn entry<'a>(
    tracked: &'a mut BTreeMap<Vec<u8>, FileChange>,
    location: &[u8],
) -> Result<&'a mut FileChange> {
    if !tracked.contains_key(location) {
        tracked.insert(
            location.to_vec(),
            FileChange {
                path: RepoPath::new(location.to_vec())?,
                original_path: None,
                staged: None,
                worktree: None,
                submodule: false,
                head_oid: None,
                index_oid: None,
            },
        );
    }
    tracked
        .get_mut(location)
        .context("Tracked entry vanished during status assembly")
}
