use crate::{
    state::{ACTIONS, App, Modal, fuzzy_match},
    view::{ACCENT, MUTED, PANEL, SELECTED, TEXT, panel, tail},
};
use kiri_ai::config::Provider;
use kiri_core::model::{terminal_message, terminal_text};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Style, Stylize},
    text::Line,
    widgets::{Clear, Paragraph, Wrap},
};

fn format_count(value: usize) -> String {
    let digits = value.to_string();
    let mut formatted = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

pub fn draw(frame: &mut Frame, app: &App) {
    if matches!(app.modal, Modal::None) {
        return;
    }
    let (title, height) = match &app.modal {
        Modal::Help => ("Keyboard shortcuts", 34),
        Modal::AddWorkspace(_) => ("Open workspace", 10),
        Modal::Error(_) => ("Could not finish", 18),
        Modal::ConfirmSync { action, .. } => (action.label(), 12),
        Modal::ConfirmBranch(_) => ("Switch branch", 12),
        Modal::ConfirmStage { .. } => ("Update selected files", 12),
        Modal::ConfirmAnalysis(_) => ("Review AI analysis", 20),
        Modal::Branches { .. } => ("Local branches", 26),
        Modal::History { .. } => ("Recent history", 30),
        Modal::Providers { .. } => ("AI providers", 18),
        Modal::Connect(_) => ("Connect provider", 21),
        Modal::Busy { .. } => ("Working", 10),
        Modal::Draft { .. } => ("Review commit", 28),
        Modal::Plan { .. } => ("Review commit plan", 30),
        Modal::PlanEdit { .. } => ("Edit group message", 20),
        Modal::Palette { .. } => ("Commands", 18),
        Modal::None => return,
    };
    let area = centered(frame.area(), 96, height);
    frame.render_widget(Clear, area);
    let block = panel(title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let content = Rect::new(
        inner.x + 2,
        inner.y + 1,
        inner.width.saturating_sub(4),
        inner.height.saturating_sub(2),
    );
    match &app.modal {
        Modal::Error(message) => {
            frame.render_widget(Paragraph::new(format!("{}\n\nEsc or Enter returns to your workspace.\nSaved drafts are still available with c.", terminal_message(message))).wrap(Wrap { trim: false }), content);
        }
        Modal::ConfirmAnalysis(review) => {
            let controls = analysis_controls(frame.area());
            let deep = review.mode == kiri_ai::analysis::AnalysisMode::Deep;
            frame.render_widget(
                Paragraph::new(" f Fast ").style(Style::default().fg(ACCENT).bg(if deep {
                    PANEL
                } else {
                    SELECTED
                })),
                controls.fast,
            );
            frame.render_widget(
                Paragraph::new(" d Deep ").style(Style::default().fg(ACCENT).bg(if deep {
                    SELECTED
                } else {
                    PANEL
                })),
                controls.deep,
            );
            frame.render_widget(
                Paragraph::new(if deep {
                    "Complete coverage + optional source checks; final answer reserved."
                } else {
                    "Complete coverage, direct synthesis; no optional inspection loop."
                })
                .style(Style::default().fg(MUTED)),
                Rect::new(content.x, content.y + 1, content.width, 2),
            );
            let text = format!(
                "Prepared locally. No model calls have been sent.\n{} files · {:.1} MiB · {} source chunks\nEstimated cold-cache work: {} calls · {} concurrent\nCall budget before transport retries: {} · {}\n\n{}\nProvider charges start only after approval. No commits are created.",
                format_count(review.files),
                review.bytes as f64 / 1048576.0,
                format_count(review.chunks),
                format_count(review.estimated_calls()),
                review.concurrency,
                format_count(review.max_calls),
                terminal_text(&review.provider),
                if review.estimated_calls() > review.max_calls {
                    "Estimate exceeds the limit; a cold run may not finish.\nSelect fewer paths or adjust the analysis budget before proceeding."
                } else {
                    "Cache hits may reduce calls; context recovery can add calls.\nBinary/sensitive exclusions are explicit. All selected text is retained."
                }
            );
            frame.render_widget(
                Paragraph::new(text).wrap(Wrap { trim: false }),
                Rect::new(
                    content.x,
                    content.y + 3,
                    content.width,
                    content.height.saturating_sub(5),
                ),
            );
            frame.render_widget(
                Paragraph::new(" Enter generate ").style(Style::default().fg(ACCENT).bg(SELECTED)),
                controls.confirm,
            );
            frame.render_widget(
                Paragraph::new(" Esc cancel ").style(Style::default().fg(MUTED)),
                controls.cancel,
            );
        }
        Modal::ConfirmSync {
            action,
            branch,
            upstream,
            ..
        } => {
            let description = match action {
                kiri_core::sync::SyncAction::Pull => {
                    "Fetch and fast-forward this branch. No automatic merge, rebase, or stash."
                }
                kiri_core::sync::SyncAction::Push => {
                    "Publish this branch's committed changes to its upstream. No force push."
                }
                kiri_core::sync::SyncAction::Fetch => {
                    "Update remote-tracking refs. Working files stay unchanged."
                }
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "{}  →  {}\n\n{}",
                    terminal_text(branch),
                    terminal_text(upstream.as_deref().unwrap_or("no upstream configured")),
                    description
                ))
                .wrap(Wrap { trim: false }),
                Rect::new(
                    content.x,
                    content.y,
                    content.width,
                    content.height.saturating_sub(2),
                ),
            );
            frame.render_widget(
                Paragraph::new(format!(" Enter {} ", action.label()))
                    .style(Style::default().fg(ACCENT).bg(SELECTED)),
                confirmation_buttons(frame.area()).0,
            );
            frame.render_widget(
                Paragraph::new(" Esc cancel ").style(Style::default().fg(MUTED)),
                confirmation_buttons(frame.area()).1,
            );
        }
        Modal::ConfirmStage { paths, side, label } => {
            let verb = if *side == kiri_core::model::DiffSide::Worktree {
                "Stage"
            } else {
                "Unstage"
            };
            frame.render_widget(Paragraph::new(format!("{verb} {} files?\n{}\n\nOnly these listed paths change in the index. Working files are preserved.", paths.len(), terminal_text(label))).wrap(Wrap { trim: false }), content);
            frame.render_widget(
                Paragraph::new(format!(" Enter {verb} "))
                    .style(Style::default().fg(ACCENT).bg(SELECTED)),
                confirmation_buttons(frame.area()).0,
            );
            frame.render_widget(
                Paragraph::new(" Esc cancel ").style(Style::default().fg(MUTED)),
                confirmation_buttons(frame.area()).1,
            );
        }
        Modal::ConfirmBranch(name) => {
            frame.render_widget(Paragraph::new(format!("Switch to {}?\n\nTracked changes must be committed or stashed first.\nKiri never discards them to switch branches.", terminal_text(name))).wrap(Wrap { trim: false }), content);
            frame.render_widget(
                Paragraph::new(" Enter switch ").style(Style::default().fg(ACCENT).bg(SELECTED)),
                confirmation_buttons(frame.area()).0,
            );
            frame.render_widget(
                Paragraph::new(" Esc cancel ").style(Style::default().fg(MUTED)),
                confirmation_buttons(frame.area()).1,
            );
        }
        Modal::Branches { entries, selected } => {
            let height = content.height.saturating_sub(3) as usize;
            let start = crate::view::list_start(*selected, height);
            let lines: Vec<_> = entries
                .iter()
                .enumerate()
                .skip(start)
                .take(height)
                .map(|(index, branch)| {
                    Line::styled(
                        format!(
                            " {} {} {:34} {}",
                            if index == *selected { "›" } else { " " },
                            if branch.current { "*" } else { " " },
                            terminal_text(&branch.name),
                            terminal_text(&branch.upstream)
                        ),
                        Style::default().bg(if index == *selected { SELECTED } else { PANEL }),
                    )
                })
                .collect();
            frame.render_widget(Paragraph::new(lines), content);
            frame.render_widget(
                Paragraph::new("j/k select   Enter switch branch   Esc close")
                    .style(Style::default().fg(ACCENT)),
                Rect::new(content.x, content.bottom() - 1, content.width, 1),
            );
        }
        Modal::History { entries, selected } => {
            let height = content.height.saturating_sub(5) as usize;
            let start = crate::view::list_start(*selected, height);
            let lines: Vec<_> = entries
                .iter()
                .enumerate()
                .skip(start)
                .take(height)
                .map(|(index, entry)| {
                    Line::styled(
                        format!(
                            " {} {}  {}",
                            if index == *selected { "›" } else { " " },
                            entry.short_oid,
                            terminal_text(&entry.subject)
                        ),
                        Style::default().bg(if index == *selected { SELECTED } else { PANEL }),
                    )
                })
                .collect();
            frame.render_widget(Paragraph::new(lines), content);
            if let Some(entry) = entries.get(*selected) {
                frame.render_widget(
                    Paragraph::new(format!(
                        "{} · {}\n{}",
                        terminal_text(&entry.author),
                        terminal_text(&entry.age),
                        terminal_text(&entry.refs)
                    ))
                    .style(Style::default().fg(MUTED)),
                    Rect::new(content.x, content.bottom() - 4, content.width, 2),
                );
            }
            frame.render_widget(
                Paragraph::new("j/k select   PgUp/PgDn page   Esc close")
                    .style(Style::default().fg(ACCENT)),
                Rect::new(content.x, content.bottom() - 1, content.width, 1),
            );
        }
        Modal::Help => {
            let text = vec![
                "REVIEW",
                "  j/k or ↑/↓       Move through files or diff rows",
                "  Tab / Shift+Tab  Switch pane focus",
                "  Enter / Esc      Open folder or diff / back to files",
                "  s / u            Staged / working changes",
                "  /                Fuzzy file filter",
                "  [ / ]            Previous / next hunk",
                "  Space / S        Stage file or folder / all shown",
                "  H                Stage / unstage selected hunk",
                "  v / L            Split view / load a large preview",
                "  ←/→              Parent / expand folder; scroll in diff",
                "  t                Toggle colors, overriding NO_COLOR",
                "",
                "COMMITS",
                "  a                AI message for staged file/folder",
                "  A                AI message for all staged files",
                "  c / b            Write message / AI commit groups",
                "  !                Reopen the last error",
                "  Ctrl+S           Create the reviewed commit",
                "",
                "WORKSPACES",
                "  w / 1–9          Open a project / switch project",
                "  Alt+←/→          Cycle workspaces",
                "  P / Ctrl+P       Provider picker / command palette",
                "  r / q            Refresh / quit",
                "",
                "SYNC",
                "  f / d / U        Fetch / pull fast-forward / push",
                "  B / l            Branch picker / recent history",
                "  Sync buttons also respond to clicks.",
                "",
                "  Esc closes this panel. No discard or force-push shortcuts.",
            ];
            frame.render_widget(
                Paragraph::new(text.join("\n")).style(Style::default().fg(TEXT)),
                content,
            );
        }
        Modal::AddWorkspace(input) => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from("Repository path. Existing workspaces keep their place."),
                    Line::from(""),
                    Line::styled(
                        format!(" › {}▏", terminal_text(input)),
                        Style::default().fg(ACCENT).bg(SELECTED),
                    ),
                    Line::from(""),
                    Line::styled(
                        "Enter open   Esc cancel   ~ expands to your home directory",
                        Style::default().fg(MUTED),
                    ),
                ]),
                content,
            );
        }
        Modal::Providers { selected } => {
            let mut lines = vec![
                Line::styled(
                    "Your account. Your models. No Kiri subscription.",
                    Style::default().fg(MUTED),
                ),
                Line::from(""),
            ];
            for (index, provider) in Provider::ALL.iter().enumerate() {
                let configured = app.settings.providers.get(provider);
                let active = app.settings.active == Some(*provider);
                let suffix = if active {
                    "active"
                } else if configured.is_some() {
                    "configured"
                } else {
                    "connect"
                };
                let label = format!(
                    " {} {:24} {}",
                    if *selected == index { "›" } else { " " },
                    provider.label(),
                    suffix
                );
                lines.push(Line::styled(
                    label,
                    Style::default()
                        .fg(if active { ACCENT } else { TEXT })
                        .bg(if *selected == index { SELECTED } else { PANEL }),
                ));
            }
            lines.extend([
                Line::from(""),
                Line::styled(
                    "Enter select / connect   e edit settings   Esc close",
                    Style::default().fg(MUTED),
                ),
                Line::styled(
                    "API keys stay local. ChatGPT uses browser sign-in.",
                    Style::default().fg(MUTED),
                ),
            ]);
            frame.render_widget(Paragraph::new(lines), content);
        }
        Modal::Connect(form) => {
            let mut lines = vec![
                Line::styled(form.provider.label(), Style::default().fg(ACCENT).bold()),
                Line::from(""),
            ];
            let values = [
                form.settings.model.clone(),
                form.settings.endpoint.clone().unwrap_or_default(),
                form.settings.region.clone().unwrap_or_default(),
                "•".repeat(form.key.chars().count().min(48)),
            ];
            let labels = [
                "Model",
                "Endpoint · optional except Azure",
                "Region · Bedrock only",
                "API key · blank uses saved key or environment",
            ];
            for index in 0..4 {
                lines.push(Line::styled(labels[index], Style::default().fg(MUTED)));
                lines.push(Line::styled(
                    format!(
                        " {} {}{}",
                        if form.field == index { "›" } else { " " },
                        tail(&values[index], content.width.saturating_sub(5) as usize),
                        if form.field == index { "▏" } else { "" }
                    ),
                    Style::default().bg(if form.field == index { SELECTED } else { PANEL }),
                ));
                lines.push(Line::from(""));
            }
            lines.push(Line::styled(
                "Tab next field   Enter save   Esc cancel",
                Style::default().fg(ACCENT),
            ));
            frame.render_widget(Paragraph::new(lines), content);
        }
        Modal::Busy {
            message,
            cancellable,
        } => {
            let spinner = ["·", "•", "●", "•"][(app.tick % 4) as usize];
            frame.render_widget(
                Paragraph::new(format!(
                    "{spinner}  {}\n\n{}",
                    message,
                    if *cancellable {
                        "Esc cancel. Your repository has not been changed."
                    } else {
                        "Git is running with normal hooks. Wait for the result before closing."
                    }
                ))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(ACCENT)),
                content,
            );
        }
        Modal::Draft { draft, editor } => {
            let head = draft.snapshot.head().unwrap_or("initial commit");
            let scope = draft
                .paths
                .as_ref()
                .map(|paths| format!("{} selected files only", paths.len()))
                .unwrap_or("All staged changes".into());
            frame.render_widget(
                Paragraph::new(format!(
                    "{scope} · {} · Review before committing",
                    &head[..head.len().min(12)]
                ))
                .style(Style::default().fg(MUTED)),
                Rect::new(content.x, content.y, content.width, 1),
            );
            if let Some(paths) = &draft.paths {
                let mut names: Vec<_> = paths.iter().take(3).map(|path| path.display()).collect();
                if paths.len() > 3 {
                    names.push(format!(
                        "… {} more; Esc to inspect in Staged",
                        paths.len() - 3
                    ));
                }
                frame.render_widget(
                    Paragraph::new(names.join("\n")).style(Style::default().fg(MUTED)),
                    Rect::new(content.x, content.y + 1, content.width, 4),
                );
            }
            let offset = if draft.paths.is_some() { 6 } else { 2 };
            frame.render_widget(
                editor,
                Rect::new(
                    content.x,
                    content.y + offset,
                    content.width,
                    content.height.saturating_sub(offset + 4),
                ),
            );
            let warning = draft.warnings.first().map(String::as_str).unwrap_or(
                if draft.paths.is_some() { "Only these paths are committed. Other staged changes and working files stay untouched." } else { "All reviewed staged changes are committed. Working files stay untouched." },
            );
            frame.render_widget(
                Paragraph::new(warning)
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(MUTED)),
                Rect::new(
                    content.x,
                    content.bottom().saturating_sub(3),
                    content.width,
                    2,
                ),
            );
            frame.render_widget(
                Paragraph::new("Ctrl+S create commit   Esc keep draft and return")
                    .style(Style::default().fg(ACCENT)),
                Rect::new(
                    content.x,
                    content.bottom().saturating_sub(1),
                    content.width,
                    1,
                ),
            );
        }
        Modal::Plan {
            plan,
            selected,
            offset,
            confirming,
        } => {
            let mut lines = vec![
                Line::styled(
                    format!(
                        "{} commits · {} files · file-level groups",
                        plan.groups.len(),
                        plan.files.len()
                    ),
                    Style::default().fg(ACCENT),
                ),
                Line::from(""),
            ];
            for (index, group) in plan.groups.iter().enumerate() {
                lines.push(Line::styled(
                    format!(
                        " {} {}. {}",
                        if index == *selected { "›" } else { " " },
                        index + 1,
                        terminal_text(group.message.lines().next().unwrap_or_default())
                    ),
                    Style::default()
                        .bg(if index == *selected { SELECTED } else { PANEL })
                        .bold(),
                ));
                if index == *selected {
                    lines.push(Line::styled(
                        format!("     {}", terminal_text(&group.reason)),
                        Style::default().fg(MUTED),
                    ));
                    for file in plan.files.iter().filter(|f| group.files.contains(&f.id)) {
                        lines.push(Line::from(format!("     {}", file.path)));
                    }
                }
                lines.push(Line::from(""));
            }
            frame.render_widget(
                Paragraph::new(
                    lines
                        .into_iter()
                        .skip(*offset)
                        .take(content.height.saturating_sub(3) as usize)
                        .collect::<Vec<_>>(),
                ),
                Rect::new(
                    content.x,
                    content.y,
                    content.width,
                    content.height.saturating_sub(3),
                ),
            );
            let footer = if *confirming {
                "Create all these commits? Enter confirm   Esc go back"
            } else {
                "j/k group   PgUp/PgDn scroll   e edit message   Ctrl+S create all"
            };
            frame.render_widget(
                Paragraph::new(footer).style(Style::default().fg(ACCENT)),
                Rect::new(
                    content.x,
                    content.bottom().saturating_sub(1),
                    content.width,
                    1,
                ),
            );
            let warning = plan
                .warnings
                .first()
                .map(String::as_str)
                .unwrap_or("Review each group. Esc keeps this plan without creating commits.");
            frame.render_widget(
                Paragraph::new(warning).style(Style::default().fg(MUTED)),
                Rect::new(
                    content.x,
                    content.bottom().saturating_sub(3),
                    content.width,
                    2,
                ),
            );
        }
        Modal::PlanEdit { editor, .. } => {
            frame.render_widget(
                editor,
                Rect::new(
                    content.x,
                    content.y,
                    content.width,
                    content.height.saturating_sub(2),
                ),
            );
            frame.render_widget(
                Paragraph::new("Ctrl+S save message   Esc cancel")
                    .style(Style::default().fg(ACCENT)),
                Rect::new(
                    content.x,
                    content.bottom().saturating_sub(1),
                    content.width,
                    1,
                ),
            );
        }
        Modal::Palette { query, selected } => {
            let mut lines = vec![
                Line::styled(format!(" › {query}▏"), Style::default().fg(ACCENT)),
                Line::from(""),
            ];
            for (index, (name, key)) in ACTIONS
                .iter()
                .filter(|(name, _)| fuzzy_match(&query.to_lowercase(), &name.to_lowercase()))
                .enumerate()
            {
                lines.push(Line::styled(
                    format!(
                        " {} {name:45} {key}",
                        if index == *selected { "›" } else { " " }
                    ),
                    Style::default().bg(if index == *selected { SELECTED } else { PANEL }),
                ));
            }
            frame.render_widget(Paragraph::new(lines), content);
        }
        Modal::None => {}
    }
}

pub struct AnalysisControls {
    pub fast: Rect,
    pub deep: Rect,
    pub confirm: Rect,
    pub cancel: Rect,
}
pub fn analysis_controls(area: Rect) -> AnalysisControls {
    let popup = centered(area, 96, 20);
    AnalysisControls {
        fast: Rect::new(popup.x + 3, popup.y + 2, 14, 1),
        deep: Rect::new(popup.x + 21, popup.y + 2, 14, 1),
        confirm: Rect::new(popup.x + 3, popup.bottom().saturating_sub(3), 20, 1),
        cancel: Rect::new(popup.x + 27, popup.bottom().saturating_sub(3), 16, 1),
    }
}

pub fn confirmation_buttons(area: Rect) -> (Rect, Rect) {
    let popup = centered(area, 96, 12);
    let y = popup.bottom().saturating_sub(3);
    (
        Rect::new(popup.x + 3, y, 20, 1),
        Rect::new(popup.x + 27, y, 16, 1),
    )
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}
