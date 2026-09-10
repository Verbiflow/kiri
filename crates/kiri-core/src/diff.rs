use crate::model::digest;
use anyhow::{Result, bail};
use serde::Serialize;
use std::ops::Range;

pub const PREVIEW_BYTES: usize = 512 * 1024;
pub const LARGE_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Header,
    Hunk,
    Context,
    Added,
    Removed,
    Notice,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub bytes: Range<usize>,
    pub kind: LineKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    pub lines: Range<usize>,
    pub bytes: Range<usize>,
}

#[derive(Clone, Debug)]
pub struct DiffDocument {
    pub raw: Vec<u8>,
    pub lines: Vec<DiffLine>,
    pub hunks: Vec<Hunk>,
    pub truncated: bool,
    pub binary: bool,
    pub notice: Option<String>,
    pub fingerprint: String,
}

impl DiffDocument {
    pub fn parse(mut raw: Vec<u8>, mut truncated: bool) -> Self {
        if let Some((end, _)) = raw
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .nth(19999)
            && end + 1 < raw.len()
        {
            raw.truncate(end + 1);
            truncated = true;
        }
        let fingerprint = digest(&raw);
        let mut lines = Vec::new();
        let mut hunks: Vec<Hunk> = Vec::new();
        let mut offset = 0;
        let (mut old, mut new) = (0, 0);
        let mut in_hunk = false;
        let mut binary = false;
        for line in raw.split_inclusive(|b| *b == b'\n') {
            let end = offset + line.len();
            let text_end = end - usize::from(line.last() == Some(&b'\n'));
            let kind = if line.starts_with(b"@@ ") {
                if let Some(hunk) = hunks.last_mut() {
                    hunk.lines.end = lines.len();
                    hunk.bytes.end = offset;
                }
                if let Some((a, b)) = hunk_numbers(line) {
                    old = a;
                    new = b;
                }
                in_hunk = true;
                hunks.push(Hunk {
                    lines: lines.len()..0,
                    bytes: offset..0,
                });
                LineKind::Hunk
            } else if in_hunk {
                match line.first() {
                    Some(b'+') => LineKind::Added,
                    Some(b'-') => LineKind::Removed,
                    Some(b' ') => LineKind::Context,
                    _ => LineKind::Notice,
                }
            } else {
                binary |=
                    line.starts_with(b"Binary files ") || line.starts_with(b"GIT binary patch");
                LineKind::Header
            };
            let old_number = matches!(kind, LineKind::Context | LineKind::Removed).then_some(old);
            let new_number = matches!(kind, LineKind::Context | LineKind::Added).then_some(new);
            if old_number.is_some() {
                old += 1;
            }
            if new_number.is_some() {
                new += 1;
            }
            lines.push(DiffLine {
                bytes: offset..text_end,
                kind,
                old: old_number,
                new: new_number,
            });
            offset = end;
        }
        if let Some(hunk) = hunks.last_mut() {
            hunk.lines.end = lines.len();
            hunk.bytes.end = raw.len();
        }
        Self {
            raw,
            lines,
            hunks,
            truncated,
            binary,
            notice: None,
            fingerprint,
        }
    }

    pub fn notice(message: impl Into<String>) -> Self {
        let mut doc = Self::parse(Vec::new(), false);
        doc.notice = Some(message.into());
        doc
    }

    pub fn line(&self, index: usize) -> &[u8] {
        self.lines
            .get(index)
            .map(|l| &self.raw[l.bytes.clone()])
            .unwrap_or_default()
    }

    pub fn hunk_patch(&self, index: usize) -> Result<Vec<u8>> {
        if self.truncated || self.binary {
            bail!("Partial or binary previews cannot be staged by hunk. Stage the file instead.");
        }
        let hunk = self
            .hunks
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Select a hunk first"))?;
        let first = self
            .hunks
            .first()
            .ok_or_else(|| anyhow::anyhow!("No text hunks"))?;
        let header = &self.raw[..first.bytes.start];
        if header.windows(14).any(|s| s == b"new file mode ")
            || header.windows(18).any(|s| s == b"deleted file mode ")
            || header.windows(12).any(|s| s == b"rename from ")
            || header.windows(9).any(|s| s == b"old mode ")
        {
            bail!(
                "Stage this file as a whole to preserve its creation, deletion, rename, or mode change"
            );
        }
        let mut patch = header.to_vec();
        patch.extend_from_slice(&self.raw[hunk.bytes.clone()]);
        Ok(patch)
    }
}

fn hunk_numbers(line: &[u8]) -> Option<(usize, usize)> {
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.split_whitespace().skip(1);
    let old = parts
        .next()?
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new = parts
        .next()?
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_numbers_and_hunk_boundaries_are_exact() -> Result<()> {
        let patch = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n-old\n+new\n same\n@@ -10 +10 @@\n-last\n+next\n";
        let doc = DiffDocument::parse(patch.to_vec(), false);
        assert_eq!(doc.hunks.len(), 2);
        assert_eq!(doc.lines[4].old, Some(1));
        assert_eq!(doc.lines[5].new, Some(1));
        assert_eq!(doc.lines[6].old, Some(2));
        assert!(!String::from_utf8(doc.hunk_patch(0)?)?.contains("last"));
        assert!(
            DiffDocument::parse(patch.to_vec(), true)
                .hunk_patch(0)
                .is_err()
        );
        Ok(())
    }
}
