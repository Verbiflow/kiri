use super::*;
use crate::progress::Progress;
use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use kiri_core::{commit::StagedSnapshot, model::digest, repo::Repository, review::CapturedChanges};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom, Write},
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::task::JoinSet;

#[derive(Default)]
struct Cancellation(Arc<AtomicBool>);
impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub async fn prepare(
    repo: &Repository,
    paths: Option<&[RepoPath]>,
    options: AnalysisOptions,
    observer: Observer,
) -> Result<PreparedAnalysis> {
    options.validate()?;
    let snapshot = repo.staged_snapshot().await?;
    prepare_snapshot(repo, snapshot, paths, options, observer).await
}
pub async fn prepare_snapshot(
    repo: &Repository,
    snapshot: StagedSnapshot,
    paths: Option<&[RepoPath]>,
    options: AnalysisOptions,
    observer: Observer,
) -> Result<PreparedAnalysis> {
    let evidence = EvidenceTree::staged(repo, &snapshot).await?;
    prepare_captured(
        repo,
        CapturedChanges {
            snapshot: snapshot.into(),
            evidence,
        },
        paths,
        options,
        observer,
    )
    .await
}
pub async fn prepare_captured(
    repo: &Repository,
    captured: CapturedChanges,
    paths: Option<&[RepoPath]>,
    options: AnalysisOptions,
    observer: Observer,
) -> Result<PreparedAnalysis> {
    options.validate()?;
    let CapturedChanges {
        mut snapshot,
        evidence,
    } = captured;
    let mut changes = evidence.changes().await?;
    if let Some(paths) = paths {
        ensure!(!paths.is_empty(), "Select at least one path");
        let includes = |scope: &RepoPath, path: &RepoPath| {
            let prefix = scope.bytes().strip_suffix(b"/").unwrap_or(scope.bytes());
            path.bytes() == prefix
                || path
                    .bytes()
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with(b"/"))
        };
        for scope in paths {
            ensure!(
                changes.iter().any(|(path, _)| includes(scope, path)),
                "No captured changes match {scope}"
            );
        }
        changes.retain(|(path, _)| paths.iter().any(|scope| includes(scope, path)));
    }
    ensure!(!changes.is_empty(), "No changes in this selection");
    if let ReviewSnapshot::Worktree { snapshot } = &mut snapshot {
        let selected: HashSet<_> = changes.iter().map(|(path, _)| path).collect();
        snapshot.files.retain(|file| selected.contains(&file.path));
    }
    snapshot.verify(repo).await?;
    let directory = Arc::new(tempfile::tempdir()?);
    let mut files: Vec<_> = changes
        .into_iter()
        .map(|(path, change)| {
            let withheld = sensitive_path(&path.display())
                .then(|| "Sensitive filename: content withheld".to_owned());
            EvidenceFile {
                path,
                change,
                bytes: 0,
                withheld,
            }
        })
        .collect();
    let indices: Arc<HashMap<_, _>> = Arc::new(
        files
            .iter()
            .enumerate()
            .map(|(index, file)| (file.path.clone(), index))
            .collect(),
    );
    let mut stored = BTreeMap::new();
    let mut path_batches = Vec::new();
    let mut batch = Vec::new();
    let mut size = 0;
    let withheld_path = directory.path().join("withheld");
    let mut withheld = File::create(&withheld_path)?;
    let mut withheld_offset = 0;
    for (index, file) in files.iter().enumerate() {
        if let Some(reason) = &file.withheld {
            withheld.write_all(reason.as_bytes())?;
            stored.insert(
                index,
                StoredPatch {
                    content: withheld_path.clone(),
                    offset: withheld_offset,
                    bytes: reason.len() as u64,
                },
            );
            withheld_offset += reason.len() as u64;
            continue;
        }
        let bytes = file.path.bytes().len() + 1;
        ensure!(
            bytes <= 48 * 1024,
            "Selected path exceeds the patch batch argument budget"
        );
        if !batch.is_empty() && size + bytes > 48 * 1024 {
            path_batches.push(std::mem::take(&mut batch));
            size = 0;
        }
        batch.push(file.path.clone());
        size += bytes;
    }
    drop(withheld);
    if !batch.is_empty() {
        path_batches.push(batch);
    }
    let mut pending = path_batches.into_iter().enumerate();
    let mut jobs = JoinSet::new();
    observer(Progress::Reading {
        done: stored.len(),
        total: files.len(),
    });
    loop {
        while jobs.len() < options.concurrency {
            let Some((number, paths)) = pending.next() else {
                break;
            };
            let evidence = evidence.clone();
            let directory = directory.clone();
            let indices = indices.clone();
            jobs.spawn(async move {
                let content = directory.path().join(format!("batch-{number}"));
                let output = tokio::fs::OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .open(&content)
                    .await?
                    .into_std()
                    .await;
                let locations = evidence.patches(&paths, output).await?;
                locations
                    .into_iter()
                    .map(|location| {
                        let index = *indices
                            .get(&location.path)
                            .context("Unexpected path in captured patch batch")?;
                        Ok((
                            index,
                            StoredPatch {
                                content: content.clone(),
                                offset: location.offset,
                                bytes: location.bytes,
                            },
                        ))
                    })
                    .collect::<Result<Vec<_>>>()
            });
        }
        let Some(done) = jobs.join_next().await else {
            break;
        };
        for (index, span) in done?? {
            ensure!(
                stored.insert(index, span).is_none(),
                "A selected patch was captured twice"
            );
        }
        observer(Progress::Reading {
            done: stored.len(),
            total: files.len(),
        });
    }
    ensure!(
        stored.len() == files.len(),
        "Captured evidence omitted a selected file"
    );
    let stored: Vec<_> = stored.into_values().collect();
    for (file, span) in files.iter_mut().zip(&stored) {
        file.bytes = span.bytes;
    }
    snapshot.verify(repo).await?;
    let withheld_count = files.iter().filter(|file| file.withheld.is_some()).count();
    let warnings = if withheld_count > 0 {
        vec![format!(
            "{withheld_count} sensitive files: metadata included, content withheld."
        )]
    } else {
        Vec::new()
    };
    let mut prepared = PreparedAnalysis {
        repository: repo.root().to_path_buf(),
        snapshot,
        evidence,
        input_bytes: files.iter().map(|file| file.bytes).sum(),
        files,
        units: Vec::new(),
        batches: Vec::new(),
        options,
        estimated_calls: 0,
        warnings,
        directory,
        stored,
    };
    organize(&mut prepared, observer).await?;
    prepared.snapshot.verify(repo).await?;
    Ok(prepared)
}

