use crate::{
    input::Action,
    state::{
        AnalysisReview, App, DiffView, FileScan, Load, Modal, ProposalKind, TreeSelection,
        WorkspaceView,
    },
};
use anyhow::{Context, Result};
use kiri_ai::{
    config::{Credential, Provider, ProviderSettings, Settings, api_key, save_credential},
    oauth,
    provider::AiClient,
    workflow::{self, CommitDraft, CommitPlan},
};
use kiri_core::sync::{Branch, HistoryEntry, SyncAction};
use kiri_core::{
    model::{DiffSide, FileChange, RepoPath, RepoStatus},
    review::CaptureScope,
    storage::Store,
    workspace::{Workspace, Workspaces},
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};

mod highlighting;
#[cfg(test)]
mod tests;

pub enum Message {
    Status {
        workspace: usize,
        revision: u64,
        scan: FileScan,
        result: Result<(Arc<kiri_service::RepositoryService>, RepoStatus)>,
    },
    Diff {
        request: u64,
        workspace: usize,
        path: RepoPath,
        side: DiffSide,
        result: Result<Arc<DiffView>>,
    },
    Highlighted {
        request: u64,
        workspace: usize,
        path: RepoPath,
        side: DiffSide,
        view: Arc<DiffView>,
    },
    Job {
        request: u64,
        workspace: usize,
        result: Result<JobResult>,
    },
    Progress {
        request: u64,
        text: String,
    },
    Registered {
        workspace: usize,
        result: Result<String>,
    },
}

pub struct PendingAnalysis {
    prepared: Arc<kiri_ai::analysis::PreparedAnalysis>,
    client: AiClient,
    kind: ProposalKind,
}

pub enum JobResult {
    Prepared(PendingAnalysis),
    Draft(CommitDraft),
    Plan(CommitPlan),
    Committed(String),
    Applied(usize),
    Staged {
        selection: Option<TreeSelection>,
        side: DiffSide,
    },
    Connected,
    Workspace(Workspace),
    Synced(SyncAction),
    Branches(Vec<Branch>),
    Switched(String),
    History(Vec<HistoryEntry>),
}

#[derive(Eq, PartialEq, Hash)]
enum ReadSlot {
    Status(usize),
    Preview,
    Prefetch(RepoPath, DiffSide),
}

/// Rows around the selection whose patches are read ahead once the selected patch is visible.
/// Sequential review then finds the next file already cached instead of waiting for Git.
const PREFETCH_OFFSETS: [isize; 3] = [1, 2, -1];
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub struct Runtime {
    pub app: App,
    store: Store,
    service: kiri_service::Service,
    tx: mpsc::UnboundedSender<Message>,
    rx: mpsc::UnboundedReceiver<Message>,
    pending_message: Option<Message>,
    reads: kiri_service::jobs::TaskGroup<ReadSlot>,
    highlight_task: Option<highlighting::HighlightTask>,
    job: Option<JoinHandle<()>>,
    pending_analysis: Option<PendingAnalysis>,
    diff_request: u64,
    job_request: u64,
    pub refreshed: Instant,
}

impl Runtime {
    pub fn new(path: PathBuf, store: Store, side: DiffSide) -> Result<Self> {
        let path = if path.is_absolute() {
            path
        } else {
            std::env::current_dir()?.join(path)
        };
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let settings = Settings::load(&store)?;
        let workspaces = Workspaces::load(&store)?.entries;
        let (tx, rx) = mpsc::unbounded_channel();
        Ok(Self {
            app: App::new(workspaces, path, side, settings),
            store,
            service: kiri_service::Service::default(),
            tx,
            rx,
            pending_message: None,
            reads: kiri_service::jobs::TaskGroup::default(),
            highlight_task: None,
            job: None,
            pending_analysis: None,
            diff_request: 0,
            job_request: 0,
            refreshed: Instant::now(),
        })
    }

