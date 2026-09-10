use crate::model::RepoPath;
use anyhow::{Context, Result, bail};
use std::{
    ops::Range,
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    Comment,
    Keyword,
    String,
    Number,
    Function,
    Type,
    Constant,
    Property,
    Operator,
    Punctuation,
    Tag,
    Variable,
    Builtin,
    Attribute,
    Namespace,
}

const CAPTURES: &[(&str, TokenKind)] = &[
    ("comment", TokenKind::Comment),
    ("keyword", TokenKind::Keyword),
    ("string", TokenKind::String),
    ("number", TokenKind::Number),
    ("constant", TokenKind::Constant),
    ("boolean", TokenKind::Constant),
    ("function", TokenKind::Function),
    ("constructor", TokenKind::Type),
    ("type", TokenKind::Type),
    ("property", TokenKind::Property),
    ("variable.member", TokenKind::Property),
    ("operator", TokenKind::Operator),
    ("punctuation", TokenKind::Punctuation),
    ("tag", TokenKind::Tag),
    ("variable", TokenKind::Variable),
    ("variable.builtin", TokenKind::Builtin),
    ("attribute", TokenKind::Attribute),
    ("module", TokenKind::Namespace),
    ("namespace", TokenKind::Namespace),
];

#[derive(Clone, Debug)]
pub struct TokenSpan {
    pub bytes: Range<usize>,
    pub kind: TokenKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    Go,
    Bash,
    Css,
    Html,
    Toml,
}

impl Language {
    pub const ALL: [Self; 11] = [
        Self::Rust,
        Self::Python,
        Self::JavaScript,
        Self::TypeScript,
        Self::Tsx,
        Self::Json,
        Self::Go,
        Self::Bash,
        Self::Css,
        Self::Html,
        Self::Toml,
    ];

    pub fn for_path(path: &RepoPath) -> Option<Self> {
        let path = path.to_path_buf();
        Some(
            match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
                "rs" => Self::Rust,
                "py" | "pyi" => Self::Python,
                "js" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
                "ts" | "mts" | "cts" => Self::TypeScript,
                "tsx" => Self::Tsx,
                "json" | "jsonc" => Self::Json,
                "go" => Self::Go,
                "sh" | "bash" | "zsh" => Self::Bash,
                "css" => Self::Css,
                "html" | "htm" => Self::Html,
                "toml" => Self::Toml,
                _ => return None,
            },
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::Json => "JSON",
            Self::Go => "Go",
            Self::Bash => "Shell",
            Self::Css => "CSS",
            Self::Html => "HTML",
            Self::Toml => "TOML",
        }
    }

    fn config(self) -> Result<&'static HighlightConfiguration> {
        static CONFIGS: [OnceLock<Result<HighlightConfiguration, String>>; 11] =
            [const { OnceLock::new() }; 11];
        CONFIGS[self as usize]
            .get_or_init(|| {
                let (language, query): (tree_sitter::Language, String) = match self {
                    Self::Rust => (
                        tree_sitter_rust::LANGUAGE.into(),
                        tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
                    ),
                    Self::Python => (
                        tree_sitter_python::LANGUAGE.into(),
                        tree_sitter_python::HIGHLIGHTS_QUERY.into(),
                    ),
                    Self::JavaScript => (
                        tree_sitter_javascript::LANGUAGE.into(),
                        format!(
                            "{}\n{}",
                            tree_sitter_javascript::HIGHLIGHT_QUERY,
                            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
                        ),
                    ),
                    Self::TypeScript => (
                        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                        format!(
                            "{}\n{}",
                            tree_sitter_javascript::HIGHLIGHT_QUERY,
                            tree_sitter_typescript::HIGHLIGHTS_QUERY
                        ),
                    ),
                    Self::Tsx => (
                        tree_sitter_typescript::LANGUAGE_TSX.into(),
                        format!(
                            "{}\n{}\n{}",
                            tree_sitter_javascript::HIGHLIGHT_QUERY,
                            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                            tree_sitter_typescript::HIGHLIGHTS_QUERY
                        ),
                    ),
                    Self::Json => (
                        tree_sitter_json::LANGUAGE.into(),
                        tree_sitter_json::HIGHLIGHTS_QUERY.into(),
                    ),
                    Self::Go => (
                        tree_sitter_go::LANGUAGE.into(),
                        tree_sitter_go::HIGHLIGHTS_QUERY.into(),
                    ),
                    Self::Bash => (
                        tree_sitter_bash::LANGUAGE.into(),
                        tree_sitter_bash::HIGHLIGHT_QUERY.into(),
                    ),
                    Self::Css => (
                        tree_sitter_css::LANGUAGE.into(),
                        tree_sitter_css::HIGHLIGHTS_QUERY.into(),
                    ),
                    Self::Html => (
                        tree_sitter_html::LANGUAGE.into(),
                        tree_sitter_html::HIGHLIGHTS_QUERY.into(),
                    ),
                    Self::Toml => (
                        tree_sitter_toml_ng::LANGUAGE.into(),
                        tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
                    ),
                };
                let mut config = HighlightConfiguration::new(language, self.name(), &query, "", "")
                    .map_err(|e| e.to_string())?;
                config.configure(&CAPTURES.iter().map(|(name, _)| *name).collect::<Vec<_>>());
                Ok(config)
            })
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{} highlighting unavailable: {e}", self.name()))
    }
}

