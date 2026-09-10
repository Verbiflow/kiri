mod color;
mod file_tree;
pub mod highlight;
pub use color::ColorMode;
mod input;
mod modals;
mod motion;
mod runtime;
pub mod state;
pub mod view;

use anyhow::{Context, Result};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, Event, EventStream, KeyEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use kiri_core::{model::DiffSide, storage::Store};
use ratatui::{Terminal, backend::CrosstermBackend};
use runtime::Runtime;
use state::Modal;
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

pub async fn run(path: PathBuf, store: Store, side: DiffSide, color: ColorMode) -> Result<()> {
    let mut runtime = Runtime::new(path, store, side)?;
    runtime.app.color_enabled = color.enabled(std::env::var("NO_COLOR").ok().as_deref());
    crossterm::style::force_color_output(runtime.app.color_enabled);
    if !runtime.app.color_enabled {
        runtime.app.notice =
            "Colors disabled. Press t to enable, or start with --color always.".into();
    }
    runtime.app.reduced_motion = std::env::var_os("KIRI_REDUCED_MOTION").is_some();
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous_hook(info);
    }));
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        EnableFocusChange
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    terminal.draw(|frame| view::draw(frame, &runtime.app))?;
    runtime.refresh();
    let mut tick = Instant::now();
    let mut events = EventStream::new();
    while !runtime.app.quit {
        runtime.app.now = Instant::now();
        runtime.drain();
        if let Some(started) = runtime.app.completion {
            runtime.app.dirty = true;
            if runtime.app.reduced_motion || started.elapsed() >= Duration::from_millis(600) {
                runtime.app.completion = None;
            }
        }
        if tick.elapsed() >= Duration::from_millis(120) {
            runtime.app.tick += 1;
            if runtime.app.busy() {
                runtime.app.dirty = true;
            }
            tick = Instant::now();
        }
        if runtime.refresh_due() && matches!(runtime.app.modal, Modal::None) {
            runtime.refresh();
        }
        if runtime.app.clear {
            crossterm::style::force_color_output(runtime.app.color_enabled);
            terminal.clear()?;
            runtime.app.clear = false;
        }
        if runtime.app.dirty {
            terminal.draw(|frame| view::draw(frame, &runtime.app))?;
            runtime.app.dirty = false;
        }
        let event = tokio::select! {
            _ = runtime.wait() => continue,
            _ = tokio::time::sleep(Duration::from_millis(16)) => continue,
            event = events.next() => event.context("Terminal input closed")??,
        };
        let size = terminal.size()?;
        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        match event {
            Event::Key(event) if event.kind != KeyEventKind::Release => {
                let action = input::key(&mut runtime.app, event, area);
                runtime.action(action);
            }
            Event::Mouse(event) => {
                let action = input::mouse(&mut runtime.app, event, area);
                runtime.action(action);
            }
            Event::Resize(_, _) => runtime.app.dirty = true,
            Event::FocusGained if matches!(runtime.app.modal, Modal::None) => runtime.refresh(),
            Event::Paste(text) => {
                let text: String = text
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .take(16384)
                    .collect();
                match &mut runtime.app.modal {
                    Modal::Draft { editor, .. } | Modal::PlanEdit { editor, .. } => {
                        editor.insert_str(text);
                    }
                    Modal::AddWorkspace(input) => input.push_str(text.trim()),
                    Modal::Connect(form) => match form.field {
                        0 => form.settings.model.push_str(text.trim()),
                        1 => form
                            .settings
                            .endpoint
                            .get_or_insert_with(String::new)
                            .push_str(text.trim()),
                        2 => form
                            .settings
                            .region
                            .get_or_insert_with(String::new)
                            .push_str(text.trim()),
                        _ => form.key.push_str(text.trim()),
                    },
                    _ => {}
                }
                runtime.app.dirty = true;
            }
            _ => {}
        }
    }
    Ok(())
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        crossterm::style::ResetColor,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange
    );
}