    pub fn refresh_due(&self) -> bool {
        self.refreshed.elapsed() >= REFRESH_INTERVAL
            && !self.reads.contains(&ReadSlot::Status(self.app.active))
    }

    /// How long the idle loop may sleep. Input, messages, and animations wake it earlier; when
    /// a refresh cannot start because a modal is open or a scan is in flight, the loop waits a
    /// full interval rather than spinning until that condition clears.
    pub fn idle_wake(&self) -> Duration {
        if !matches!(self.app.modal, Modal::None)
            || self.reads.contains(&ReadSlot::Status(self.app.active))
        {
            return REFRESH_INTERVAL;
        }
        REFRESH_INTERVAL
            .saturating_sub(self.refreshed.elapsed())
            .max(Duration::from_millis(1))
    }

    pub fn refresh(&mut self) {
        let index = self.app.active;
        self.app.current_mut().scan = FileScan::Pending;
        self.refreshed = Instant::now();
        self.reads.cancel(&ReadSlot::Status(index));
        self.app.current_mut().revision += 1;
        let revision = self.app.current().revision;
        let path = self.app.current().workspace.root.clone();
        let existing = self.app.current().repo.clone();
        let service = self.service.clone();
        let tx = self.tx.clone();
        self.reads.replace(ReadSlot::Status(index), async move {
            let result = async {
                let partial_tx = tx.clone();
                let partial = move |entry: &Arc<kiri_service::RepositoryService>, status| {
                    let _ = partial_tx.send(Message::Status {
                        workspace: index,
                        revision,
                        scan: FileScan::Pending,
                        result: Ok((entry.clone(), status)),
                    });
                };
                let (entry, status) = if let Some(entry) = existing {
                    let status = entry
                        .inventory(true, |status| partial(&entry, status))
                        .await?;
                    (entry, status)
                } else {
                    service.open_with_inventory(path, partial).await?
                };
                Ok((entry, (*status.status).clone()))
            }
            .await;
            let _ = tx.send(Message::Status {
                workspace: index,
                revision,
                scan: FileScan::Complete,
                result,
            });
        });
    }

    /// Read the patches of neighbouring rows into the shared cache. Runs only after the selected
    /// patch is visible and never while a job holds the screen. Reads that are still wanted after
    /// a selection change keep running; only reads for rows that left the window are cancelled.
    fn prefetch(&mut self, workspace: usize) {
        if self.app.busy() || workspace != self.app.active {
            self.reads
                .retain(|slot| !matches!(slot, ReadSlot::Prefetch(..)));
            return;
        }
        let view = &self.app.workspaces[workspace];
        let Some(repo) = view.repo.clone() else {
            return;
        };
        let side = view.side;
        let files: Vec<FileChange> = PREFETCH_OFFSETS
            .iter()
            .filter_map(|offset| view.selected.checked_add_signed(*offset))
            .filter_map(|row| view.file_at(row))
            .filter(|file| !view.expanded.contains(&file.path))
            .cloned()
            .collect();
        self.reads.retain(|slot| match slot {
            ReadSlot::Prefetch(path, prefetched) => {
                *prefetched == side && files.iter().any(|file| file.path == *path)
            }
            _ => true,
        });
        for file in files {
            let slot = ReadSlot::Prefetch(file.path.clone(), side);
            if self.reads.contains(&slot) {
                continue;
            }
            let repo = repo.clone();
            self.reads.replace(slot, async move {
                let _ = repo.preview(&file, side, false).await;
            });
        }
    }

