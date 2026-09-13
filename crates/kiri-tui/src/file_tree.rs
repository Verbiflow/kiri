use crate::{
    state::{App, FileScan, Focus, Load},
    view,
};
use kiri_core::{
    model::{DiffSide, terminal_text},
    tree::Entry,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

pub fn rows_area(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 4,
        area.width.saturating_sub(2),
        area.height.saturating_sub(7),
    )
}

pub fn tabs(area: Rect) -> [(Rect, char); 2] {
    let half = area.width.saturating_sub(2) / 2;
    [
        (Rect::new(area.x + 1, area.y + 1, half, 1), 'u'),
        (
            Rect::new(
                area.x + 1 + half,
                area.y + 1,
                area.width.saturating_sub(half + 2),
                1,
            ),
            's',
        ),
    ]
}

pub fn folder_buttons(area: Rect, _side: DiffSide) -> [(Rect, char); 4] {
    [
        (
            Rect::new(
                area.x + 3,
                area.y + 6,
                23.min(area.width.saturating_sub(6)),
                1,
            ),
            ' ',
        ),
        (
            Rect::new(
                area.x + 28,
                area.y + 6,
                21.min(area.width.saturating_sub(31)),
                1,
            ),
            '\n',
        ),
        (
            Rect::new(
                area.x + 3,
                area.y + 8,
                23.min(area.width.saturating_sub(6)),
                1,
            ),
            'a',
        ),
        (
            Rect::new(
                area.x + 28,
                area.y + 8,
                21.min(area.width.saturating_sub(31)),
                1,
            ),
            'b',
        ),
    ]
}

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let workspace = app.current();
    let title = match workspace.scan {
        FileScan::Pending => "Changes · scanning",
        FileScan::Incomplete => "Changes · incomplete",
        FileScan::Complete => "Changes",
    };
    frame.render_widget(view::panel(app, title, app.focus == Focus::Files), area);
    let (working, staged) = (workspace.working_count, workspace.staged_count);
    for ((rect, key), (label, count, active)) in tabs(area).into_iter().zip([
        ("Working", working, workspace.side == DiffSide::Worktree),
        ("Staged", staged, workspace.side == DiffSide::Staged),
    ]) {
        frame.render_widget(
            Paragraph::new(format!(" {key} {label} {count} ")).style(
                Style::default()
                    .fg(if active { theme.accent } else { theme.muted })
                    .bg(if active { theme.selected } else { theme.panel }),
            ),
            rect,
        );
    }
    let search = if workspace.filter.is_empty() {
        " / Filter files or folders".into()
    } else {
        format!(" / {}", terminal_text(&workspace.filter))
    };
    frame.render_widget(
        Paragraph::new(search).style(Style::default().fg(if app.filtering {
            theme.accent
        } else {
            theme.muted
        })),
        Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
    );
    let rows = rows_area(area);
    match &workspace.status {
        Load::Loading => {
            frame.render_widget(
                Paragraph::new("\n Reading changed paths…\n Diffs load only on selection.")
                    .style(Style::default().fg(theme.muted)),
                rows,
            );
        }
        Load::Failed(error) => {
            frame.render_widget(
                Paragraph::new(format!("{}\n\nPress r to retry.", terminal_text(error)))
                    .wrap(Wrap { trim: false })
                    .style(Style::default().fg(theme.remove)),
                rows,
            );
        }
        Load::Ready(status) => {
            if workspace.visible.is_empty() {
                let message = if workspace.scan == FileScan::Incomplete {
                    "File scan incomplete.\nSome changes may be missing.\nPress r to retry."
                } else if !workspace.filter.is_empty() {
                    "No matching changes.\nEsc clears the filter."
                } else if workspace.side == DiffSide::Staged {
                    "Nothing staged yet.\n\nPress u to see working changes.\nSpace stages a file or folder."
                } else if workspace.scan == FileScan::Pending {
                    "Looking for new files…"
                } else {
                    "Working tree is clean.\n\nPress s for staged changes."
                };
                frame.render_widget(
                    Paragraph::new(message).style(Style::default().fg(theme.muted)),
                    rows,
                );
            }
            let start = view::list_start(workspace.selected, rows.height as usize);
            for (row, &index) in workspace
                .visible
                .iter()
                .skip(start)
                .take(rows.height as usize)
                .enumerate()
            {
                let node = &workspace.tree.nodes[index];
                let selected = row + start == workspace.selected;
                let bg = if selected {
                    theme.selected
                } else {
                    theme.panel
                };
                let indent = "  ".repeat(node.depth.min(12));
                let (marker, color, label, count) = match &node.entry {
                    Entry::Folder { path, .. } => (
                        if workspace.collapsed_dirs.contains(path) && workspace.filter.is_empty() {
                            "▸"
                        } else {
                            "▾"
                        },
                        theme.folder,
                        format!("{}/", node.name),
                        format!(" {} ", workspace.tree.members(index).len()),
                    ),
                    Entry::File { index } => {
                        let kind = status.files[*index]
                            .kind(workspace.side)
                            .map(|k| k.letter())
                            .unwrap_or(' ');
                        let color = match kind {
                            'A' | '?' => theme.add,
                            'D' | 'U' => theme.remove,
                            _ => theme.modified,
                        };
                        let marker = match kind {
                            'A' => "A",
                            '?' => "+",
                            'D' => "D",
                            'U' => "!",
                            'R' => "R",
                            'T' => "T",
                            _ => "M",
                        };
                        (marker, color, node.name.clone(), String::new())
                    }
                };
                let badge = if workspace.side == DiffSide::Staged && node.working == 0 {
                    "[+]"
                } else if node.staged > 0 {
                    "[~]"
                } else {
                    "[ ]"
                };
                let y = rows.y + row as u16;
                frame.render_widget(
                    Paragraph::new("").style(Style::default().bg(bg)),
                    Rect::new(rows.x, y, rows.width, 1),
                );
                let width = rows
                    .width
                    .saturating_sub(indent.len() as u16 + count.len() as u16 + 9)
                    as usize;
                let name = if label.chars().count() > width {
                    format!("{}…", view::clip(&label, 0, width.saturating_sub(1)))
                } else {
                    label
                };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            if selected { "› " } else { "  " },
                            Style::default().fg(theme.accent),
                        ),
                        Span::styled(indent, Style::default().fg(theme.border)),
                        Span::styled(format!("{marker} "), Style::default().fg(color)),
                        Span::styled(
                            name,
                            Style::default()
                                .fg(if node.is_folder() {
                                    theme.folder
                                } else {
                                    theme.text
                                })
                                .add_modifier(if selected {
                                    ratatui::style::Modifier::BOLD
                                } else {
                                    ratatui::style::Modifier::empty()
                                }),
                        ),
                    ]))
                    .style(Style::default().bg(bg)),
                    Rect::new(rows.x, y, rows.width.saturating_sub(4), 1),
                );
                if !count.is_empty() {
                    frame.render_widget(
                        Paragraph::new(count.clone())
                            .style(Style::default().fg(theme.muted).bg(bg)),
                        Rect::new(
                            rows.right().saturating_sub(count.len() as u16 + 4),
                            y,
                            count.len() as u16,
                            1,
                        ),
                    );
                }
                frame.render_widget(
                    Paragraph::new(badge).style(
                        Style::default()
                            .fg(if node.staged > 0 {
                                theme.add
                            } else {
                                theme.muted
                            })
                            .bg(bg),
                    ),
                    Rect::new(rows.right().saturating_sub(4), y, 3, 1),
                );
            }
        }
    }
    let action = if workspace.side == DiffSide::Worktree {
        "Space stage"
    } else {
        "Space unstage"
    };
    let kind = if workspace.node().is_some_and(|n| n.is_folder()) {
        "folder"
    } else {
        "file"
    };
    if area.height >= 9 {
        frame.render_widget(
            Paragraph::new(format!(" a AI commit {kind} · {action} · S all shown"))
                .style(Style::default().fg(theme.accent)),
            Rect::new(
                area.x + 1,
                area.bottom() - 3,
                area.width.saturating_sub(2),
                1,
            ),
        );
        frame.render_widget(
            Paragraph::new(" [ ] working  [+] staged  [~] mixed")
                .style(Style::default().fg(theme.muted)),
            Rect::new(
                area.x + 1,
                area.bottom() - 2,
                area.width.saturating_sub(2),
                1,
            ),
        );
    }
}

