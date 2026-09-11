use anyhow::Result;
use kiri_ai::config::Settings;
use kiri_core::{
    diff::DiffDocument,
    model::{ChangeKind, DiffSide, FileChange, RepoPath, RepoStatus},
};
use kiri_tui::{
    state::{App, DiffView, Load},
    view,
};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

fn app() -> Result<App> {
    let mut app = App::new(
        Vec::new(),
        PathBuf::from("/projects/kiri"),
        DiffSide::Worktree,
        Settings::default(),
    );
    let files = (0..1000)
        .map(|i| {
            Ok(FileChange {
                path: RepoPath::new(format!("src/module_{i:04}.rs").into_bytes())?,
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
        branch: "main".into(),
        files,
        ..RepoStatus::default()
    });
    let mut patch = String::from(
        "diff --git a/src/module_0000.rs b/src/module_0000.rs\n--- a/src/module_0000.rs\n+++ b/src/module_0000.rs\n@@ -1,30000 +1,30000 @@\n",
    );
    for _ in 0..30000 {
        patch.push_str("-old value\n+new value\n");
    }
    app.current_mut().diff = Load::Ready(Arc::new(DiffView::new(DiffDocument::parse(
        patch.into_bytes(),
        false,
    ))));
    Ok(app)
}

#[test]
fn analysis_review_distinguishes_preparation_estimate_and_call_limit() -> Result<()> {
    let mut app = app()?;
    app.modal = kiri_tui::state::Modal::ConfirmAnalysis(kiri_tui::state::AnalysisReview {
        mode: kiri_ai::analysis::AnalysisMode::Fast,
        inspection_rounds: 6,
        files: 9099,
        bytes: 179051536,
        chunks: 17499,
        calls: 11545,
        max_calls: 4096,
        concurrency: 8,
        provider: "test-model".into(),
    });
    let mut terminal = Terminal::new(TestBackend::new(160, 42))?;
    terminal.draw(|frame| view::draw(frame, &app))?;
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("No model calls have been sent"));
    assert!(screen.contains("11,545"));
    assert!(screen.contains("4,096"));
    assert!(screen.contains("Estimate exceeds the limit"));
    assert!(!screen.contains("Up to 11545 model calls"));
    Ok(())
}

#[test]
fn draws_actual_viewports_at_wide_narrow_and_tiny_sizes() -> Result<()> {
    let app = app()?;
    for (width, height) in [(160, 42), (100, 28), (64, 18), (30, 10)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| view::draw(frame, &app))?;
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        if width >= 60 {
            assert!(screen.contains("Working"));
            assert!(screen.contains("PARTIAL PREVIEW") || width < 100);
            assert!(screen.contains("old value") || width < 100);
        } else {
            assert!(screen.contains("Kiri needs"));
        }
    }
    Ok(())
}

#[test]
fn rendering_large_patch_only_pays_for_visible_rows() -> Result<()> {
    let mut app = app()?;
    let mut terminal = Terminal::new(TestBackend::new(160, 42))?;
    let started = Instant::now();
    let cpu_started = cpu_time::ThreadTime::try_now()?;
    for row in 0..100 {
        app.current_mut().scroll = row * 10;
        terminal.draw(|frame| view::draw(frame, &app))?;
    }
    let cpu_elapsed = cpu_started.try_elapsed()?;
    eprintln!(
        "100 viewport frames: {:.2} ms CPU, {:.2} ms wall",
        cpu_elapsed.as_secs_f64() * 1000.0,
        started.elapsed().as_secs_f64() * 1000.0
    );
    assert!(cpu_elapsed < Duration::from_secs(3));
    Ok(())
}

#[test]
fn syntax_colors_are_visible_without_losing_diff_backgrounds() -> Result<()> {
    use kiri_core::syntax::{Language, TokenKind};
    use kiri_tui::highlight::{SyntaxState, highlight_patch, token_color};
    use std::sync::atomic::AtomicUsize;
    let mut app = App::new(
        Vec::new(),
        PathBuf::from("/project"),
        DiffSide::Worktree,
        Settings::default(),
    );
    app.current_mut().set_status(RepoStatus {
        files: vec![FileChange {
            path: RepoPath::new(b"demo.py".to_vec())?,
            original_path: None,
            staged: None,
            worktree: Some(ChangeKind::Modified),
            submodule: false,
            head_oid: None,
            index_oid: None,
        }],
        ..RepoStatus::default()
    });
    let mut diff = DiffView::new(DiffDocument::parse(
        b"@@ -1,2 +1,2 @@\n def greet():\n-    return 'old'\n+    return 'hello' + str(42)\n"
            .to_vec(),
        false,
    ));
    let tokens = highlight_patch(&diff, Language::Python, &AtomicUsize::new(0))?;
    diff.syntax = SyntaxState::Ready {
        language: Language::Python,
        tokens: Arc::new(tokens),
    };
    app.current_mut().diff = Load::Ready(Arc::new(diff));
    let mut terminal = Terminal::new(TestBackend::new(160, 42))?;
    terminal.draw(|frame| view::draw(frame, &app))?;
    let cells = &terminal.backend().buffer().content;
    for kind in [
        TokenKind::Keyword,
        TokenKind::Function,
        TokenKind::String,
        TokenKind::Number,
    ] {
        assert!(
            cells.iter().any(|cell| cell.fg == token_color(kind)),
            "missing {kind:?} color"
        );
    }
    assert!(
        cells
            .iter()
            .any(|cell| cell.bg == view::ADD_BG && cell.fg == token_color(TokenKind::String))
    );
    assert!(
        cells
            .iter()
            .any(|cell| cell.bg == view::REMOVE_BG && cell.fg == token_color(TokenKind::String))
    );
    Ok(())
}

#[test]
fn incomplete_scans_never_claim_the_worktree_is_clean() -> Result<()> {
    let mut app = app()?;
    app.current_mut().set_status(RepoStatus::default());
    app.current_mut().scan = kiri_tui::state::FileScan::Incomplete;
    let mut terminal = Terminal::new(TestBackend::new(160, 42))?;
    terminal.draw(|frame| view::draw(frame, &app))?;
    let screen: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("Some changes may be missing"));
    assert!(!screen.contains("Working tree is clean"));
    Ok(())
}

#[test]
fn escapes_control_sequences_in_diff_content() -> Result<()> {
    let view = DiffView::new(DiffDocument::parse(
        b"@@ -1 +1 @@\n+\x1b]52;c;danger\x07\n".to_vec(),
        false,
    ));
    assert!(!view.text[1].contains('\x1b'));
    assert!(!view.text[1].contains('\x07'));
    Ok(())
}