    pub fn load_diff(&mut self, reset: bool, large: bool) {
        self.highlight_task = None;
        self.reads.cancel(&ReadSlot::Preview);
        self.diff_request += 1;
        let request = self.diff_request;
        let index = self.app.active;
        let workspace = self.app.current_mut();
        if reset {
            workspace.scroll = 0;
            workspace.horizontal = 0;
            workspace.hunk = 0;
        }
        let Some(file) = workspace.file().cloned() else {
            workspace.diff = Load::Loading;
            return;
        };
        let side = workspace.side;
        if large {
            workspace.expanded.insert(file.path.clone());
        }
        let Some(repo) = workspace.repo.clone() else {
            return;
        };
        if reset || large || workspace.displayed_path.as_ref() != Some(&(file.path.clone(), side)) {
            workspace.diff = Load::Loading;
        }
        let large = workspace.expanded.contains(&file.path);
        let tx = self.tx.clone();
        self.reads.replace(ReadSlot::Preview, async move {
            let result = repo
                .preview(&file, side, large)
                .await
                .map(|doc| Arc::new(DiffView::from_shared(doc)));
            let _ = tx.send(Message::Diff {
                request,
                workspace: index,
                path: file.path,
                side,
                result,
            });
        });
    }

    pub fn action(&mut self, action: Action) {
        self.app.dirty = true;
        match action {
            Action::None => {}
            Action::ToggleColor => {
                self.app.color_enabled = !self.app.color_enabled;
                self.app.clear = true;
                self.app.notice = if self.app.color_enabled {
                    "Colors enabled for this session."
                } else {
                    "Colors disabled. Press t to enable."
                }
                .into();
                self.load_diff(false, false);
            }
            Action::SaveTheme => {
                let id = self.app.theme.id.to_owned();
                match Settings::update_ui(&self.store, |ui| ui.theme = Some(id.clone())) {
                    Ok(()) => {
                        self.app.settings.ui.theme = Some(id);
                        self.app.notice = format!("Theme: {}", self.app.theme.name);
                    }
                    Err(error) => self.app.show_error(error.to_string()),
                }
            }
            Action::SaveBorders => {
                let id = self.app.borders.id().to_owned();
                match Settings::update_ui(&self.store, |ui| ui.borders = Some(id.clone())) {
                    Ok(()) => {
                        self.app.settings.ui.borders = Some(id);
                        self.app.notice = format!("Panel borders: {}", self.app.borders.id());
                    }
                    Err(error) => self.app.show_error(error.to_string()),
                }
            }
            Action::Refresh => {
                self.app.notice = "Refreshing repository…".into();
                self.refresh();
            }
            Action::LoadDiff { reset, large } => self.load_diff(reset, large),
            Action::Switch(index) => {
                if index == self.app.active {
                    return;
                }
                self.reads.cancel(&ReadSlot::Status(self.app.active));
                self.app.active = index;
                self.app.filtering = false;
                self.load_diff(false, false);
                self.refresh();
            }
            Action::Cancel => {
                self.pending_analysis = None;
                if let Some(task) = self.job.take() {
                    task.abort();
                }
                self.job_request += 1;
                self.app.modal = Modal::None;
                self.app.notice = "Cancelled. No Git changes were made.".into();
            }
            action => self.start_job(action),
        }
    }

