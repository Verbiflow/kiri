use crate::{
    highlight::SyntaxState,
    state::{App, DiffView, Focus, Load, WorkspaceView},
};
use kiri_core::{
    diff::LineKind,
    model::{DiffSide, terminal_text},
};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

pub const BG: Color = Color::Rgb(15, 19, 24);
pub const PANEL: Color = Color::Rgb(20, 25, 31);
pub const TEXT: Color = Color::Rgb(218, 224, 231);
pub const MUTED: Color = Color::Rgb(139, 154, 172);
pub const ACCENT: Color = Color::Rgb(114, 215, 187);
pub const BORDER: Color = Color::Rgb(52, 69, 88);
pub const SELECTED: Color = Color::Rgb(30, 51, 70);
pub const ADD: Color = Color::Rgb(140, 215, 166);
pub const REMOVE: Color = Color::Rgb(235, 155, 157);
pub const ADD_BG: Color = Color::Rgb(22, 43, 35);
pub const REMOVE_BG: Color = Color::Rgb(47, 29, 35);

pub struct Geometry {
    pub workspaces: Rect,
    pub files: Rect,
    pub diff: Rect,
}

pub fn geometry(area: Rect) -> Geometry {
    let body = Rect {
        y: area.y + 4,
        height: area.height.saturating_sub(7),
        ..area
    };
    let sidebar = if area.width >= 140 { 20 } else { 0 };
    let files = if area.width >= 110 {
        44
    } else if area.width >= 85 {
        38
    } else {
        28
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(sidebar),
            Constraint::Length(files),
            Constraint::Min(20),
        ])
        .split(body);
    Geometry {
        workspaces: columns[0],
        files: columns[1],
        diff: columns[2],
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    if area.width < 60 || area.height < 16 {
        frame.render_widget(Paragraph::new("Kiri needs at least 60 columns and 16 rows.\nResize your terminal, or use `kiri status --json`.\nPress q to exit.").style(Style::default().fg(TEXT)).wrap(Wrap { trim: false }), area);
        return;
    }
    let workspace = app.current();
    let branch = match &workspace.status {
        Load::Ready(s) => terminal_text(&s.branch),
        Load::Loading => "opening repository".into(),
        Load::Failed(_) => "unavailable".into(),
    };
    let provider = app
        .settings
        .active()
        .map(|(p, s)| format!("{} / {}", p.id(), s.model))
        .unwrap_or_else(|_| "AI not connected · P to connect".into());
    let header = Line::from(vec![
        Span::styled("  kiri ", Style::default().fg(ACCENT).bold()),
        Span::styled(" / ", Style::default().fg(BORDER)),
        Span::styled(
            format!("{}  ", terminal_text(&workspace.workspace.name)),
            Style::default().bold(),
        ),
        Span::styled(branch, Style::default().fg(ACCENT)),
    ]);
    frame.render_widget(Paragraph::new(header), Rect::new(0, 1, area.width, 1));
    if area.width >= 110 {
        frame.render_widget(
            Paragraph::new(terminal_text(&provider))
                .right_aligned()
                .style(Style::default().fg(MUTED)),
            Rect::new(area.width.saturating_sub(52), 1, 50, 1),
        );
    }
    sync_bar(frame, app, area);
    let geo = geometry(area);
    if geo.workspaces.width > 0 {
        workspaces(frame, app, geo.workspaces);
    }
    crate::file_tree::draw(frame, app, geo.files);
    if workspace.node().is_some_and(|node| node.is_folder()) {
        crate::file_tree::folder(frame, app, geo.diff);
    } else {
        diff(frame, app, geo.diff);
    }
    let notice = if app.filtering {
        format!(
            "Filter files: {}  |  Enter done  Esc clear",
            workspace.filter
        )
    } else {
        app.notice.clone()
    };
    let notice_bg = app
        .completion
        .filter(|_| !app.reduced_motion)
        .map(|started| {
            crate::motion::completion_color(
                app.now.saturating_duration_since(started).as_secs_f32() / 0.6,
            )
        })
        .unwrap_or(BG);
    frame.render_widget(
        Paragraph::new(format!("  {}", terminal_text(&notice)))
            .style(Style::default().fg(MUTED).bg(notice_bg)),
        Rect::new(0, area.height - 3, area.width, 1),
    );
    let keys = if workspace.side == DiffSide::Staged {
        "  a AI message for selection   c write message   b AI split   A all staged   Space unstage   u working   ? help"
    } else if workspace.node().is_some_and(|node| node.is_folder()) {
        "  Space stage folder   s review staged → a AI message   Enter expand/collapse   ← parent   A all staged   ? help"
    } else if app.focus == Focus::Diff {
        "  j/k scroll   [/] hunk   H stage hunk   Space stage file   Tab files   v split   t colors   ? help"
    } else {
        "  j/k move   Space stage file   ← parent folder   s review staged   A AI all staged   t colors   ? help"
    };
    frame.render_widget(
        Paragraph::new(keys).style(Style::default().bg(PANEL).fg(TEXT)),
        Rect::new(0, area.height - 2, area.width, 1),
    );
    crate::modals::draw(frame, app);
}

