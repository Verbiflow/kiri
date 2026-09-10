use crate::{
    state::{ACTIONS, App, ConnectForm, Focus, Load, Modal, editor, fuzzy_match},
    view::{geometry, list_start},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use kiri_ai::{
    config::{Provider, ProviderSettings},
    workflow::{CommitDraft, CommitPlan},
};
use kiri_core::{model::DiffSide, sync::SyncAction, tree::Entry};
use ratatui::layout::Rect;
use std::path::PathBuf;

#[cfg(test)]
mod tests;

pub enum Action {
    None,
    Refresh,
    ToggleColor,
    LoadDiff {
        reset: bool,
        large: bool,
    },
    Switch(usize),
    StageFile,
    StageHunk,
    StageMany {
        paths: Vec<kiri_core::model::RepoPath>,
        side: DiffSide,
    },
    Draft {
        ai: bool,
        paths: Option<Vec<kiri_core::model::RepoPath>>,
    },
    Plan {
        paths: Option<Vec<kiri_core::model::RepoPath>>,
    },
    Analyze,
    Commit(CommitDraft),
    Apply(CommitPlan),
    Connect(ConnectForm),
    Login,
    AddWorkspace(PathBuf),
    Cancel,
    Sync {
        action: SyncAction,
        target: Option<kiri_core::sync::SyncTarget>,
    },
    Branches,
    SwitchBranch(String),
    History,
}

pub fn key(app: &mut App, event: KeyEvent, area: Rect) -> Action {
    app.dirty = true;
    if geometry(area).workspaces.width == 0 && app.focus == Focus::Workspaces {
        app.focus = Focus::Files;
    }
    if !matches!(app.modal, Modal::None) {
        return modal_key(app, event);
    }
    if app.filtering {
        match event.code {
            KeyCode::Esc => {
                app.current_mut().filter.clear();
                app.filtering = false;
            }
            KeyCode::Enter => {
                app.filtering = false;
                return Action::None;
            }
            KeyCode::Backspace => {
                app.current_mut().filter.pop();
            }
            KeyCode::Char(c) if !event.modifiers.contains(KeyModifiers::CONTROL) => {
                app.current_mut().filter.push(c);
            }
            _ => return Action::None,
        }
        app.current_mut().selected = 0;
        app.current_mut().rebuild_files();
        return Action::LoadDiff {
            reset: true,
            large: false,
        };
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        match event.code {
            KeyCode::Right => return Action::Switch((app.active + 1) % app.workspaces.len()),
            KeyCode::Left => {
                return Action::Switch(
                    (app.active + app.workspaces.len() - 1) % app.workspaces.len(),
                );
            }
            _ => {}
        }
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        match event.code {
            KeyCode::Char('c') => {
                app.quit = true;
                return Action::None;
            }
            KeyCode::Char('p') => {
                app.modal = Modal::Palette {
                    query: String::new(),
                    selected: 0,
                };
                return Action::None;
            }
            KeyCode::Char('d') => return move_selection(app, 15, area),
            KeyCode::Char('u') => return move_selection(app, -15, area),
            _ => {}
        }
    }
    match event.code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('?') => app.modal = Modal::Help,
        KeyCode::Char('P' | 'p') => {
            app.modal = Modal::Providers {
                selected: app
                    .settings
                    .active
                    .and_then(|p| Provider::ALL.iter().position(|item| *item == p))
                    .unwrap_or(0),
            }
        }
        KeyCode::Char('w') => app.modal = Modal::AddWorkspace(String::new()),
        KeyCode::Char('r') => return Action::Refresh,
        KeyCode::Char('t') => return Action::ToggleColor,
        KeyCode::Char('f') => {
            return Action::Sync {
                action: SyncAction::Fetch,
                target: None,
            };
        }
        KeyCode::Char('B') => return Action::Branches,
        KeyCode::Char('l') => return Action::History,
        KeyCode::Char('d' | 'U') => {
            if let Load::Ready(status) = &app.current().status {
                app.modal = Modal::ConfirmSync {
                    action: if event.code == KeyCode::Char('d') {
                        SyncAction::Pull
                    } else {
                        SyncAction::Push
                    },
                    head: status.head.clone(),
                    branch: status.branch.clone(),
                    upstream: status.upstream.clone(),
                };
            }
        }
        KeyCode::Char('a' | 'A') => {
            if event.code == KeyCode::Char('a') && app.current().draft_scope().is_none() {
                app.notice =
                    "Press s, select a staged file or folder, then a. A drafts all staged files."
                        .into();
                return Action::None;
            }
            return Action::Draft {
                ai: true,
                paths: if event.code == KeyCode::Char('A') {
                    None
                } else {
                    app.current().draft_scope()
                },
            };
        }
        KeyCode::Char('!') => {
            if let Some(message) = &app.last_error {
                app.modal = Modal::Error(message.clone());
            }
        }
        KeyCode::Char('c') => {
            let paths = app.current().draft_scope();
            if let Some(draft) = app.current().saved_draft.clone()
                && paths.as_ref().is_none_or(|paths| {
                    let expected: std::collections::HashSet<_> = paths.iter().collect();
                    draft.paths.as_ref().is_some_and(|actual| {
                        actual.iter().collect::<std::collections::HashSet<_>>() == expected
                    })
                })
            {
                app.show_draft(draft);
            } else {
                return Action::Draft { ai: false, paths };
            }
        }
        KeyCode::Char('b') => {
            let paths = app.current().draft_scope();
            if let Some(plan) = app.current().saved_plan.clone()
                && paths.as_ref().is_none_or(|paths| {
                    plan.files
                        .iter()
                        .map(|file| &file.path)
                        .collect::<std::collections::HashSet<_>>()
                        == paths.iter().collect()
                })
            {
                app.modal = Modal::Plan {
                    plan,
                    selected: 0,
                    offset: 0,
                    confirming: false,
                };
            } else {
                return Action::Plan { paths };
            }
        }
        KeyCode::Char(' ') => {
            let workspace = app.current();
            if let Some(node) = workspace.node()
                && let Entry::Folder { path, .. } = &node.entry
            {
                app.modal = Modal::ConfirmStage {
                    paths: workspace.selected_paths(),
                    side: workspace.side,
                    label: format!("listed changes in {path}/"),
                };
            } else {
                return Action::StageFile;
            }
        }
        KeyCode::Char('S') => {
            let workspace = app.current();
            if let Load::Ready(status) = &workspace.status {
                let paths: Vec<_> = workspace
                    .matching
                    .iter()
                    .flat_map(|i| {
                        let file = &status.files[*i];
                        std::iter::once(file.path.clone()).chain(file.original_path.clone())
                    })
                    .collect();
                if !paths.is_empty() {
                    app.modal = Modal::ConfirmStage {
                        paths,
                        side: workspace.side,
                        label: "current filtered list".into(),
                    };
                }
            }
        }
        KeyCode::Char('H') => return Action::StageHunk,
        KeyCode::Char('L') => {
            return Action::LoadDiff {
                reset: false,
                large: true,
            };
        }
        KeyCode::Char('s' | 'u') => {
            app.current_mut().side = if event.code == KeyCode::Char('s') {
                DiffSide::Staged
            } else {
                DiffSide::Worktree
            };
            app.current_mut().selected = 0;
            app.current_mut().rebuild_files();
            app.current_mut().review_staged();
            return Action::LoadDiff {
                reset: true,
                large: false,
            };
        }
        KeyCode::Char('/') => app.filtering = true,
        KeyCode::Char('v') => {
            app.split = !app.split;
            app.current_mut().scroll = 0;
        }
        KeyCode::Tab | KeyCode::BackTab => {
            if geometry(area).workspaces.width == 0 {
                app.focus = if app.focus == Focus::Files {
                    Focus::Diff
                } else {
                    Focus::Files
                };
                return Action::None;
            }
            app.focus = match (app.focus, event.code) {
                (Focus::Workspaces, KeyCode::BackTab) | (Focus::Files, KeyCode::Tab) => Focus::Diff,
                (Focus::Diff, KeyCode::BackTab) | (Focus::Workspaces, KeyCode::Tab) => Focus::Files,
                _ => Focus::Workspaces,
            };
        }
        KeyCode::Enter
            if app.focus == Focus::Files
                && app.current().node().is_some_and(|node| node.is_folder()) =>
        {
            if !app.current_mut().toggle_folder() {
                app.notice = "Search expands folders. Esc clears the filter.".into();
            }
            return Action::LoadDiff {
                reset: true,
                large: false,
            };
        }
        KeyCode::Enter => app.focus = Focus::Diff,
        KeyCode::Esc => {
            if !app.current().filter.is_empty() {
                app.current_mut().filter.clear();
                app.current_mut().rebuild_files();
                return Action::LoadDiff {
                    reset: true,
                    large: false,
                };
            }
            app.focus = Focus::Files;
        }
        KeyCode::Char('j') | KeyCode::Down => return move_selection(app, 1, area),
        KeyCode::Char('k') | KeyCode::Up => return move_selection(app, -1, area),
        KeyCode::PageDown => {
            return move_selection(app, area.height.saturating_sub(12) as isize, area);
        }
        KeyCode::PageUp => {
            return move_selection(app, -(area.height.saturating_sub(12) as isize), area);
        }
        KeyCode::Home | KeyCode::Char('g') => return move_selection(app, -1_000_000, area),
        KeyCode::End | KeyCode::Char('G') => return move_selection(app, 1_000_000, area),
        KeyCode::Left if app.focus == Focus::Files => {
            let collapse = app.current().node().is_some_and(|node| match &node.entry {
                Entry::Folder { path, .. } => {
                    !app.current().collapsed_dirs.contains(path) && app.current().filter.is_empty()
                }
                Entry::File { .. } => false,
            });
            if collapse {
                app.current_mut().toggle_folder();
            } else {
                app.current_mut().select_parent();
            }
            return Action::LoadDiff {
                reset: true,
                large: false,
            };
        }
        KeyCode::Right if app.focus == Focus::Files => {
            if let Some(node) = app.current().node()
                && let Entry::Folder { path, .. } = &node.entry
            {
                if app.current().collapsed_dirs.contains(path) && app.current().filter.is_empty() {
                    app.current_mut().toggle_folder();
                } else {
                    app.current_mut().selected = (app.current().selected + 1)
                        .min(app.current().visible.len().saturating_sub(1));
                }
                return Action::LoadDiff {
                    reset: true,
                    large: false,
                };
            }
            app.focus = Focus::Diff;
        }
        KeyCode::Left => app.current_mut().horizontal = app.current().horizontal.saturating_sub(8),
        KeyCode::Right => app.current_mut().horizontal = (app.current().horizontal + 8).min(16000),
        KeyCode::Char('[' | ']') => {
            let split = app.split && geometry(area).diff.width >= 76;
            let workspace = app.current_mut();
            if let Load::Ready(view) = &workspace.diff {
                workspace.hunk = if event.code == KeyCode::Char(']') {
                    (workspace.hunk + 1).min(view.document.hunks.len().saturating_sub(1))
                } else {
                    workspace.hunk.saturating_sub(1)
                };
                if let Some(hunk) = view.document.hunks.get(workspace.hunk) {
                    workspace.scroll = if split {
                        view.pairs
                            .iter()
                            .position(|(left, right)| {
                                *left == Some(hunk.lines.start) || *right == Some(hunk.lines.start)
                            })
                            .unwrap_or(0)
                    } else {
                        hunk.lines.start
                    };
                }
            }
            app.focus = Focus::Diff;
        }
        KeyCode::Char(c @ '1'..='9') => {
            let index = c as usize - '1' as usize;
            if index < app.workspaces.len() {
                return Action::Switch(index);
            }
        }
        _ => {}
    }
    Action::None
}

