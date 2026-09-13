use super::*;
use anyhow::Result;
use kiri_ai::config::Settings;
use kiri_core::{
    commit::StagedSnapshot,
    model::{ChangeKind, FileChange, RepoPath, RepoStatus},
};

fn app() -> App {
    App::new(
        Vec::new(),
        PathBuf::from("/project"),
        DiffSide::Worktree,
        Settings::default(),
    )
}

#[test]
fn clicking_incoming_and_confirming_uses_the_reviewed_head() {
    let mut app = app();
    app.current_mut().set_status(RepoStatus {
        branch: "main".into(),
        head: Some("a".repeat(40)),
        upstream: Some("origin/main".into()),
        behind: 1,
        ..RepoStatus::default()
    });
    let area = Rect::new(0, 0, 160, 42);
    let click = |x, y| MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(matches!(mouse(&mut app, click(20, 2), area), Action::None));
    assert!(matches!(
        &app.modal,
        Modal::ConfirmSync {
            action: SyncAction::Pull,
            ..
        }
    ));
    let button = crate::modals::confirmation_buttons(area).0;
    let action = mouse(&mut app, click(button.x + 1, button.y), area);
    assert!(
        matches!(action, Action::Sync { action: SyncAction::Pull, target: Some(ref target) } if target.head == Some("a".repeat(40)) && target.branch == "main")
    );
}

#[test]
fn bulk_stage_respects_the_filter_and_runs_immediately() -> Result<()> {
    let mut app = app();
    let files = ["src/api.rs", "tests/api.rs", "other.rs"]
        .iter()
        .map(|path| {
            Ok(FileChange {
                path: RepoPath::new(path.as_bytes().to_vec())?,
                original_path: None,
                staged: None,
                worktree: Some(ChangeKind::Modified),
                submodule: false,
                head_oid: None,
                index_oid: None,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    app.current_mut().set_status(RepoStatus {
        files,
        ..RepoStatus::default()
    });
    app.current_mut().filter = "api".into();
    app.current_mut().rebuild_files();
    let area = Rect::new(0, 0, 160, 42);
    let result = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        area,
    );
    assert!(
        matches!(result, Action::StageMany { ref paths, side: DiffSide::Worktree } if paths.len() == 2)
    );
    Ok(())
}

#[test]
fn commit_shortcut_is_available_without_extended_keyboard_protocol() {
    let mut app = app();
    let draft = CommitDraft {
        repository: PathBuf::from("/project"),
        snapshot: StagedSnapshot {
            head: None,
            head_ref: None,
            tree: "a".repeat(40),
            index_digest: "b".repeat(64),
        }
        .into(),
        message: "fix: preserve files".into(),
        warnings: Vec::new(),
        paths: None,
        analysis: None,
    };
    app.show_draft(draft);
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        Rect::new(0, 0, 160, 42),
    );
    assert!(matches!(action, Action::Commit(ref draft) if draft.message == "fix: preserve files"));
    assert!(app.current().saved_draft.is_some());
}

#[test]
fn folder_navigation_and_space_include_collapsed_descendants() -> Result<()> {
    let mut app = app();
    let files = ["src/a.rs", "src/deep/b.rs", "src-other/c.rs"]
        .into_iter()
        .map(|name| {
            Ok(FileChange {
                path: RepoPath::new(name.as_bytes().to_vec())?,
                original_path: None,
                staged: None,
                worktree: Some(ChangeKind::Modified),
                submodule: false,
                head_oid: None,
                index_oid: None,
            })
        })
        .collect::<Result<_>>()?;
    app.current_mut().set_status(RepoStatus {
        files,
        ..RepoStatus::default()
    });
    let area = Rect::new(0, 0, 160, 42);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        area,
    );
    assert_eq!(
        app.current().selected_path().map(RepoPath::bytes),
        Some(b"src".as_slice())
    );
    key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        area,
    );
    assert!(
        app.current()
            .collapsed_dirs
            .contains(&RepoPath::new(b"src".to_vec())?)
    );
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
        area,
    );
    assert!(matches!(
        action,
        Action::StageMany { ref paths, side: DiffSide::Worktree }
            if paths.len() == 2 && paths.iter().all(|path| path.bytes().starts_with(b"src/"))
    ));
    Ok(())
}