pub fn sync_buttons() -> [(Rect, char); 5] {
    [
        (Rect::new(2, 2, 11, 1), 'f'),
        (Rect::new(14, 2, 18, 1), 'd'),
        (Rect::new(33, 2, 18, 1), 'U'),
        (Rect::new(52, 2, 14, 1), 'B'),
        (Rect::new(67, 2, 13, 1), 'l'),
    ]
}

fn sync_bar(frame: &mut Frame, app: &App, area: Rect) {
    let workspace = app.current();
    let (incoming, outgoing) = match &workspace.status {
        Load::Ready(s) => (s.behind, s.ahead),
        _ => (0, 0),
    };
    let tracking = matches!(&workspace.status, Load::Ready(s) if s.upstream.is_some());
    let labels = [
        " f Fetch ".to_owned(),
        if tracking {
            format!(" d ↓ {incoming} incoming ")
        } else {
            " d No upstream ".into()
        },
        if tracking {
            format!(" U ↑ {outgoing} outgoing ")
        } else {
            " U Publish in Git ".into()
        },
        " B Branches ".to_owned(),
        " l History ".to_owned(),
    ];
    for ((rect, _), label) in sync_buttons().into_iter().zip(labels) {
        if rect.right() <= area.right() {
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(ACCENT).bg(SELECTED)),
                rect,
            );
        }
    }
    if area.width > 100 {
        let freshness = match workspace.last_fetch {
            Some(time) => format!(
                "fetched {}s ago",
                app.now.saturating_duration_since(time).as_secs()
            ),
            None => "cached remote refs · f to check".into(),
        };
        frame.render_widget(
            Paragraph::new(freshness)
                .right_aligned()
                .style(Style::default().fg(MUTED)),
            Rect::new(82, 2, area.width.saturating_sub(84), 1),
        );
    }
}

pub fn panel(title: impl Into<String>, focused: bool) -> Block<'static> {
    Block::default()
        .title(format!(" {} ", title.into()))
        .title_style(
            Style::default()
                .fg(if focused { ACCENT } else { TEXT })
                .bold(),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { BORDER }))
        .style(Style::default().bg(PANEL).fg(TEXT))
}

