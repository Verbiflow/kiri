use crate::model::{ChangeKind, FileChange, RepoPath, RepoStatus};
use anyhow::{Context, Result, bail};

pub fn parse_status(bytes: &[u8]) -> Result<RepoStatus> {
    let mut status = RepoStatus::default();
    let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    while let Some(record) = records.next() {
        match record[0] {
            b'#' => {
                let header = std::str::from_utf8(record).context("Invalid Git status header")?;
                if let Some(value) = header.strip_prefix("# branch.oid ") {
                    status.head = (value != "(initial)").then(|| value.to_owned());
                } else if let Some(value) = header.strip_prefix("# branch.head ") {
                    status.branch = value.to_owned();
                } else if let Some(value) = header.strip_prefix("# branch.upstream ") {
                    status.upstream = Some(value.to_owned());
                } else if let Some(value) = header.strip_prefix("# branch.ab ") {
                    let (a, b) = value
                        .split_once(' ')
                        .context("Invalid ahead/behind header")?;
                    status.ahead = a.trim_start_matches('+').parse()?;
                    status.behind = b.trim_start_matches('-').parse()?;
                }
            }
            b'?' => status.files.push(FileChange {
                path: RepoPath::new(
                    record
                        .get(2..)
                        .context("Invalid untracked status")?
                        .to_vec(),
                )?,
                original_path: None,
                staged: None,
                worktree: Some(ChangeKind::Untracked),
                submodule: false,
            }),
            b'1' | b'2' | b'u' => {
                let count = match record[0] {
                    b'1' => 9,
                    b'2' => 10,
                    _ => 11,
                };
                let fields: Vec<_> = record.splitn(count, |b| *b == b' ').collect();
                if fields.len() != count || fields[1].len() != 2 {
                    bail!("Malformed Git status record");
                }
                let conflicted = record[0] == b'u';
                status.files.push(FileChange {
                    path: RepoPath::new(fields[count - 1].to_vec())?,
                    original_path: if record[0] == b'2' {
                        Some(RepoPath::new(
                            records.next().context("Missing rename source")?.to_vec(),
                        )?)
                    } else {
                        None
                    },
                    staged: if conflicted {
                        Some(ChangeKind::Unmerged)
                    } else {
                        kind(fields[1][0])?
                    },
                    worktree: if conflicted {
                        Some(ChangeKind::Unmerged)
                    } else {
                        kind(fields[1][1])?
                    },
                    submodule: fields[2].first() == Some(&b'S'),
                });
            }
            b'!' => {}
            _ => bail!("Unknown Git status record"),
        }
    }
    Ok(status)
}

fn kind(byte: u8) -> Result<Option<ChangeKind>> {
    Ok(Some(match byte {
        b'.' => return Ok(None),
        b'M' => ChangeKind::Modified,
        b'A' => ChangeKind::Added,
        b'D' => ChangeKind::Deleted,
        b'R' => ChangeKind::Renamed,
        b'C' => ChangeKind::Copied,
        b'T' => ChangeKind::TypeChanged,
        b'U' => ChangeKind::Unmerged,
        _ => bail!("Unknown Git change kind"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nul_records_without_losing_paths() -> Result<()> {
        let status = parse_status(b"# branch.oid abc\0# branch.head main\0# branch.ab +2 -3\x001 MM N... 100644 100644 100644 a b path with\nnewline\x002 R. N... 100644 100644 100644 a b R100 new name\0old name\0? untracked\xff\0")?;
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 3);
        assert_eq!(status.files.len(), 3);
        assert_eq!(status.files[0].path.bytes(), b"path with\nnewline");
        assert_eq!(
            status.files[1].original_path.as_ref().map(RepoPath::bytes),
            Some(b"old name".as_slice())
        );
        assert_eq!(status.files[2].path.bytes(), b"untracked\xff");
        assert!(!status.files[0].path.display().contains('\n'));
        Ok(())
    }

    #[test]
    fn rejects_traversal_and_malformed_status() {
        assert!(RepoPath::new(b"../outside".to_vec()).is_err());
        assert!(RepoPath::new(b".git/config".to_vec()).is_err());
        assert!(parse_status(b"1 M\0").is_err());
    }
}