#[test]
fn folder_checkbox_and_inspector_are_mouse_actions() -> Result<()> {
    let mut app = app();
    app.current_mut().set_status(RepoStatus {
        files: vec![FileChange {
            path: RepoPath::new(b"folder/file.rs".to_vec())?,
            original_path: None,
            staged: None,
            worktree: Some(ChangeKind::Modified),
            submodule: false,
            head_oid: None,
            index_oid: None,
        }],
        ..RepoStatus::default()
    });
    let area = Rect::new(0, 0, 160, 42);
    let rows = crate::file_tree::rows_area(geometry(area).files);
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rows.right() - 3,
        row: rows.y,
        modifiers: KeyModifiers::NONE,
    };
    let action = mouse(&mut app, click, area);
    assert!(matches!(action, Action::StageMany { ref paths, .. } if paths.len() == 1));
    let button = crate::file_tree::folder_buttons(geometry(area).diff, app.current().side)[0].0;
    let action = mouse(
        &mut app,
        MouseEvent {
            column: button.x + 1,
            row: button.y,
            ..click
        },
        area,
    );
    assert!(matches!(action, Action::StageMany { ref paths, .. } if paths.len() == 1));
    Ok(())
}

#[test]
fn staged_selection_drafts_only_its_paths_and_all_is_explicit() -> Result<()> {
    let mut app = app();
    let files = ["older/file.rs", "selected/file.rs"]
        .into_iter()
        .map(|path| {
            Ok(FileChange {
                path: RepoPath::new(path.as_bytes().to_vec())?,
                original_path: None,
                staged: Some(ChangeKind::Modified),
                worktree: None,
                submodule: false,
                head_oid: None,
                index_oid: None,
            })
        })
        .collect::<Result<_>>()?;
    app.current_mut().side = DiffSide::Staged;
    app.current_mut().pending_review = Some(crate::state::TreeSelection::Folder(RepoPath::new(
        b"selected".to_vec(),
    )?));
    app.current_mut().set_status(RepoStatus {
        files,
        ..RepoStatus::default()
    });
    assert_eq!(
        app.current().selected_path().map(RepoPath::bytes),
        Some(b"selected".as_slice())
    );
    let area = Rect::new(0, 0, 160, 42);
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        area,
    );
    assert!(
        matches!(action, Action::Draft { ai: true, scope } if scope.side == DiffSide::Staged && matches!(scope.paths, Some(ref paths) if paths.len() == 1 && paths[0].bytes() == b"selected/file.rs"))
    );
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE),
        area,
    );
    assert!(matches!(
        action,
        Action::Draft {
            ai: true,
            scope: crate::state::Scope {
                side: DiffSide::Staged,
                paths: None
            }
        }
    ));
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
        area,
    );
    assert!(
        matches!(action, Action::Draft { ai: false, scope } if scope.side == DiffSide::Staged && matches!(scope.paths, Some(ref paths) if paths.len() == 1))
    );
    let selected_plan = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        area,
    );
    assert!(
        matches!(selected_plan, Action::Plan { scope } if scope.side == DiffSide::Staged && matches!(scope.paths, Some(ref paths) if paths.len() == 1 && paths[0].bytes() == b"selected/file.rs"))
    );
    let visible_plan = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
        area,
    );
    assert!(matches!(
        visible_plan,
        Action::Plan {
            scope: crate::state::Scope {
                side: DiffSide::Staged,
                paths: None
            }
        }
    ));
    Ok(())
}

#[test]
fn errors_remain_readable_after_closing_the_dialog() {
    let mut app = app();
    let area = Rect::new(0, 0, 160, 42);
    app.show_error("Provider returned HTTP 401. Reconnect the provider.".into());
    key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        area,
    );
    assert!(matches!(app.modal, Modal::None));
    assert!(app.notice.contains("401"));
    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        area,
    );
    assert!(matches!(app.modal, Modal::Error(ref error) if error.contains("401")));
}