fn modal_key(app: &mut App, event: KeyEvent) -> Action {
    let mut modal = std::mem::replace(&mut app.modal, Modal::None);
    let ctrl_enter = matches!(event.code, KeyCode::Enter | KeyCode::Char('s'))
        && event.modifiers.contains(KeyModifiers::CONTROL);
    match &mut modal {
        Modal::Busy { cancellable, .. } => {
            if event.code == KeyCode::Esc && *cancellable {
                return Action::Cancel;
            }
        }
        Modal::ConfirmAnalysis(review) => {
            if matches!(event.code, KeyCode::Char('f') | KeyCode::Char('d')) {
                review.mode = if event.code == KeyCode::Char('d') {
                    kiri_ai::analysis::AnalysisMode::Deep
                } else {
                    kiri_ai::analysis::AnalysisMode::Fast
                };
                app.settings.analysis.mode = review.mode;
            }
            if event.code == KeyCode::Esc {
                return Action::Cancel;
            }
            if event.code == KeyCode::Enter {
                return Action::Analyze;
            }
        }
        Modal::Help | Modal::Error(_) => {
            if matches!(
                event.code,
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                return Action::None;
            }
        }
        Modal::ConfirmSync {
            action,
            head,
            branch,
            ..
        } => {
            if event.code == KeyCode::Esc {
                return Action::None;
            }
            if event.code == KeyCode::Enter {
                return Action::Sync {
                    action: *action,
                    target: Some(kiri_core::sync::SyncTarget {
                        head: head.clone(),
                        branch: branch.clone(),
                    }),
                };
            }
        }
        Modal::Branches { entries, selected } => match event.code {
            KeyCode::Esc => return Action::None,
            KeyCode::Char('j') | KeyCode::Down => {
                *selected = (*selected + 1).min(entries.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(entry) = entries.get(*selected) {
                    modal = Modal::ConfirmBranch(entry.name.clone());
                }
            }
            _ => {}
        },
        Modal::ConfirmStage { paths, side, .. } => {
            if event.code == KeyCode::Esc {
                return Action::None;
            }
            if event.code == KeyCode::Enter {
                return Action::StageMany {
                    paths: paths.clone(),
                    side: *side,
                };
            }
        }
        Modal::ConfirmBranch(name) => {
            if event.code == KeyCode::Esc {
                return Action::None;
            }
            if event.code == KeyCode::Enter {
                return Action::SwitchBranch(name.clone());
            }
        }
        Modal::History { entries, selected } => match event.code {
            KeyCode::Esc => return Action::None,
            KeyCode::Char('j') | KeyCode::Down => {
                *selected = (*selected + 1).min(entries.len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::PageDown => *selected = (*selected + 12).min(entries.len().saturating_sub(1)),
            KeyCode::PageUp => *selected = selected.saturating_sub(12),
            _ => {}
        },
        Modal::AddWorkspace(input) => {
            if event.code == KeyCode::Esc {
                return Action::None;
            }
            if event.code == KeyCode::Enter && !input.is_empty() {
                let path = if let Some(suffix) = input.strip_prefix("~/") {
                    std::env::var_os("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_default()
                        .join(suffix)
                } else {
                    PathBuf::from(&*input)
                };
                return Action::AddWorkspace(path);
            }
            text_input(input, event);
        }
        Modal::Providers { selected } => {
            if event.code == KeyCode::Esc {
                return Action::None;
            }
            match event.code {
                KeyCode::Char('j') | KeyCode::Down => *selected = (*selected + 1).min(6),
                KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Enter | KeyCode::Char('e') => {
                    let provider = Provider::ALL[*selected];
                    if provider == Provider::Codex
                        && !app.settings.providers.contains_key(&provider)
                    {
                        return Action::Login;
                    }
                    let settings = app
                        .settings
                        .providers
                        .get(&provider)
                        .cloned()
                        .unwrap_or_else(|| ProviderSettings::defaults(provider));
                    if event.code == KeyCode::Enter
                        && app.settings.providers.contains_key(&provider)
                    {
                        return Action::Connect(ConnectForm {
                            provider,
                            settings,
                            key: String::new(),
                            field: 0,
                        });
                    }
                    modal = Modal::Connect(ConnectForm {
                        provider,
                        settings,
                        key: String::new(),
                        field: 3,
                    });
                }
                _ => {}
            }
        }
        Modal::Connect(form) => {
            if event.code == KeyCode::Esc {
                app.modal = Modal::Providers { selected: 0 };
                return Action::None;
            }
            match event.code {
                KeyCode::Tab | KeyCode::Down => form.field = (form.field + 1) % 4,
                KeyCode::BackTab | KeyCode::Up => form.field = (form.field + 3) % 4,
                KeyCode::Enter => {
                    if let Modal::Connect(form) = modal {
                        return Action::Connect(form);
                    }
                }
                _ => match form.field {
                    0 => text_input(&mut form.settings.model, event),
                    1 => text_input(
                        form.settings.endpoint.get_or_insert_with(String::new),
                        event,
                    ),
                    2 => text_input(form.settings.region.get_or_insert_with(String::new), event),
                    _ => text_input(&mut form.key, event),
                },
            }
        }
        Modal::Draft { draft, editor } => {
            if ctrl_enter || event.code == KeyCode::Esc {
                draft.message = editor.lines().join("\n");
                app.current_mut().saved_draft = Some(draft.clone());
                return if ctrl_enter {
                    Action::Commit(draft.clone())
                } else {
                    Action::None
                };
            }
            editor.input(event);
        }
        Modal::Plan {
            plan,
            selected,
            offset,
            confirming,
        } => {
            if *confirming {
                if event.code == KeyCode::Enter {
                    return Action::Apply(plan.clone());
                }
                if event.code == KeyCode::Esc {
                    *confirming = false;
                }
            } else {
                match event.code {
                    KeyCode::Esc => {
                        app.current_mut().saved_plan = Some(plan.clone());
                        return Action::None;
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        *selected = (*selected + 1).min(plan.groups.len().saturating_sub(1));
                        *offset = 0;
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        *offset = 0;
                    }
                    KeyCode::PageDown => {
                        *offset = (*offset + 10).min(plan.files.len() + plan.groups.len() * 4)
                    }
                    KeyCode::PageUp => *offset = offset.saturating_sub(10),
                    KeyCode::Char('e') => {
                        let edit = editor(&plan.groups[*selected].message);
                        modal = Modal::PlanEdit {
                            plan: plan.clone(),
                            selected: *selected,
                            editor: edit,
                        };
                    }
                    _ if ctrl_enter => *confirming = true,
                    _ => {}
                }
            }
        }
        Modal::PlanEdit {
            plan,
            selected,
            editor,
        } => {
            if ctrl_enter || event.code == KeyCode::Esc {
                if ctrl_enter {
                    plan.groups[*selected].message = editor.lines().join("\n");
                }
                modal = Modal::Plan {
                    plan: plan.clone(),
                    selected: *selected,
                    offset: 0,
                    confirming: false,
                };
            } else {
                editor.input(event);
            }
        }
        Modal::Palette { query, selected } => {
            if event.code == KeyCode::Esc {
                return Action::None;
            }
            let filtered: Vec<_> = ACTIONS
                .iter()
                .filter(|(name, _)| fuzzy_match(&query.to_lowercase(), &name.to_lowercase()))
                .collect();
            match event.code {
                KeyCode::Down => *selected = (*selected + 1).min(filtered.len().saturating_sub(1)),
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Enter => {
                    if let Some((_, shortcut)) = filtered.get(*selected)
                        && let Some(c) = shortcut.chars().next()
                    {
                        return key(
                            app,
                            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                            Rect::new(0, 0, 140, 40),
                        );
                    }
                }
                _ => {
                    text_input(query, event);
                    *selected = 0;
                }
            }
        }
        Modal::None => {}
    }
    app.modal = modal;
    Action::None
}

pub fn text_input(text: &mut String, event: KeyEvent) {
    match event.code {
        KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Char(c)
            if !event.modifiers.contains(KeyModifiers::CONTROL)
                && !c.is_control()
                && text.len() < 16384 =>
        {
            text.push(c)
        }
        KeyCode::Backspace => {
            text.pop();
        }
        _ => {}
    }
}

fn move_selection(app: &mut App, delta: isize, area: Rect) -> Action {
    match app.focus {
        Focus::Workspaces => {
            let index = app
                .active
                .saturating_add_signed(delta)
                .min(app.workspaces.len().saturating_sub(1));
            return Action::Switch(index);
        }
        Focus::Files => {
            let workspace = app.current_mut();
            workspace.selected = workspace
                .selected
                .saturating_add_signed(delta)
                .min(workspace.visible.len().saturating_sub(1));
            return Action::LoadDiff {
                reset: true,
                large: false,
            };
        }
        Focus::Diff => {
            let split = app.split && geometry(area).diff.width >= 76;
            let workspace = app.current_mut();
            if let Load::Ready(view) = &workspace.diff {
                let count = if split {
                    view.pairs.len()
                } else {
                    view.document.lines.len()
                };
                workspace.scroll = workspace
                    .scroll
                    .saturating_add_signed(delta)
                    .min(count.saturating_sub(1));
                let row = if split {
                    view.pairs
                        .get(workspace.scroll)
                        .and_then(|(a, b)| a.or(*b))
                        .unwrap_or(0)
                } else {
                    workspace.scroll
                };
                workspace.hunk = view
                    .document
                    .hunks
                    .iter()
                    .rposition(|h| h.lines.start <= row)
                    .unwrap_or(0);
            }
        }
    }
    Action::None
}

pub fn mouse(app: &mut App, event: MouseEvent, area: Rect) -> Action {
    let point = ratatui::layout::Position::new(event.column, event.row);
    if matches!(app.modal, Modal::ConfirmAnalysis(_))
        && event.kind == MouseEventKind::Down(MouseButton::Left)
    {
        let controls = crate::modals::analysis_controls(area);
        for (rect, code) in [
            (controls.fast, KeyCode::Char('f')),
            (controls.deep, KeyCode::Char('d')),
            (controls.confirm, KeyCode::Enter),
            (controls.cancel, KeyCode::Esc),
        ] {
            if rect.contains(point) {
                return key(app, KeyEvent::new(code, KeyModifiers::NONE), area);
            }
        }
    }
    if matches!(
        app.modal,
        Modal::ConfirmSync { .. } | Modal::ConfirmBranch(_) | Modal::ConfirmStage { .. }
    ) && event.kind == MouseEventKind::Down(MouseButton::Left)
    {
        let (confirm, cancel) = crate::modals::confirmation_buttons(area);
        if confirm.contains(point) {
            return key(app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), area);
        }
        if cancel.contains(point) {
            return key(app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), area);
        }
    }
    if !matches!(app.modal, Modal::None) {
        return Action::None;
    }
    if event.kind == MouseEventKind::Down(MouseButton::Left)
        && let Some((_, shortcut)) = crate::view::sync_buttons()
            .into_iter()
            .find(|(rect, _)| rect.right() <= area.right() && rect.contains(point))
    {
        return key(
            app,
            KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::NONE),
            area,
        );
    }
    let geo = geometry(area);
    if event.kind == MouseEventKind::Down(MouseButton::Left) {
        if let Some((_, shortcut)) = crate::file_tree::tabs(geo.files)
            .into_iter()
            .find(|(rect, _)| rect.contains(point))
        {
            app.focus = Focus::Files;
            return key(
                app,
                KeyEvent::new(KeyCode::Char(shortcut), KeyModifiers::NONE),
                area,
            );
        }
        if app.current().node().is_some_and(|node| node.is_folder())
            && let Some((_, shortcut)) =
                crate::file_tree::folder_buttons(geo.diff, app.current().side)
                    .into_iter()
                    .find(|(rect, _)| rect.contains(point))
        {
            app.focus = Focus::Files;
            return key(
                app,
                KeyEvent::new(
                    if shortcut == '\n' {
                        KeyCode::Enter
                    } else {
                        KeyCode::Char(shortcut)
                    },
                    KeyModifiers::NONE,
                ),
                area,
            );
        }
        if geo.files.contains(point) && event.row == geo.files.y + 2 {
            app.focus = Focus::Files;
            app.filtering = true;
            app.dirty = true;
            return Action::None;
        }
    }
    let focus = if geo.workspaces.contains(point) {
        Focus::Workspaces
    } else if geo.files.contains(point) {
        Focus::Files
    } else if geo.diff.contains(point) {
        Focus::Diff
    } else {
        return Action::None;
    };
    app.focus = focus;
    app.dirty = true;
    match event.kind {
        MouseEventKind::ScrollDown => move_selection(app, 3, area),
        MouseEventKind::ScrollUp => move_selection(app, -3, area),
        MouseEventKind::Down(MouseButton::Left) if focus == Focus::Files => {
            let rows = crate::file_tree::rows_area(geo.files);
            if !rows.contains(point) {
                return Action::None;
            }
            let start = list_start(app.current().selected, rows.height as usize);
            let selected = start + (event.row - rows.y) as usize;
            if selected >= app.current().visible.len() {
                return Action::None;
            }
            app.current_mut().selected = selected;
            if event.column >= rows.right().saturating_sub(4) {
                return key(
                    app,
                    KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
                    area,
                );
            }
            if let Some(node) = app.current().node()
                && node.is_folder()
                && event.column >= rows.x + 2 + node.depth.min(12) as u16 * 2
                && event.column < rows.x + 4 + node.depth.min(12) as u16 * 2
            {
                app.current_mut().toggle_folder();
            }
            Action::LoadDiff {
                reset: true,
                large: false,
            }
        }
        MouseEventKind::Down(MouseButton::Left) if focus == Focus::Workspaces => {
            let start = app
                .active
                .saturating_sub(geo.workspaces.height.saturating_sub(8) as usize);
            let index = start + event.row.saturating_sub(geo.workspaces.y + 1) as usize;
            if index < app.workspaces.len() {
                Action::Switch(index)
            } else {
                Action::None
            }
        }
        _ => Action::None,
    }
}