    fn start_job(&mut self, action: Action) {
        let pending = if matches!(action, Action::Analyze) {
            self.pending_analysis.take()
        } else {
            None
        };
        let analysis_mode = self.app.settings.analysis.mode;
        let kind = if matches!(action, Action::Plan { .. }) {
            ProposalKind::Plan
        } else {
            ProposalKind::Draft
        };
        let selection = self.app.current().selection();
        self.job_request += 1;
        let request = self.job_request;
        let index = self.app.active;
        let store = self.store.clone();
        let repo = self.app.current().repo.clone();
        let file = self.app.current().file().cloned();
        let side = self.app.current().side;
        let hunk = self.app.current().hunk;
        let preview = match &self.app.current().diff {
            Load::Ready(view) => Some(view.clone()),
            _ => None,
        };
        let (message, cancellable) = match &action {
            Action::Draft { ai: true, scope } | Action::Plan { scope } => (
                if scope.side == DiffSide::Staged {
                    "Preparing staged changes for analysis…"
                } else {
                    "Capturing working changes for analysis…"
                },
                true,
            ),
            Action::Draft { ai: false, scope } => (
                if scope.side == DiffSide::Staged {
                    "Capturing the staged snapshot…"
                } else {
                    "Capturing working changes…"
                },
                true,
            ),
            Action::Analyze => ("Analyzing selected changes in parallel…", true),
            Action::Commit(_) => ("Creating the reviewed commit…", false),
            Action::Apply(_) => ("Creating the reviewed commits…", false),
            Action::StageFile | Action::StageHunk | Action::StageMany { .. } => {
                ("Updating the index…", false)
            }
            Action::Login => ("Starting ChatGPT sign-in…", true),
            Action::AddWorkspace(_) => ("Opening workspace…", true),
            Action::Connect(_) => ("Saving provider settings…", false),
            Action::Sync {
                action: SyncAction::Fetch,
                ..
            } => ("Fetching remote updates…", false),
            Action::Sync {
                action: SyncAction::Pull,
                ..
            } => ("Pulling incoming commits, fast-forward only…", false),
            Action::Sync {
                action: SyncAction::Push,
                ..
            } => ("Pushing this branch to its upstream…", false),
            Action::Branches => ("Reading local branches…", true),
            Action::History => ("Reading recent history…", true),
            Action::SwitchBranch(_) => ("Switching branch…", false),
            _ => return,
        };
        self.app.modal = Modal::Busy {
            message: message.into(),
            cancellable,
        };
        let tx = self.tx.clone();
        let progress_tx = tx.clone();
        let observer: kiri_ai::progress::Observer = Arc::new(move |event| {
            let _ = progress_tx.send(Message::Progress {
                request,
                text: event.to_string(),
            });
        });
        self.job = Some(tokio::spawn(async move {
            let result = async {
                match action {
                    Action::Connect(mut form) => {
                        if form.settings.endpoint.as_deref() == Some("") {
                            form.settings.endpoint = None;
                        }
                        if form.settings.region.as_deref() == Some("") {
                            form.settings.region = None;
                        }
                        form.settings.validate(form.provider)?;
                        if !form.key.is_empty() {
                            save_credential(
                                &store,
                                form.provider,
                                Credential::ApiKey {
                                    key: form.key.trim().to_owned(),
                                },
                            )?;
                        }
                        if form.provider != Provider::Codex && form.settings.aws_profile.is_none() {
                            api_key(&store, form.provider)?;
                        }
                        Settings::select(&store, form.provider, form.settings)?;
                        Ok(JobResult::Connected)
                    }
                    Action::Login => {
                        let login = oauth::begin().await?;
                        let _ = tx.send(Message::Progress {
                            request,
                            text: format!(
                                "Open {}\nEnter code: {}\nWaiting for approval…",
                                oauth::DEVICE_URL,
                                login.user_code
                            ),
                        });
                        oauth::finish(&store, login).await?;
                        Settings::select(
                            &store,
                            Provider::Codex,
                            ProviderSettings::defaults(Provider::Codex),
                        )?;
                        Ok(JobResult::Connected)
                    }
                    Action::AddWorkspace(path) => Ok(JobResult::Workspace(
                        Workspaces::add(&store, &path, None).await?,
                    )),
                    action => {
                        let entry = repo.context("Wait for the repository to finish opening")?;
                        let repo = entry.repository();
                        match action {
                            Action::Sync { action, target } => {
                                entry.sync(action, target).await?;
                                Ok(JobResult::Synced(action))
                            }
                            Action::Branches => Ok(JobResult::Branches(repo.branches().await?)),
                            Action::History => Ok(JobResult::History(repo.history(200).await?)),
                            Action::SwitchBranch(name) => {
                                entry.switch_branch(name.clone()).await?;
                                Ok(JobResult::Switched(name))
                            }
                            Action::StageMany { paths, side } => {
                                entry.stage(paths, side).await?;
                                Ok(JobResult::Staged { selection, side })
                            }
                            Action::StageFile => {
                                let file = file.context("Select a changed file first")?;
                                let mut paths = vec![file.path];
                                if let Some(original) = file.original_path {
                                    paths.push(original);
                                }
                                entry.stage(paths, side).await?;
                                Ok(JobResult::Staged { selection, side })
                            }
                            Action::StageHunk => {
                                entry
                                    .stage_hunk(
                                        file.context("Select a changed file first")?,
                                        side,
                                        preview
                                            .context("Wait for the patch preview")?
                                            .document
                                            .clone(),
                                        hunk,
                                    )
                                    .await?;
                                Ok(JobResult::Staged { selection, side })
                            }
                            Action::Draft { ai: true, scope } | Action::Plan { scope } => {
                                let client = AiClient::configured(&store)?.with_observer(observer);
                                let captured = entry
                                    .capture_changes(match scope.side {
                                        DiffSide::Staged => CaptureScope::Staged,
                                        DiffSide::Worktree => CaptureScope::Worktree,
                                    })
                                    .await?;
                                let options = Settings::load(&store)?.analysis;
                                let prepared = Arc::new(
                                    kiri_ai::analysis::prepare_captured(
                                        repo,
                                        captured,
                                        scope.paths.as_deref(),
                                        options,
                                        client.observer(),
                                    )
                                    .await?,
                                );
                                Ok(JobResult::Prepared(PendingAnalysis {
                                    prepared,
                                    client,
                                    kind,
                                }))
                            }
                            Action::Draft { ai: false, scope } => Ok(JobResult::Draft(
                                entry
                                    .manual_draft_changes(
                                        match scope.side {
                                            DiffSide::Staged => CaptureScope::Staged,
                                            DiffSide::Worktree => CaptureScope::Worktree,
                                        },
                                        scope.paths,
                                    )
                                    .await?,
                            )),
                            Action::Analyze => {
                                let mut pending =
                                    pending.context("No prepared AI analysis to resume")?;
                                Arc::make_mut(&mut pending.prepared).options.mode = analysis_mode;
                                let client = pending.client.with_observer(observer);
                                match pending.kind {
                                    ProposalKind::Draft => Ok(JobResult::Draft(
                                        workflow::draft_prepared(repo, &client, pending.prepared)
                                            .await?,
                                    )),
                                    ProposalKind::Plan => Ok(JobResult::Plan(
                                        workflow::plan_prepared(repo, &client, pending.prepared)
                                            .await?,
                                    )),
                                }
                            }
                            Action::Commit(draft) => {
                                Ok(JobResult::Committed(entry.commit(draft).await?))
                            }
                            Action::Apply(plan) => {
                                let total = plan.groups.len();
                                let progress = tx.clone();
                                let commits = entry
                                    .apply_plan(plan, move |index, oid| {
                                        let _ = progress.send(Message::Progress {
                                            request,
                                            text: format!(
                                                "Created {}/{} commits · {}",
                                                index + 1,
                                                total,
                                                &oid[..oid.len().min(12)]
                                            ),
                                        });
                                    })
                                    .await?;
                                Ok(JobResult::Applied(commits.len()))
                            }
                            _ => unreachable!(),
                        }
                    }
                }
            }
            .await;
            let _ = tx.send(Message::Job {
                request,
                workspace: index,
                result,
            });
        }));
    }