#[test]
fn large_analysis_requires_explicit_approval() {
    let mut app = app();
    let area = Rect::new(0, 0, 160, 42);
    app.modal = Modal::ConfirmAnalysis(crate::state::AnalysisReview {
        mode: kiri_ai::analysis::AnalysisMode::Fast,
        inspection_rounds: 6,
        files: 9000,
        bytes: 1000000,
        chunks: 20,
        calls: 25,
        max_calls: 4096,
        concurrency: 8,
        provider: "test".into(),
    });
    assert!(matches!(
        key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
            area
        ),
        Action::None
    ));
    assert_eq!(
        app.settings.analysis.mode,
        kiri_ai::analysis::AnalysisMode::Deep
    );
    if let Modal::ConfirmAnalysis(review) = &app.modal {
        assert_eq!(review.estimated_calls(), 31);
    }
    let fast = crate::modals::analysis_controls(area).fast;
    mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: fast.x,
            row: fast.y,
            modifiers: KeyModifiers::NONE,
        },
        area,
    );
    assert_eq!(
        app.settings.analysis.mode,
        kiri_ai::analysis::AnalysisMode::Fast
    );
    assert!(matches!(
        key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            area
        ),
        Action::Analyze
    ));
}

#[test]
fn narrow_layout_never_focuses_an_invisible_workspace_pane() {
    let mut app = app();
    let area = Rect::new(0, 0, 110, 35);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        area,
    );
    assert!(app.focus == Focus::Diff);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        area,
    );
    assert!(app.focus == Focus::Files);
    app.focus = Focus::Workspaces;
    key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        area,
    );
    assert!(app.focus == Focus::Files);
}

#[test]
fn quit_does_not_interrupt_a_git_mutation() {
    let mut app = app();
    app.modal = Modal::Busy {
        message: "Committing".into(),
        cancellable: false,
    };
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        Rect::new(0, 0, 160, 42),
    );
    assert!(matches!(action, Action::None));
    assert!(!app.quit);
    assert!(app.busy());
}

#[test]
fn working_tab_ai_actions_keep_their_worktree_scope() -> Result<()> {
    let mut app = app();
    app.current_mut().set_status(RepoStatus {
        files: vec![
            FileChange {
                path: RepoPath::new(b"src/app.rs".to_vec())?,
                original_path: None,
                staged: None,
                worktree: Some(ChangeKind::Modified),
                submodule: false,
                head_oid: None,
                index_oid: None,
            },
            FileChange {
                path: RepoPath::new(b"README.md".to_vec())?,
                original_path: None,
                staged: None,
                worktree: Some(ChangeKind::Modified),
                submodule: false,
                head_oid: None,
                index_oid: None,
            },
        ],
        ..RepoStatus::default()
    });
    let area = Rect::new(0, 0, 160, 42);
    let selected = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
        area,
    );
    assert!(matches!(
        selected,
        Action::Draft { ai: true, scope }
            if scope.side == DiffSide::Worktree
                && matches!(scope.paths, Some(ref paths) if paths.len() == 1)
    ));
    let all = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE),
        area,
    );
    assert!(matches!(
        all,
        Action::Draft { ai: true, scope }
            if scope.side == DiffSide::Worktree
                && matches!(scope.paths, Some(ref paths) if paths.len() == 2)
    ));
    let selected_plan = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        area,
    );
    assert!(matches!(
        selected_plan,
        Action::Plan { scope }
            if scope.side == DiffSide::Worktree
                && matches!(scope.paths, Some(ref paths) if paths.len() == 1)
    ));
    let visible_plan = key(
        &mut app,
        KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
        area,
    );
    assert!(matches!(
        visible_plan,
        Action::Plan { scope }
            if scope.side == DiffSide::Worktree
                && matches!(scope.paths, Some(ref paths) if paths.len() == 2)
    ));
    Ok(())
}

#[test]
fn theme_picker_previews_then_saves_or_restores() {
    let mut app = app();
    let area = Rect::new(0, 0, 160, 42);
    let original = app.theme.id;
    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
        area,
    );
    key(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        area,
    );
    assert_ne!(app.theme.id, original);
    key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        area,
    );
    assert_eq!(app.theme.id, original);

    key(
        &mut app,
        KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
        area,
    );
    key(
        &mut app,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        area,
    );
    let selected = app.theme.id;
    let action = key(
        &mut app,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        area,
    );
    assert!(matches!(action, Action::SaveTheme));
    assert_eq!(app.theme.id, selected);
}