pub fn folder(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let workspace = app.current();
    let (Some(node), Load::Ready(status)) = (workspace.node(), &workspace.status) else {
        return;
    };
    let Entry::Folder { path, .. } = &node.entry else {
        return;
    };
    frame.render_widget(
        view::panel(app, format!("{path}/"), app.focus == Focus::Diff),
        area,
    );
    let content = Rect::new(
        area.x + 3,
        area.y + 2,
        area.width.saturating_sub(6),
        area.height.saturating_sub(4),
    );
    let members = workspace.selected_members();
    frame.render_widget(
        Paragraph::new(Line::styled(
            format!(
                "{} {} files in this folder",
                members.len(),
                if workspace.side == DiffSide::Worktree {
                    "working"
                } else {
                    "staged"
                }
            ),
            Style::default().fg(theme.folder).bold(),
        )),
        Rect::new(content.x, content.y, content.width, 1),
    );
    frame.render_widget(
        Paragraph::new(format!(
            "{} with working changes  ·  {} with staged changes",
            node.working, node.staged
        ))
        .style(Style::default().fg(theme.muted)),
        Rect::new(content.x, content.y + 1, content.width, 1),
    );
    let action = if workspace.side == DiffSide::Worktree {
        " Space Stage folder "
    } else {
        " Space Unstage folder "
    };
    let buttons = folder_buttons(area, workspace.side);
    frame.render_widget(
        Paragraph::new(action).style(Style::default().fg(theme.accent).bg(theme.selected).bold()),
        buttons[0].0,
    );
    if buttons[1].0.width > 0 {
        frame.render_widget(
            Paragraph::new(" Enter expand / fold ")
                .style(Style::default().fg(theme.folder).bg(theme.selected)),
            buttons[1].0,
        );
    }
    let labels = [" a AI commit folder ", " b plan folder "];
    for (index, label) in labels.into_iter().enumerate() {
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(theme.accent).bg(theme.selected)),
            buttons[index + 2].0,
        );
    }
    let note = if workspace.filter.is_empty() {
        "Only listed changed paths are selected, including collapsed children."
    } else {
        "Filter active: only matching files are selected. Esc clears the filter."
    };
    frame.render_widget(
        Paragraph::new(note)
            .style(Style::default().fg(theme.muted))
            .wrap(Wrap { trim: false }),
        Rect::new(content.x, content.y + 8, content.width, 2),
    );
    let list = Rect::new(
        content.x,
        content.y + 11,
        content.width,
        content.height.saturating_sub(11),
    );
    let prefix = path.bytes().len() + 1;
    for (row, &index) in members.iter().take(list.height as usize).enumerate() {
        let file = &status.files[index];
        let name = terminal_text(&String::from_utf8_lossy(
            file.path.bytes().get(prefix..).unwrap_or(file.path.bytes()),
        ));
        let kind = file.kind(workspace.side).map(|k| k.letter()).unwrap_or(' ');
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!(" {kind}  "), Style::default().fg(theme.modified)),
                Span::raw(name),
            ])),
            Rect::new(list.x, list.y + row as u16, list.width, 1),
        );
    }
}
