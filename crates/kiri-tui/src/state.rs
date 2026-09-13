use crate::theme::{Borders, Theme};
use kiri_ai::{
    config::{Provider, ProviderSettings, Settings},
    workflow::{CommitDraft, CommitPlan},
};
use kiri_core::sync::{Branch, HistoryEntry, SyncAction};
use kiri_core::{
    diff::{DiffDocument, LineKind},
    model::{DiffSide, FileChange, RepoPath, RepoStatus},
    tree::{ChangeTree, Entry, TreeNode},
    workspace::Workspace,
};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};
use tui_textarea::TextArea;

pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}

#[derive(Clone)]
pub struct DiffView {
    pub document: Arc<DiffDocument>,
    pub pairs: Arc<Vec<(Option<usize>, Option<usize>)>>,
    pub text: Arc<Vec<String>>,
    pub syntax: crate::highlight::SyntaxState,
}

impl DiffView {
    pub fn new(document: DiffDocument) -> Self {
        Self::from_shared(Arc::new(document))
    }

    pub fn from_shared(document: Arc<DiffDocument>) -> Self {
        let mut pairs = Vec::new();
        let mut index = 0;
        while index < document.lines.len() {
            if document.lines[index].kind == LineKind::Removed {
                let start = index;
                while index < document.lines.len()
                    && document.lines[index].kind == LineKind::Removed
                {
                    index += 1;
                }
                let added = index;
                while index < document.lines.len() && document.lines[index].kind == LineKind::Added
                {
                    index += 1;
                }
                for n in 0..(added - start).max(index - added) {
                    pairs.push((
                        (start + n < added).then_some(start + n),
                        (added + n < index).then_some(added + n),
                    ));
                }
            } else if document.lines[index].kind == LineKind::Added {
                pairs.push((None, Some(index)));
                index += 1;
            } else {
                pairs.push((Some(index), Some(index)));
                index += 1;
            }
        }
        let text: Vec<_> = (0..document.lines.len())
            .map(|i| {
                let bytes = document.line(i);
                let mut text = kiri_core::model::terminal_text(&String::from_utf8_lossy(
                    &bytes[..bytes.len().min(16384)],
                ));
                if bytes.len() > 16384 {
                    text.push_str("  [line clipped at 16 KiB]");
                }
                text
            })
            .collect();
        Self {
            document,
            pairs: Arc::new(pairs),
            text: Arc::new(text),
            syntax: crate::highlight::SyntaxState::Plain,
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum FileScan {
    Pending,
    Complete,
    Incomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TreeSelection {
    File(RepoPath),
    Folder(RepoPath),
}
impl TreeSelection {
    pub fn path(&self) -> &RepoPath {
        match self {
            Self::File(path) | Self::Folder(path) => path,
        }
    }
    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

#[derive(Clone, Copy)]
pub enum ProposalKind {
    Draft,
    Plan,
}

/// What an AI or manual commit action covers: which side of the index the evidence comes
/// from, and which paths. `None` means every change on that side.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scope {
    pub side: DiffSide,
    pub paths: Option<Vec<RepoPath>>,
}

impl Scope {
    pub fn source(&self) -> &'static str {
        match self.side {
            DiffSide::Staged => "staged",
            DiffSide::Worktree => "working-tree",
        }
    }
    pub fn matches_paths(&self, paths: Option<&[RepoPath]>) -> bool {
        match (&self.paths, paths) {
            (None, None) => true,
            (Some(mine), Some(theirs)) => {
                mine.iter().collect::<HashSet<_>>() == theirs.iter().collect::<HashSet<_>>()
            }
            _ => false,
        }
    }
}

pub struct AnalysisReview {
    pub mode: kiri_ai::analysis::AnalysisMode,
    pub inspection_rounds: usize,
    pub files: usize,
    pub bytes: u64,
    pub chunks: usize,
    pub calls: usize,
    pub max_calls: usize,
    pub concurrency: usize,
    pub provider: String,
}

impl AnalysisReview {
    pub fn estimated_calls(&self) -> usize {
        self.calls
            + match self.mode {
                kiri_ai::analysis::AnalysisMode::Fast => 0,
                kiri_ai::analysis::AnalysisMode::Deep => self.inspection_rounds,
            }
    }
}

pub struct WorkspaceView {
    pub workspace: Workspace,
    pub repo: Option<Arc<kiri_service::RepositoryService>>,
    pub status: Load<RepoStatus>,
    pub scan: FileScan,
    pub working_count: usize,
    pub staged_count: usize,
    pub revision: u64,
    pub side: DiffSide,
    pub selected: usize,
    pub filter: String,
    pub matching: Vec<usize>,
    pub tree: ChangeTree,
    pub collapsed_dirs: HashSet<RepoPath>,
    known_dirs: HashSet<RepoPath>,
    pub visible: Vec<usize>,
    pub diff: Load<Arc<DiffView>>,
    pub displayed_path: Option<(RepoPath, DiffSide)>,
    pub expanded: HashSet<RepoPath>,
    pub last_fetch: Option<Instant>,
    pub scroll: usize,
    pub horizontal: usize,
    pub hunk: usize,
    /// Recently built views addressed by patch fingerprint, so an unchanged patch keeps its
    /// syntax colors across refreshes and revisits without rebuilding or re-tokenizing.
    cache: VecDeque<Arc<DiffView>>,
    /// Lowercased display paths aligned with `status.files`, computed once per inventory so the
    /// filter never allocates per keystroke.
    lower: Vec<String>,
    pub saved_draft: Option<CommitDraft>,
    pub saved_plan: Option<CommitPlan>,
    pub pending_review: Option<TreeSelection>,
}

impl WorkspaceView {
    pub fn new(workspace: Workspace, side: DiffSide) -> Self {
        Self {
            workspace,
            repo: None,
            status: Load::Loading,
            scan: FileScan::Pending,
            working_count: 0,
            staged_count: 0,
            revision: 0,
            side,
            selected: 0,
            filter: String::new(),
            matching: Vec::new(),
            tree: ChangeTree::default(),
            collapsed_dirs: HashSet::new(),
            known_dirs: HashSet::new(),
            visible: Vec::new(),
            diff: Load::Loading,
            displayed_path: None,
            expanded: HashSet::new(),
            last_fetch: None,
            scroll: 0,
            horizontal: 0,
            hunk: 0,
            cache: VecDeque::new(),
            lower: Vec::new(),
            saved_draft: None,
            saved_plan: None,
            pending_review: None,
        }
    }

    pub fn selection(&self) -> Option<TreeSelection> {
        let path = self.selected_path()?.clone();
        Some(if self.node()?.is_folder() {
            TreeSelection::Folder(path)
        } else {
            TreeSelection::File(path)
        })
    }

    /// The selected file or folder on the current tab, as a commit scope.
    pub fn selection_scope(&self) -> Option<Scope> {
        let paths = self.selected_paths();
        (!paths.is_empty()).then_some(Scope {
            side: self.side,
            paths: Some(paths),
        })
    }

    /// Every change listed on the current tab, honoring the filter. On the Staged tab with no
    /// filter this is "all staged changes"; on the Working tab it is always explicit, so
    /// staged-only files never slip into a working-tree commit.
    pub fn tab_scope(&self) -> Option<Scope> {
        let Load::Ready(status) = &self.status else {
            return None;
        };
        if self.matching.is_empty() {
            return None;
        }
        let paths = if self.side == DiffSide::Staged && self.filter.is_empty() {
            None
        } else {
            Some(
                self.matching
                    .iter()
                    .flat_map(|&i| {
                        let file = &status.files[i];
                        std::iter::once(file.path.clone()).chain(file.original_path.clone())
                    })
                    .collect(),
            )
        };
        Some(Scope {
            side: self.side,
            paths,
        })
    }

    pub fn review_staged(&mut self) {
        if self.side != DiffSide::Staged {
            return;
        }
        let Some(target) = &self.pending_review else {
            return;
        };
        let Load::Ready(status) = &self.status else {
            return;
        };
        let Some(index) = self.tree.nodes.iter().position(|node| {
            node.path(&status.files) == target.path() && node.is_folder() == target.is_folder()
        }) else {
            return;
        };
        let mut parent = self.tree.nodes[index].parent;
        while let Some(index) = parent {
            self.collapsed_dirs
                .remove(self.tree.nodes[index].path(&status.files));
            parent = self.tree.nodes[index].parent;
        }
        self.visible = self
            .tree
            .visible(&self.collapsed_dirs, !self.filter.is_empty());
        if let Some(row) = self.visible.iter().position(|&i| i == index) {
            self.selected = row;
            self.pending_review = None;
        }
    }

    pub fn node(&self) -> Option<&TreeNode> {
        self.visible
            .get(self.selected)
            .and_then(|i| self.tree.nodes.get(*i))
    }

    pub fn file(&self) -> Option<&FileChange> {
        self.file_at(self.selected)
    }

    /// The changed file shown on a visible row, or `None` for folders and rows out of range.
    pub fn file_at(&self, row: usize) -> Option<&FileChange> {
        let Load::Ready(status) = &self.status else {
            return None;
        };
        let node = self.tree.nodes.get(*self.visible.get(row)?)?;
        match &node.entry {
            Entry::File { index } => status.files.get(*index),
            Entry::Folder { .. } => None,
        }
    }

    pub fn selected_path(&self) -> Option<&RepoPath> {
        let Load::Ready(status) = &self.status else {
            return None;
        };
        self.node().map(|node| node.path(&status.files))
    }

    pub fn selected_members(&self) -> &[usize] {
        self.visible
            .get(self.selected)
            .map(|&i| self.tree.members(i))
            .unwrap_or_default()
    }

    pub fn selected_paths(&self) -> Vec<RepoPath> {
        let Load::Ready(status) = &self.status else {
            return Vec::new();
        };
        self.selected_members()
            .iter()
            .flat_map(|&i| {
                let file = &status.files[i];
                std::iter::once(file.path.clone()).chain(file.original_path.clone())
            })
            .collect()
    }

    pub fn toggle_folder(&mut self) -> bool {
        if !self.filter.is_empty() {
            return false;
        }
        let Some(&index) = self.visible.get(self.selected) else {
            return false;
        };
        let Entry::Folder { path, .. } = &self.tree.nodes[index].entry else {
            return false;
        };
        if !self.collapsed_dirs.remove(path) {
            self.collapsed_dirs.insert(path.clone());
        }
        self.visible = self.tree.visible(&self.collapsed_dirs, false);
        self.selected = self.visible.iter().position(|i| *i == index).unwrap_or(0);
        true
    }

    pub fn select_parent(&mut self) {
        if let Some(parent) = self.node().and_then(|node| node.parent)
            && let Some(row) = self.visible.iter().position(|i| *i == parent)
        {
            self.selected = row;
        }
    }

    pub fn rebuild_files(&mut self) {
        let filter = self.filter.to_lowercase();
        if let Load::Ready(status) = &self.status {
            self.matching = status
                .files
                .iter()
                .enumerate()
                .filter(|(i, f)| {
                    f.kind(self.side).is_some()
                        && (filter.is_empty()
                            || self
                                .lower
                                .get(*i)
                                .is_some_and(|text| fuzzy_match(&filter, text)))
                })
                .map(|(i, _)| i)
                .collect();
            self.tree = ChangeTree::build(&status.files, &self.matching);
            let preferred = self.matching.first().map(|&i| &status.files[i].path);
            for node in &self.tree.nodes {
                if let Entry::Folder { path, .. } = &node.entry
                    && self.known_dirs.insert(path.clone())
                    && !preferred.is_some_and(|file| {
                        file.bytes()
                            .strip_prefix(path.bytes())
                            .is_some_and(|rest| rest.first() == Some(&b'/'))
                    })
                {
                    self.collapsed_dirs.insert(path.clone());
                }
            }
            self.visible = self.tree.visible(&self.collapsed_dirs, !filter.is_empty());
        } else {
            self.matching.clear();
            self.tree = ChangeTree::default();
            self.visible.clear();
        }
        self.selected = self.visible.iter().position(|&i| matches!(self.tree.nodes[i].entry, Entry::File { index } if Some(&index) == self.matching.first()))
            .or_else(|| self.visible.iter().position(|&i| !self.tree.nodes[i].is_folder())).unwrap_or(0);
    }

    pub fn set_status(&mut self, status: RepoStatus) {
        let selected = self
            .selected_path()
            .map(|path| (path.clone(), self.node().is_some_and(TreeNode::is_folder)));
        (self.working_count, self.staged_count) =
            status.files.iter().fold((0, 0), |(w, s), file| {
                (
                    w + usize::from(file.worktree.is_some()),
                    s + usize::from(file.staged.is_some()),
                )
            });
        self.lower = status
            .files
            .iter()
            .map(|file| file.path.display().to_lowercase())
            .collect();
        self.status = Load::Ready(status);
        self.rebuild_files();
        if let (Some((path, folder)), Load::Ready(status)) = (selected, &self.status)
            && let Some(index) = self.visible.iter().position(|&i| {
                let node = &self.tree.nodes[i];
                node.path(&status.files) == &path && node.is_folder() == folder
            })
        {
            self.selected = index;
        }
        self.review_staged();
    }

    pub fn remember_diff(&mut self, view: Arc<DiffView>) {
        let fingerprint = view.document.fingerprint.clone();
        self.cache
            .retain(|cached| cached.document.fingerprint != fingerprint);
        self.cache.push_front(view);
        while self.cache.len() > 16 {
            self.cache.pop_back();
        }
    }

    pub fn cached_diff(&self, fingerprint: &str) -> Option<&Arc<DiffView>> {
        self.cache
            .iter()
            .find(|cached| cached.document.fingerprint == fingerprint)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Focus {
    Workspaces,
    Files,
    Diff,
}

pub struct ConnectForm {
    pub provider: Provider,
    pub settings: ProviderSettings,
    pub key: String,
    pub field: usize,
}

pub enum Modal {
    None,
    Help,
    Error(String),
    ConfirmAnalysis(AnalysisReview),
    ConfirmSync {
        action: SyncAction,
        head: Option<String>,
        branch: String,
        upstream: Option<String>,
    },
    Branches {
        entries: Vec<Branch>,
        selected: usize,
    },
    ConfirmBranch(String),
    History {
        entries: Vec<HistoryEntry>,
        selected: usize,
    },
    AddWorkspace(String),
    Providers {
        selected: usize,
    },
    Connect(ConnectForm),
    Busy {
        message: String,
        cancellable: bool,
    },
    Draft {
        draft: CommitDraft,
        editor: TextArea<'static>,
    },
    Plan {
        plan: CommitPlan,
        selected: usize,
        offset: usize,
    },
    PlanEdit {
        plan: CommitPlan,
        selected: usize,
        editor: TextArea<'static>,
    },
    Palette {
        query: String,
        selected: usize,
    },
    /// Theme picker. Moving through the list applies the theme immediately; Esc restores
    /// `previous`, Enter keeps and saves the current one.
    Themes {
        query: String,
        selected: usize,
        previous: &'static Theme,
    },
}

impl Modal {
    pub fn palette() -> Self {
        Self::Palette {
            query: String::new(),
            selected: 0,
        }
    }
}

pub struct App {
    pub workspaces: Vec<WorkspaceView>,
    pub active: usize,
    pub focus: Focus,
    pub modal: Modal,
    pub filtering: bool,
    pub split: bool,
    pub settings: Settings,
    pub notice: String,
    pub last_error: Option<String>,
    pub dirty: bool,
    pub clear: bool,
    pub color_enabled: bool,
    pub theme: &'static Theme,
    pub borders: Borders,
    pub quit: bool,
    pub tick: u64,
    pub now: Instant,
    pub completion: Option<Instant>,
    pub reduced_motion: bool,
}

impl App {
    pub fn new(
        mut workspaces: Vec<Workspace>,
        path: PathBuf,
        side: DiffSide,
        settings: Settings,
    ) -> Self {
        let active = workspaces
            .iter()
            .position(|w| w.root == path)
            .unwrap_or_else(|| {
                workspaces.push(Workspace {
                    name: path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    root: path,
                });
                workspaces.len() - 1
            });
        let theme = settings
            .ui
            .theme
            .as_deref()
            .and_then(Theme::find)
            .unwrap_or(&crate::theme::KIRI);
        let borders = settings
            .ui
            .borders
            .as_deref()
            .and_then(Borders::parse)
            .unwrap_or_default();
        Self {
            workspaces: workspaces
                .into_iter()
                .map(|w| WorkspaceView::new(w, side))
                .collect(),
            active,
            focus: Focus::Files,
            modal: Modal::None,
            filtering: false,
            split: true,
            settings,
            notice: "Local first. AI only runs when you ask.".into(),
            last_error: None,
            dirty: true,
            clear: false,
            color_enabled: true,
            theme,
            borders,
            quit: false,
            tick: 0,
            now: Instant::now(),
            completion: None,
            reduced_motion: false,
        }
    }
    pub fn current(&self) -> &WorkspaceView {
        &self.workspaces[self.active]
    }
    pub fn current_mut(&mut self) -> &mut WorkspaceView {
        &mut self.workspaces[self.active]
    }
    pub fn busy(&self) -> bool {
        matches!(self.modal, Modal::Busy { .. })
    }
    pub fn show_error(&mut self, message: String) {
        let summary: String = message
            .lines()
            .next()
            .unwrap_or("Operation failed")
            .chars()
            .take(120)
            .collect();
        self.notice = format!("{summary} · ! details");
        self.last_error = Some(message.clone());
        self.modal = Modal::Error(message);
    }

    pub fn show_draft(&mut self, draft: CommitDraft) {
        let editor = editor(self.theme, &draft.message);
        self.current_mut().saved_draft = Some(draft.clone());
        self.modal = Modal::Draft { draft, editor };
    }

    pub fn show_plan(&mut self, plan: CommitPlan) {
        self.current_mut().saved_plan = Some(plan.clone());
        self.modal = Modal::Plan {
            plan,
            selected: 0,
            offset: 0,
        };
    }

    /// Whether `draft` was produced for exactly `scope`, so it can be reopened instead of
    /// recaptured.
    pub fn draft_matches(draft: &CommitDraft, scope: &Scope) -> bool {
        draft.snapshot.source() == scope.source() && scope.matches_paths(draft.paths.as_deref())
    }

    pub fn plan_matches(plan: &CommitPlan, scope: &Scope) -> bool {
        plan.snapshot.source() == scope.source()
            && match &scope.paths {
                None => true,
                Some(paths) => {
                    plan.files
                        .iter()
                        .map(|file| &file.path)
                        .collect::<HashSet<_>>()
                        == paths.iter().collect()
                }
            }
    }
}

pub fn editor(theme: &Theme, text: &str) -> TextArea<'static> {
    let mut editor = TextArea::new(text.split('\n').map(str::to_owned).collect());
    editor.set_cursor_line_style(ratatui::style::Style::default());
    editor.set_style(
        ratatui::style::Style::default()
            .fg(theme.text)
            .bg(theme.panel),
    );
    editor.set_cursor_style(
        ratatui::style::Style::default()
            .fg(theme.bg)
            .bg(theme.accent),
    );
    editor
}

pub fn fuzzy_match(query: &str, text: &str) -> bool {
    let mut chars = text.chars();
    query.chars().all(|q| chars.any(|c| c == q))
}

/// Everything the command palette can run. Each entry names the key it mirrors, so the
/// palette and the help panel never disagree with the keymap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    AiCommitSelection,
    AiCommitTab,
    AiPlanSelection,
    AiPlanVisible,
    WriteMessage,
    Stage,
    StageAll,
    StageHunk,
    StagedTab,
    WorkingTab,
    ShowError,
    Providers,
    Workspace,
    Fetch,
    Pull,
    Push,
    Branches,
    History,
    Themes,
    NextTheme,
    PreviousTheme,
    CycleBorders,
    ToggleColor,
    ToggleSplit,
    Refresh,
    Help,
    Quit,
}

pub const COMMANDS: &[(Command, &str, &str)] = &[
    (
        Command::AiCommitSelection,
        "AI: commit selected file or folder",
        "a",
    ),
    (
        Command::AiCommitTab,
        "AI: commit everything on this tab",
        "A",
    ),
    (
        Command::AiPlanSelection,
        "AI: plan commits from selected changes",
        "b",
    ),
    (
        Command::AiPlanVisible,
        "AI: plan commits from all visible changes",
        "Ctrl+B",
    ),
    (
        Command::WriteMessage,
        "Write a commit message for the selection",
        "c",
    ),
    (
        Command::Stage,
        "Stage / unstage selected file or folder",
        "Space",
    ),
    (Command::StageAll, "Stage / unstage everything shown", "S"),
    (Command::StageHunk, "Stage / unstage the current hunk", "H"),
    (Command::StagedTab, "Show staged changes", "s"),
    (Command::WorkingTab, "Show working changes", "u"),
    (Command::Themes, "Theme picker", "T"),
    (Command::NextTheme, "Next theme", ""),
    (Command::PreviousTheme, "Previous theme", ""),
    (Command::CycleBorders, "Cycle panel border style", ""),
    (Command::ToggleColor, "Toggle terminal colors", "t"),
    (Command::ToggleSplit, "Toggle split diff", "v"),
    (Command::ShowError, "Show the last error", "!"),
    (Command::Providers, "Connect or select AI provider", "P"),
    (Command::Workspace, "Open another workspace", "w"),
    (Command::Fetch, "Fetch remote updates", "f"),
    (Command::Pull, "Pull incoming commits", "d"),
    (Command::Push, "Push outgoing commits", "U"),
    (Command::Branches, "Switch branch", "B"),
    (Command::History, "Recent commit history", "l"),
    (Command::Refresh, "Refresh repository", "r"),
    (Command::Help, "Keyboard shortcuts", "?"),
    (Command::Quit, "Quit", "q"),
];

pub fn palette_matches(query: &str) -> Vec<&'static (Command, &'static str, &'static str)> {
    let query = query.to_lowercase();
    COMMANDS
        .iter()
        .filter(|(_, name, _)| fuzzy_match(&query, &name.to_lowercase()))
        .collect()
}

pub fn theme_matches(query: &str) -> Vec<&'static Theme> {
    let query = query.to_lowercase();
    Theme::all()
        .iter()
        .filter(|theme| {
            fuzzy_match(&query, &theme.name.to_lowercase()) || fuzzy_match(&query, theme.id)
        })
        .collect()
}
