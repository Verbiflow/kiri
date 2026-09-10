use super::*;
use crate::highlight::{SyntaxState, highlight_patch};
use kiri_core::syntax::Language;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct HighlightTask {
    handle: JoinHandle<()>,
    cancelled: Arc<AtomicUsize>,
}

impl Drop for HighlightTask {
    fn drop(&mut self) {
        self.cancelled.store(1, Ordering::Relaxed);
        self.handle.abort();
    }
}

impl Runtime {
    pub(super) fn show_diff(
        &mut self,
        workspace: usize,
        path: RepoPath,
        side: DiffSide,
        mut diff: Arc<DiffView>,
        reset: bool,
    ) {
        let previous = &self.app.workspaces[workspace];
        if previous.displayed_path.as_ref() == Some(&(path.clone(), side))
            && let Load::Ready(existing) = &previous.diff
            && existing.document.fingerprint == diff.document.fingerprint
            && matches!(existing.syntax, SyntaxState::Ready { .. })
        {
            let mut reused = (*diff).clone();
            reused.syntax = existing.syntax.clone();
            diff = Arc::new(reused);
        }
        let language = Language::for_path(&path).filter(|_| {
            self.app.color_enabled
                && matches!(diff.syntax, SyntaxState::Plain | SyntaxState::Loading(_))
                && !diff.document.hunks.is_empty()
                && !diff.document.binary
        });
        let view = &mut self.app.workspaces[workspace];
        if reset || matches!(view.diff, Load::Loading) {
            view.scroll = diff
                .document
                .hunks
                .first()
                .map(|h| h.lines.start)
                .unwrap_or(0);
        }
        view.displayed_path = Some((path.clone(), side));
        view.remember_diff(path.clone(), side, diff.clone());
        if let Some(language) = language {
            let mut pending = (*diff).clone();
            pending.syntax = SyntaxState::Loading(language);
            view.diff = Load::Ready(Arc::new(pending));
            self.start_highlight(workspace, path, side, diff, language);
        } else {
            view.diff = Load::Ready(diff);
        }
    }

    fn start_highlight(
        &mut self,
        workspace: usize,
        path: RepoPath,
        side: DiffSide,
        diff: Arc<DiffView>,
        language: Language,
    ) {
        self.highlight_task = None;
        let request = self.diff_request;
        let tx = self.tx.clone();
        let cancelled = Arc::new(AtomicUsize::new(0));
        let cancel = cancelled.clone();
        let handle = tokio::spawn(async move {
            let input = diff.clone();
            let worker_cancel = cancel.clone();
            let worker = tokio::task::spawn_blocking(move || {
                highlight_patch(&input, language, &worker_cancel)
            });
            let result = tokio::time::timeout(Duration::from_millis(500), worker).await;
            let mut highlighted = (*diff).clone();
            highlighted.syntax = match result {
                Ok(Ok(Ok(tokens))) => SyntaxState::Ready {
                    language,
                    tokens: Arc::new(tokens),
                },
                _ => {
                    cancel.store(1, Ordering::Relaxed);
                    SyntaxState::Skipped(language)
                }
            };
            let _ = tx.send(Message::Highlighted {
                request,
                workspace,
                path,
                side,
                view: Arc::new(highlighted),
            });
        });
        self.highlight_task = Some(HighlightTask { handle, cancelled });
    }
}
