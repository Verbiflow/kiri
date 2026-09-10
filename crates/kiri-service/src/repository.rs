use anyhow::{Context, Result};
use kiri_analysis::proposal::{CommitDraft, CommitPlan};
use kiri_core::{
    commit::StagedSnapshot,
    diff::DiffDocument,
    model::{DiffSide, FileChange, RepoPath, RepoStatus},
    repo::Repository,
    sync::{SyncAction, SyncTarget},
    workbench::{Comparison, Preview},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};

#[derive(Clone, Debug)]
pub struct StatusSnapshot {
    pub revision: u64,
    pub status: Arc<RepoStatus>,
}
#[derive(Clone, Default)]
pub struct Service {
    repositories: Arc<Mutex<HashMap<PathBuf, Weak<RepositoryService>>>>,
}
impl Service {
    pub async fn open(&self, path: impl AsRef<Path>) -> Result<Arc<RepositoryService>> {
        self.adopt(Repository::open(path).await?).await
    }
    pub async fn discover(&self, path: impl AsRef<Path>) -> Result<Option<Arc<RepositoryService>>> {
        match Repository::discover(path).await? {
            Some(repo) => Ok(Some(self.adopt(repo).await?)),
            None => Ok(None),
        }
    }
    async fn adopt(&self, repo: Repository) -> Result<Arc<RepositoryService>> {
        let mut entries = self.repositories.lock().await;
        entries.retain(|_, entry| entry.strong_count() > 0);
        if let Some(entry) = entries.get(repo.root()).and_then(Weak::upgrade) {
            return Ok(entry);
        }
        let root = repo.root().to_path_buf();
        let entry = RepositoryService::new(repo);
        entries.insert(root, Arc::downgrade(&entry));
        Ok(entry)
    }
    pub async fn close(&self, root: &Path) {
        let mut entries = self.repositories.lock().await;
        if entries
            .get(root)
            .is_some_and(|entry| entry.strong_count() == 0)
        {
            entries.remove(root);
        }
    }
}