pub(crate) async fn rechunk(
    prepared: &PreparedAnalysis,
    options: AnalysisOptions,
    observer: Observer,
) -> Result<PreparedAnalysis> {
    options.validate()?;
    let mut next = PreparedAnalysis {
        repository: prepared.repository.clone(),
        snapshot: prepared.snapshot.clone(),
        evidence: prepared.evidence.clone(),
        files: prepared.files.clone(),
        units: Vec::new(),
        batches: Vec::new(),
        options,
        input_bytes: prepared.input_bytes,
        estimated_calls: 0,
        warnings: prepared.warnings.clone(),
        directory: prepared.directory.clone(),
        stored: prepared.stored.clone(),
    };
    organize(&mut next, observer).await?;
    Ok(next)
}
async fn organize(prepared: &mut PreparedAnalysis, observer: Observer) -> Result<()> {
    let cancellation = Cancellation::default();
    let cancelled = cancellation.0.clone();
    let stored = prepared.stored.clone();
    let directory = prepared.directory.clone();
    let piece_bytes = prepared.options.chunk_bytes / 4;
    prepared.units = tokio::task::spawn_blocking(move || {
        let _directory = directory;
        let mut readers = HashMap::<PathBuf, BufReader<File>>::new();
        let mut units = BTreeMap::<String, EvidenceUnit>::new();
        let mut bytes = 0;
        for (index, span) in stored.iter().enumerate() {
            ensure!(
                !cancelled.load(Ordering::Acquire),
                "Evidence preparation cancelled"
            );
            if !readers.contains_key(&span.content) {
                readers.insert(
                    span.content.clone(),
                    BufReader::new(File::open(&span.content)?),
                );
            }
            let reader = readers
                .get_mut(&span.content)
                .context("Captured patch reader")?;
            reader.seek(SeekFrom::Start(span.offset))?;
            for unit in fragments(
                reader.take(span.bytes),
                span,
                index,
                piece_bytes,
                &cancelled,
            )? {
                if let Some(existing) = units.get_mut(&unit.id) {
                    existing.sources.extend(unit.sources);
                } else {
                    units.insert(unit.id.clone(), unit);
                }
            }
            bytes += span.bytes;
            observer(Progress::Indexing {
                done: index + 1,
                total: stored.len(),
                bytes,
            });
        }
        Ok::<_, anyhow::Error>(units.into_values().collect())
    })
    .await??;
    let budget = prepared.options.chunk_bytes * 3 / 4;
    let mut batch = Vec::new();
    let mut size = 0;
    for (index, unit) in prepared.units.iter().enumerate() {
        let bytes = unit_header(unit).len() + unit.encoded_bytes + 1;
        ensure!(
            bytes <= budget,
            "Evidence record exceeds the request budget"
        );
        if !batch.is_empty() && size + bytes > budget {
            prepared.batches.push(std::mem::take(&mut batch));
            size = 0;
        }
        batch.push(index);
        size += bytes;
    }
    if !batch.is_empty() {
        prepared.batches.push(batch);
    }
    let mut calls = if prepared.batches.len() == 1 {
        1
    } else {
        prepared.batches.len() + 1
    };
    let mut remaining = prepared.batches.len();
    while remaining > 1 {
        remaining = remaining.div_ceil(prepared.options.fan_in.min(4));
        calls += remaining;
    }
    prepared.estimated_calls = calls + prepared.options.inspection_limit();
    Ok(())
}
fn fragments(
    mut reader: impl Read,
    span: &StoredPatch,
    file: usize,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<EvidenceUnit>> {
    let mut units = Vec::new();
    let mut pending = Vec::new();
    let mut offset = 0;
    loop {
        ensure!(
            !cancelled.load(Ordering::Acquire),
            "Evidence preparation cancelled"
        );
        reader
            .by_ref()
            .take((limit + 4 - pending.len()) as u64)
            .read_to_end(&mut pending)?;
        if pending.is_empty() {
            break;
        }
        let mut end = pending.len().min(limit);
        while end > 0 && end < pending.len() && pending[end] & 0xc0 == 0x80 {
            end -= 1;
        }
        if end == 0 {
            end = pending.len().min(limit);
        }
        let raw: Vec<_> = pending.drain(..end).collect();
        let encoded_bytes = if std::str::from_utf8(&raw).is_ok() {
            raw.len()
        } else {
            payload(&raw).len()
        };
        units.push(EvidenceUnit {
            id: digest(&raw),
            content: span.content.clone(),
            offset: span.offset + offset,
            bytes: raw.len(),
            encoded_bytes,
            sources: vec![SourceRef {
                file,
                start: offset,
                end: offset + raw.len() as u64,
            }],
        });
        offset += raw.len() as u64;
    }
    ensure!(
        offset == span.bytes,
        "Captured patch ended before its declared length"
    );
    Ok(units)
}
pub(crate) fn payload(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_owned(),
        Err(_) => {
            serde_json::json!({"encoding":"base64","bytes":STANDARD.encode(bytes)}).to_string()
        }
    }
}
fn unit_header(unit: &EvidenceUnit) -> String {
    format!(
        "\nSOURCE {} · {} bytes · {} occurrences; inspect this ID for all file references\n",
        unit.id,
        unit.bytes,
        unit.sources.len()
    )
}
impl PreparedAnalysis {
    pub async fn batch_input(&self, batch: &[usize]) -> Result<String> {
        let mut input = String::new();
        for &index in batch {
            let unit = self.units.get(index).context("Unknown evidence unit")?;
            input.push_str(&unit_header(unit));
            input.push_str(&payload(&unit.read().await?));
            input.push('\n');
        }
        ensure!(
            input.len() <= self.options.chunk_bytes,
            "Chunk input exceeded its declared budget"
        );
        Ok(input)
    }
}