pub fn highlight(
    language: Language,
    source: &str,
    cancelled: &AtomicUsize,
) -> Result<Vec<TokenSpan>> {
    if source.len() > 1024 * 1024 {
        bail!("Syntax source exceeds 1 MiB");
    }
    if cancelled.load(Ordering::Relaxed) != 0 {
        bail!("Highlighting cancelled");
    }
    let mut highlighter = Highlighter::new();
    let events = highlighter.highlight(
        language.config()?,
        source.as_bytes(),
        Some(cancelled),
        |_| None,
    )?;
    let mut stack = Vec::new();
    let mut spans = Vec::new();
    for event in events {
        match event? {
            HighlightEvent::HighlightStart(kind) => {
                stack.push(CAPTURES.get(kind.0).context("Unknown syntax class")?.1)
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&kind) = stack.last()
                    && start < end
                {
                    source
                        .get(start..end)
                        .context("Invalid syntax span boundary")?;
                    spans.push(TokenSpan {
                        bytes: start..end,
                        kind,
                    });
                    if spans.len() > 100000 {
                        bail!("Syntax span budget exceeded");
                    }
                }
            }
        }
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_grammar_and_query_loads() -> Result<()> {
        for language in Language::ALL {
            language.config()?;
        }
        Ok(())
    }

    #[test]
    fn python_keywords_functions_strings_and_numbers_have_distinct_classes() -> Result<()> {
        let source = "def greet(name: str):\n    return 'hello' + str(42)\n";
        let tokens = highlight(Language::Python, source, &AtomicUsize::new(0))?;
        let has = |text, kind| {
            tokens
                .iter()
                .any(|token| &source[token.bytes.clone()] == text && token.kind == kind)
        };
        assert!(has("def", TokenKind::Keyword));
        assert!(has("greet", TokenKind::Function));
        assert!(tokens.iter().any(|token| token.kind == TokenKind::String));
        assert!(has("42", TokenKind::Number));
        Ok(())
    }

    #[test]
    fn tsx_and_unicode_ranges_are_valid() -> Result<()> {
        let source = "export const Greeting = () => <div title=\"你好\">hello</div>;";
        let tokens = highlight(Language::Tsx, source, &AtomicUsize::new(0))?;
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Tag));
        assert!(tokens.iter().all(|t| source.get(t.bytes.clone()).is_some()));
        Ok(())
    }

    #[test]
    fn cancelled_work_does_not_parse() {
        assert!(highlight(Language::Rust, "fn main() {}", &AtomicUsize::new(1)).is_err());
    }
}