type PreviewKey = (RepoPath, DiffSide, bool);
type Reply<T> = oneshot::Sender<Result<T>>;
type PlanProgress = Box<dyn FnMut(usize, &str) + Send>;
enum Write {
    Hunk(FileChange, DiffSide, Arc<DiffDocument>, usize, Reply<()>),
    Sync(SyncAction, Option<SyncTarget>, Reply<()>),
    SwitchBranch(String, Reply<()>),
    Apply(Box<CommitPlan>, PlanProgress, Reply<Vec<String>>),
    Stage(Vec<RepoPath>, DiffSide, Reply<()>),
    Commit(Box<CommitDraft>, Reply<String>),
    Capture(Reply<StagedSnapshot>),
    Fence(Reply<()>),
    All(DiffSide, Reply<()>),
    Message(String, bool, Reply<String>),
    Push(String, Reply<()>),
    CaptureChanges(
        kiri_core::review::CaptureScope,
        Reply<kiri_core::review::CapturedChanges>,
    ),
}
struct QueuedWrite {
    owner: Arc<RepositoryService>,
    operation: Write,
}
pub struct RepositoryService {
    repo: Repository,
    revision: AtomicU64,
    status_cache: Mutex<Option<(Instant, StatusSnapshot)>>,
    previews: crate::read_cache::ReadCache<PreviewKey, DiffDocument>,
    comparisons: crate::read_cache::ReadCache<(RepoPath, Comparison), Preview>,
    writes: mpsc::Sender<QueuedWrite>,
    events: broadcast::Sender<u64>,
}
impl RepositoryService {
    fn new(repo: Repository) -> Arc<Self> {
        let (writes, mut requests) = mpsc::channel::<QueuedWrite>(64);
        let (events, _) = broadcast::channel(64);
        let entry = Arc::new(Self {
            repo,
            revision: AtomicU64::new(0),
            status_cache: Mutex::new(None),
            previews: Default::default(),
            comparisons: Default::default(),
            writes,
            events,
        });
        tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                let entry = request.owner;
                match request.operation {
                    Write::Hunk(file, side, preview, hunk, reply) => {
                        let result = entry.repo.stage_hunk(&file, side, &preview, hunk).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Sync(action, target, reply) => {
                        let result = entry.repo.sync(action, target.as_ref()).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::SwitchBranch(branch, reply) => {
                        let result = entry.repo.switch_branch(&branch).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Apply(plan, progress, reply) => {
                        let result = plan.apply(&entry.repo, progress).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Stage(paths, side, reply) => {
                        let result = match side {
                            DiffSide::Worktree => entry.repo.stage(&paths).await,
                            DiffSide::Staged => entry.repo.unstage(&paths).await,
                        };
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Commit(draft, reply) => {
                        let result = draft.commit(&entry.repo).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Fence(reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    Write::All(side, reply) => {
                        let result = async {
                            let status = entry.repo.status().await?;
                            let paths: Vec<_> = status
                                .files
                                .iter()
                                .filter(|file| file.kind(side).is_some())
                                .map(|file| file.path.clone())
                                .collect();
                            if paths.is_empty() {
                                return Ok(());
                            }
                            match side {
                                DiffSide::Worktree => entry.repo.stage(&paths).await,
                                DiffSide::Staged => entry.repo.unstage(&paths).await,
                            }
                        }
                        .await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Message(message, amend, reply) => {
                        let result = entry.repo.commit_message(&message, amend).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Push(branch, reply) => {
                        let result = entry.repo.publish(&branch).await;
                        entry.invalidate();
                        let _ = reply.send(result);
                    }
                    Write::Capture(mut reply) => {
                        tokio::select! { result = entry.repo.staged_snapshot() => { let _ = reply.send(result); }, _ = reply.closed() => {} }
                    }
                    Write::CaptureChanges(scope, mut reply) => {
                        tokio::select! { result = entry.repo.capture_changes(scope) => { let _ = reply.send(result); }, _ = reply.closed() => {} }
                    }
                }
            }
        });
        entry
    }
    pub fn repository(&self) -> &Repository {
        &self.repo
    }
    pub fn root(&self) -> &Path {
        self.repo.root()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.events.subscribe()
    }
    pub fn invalidate(&self) -> u64 {
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.events.send(revision);
        revision
    }
    pub async fn status(&self, fresh: bool) -> Result<StatusSnapshot> {
        self.inventory(fresh, |_| {}).await
    }
    pub async fn inventory(
        &self,
        fresh: bool,
        partial: impl Fn(RepoStatus) + Send,
    ) -> Result<StatusSnapshot> {
        let requested = Instant::now();
        let mut cached = self.status_cache.lock().await;
        loop {
            let revision = self.revision.load(Ordering::Acquire);
            if let Some((at, status)) = &*cached
                && status.revision == revision
                && ((!fresh && at.elapsed() < Duration::from_millis(250)) || *at >= requested)
            {
                return Ok(status.clone());
            }
            let status = if cached.is_none() {
                let mut status = self.repo.tracked_status().await?;
                partial(status.clone());
                status.files.extend(self.repo.untracked_files().await?);
                status
            } else {
                self.repo.status().await?
            };
            if self
                .revision
                .compare_exchange(revision, revision + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            let revision = revision + 1;
            let result = StatusSnapshot {
                revision,
                status: Arc::new(status),
            };
            *cached = Some((Instant::now(), result.clone()));
            return Ok(result);
        }
    }
    pub async fn preview(
        &self,
        file: &FileChange,
        side: DiffSide,
        large: bool,
    ) -> Result<Arc<DiffDocument>> {
        self.previews
            .get(
                (file.path.clone(), side, large),
                &self.revision,
                self.repo.diff(file, side, large),
            )
            .await
    }
    pub async fn compare(&self, path: &RepoPath, comparison: &Comparison) -> Result<Arc<Preview>> {
        self.comparisons
            .get(
                (path.clone(), comparison.clone()),
                &self.revision,
                self.repo.preview_comparison(path, comparison),
            )
            .await
    }
    fn enqueue<T: Send + 'static>(
        self: &Arc<Self>,
        make: impl FnOnce(Reply<T>) -> Write,
    ) -> impl Future<Output = Result<T>> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let admitted = self.writes.try_send(QueuedWrite {
            owner: self.clone(),
            operation: make(reply),
        });
        async move {
            admitted.map_err(|_| anyhow::anyhow!("Repository write queue is full or closed"))?;
            result.await.context(
                "Operation outcome unavailable; inspect Git state before retrying mutations",
            )?
        }
    }
    pub fn stage_hunk(
        self: &Arc<Self>,
        file: FileChange,
        side: DiffSide,
        preview: Arc<DiffDocument>,
        hunk: usize,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        self.enqueue(move |reply| Write::Hunk(file, side, preview, hunk, reply))
    }
    pub fn sync(
        self: &Arc<Self>,
        action: SyncAction,
        target: Option<SyncTarget>,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        self.enqueue(move |reply| Write::Sync(action, target, reply))
    }
    pub fn switch_branch(
        self: &Arc<Self>,
        branch: String,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        self.enqueue(move |reply| Write::SwitchBranch(branch, reply))
    }
    pub fn apply_plan(
        self: &Arc<Self>,
        plan: CommitPlan,
        progress: impl FnMut(usize, &str) + Send + 'static,
    ) -> impl Future<Output = Result<Vec<String>>> + Send + 'static {
        self.enqueue(move |reply| Write::Apply(Box::new(plan), Box::new(progress), reply))
    }
    pub fn manual_draft(
        self: &Arc<Self>,
        paths: Option<Vec<RepoPath>>,
    ) -> impl Future<Output = Result<CommitDraft>> + Send + 'static {
        let repository = self.root().to_path_buf();
        let capture = self.capture();
        async move {
            Ok(CommitDraft {
                repository,
                snapshot: capture.await?.into(),
                paths,
                message: String::new(),
                warnings: Vec::new(),
                analysis: None,
            })
        }
    }
    pub fn fence(self: &Arc<Self>) -> impl Future<Output = Result<()>> + Send + 'static {
        self.enqueue(Write::Fence)
    }
    pub fn stage_all(
        self: &Arc<Self>,
        side: DiffSide,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        self.enqueue(move |reply| Write::All(side, reply))
    }
    pub fn commit_message(
        self: &Arc<Self>,
        message: String,
        amend: bool,
    ) -> impl Future<Output = Result<String>> + Send + 'static {
        self.enqueue(move |reply| Write::Message(message, amend, reply))
    }
    pub fn push(
        self: &Arc<Self>,
        branch: String,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        self.enqueue(move |reply| Write::Push(branch, reply))
    }
    pub fn stage(
        self: &Arc<Self>,
        paths: Vec<RepoPath>,
        side: DiffSide,
    ) -> impl Future<Output = Result<()>> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let admitted = self.writes.try_send(QueuedWrite {
            owner: self.clone(),
            operation: Write::Stage(paths, side, reply),
        });
        async move {
            admitted.map_err(|_| anyhow::anyhow!("Repository write queue is full or closed"))?;
            result
                .await
                .context("Write outcome is unknown; inspect Git state before retrying")?
        }
    }
    pub fn commit(
        self: &Arc<Self>,
        draft: CommitDraft,
    ) -> impl Future<Output = Result<String>> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let admitted = self.writes.try_send(QueuedWrite {
            owner: self.clone(),
            operation: Write::Commit(Box::new(draft), reply),
        });
        async move {
            admitted.map_err(|_| anyhow::anyhow!("Repository write queue is full or closed"))?;
            result
                .await
                .context("Commit outcome is unknown; inspect Git state before retrying")?
        }
    }
    pub fn capture_changes(
        self: &Arc<Self>,
        scope: kiri_core::review::CaptureScope,
    ) -> impl Future<Output = Result<kiri_core::review::CapturedChanges>> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let admitted = self.writes.try_send(QueuedWrite {
            owner: self.clone(),
            operation: Write::CaptureChanges(scope, reply),
        });
        async move {
            admitted.map_err(|_| anyhow::anyhow!("Repository queue is full or closed"))?;
            result.await.context("Capture cancelled")?
        }
    }
    pub fn capture(
        self: &Arc<Self>,
    ) -> impl Future<Output = Result<StagedSnapshot>> + Send + 'static {
        let (reply, result) = oneshot::channel();
        let admitted = self.writes.try_send(QueuedWrite {
            owner: self.clone(),
            operation: Write::Capture(reply),
        });
        async move {
            admitted.map_err(|_| anyhow::anyhow!("Repository write queue is full or closed"))?;
            result.await.context("Snapshot request stopped")?
        }
    }
}
