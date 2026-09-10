use crate::{
    state::DiffView,
    view::{MUTED, TEXT},
};
use anyhow::{Result, bail};
use kiri_core::{
    diff::LineKind,
    syntax::{self, Language, TokenKind, TokenSpan},
};
use ratatui::{
    style::{Color, Style},
    text::Span,
};
use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use unicode_width::UnicodeWidthChar;

#[derive(Clone, Default)]
pub enum SyntaxState {
    #[default]
    Plain,
    Loading(Language),
    Ready {
        language: Language,
        tokens: Arc<PatchTokens>,
    },
    Skipped(Language),
}

impl SyntaxState {
    pub fn label(&self) -> String {
        match self {
            Self::Plain => "plain text".into(),
            Self::Loading(language) => format!("{} · coloring", language.name()),
            Self::Ready { language, tokens } => format!(
                "{}{}",
                language.name(),
                if tokens.partial {
                    " · syntax preview"
                } else {
                    ""
                }
            ),
            Self::Skipped(language) => format!("{} · plain fallback", language.name()),
        }
    }
}

pub struct PatchTokens {
    pub before: Vec<Vec<TokenSpan>>,
    pub after: Vec<Vec<TokenSpan>>,
    pub partial: bool,
}

struct SourceLine {
    row: usize,
    bytes: Range<usize>,
}

pub fn highlight_patch(
    view: &DiffView,
    language: Language,
    cancelled: &AtomicUsize,
) -> Result<PatchTokens> {
    let mut result = PatchTokens {
        before: vec![Vec::new(); view.text.len()],
        after: vec![Vec::new(); view.text.len()],
        partial: false,
    };
    let mut remaining = 512 * 1024_usize;
    for hunk in &view.document.hunks {
        for before in [true, false] {
            if cancelled.load(Ordering::Relaxed) != 0 {
                bail!("Highlighting cancelled");
            }
            if remaining == 0 {
                result.partial = true;
                continue;
            }
            let mut source = String::new();
            let mut lines = Vec::new();
            for row in hunk.lines.clone() {
                let kind = view.document.lines[row].kind;
                if kind != LineKind::Context
                    && kind
                        != if before {
                            LineKind::Removed
                        } else {
                            LineKind::Added
                        }
                {
                    continue;
                }
                let text = view.text[row].get(1..).unwrap_or_default();
                if text.len() + 1 > remaining {
                    result.partial = true;
                    break;
                }
                let start = source.len();
                source.push_str(text);
                lines.push(SourceLine {
                    row,
                    bytes: start..source.len(),
                });
                source.push('\n');
                remaining -= text.len() + 1;
            }
            if source.is_empty() {
                continue;
            }
            let target = if before {
                &mut result.before
            } else {
                &mut result.after
            };
            for token in syntax::highlight(language, &source, cancelled)? {
                let mut line_index = lines
                    .partition_point(|line| line.bytes.start <= token.bytes.start)
                    .saturating_sub(1);
                while let Some(line) = lines.get(line_index) {
                    if line.bytes.start >= token.bytes.end {
                        break;
                    }
                    let start = token.bytes.start.max(line.bytes.start);
                    let end = token.bytes.end.min(line.bytes.end);
                    if start < end {
                        target[line.row].push(TokenSpan {
                            bytes: (start - line.bytes.start + 1)..(end - line.bytes.start + 1),
                            kind: token.kind,
                        });
                    }
                    line_index += 1;
                }
            }
        }
    }
    Ok(result)
}

pub fn token_color(kind: TokenKind) -> Color {
    match kind {
        TokenKind::Comment => MUTED,
        TokenKind::Keyword => Color::Rgb(194, 152, 255),
        TokenKind::String => Color::Rgb(156, 219, 150),
        TokenKind::Number | TokenKind::Constant => Color::Rgb(247, 174, 122),
        TokenKind::Function => Color::Rgb(123, 185, 255),
        TokenKind::Type | TokenKind::Attribute => Color::Rgb(241, 207, 127),
        TokenKind::Property => Color::Rgb(114, 205, 226),
        TokenKind::Builtin | TokenKind::Tag => Color::Rgb(228, 166, 232),
        TokenKind::Namespace => Color::Rgb(174, 170, 249),
        TokenKind::Operator => Color::Rgb(137, 212, 216),
        TokenKind::Punctuation => Color::Rgb(172, 187, 204),
        TokenKind::Variable => TEXT,
    }
}

pub fn spans(
    text: &str,
    tokens: &[TokenSpan],
    offset: usize,
    width: usize,
    default: Color,
    prefix: Color,
) -> Vec<Span<'static>> {
    let mut result = Vec::new();
    let mut column = 0;
    let mut token = 0;
    let mut color = default;
    let mut segment = String::new();
    for (byte, c) in text.char_indices() {
        let size = c.width().unwrap_or(0);
        if column >= offset.saturating_add(width) {
            break;
        }
        if column >= offset && column + size <= offset.saturating_add(width) {
            while tokens.get(token).is_some_and(|span| span.bytes.end <= byte) {
                token += 1;
            }
            let current = if byte == 0 {
                prefix
            } else {
                tokens
                    .get(token)
                    .filter(|span| span.bytes.contains(&byte))
                    .map(|span| token_color(span.kind))
                    .unwrap_or(default)
            };
            if current != color && !segment.is_empty() {
                result.push(Span::styled(
                    std::mem::take(&mut segment),
                    Style::default().fg(color),
                ));
            }
            color = current;
            segment.push(c);
        }
        column += size;
    }
    if !segment.is_empty() {
        result.push(Span::styled(segment, Style::default().fg(color)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_core::diff::DiffDocument;

    #[test]
    fn before_and_after_are_highlighted_independently() -> Result<()> {
        let view = DiffView::new(DiffDocument::parse(
            b"@@ -1,2 +1,2 @@\n-value = 'old'\n+value = 42\n print(value)\n".to_vec(),
            false,
        ));
        let result = highlight_patch(&view, Language::Python, &AtomicUsize::new(0))?;
        assert!(
            result.before[1]
                .iter()
                .any(|span| span.kind == TokenKind::String)
        );
        assert!(
            result.after[2]
                .iter()
                .any(|span| span.kind == TokenKind::Number)
        );
        assert!(result.before[2].is_empty());
        assert!(result.after[1].is_empty());
        for (row, tokens) in result.after.iter().enumerate() {
            assert!(
                tokens
                    .iter()
                    .all(|token| view.text[row].get(token.bytes.clone()).is_some())
            );
        }
        Ok(())
    }

    #[test]
    fn horizontal_clipping_preserves_unicode_and_token_colors() {
        let tokens = [TokenSpan {
            bytes: 1..7,
            kind: TokenKind::String,
        }];
        let spans = spans("+你好 = 42", &tokens, 1, 4, TEXT, crate::view::ADD);
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            "你好"
        );
        assert!(
            spans
                .iter()
                .all(|s| s.style.fg == Some(token_color(TokenKind::String)))
        );
    }
}