    pub async fn wait(&mut self) {
        if self.pending_message.is_none() {
            self.pending_message = self.rx.recv().await;
        }
    }

    pub fn drain(&mut self) {
        while let Some(message) = self
            .pending_message
            .take()
            .or_else(|| self.rx.try_recv().ok())
        {
            self.app.dirty = true;
            match message {
                Message::Registered { workspace, result } => match result {
                    Ok(name) => self.app.workspaces[workspace].workspace.name = name,
                    Err(error) => {
                        self.app.notice = format!("Workspace could not be saved: {error}")
                    }
                },
                Message::Status {
                    workspace,
                    revision,
                    scan,
                    result,
                } => {
                    if self.app.workspaces[workspace].revision != revision {
                        continue;
                    }
                    if scan == FileScan::Complete {
                        self.reads.forget(&ReadSlot::Status(workspace));
                    }
                    self.app.workspaces[workspace].scan = scan;
                    match result {
                        Ok((repo, status)) => {
                            let root = repo.root().to_path_buf();
                            let view = &mut self.app.workspaces[workspace];
                            let first_open = view.repo.is_none();
                            view.repo = Some(repo);
                            view.workspace.root = root.clone();
                            let previous_path = view.file().map(|file| file.path.clone());
                            view.set_status(status);
                            let selection_changed =
                                previous_path != view.file().map(|file| file.path.clone());
                            if first_open {
                                let entry = Workspace {
                                    name: root
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned(),
                                    root,
                                };
                                let store = self.store.clone();
                                let tx = self.tx.clone();
                                tokio::task::spawn_blocking(move || {
                                    let result = store.update::<Workspaces, _>(
                                        "workspaces.json",
                                        |registry| {
                                            if let Some(existing) = registry
                                                .entries
                                                .iter()
                                                .find(|w| w.root == entry.root)
                                            {
                                                return Ok(existing.name.clone());
                                            }
                                            let name = entry.name.clone();
                                            registry.entries.push(entry);
                                            Ok(name)
                                        },
                                    );
                                    let _ = tx.send(Message::Registered { workspace, result });
                                });
                            }
                            if workspace == self.app.active
                                && (!self.reads.contains(&ReadSlot::Preview) || selection_changed)
                            {
                                self.load_diff(selection_changed, false);
                            }
                        }
                        Err(error) => {
                            let view = &mut self.app.workspaces[workspace];
                            view.scan = FileScan::Incomplete;
                            if matches!(view.status, Load::Ready(_)) {
                                self.app.notice = format!("File scan incomplete: {error}");
                            } else {
                                view.status = Load::Failed(error.to_string());
                            }
                        }
                    }
                }
                Message::Diff {
                    request,
                    workspace,
                    path,
                    side,
                    result,
                } => {
                    if request != self.diff_request || workspace != self.app.active {
                        continue;
                    }
                    self.reads.forget(&ReadSlot::Preview);
                    let view = &mut self.app.workspaces[workspace];
                    if view.side != side || view.file().is_none_or(|f| f.path != path) {
                        continue;
                    }
                    match result {
                        Ok(diff) => {
                            self.show_diff(workspace, path, side, diff, false);
                            self.prefetch(workspace);
                        }
                        Err(error) => view.diff = Load::Failed(error.to_string()),
                    }
                }
                Message::Highlighted {
                    request,
                    workspace,
                    path,
                    side,
                    view,
                } => {
                    if request != self.diff_request || workspace != self.app.active {
                        continue;
                    }
                    self.highlight_task = None;
                    let current = &mut self.app.workspaces[workspace];
                    if current.side != side || current.file().is_none_or(|file| file.path != path) {
                        continue;
                    }
                    current.remember_diff(view.clone());
                    current.diff = Load::Ready(view);
                }
                Message::Progress { request, text } => {
                    if request == self.job_request
                        && let Modal::Busy { message, .. } = &mut self.app.modal
                    {
                        *message = text;
                    }
                }
                Message::Job {
                    request,
                    workspace,
                    result,
                } => {
                    if request != self.job_request {
                        continue;
                    }
                    self.job = None;
                    self.app.modal = Modal::None;
                    if matches!(
                        &result,
                        Ok(JobResult::Synced(_)
                            | JobResult::Staged { .. }
                            | JobResult::Committed(_)
                            | JobResult::Applied(_)
                            | JobResult::Switched(_))
                    ) {
                        self.app.completion = Some(Instant::now());
                    }
                    match result {
                        Ok(JobResult::Synced(action)) => {
                            if action != SyncAction::Push {
                                self.app.workspaces[workspace].last_fetch = Some(Instant::now());
                            }
                            self.app.notice = format!("{} complete.", action.label());
                            self.refresh();
                        }
                        Ok(JobResult::Branches(entries)) => {
                            self.app.modal = Modal::Branches {
                                entries,
                                selected: 0,
                            }
                        }
                        Ok(JobResult::History(entries)) => {
                            self.app.modal = Modal::History {
                                entries,
                                selected: 0,
                            }
                        }
                        Ok(JobResult::Switched(name)) => {
                            self.app.workspaces[workspace].saved_draft = None;
                            self.app.workspaces[workspace].saved_plan = None;
                            self.app.notice = format!("Switched to {name}");
                            self.refresh();
                        }
                        Ok(JobResult::Prepared(pending)) => {
                            let prepared = &pending.prepared;
                            let review = AnalysisReview {
                                mode: self.app.settings.analysis.mode,
                                inspection_rounds: prepared.options.inspection_rounds,
                                files: prepared.files.len(),
                                bytes: prepared.input_bytes,
                                chunks: prepared.units.len(),
                                calls: prepared
                                    .estimated_calls
                                    .saturating_sub(prepared.options.inspection_limit()),
                                max_calls: prepared.options.max_calls,
                                concurrency: prepared.options.concurrency,
                                provider: pending.client.label(),
                            };
                            let threshold = self.app.settings.ui.auto_approve_calls;
                            self.pending_analysis = Some(pending);
                            if threshold > 0 && review.estimated_calls() <= threshold {
                                self.app.notice = format!(
                                    "Analyzing {} files · about {} model calls",
                                    review.files,
                                    review.estimated_calls()
                                );
                                self.action(Action::Analyze);
                            } else {
                                self.app.modal = Modal::ConfirmAnalysis(review);
                            }
                        }
                        Ok(JobResult::Draft(draft)) => self.app.show_draft(draft),
                        Ok(JobResult::Plan(plan)) => self.app.show_plan(plan),
                        Ok(JobResult::Staged { selection, side }) => {
                            self.app.workspaces[workspace].pending_review =
                                if side == DiffSide::Worktree {
                                    selection
                                } else {
                                    None
                                };
                            self.app.notice = if side == DiffSide::Worktree {
                                "Index updated. Press a for one commit, or b to plan commits from this selection."
                            } else { "Index updated. Unstaged; working files preserved." }.into();
                            self.refresh();
                        }
                        Ok(JobResult::Committed(oid)) => {
                            self.app.workspaces[workspace].saved_draft = None;
                            self.app.workspaces[workspace].saved_plan = None;
                            self.app.notice = format!(
                                "Committed {}. Nothing was pushed.",
                                &oid[..oid.len().min(12)]
                            );
                            self.refresh();
                        }
                        Ok(JobResult::Applied(count)) => {
                            self.app.workspaces[workspace].saved_draft = None;
                            self.app.workspaces[workspace].saved_plan = None;
                            self.app.notice = format!(
                                "Created {count} commits. Working files were preserved. Nothing was pushed."
                            );
                            self.refresh();
                        }
                        Ok(JobResult::Connected) => match Settings::load(&self.store) {
                            Ok(settings) => {
                                self.app.settings = settings;
                                self.app.notice =
                                    "Provider connected. Press a on any changed file or folder."
                                        .into();
                            }
                            Err(error) => self.app.notice = error.to_string(),
                        },
                        Ok(JobResult::Workspace(workspace)) => {
                            let index = self
                                .app
                                .workspaces
                                .iter()
                                .position(|w| w.workspace.root == workspace.root)
                                .unwrap_or_else(|| {
                                    self.app
                                        .workspaces
                                        .push(WorkspaceView::new(workspace, DiffSide::Worktree));
                                    self.app.workspaces.len() - 1
                                });
                            self.app.active = index;
                            self.refresh();
                        }
                        Err(error) => {
                            self.app.show_error(format!("{error:#}"));
                            self.refresh();
                        }
                    }
                }
            }
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.reads.cancel(&ReadSlot::Preview);
        if let Some(task) = self.job.take() {
            task.abort();
        }
    }
}