fn workspaces(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("Workspaces", app.focus == Focus::Workspaces);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let start = app
        .active
        .saturating_sub(inner.height.saturating_sub(6) as usize);
    for (row, (index, workspace)) in app
        .workspaces
        .iter()
        .enumerate()
        .skip(start)
        .take(inner.height.saturating_sub(4) as usize)
        .enumerate()
    {
        let selected = index == app.active;
        let marker = if selected { "›" } else { " " };
        let line = Line::from(vec![
            Span::styled(
                format!(" {marker} {} ", index + 1),
                Style::default().fg(if selected { ACCENT } else { MUTED }),
            ),
            Span::raw(terminal_text(&workspace.workspace.name)),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(if selected {
                SELECTED
            } else {
                PANEL
            })),
            Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
        );
    }
    if inner.height >= 5 {
        frame.render_widget(
            Paragraph::new(" w  open project\n 1–9 select\n Alt ←/→ cycle")
                .style(Style::default().fg(MUTED)),
            Rect::new(inner.x, inner.bottom() - 3, inner.width, 3),
        );
    }
}

fn diff(frame: &mut Frame, app: &App, area: Rect) {
    let workspace = app.current();
    let title = workspace
        .file()
        .map(|f| f.path.display())
        .unwrap_or_else(|| "Diff".into());
    let block = panel(
        tail(&title, area.width.saturating_sub(8) as usize),
        app.focus == Focus::Diff,
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if workspace.file().is_none() {
        frame.render_widget(Paragraph::new("\n\n  Review at your pace.\n\n  Select a changed file to see its patch.\n  AI stays out of the way until you ask.").style(Style::default().fg(MUTED)), inner);
        return;
    }
    let view = match &workspace.diff {
        Load::Loading => {
            frame.render_widget(
                Paragraph::new("\n  Loading this file only…\n\n  You can keep navigating.")
                    .style(Style::default().fg(MUTED)),
                inner,
            );
            return;
        }
        Load::Failed(error) => {
            frame.render_widget(Paragraph::new(format!("\n  {}\n\n  Press L for a longer, bounded preview.\n  Other files are still available.", terminal_text(error))).wrap(Wrap { trim: false }).style(Style::default().fg(REMOVE)), inner);
            return;
        }
        Load::Ready(view) => view,
    };
    let doc = &view.document;
    if let Some(notice) = &doc.notice {
        frame.render_widget(
            Paragraph::new(format!("\n  {notice}"))
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(MUTED)),
            inner,
        );
        return;
    }
    if doc.binary {
        frame.render_widget(
            Paragraph::new(
                "\n  Binary file changed.\n\n  Space stages the file without loading its contents.",
            )
            .style(Style::default().fg(MUTED)),
            inner,
        );
        return;
    }
    let split = app.split && area.width >= 76;
    let count = if split {
        view.pairs.len()
    } else {
        doc.lines.len()
    };
    let mode = if split { "split" } else { "unified" };
    let syntax = if app.color_enabled {
        view.syntax.label()
    } else {
        "colors off · t".into()
    };
    let position = if inner.width >= 90 {
        format!(" · row {}/{}", (workspace.scroll + 1).min(count), count)
    } else {
        String::new()
    };
    let hunk_label = if doc.hunks.len() == 1 {
        "hunk"
    } else {
        "hunks"
    };
    let summary = format!(
        " {syntax} · {mode} · {} {hunk_label}{}{position}",
        doc.hunks.len(),
        if doc.truncated {
            " · PARTIAL PREVIEW"
        } else {
            ""
        }
    );
    frame.render_widget(
        Paragraph::new(summary).style(Style::default().fg(if doc.truncated {
            REMOVE
        } else {
            MUTED
        })),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let (before, after) = match workspace.side {
        DiffSide::Worktree => ("INDEX · before", "WORKING TREE · after"),
        DiffSide::Staged => ("HEAD · before", "INDEX · staged"),
    };
    if split {
        let half = inner.width / 2;
        frame.render_widget(
            Paragraph::new(format!("      {before}")).style(Style::default().fg(MUTED)),
            Rect::new(inner.x, inner.y + 1, half, 1),
        );
        frame.render_widget(
            Paragraph::new(format!("      {after}")).style(Style::default().fg(MUTED)),
            Rect::new(
                inner.x + half + 1,
                inner.y + 1,
                inner.width.saturating_sub(half + 1),
                1,
            ),
        );
    } else {
        frame.render_widget(
            Paragraph::new(format!(" {before} → {after}")).style(Style::default().fg(MUTED)),
            Rect::new(inner.x, inner.y + 1, inner.width, 1),
        );
    }
    let body = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    for row in 0..body.height as usize {
        let index = workspace.scroll + row;
        let y = body.y + row as u16;
        if split {
            let Some((left, right)) = view.pairs.get(index) else {
                break;
            };
            let half = body.width / 2;
            render_line(
                frame,
                workspace,
                view,
                *left,
                Rect::new(body.x, y, half, 1),
                Some(false),
            );
            render_line(
                frame,
                workspace,
                view,
                *right,
                Rect::new(body.x + half + 1, y, body.width.saturating_sub(half + 1), 1),
                Some(true),
            );
            frame.render_widget(
                Paragraph::new("│").style(Style::default().fg(BORDER)),
                Rect::new(body.x + half, y, 1, 1),
            );
        } else {
            if index >= doc.lines.len() {
                break;
            }
            render_line(
                frame,
                workspace,
                view,
                Some(index),
                Rect::new(body.x, y, body.width, 1),
                None,
            );
        }
    }
}

fn render_line(
    frame: &mut Frame,
    workspace: &WorkspaceView,
    view: &DiffView,
    index: Option<usize>,
    area: Rect,
    side: Option<bool>,
) {
    let Some(index) = index else { return };
    let line = &view.document.lines[index];
    let (fg, mut bg) = match line.kind {
        LineKind::Added => (ADD, ADD_BG),
        LineKind::Removed => (REMOVE, REMOVE_BG),
        LineKind::Hunk => (ACCENT, SELECTED),
        LineKind::Header | LineKind::Notice => (MUTED, PANEL),
        LineKind::Context => (TEXT, PANEL),
    };
    if view
        .document
        .hunks
        .get(workspace.hunk)
        .is_some_and(|h| h.lines.start == index)
    {
        bg = Color::Rgb(37, 67, 66);
    }
    let number = |n: Option<usize>| {
        n.map(|n| format!("{n:>5}"))
            .unwrap_or_else(|| "     ".into())
    };
    let gutter = match side {
        Some(false) => format!("{} ", number(line.old)),
        Some(true) => format!("{} ", number(line.new)),
        None => format!("{} {} ", number(line.old), number(line.new)),
    };
    let tokens = if let SyntaxState::Ready { tokens, .. } = &view.syntax {
        if side == Some(false) || side.is_none() && line.kind == LineKind::Removed {
            tokens.before[index].as_slice()
        } else {
            tokens.after[index].as_slice()
        }
    } else {
        &[]
    };
    let default = if matches!(view.syntax, SyntaxState::Ready { .. })
        && matches!(
            line.kind,
            LineKind::Context | LineKind::Added | LineKind::Removed
        ) {
        TEXT
    } else {
        fg
    };
    let mut spans = vec![Span::styled(gutter.clone(), Style::default().fg(MUTED))];
    spans.extend(crate::highlight::spans(
        &view.text[index],
        tokens,
        workspace.horizontal,
        area.width.saturating_sub(gutter.len() as u16) as usize,
        default,
        fg,
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
        area,
    );
}

pub fn list_start(selected: usize, height: usize) -> usize {
    selected.saturating_sub(height.saturating_sub(1) / 2)
}

pub fn clip(text: &str, offset: usize, width: usize) -> String {
    let mut result = String::new();
    let mut column = 0;
    for c in text.chars() {
        let size = c.width().unwrap_or(0);
        if column >= offset + width {
            break;
        }
        if column >= offset && column + size <= offset + width {
            result.push(c);
        }
        column += size;
    }
    result
}

pub fn tail(text: &str, width: usize) -> String {
    let total: usize = text.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= width {
        return text.to_owned();
    }
    format!(
        "…{}",
        clip(
            text,
            total.saturating_sub(width.saturating_sub(1)),
            width.saturating_sub(1)
        )
    )
}
