use crate::{
    commit::StagedSnapshot,
    model::{RepoPath, terminal_text},
    repo::Repository,
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

pub struct PatchLocation {
    pub path: RepoPath,
    pub change: char,
    pub offset: u64,
    pub bytes: u64,
}

impl Repository {
    pub async fn snapshot_changes(&self, base: &str, tree: &str) -> Result<Vec<(RepoPath, char)>> {
        validate_trees(base, tree)?;
        let bytes = self
            .git(&["diff", "--name-status", "-z", "--no-renames", base, tree])
            .await?;
        let mut records = bytes.split(|b| *b == 0);
        let mut files = Vec::new();
        while let Some(kind) = records.next().filter(|r| !r.is_empty()) {
            if !matches!(kind, b"A" | b"M" | b"D" | b"T") {
                bail!("Unsupported staged change type");
            }
            files.push((
                RepoPath::new(records.next().context("Missing staged filename")?.to_vec())?,
                char::from(kind[0]),
            ));
        }
        Ok(files)
    }
    pub async fn snapshot_base(&self, snapshot: &StagedSnapshot) -> Result<String> {
        if let Some(head) = &snapshot.head {
            return Ok(head.clone());
        }
        Ok(String::from_utf8(
            self.git(&["hash-object", "-w", "-t", "tree", "--stdin"])
                .await?,
        )?
        .trim()
        .to_owned())
    }
    pub async fn snapshot_patch(
        &self,
        base: &str,
        tree: &str,
        path: &RepoPath,
        destination: File,
    ) -> Result<()> {
        self.write_patches(base, tree, std::slice::from_ref(path), destination, false)
            .await
    }
    pub async fn snapshot_patches(
        &self,
        base: &str,
        tree: &str,
        paths: &[RepoPath],
        destination: File,
    ) -> Result<Vec<PatchLocation>> {
        ensure!(
            !paths.is_empty()
                && paths
                    .iter()
                    .map(|path| path.bytes().len() + 1)
                    .sum::<usize>()
                    <= 48 * 1024,
            "Invalid patch batch size"
        );
        let reader = destination.try_clone()?;
        self.write_patches(base, tree, paths, destination, true)
            .await?;
        let locations = tokio::task::spawn_blocking(move || patch_locations(reader)).await??;
        let expected: std::collections::HashSet<_> = paths.iter().collect();
        let actual: std::collections::HashSet<_> =
            locations.iter().map(|location| &location.path).collect();
        ensure!(
            locations.len() == paths.len() && actual == expected,
            "Git patch batch did not cover the selected paths exactly"
        );
        Ok(locations)
    }
    async fn write_patches(
        &self,
        base: &str,
        tree: &str,
        paths: &[RepoPath],
        destination: File,
        raw: bool,
    ) -> Result<()> {
        validate_trees(base, tree)?;
        let mut command = self.command();
        command.args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--no-renames",
            "--no-abbrev",
            "--diff-algorithm=myers",
            "--no-indent-heuristic",
            "--submodule=short",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--unified=3",
        ]);
        if raw {
            command.args(["--raw", "-z", "--patch"]);
        }
        command.args([base, tree, "--"]);
        for path in paths {
            command.arg(path.to_path_buf());
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::from(destination))
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stderr = child.stderr.take().context("Missing Git error stream")?;
        let operation = async {
            let errors = async {
                let mut bytes = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let size = stderr.read(&mut buffer).await?;
                    if size == 0 {
                        break;
                    }
                    bytes.extend_from_slice(
                        &buffer[..size.min(65536_usize.saturating_sub(bytes.len()))],
                    );
                }
                Ok::<_, std::io::Error>(bytes)
            };
            let (status, errors) = tokio::try_join!(child.wait(), errors)?;
            ensure!(
                status.success(),
                "Could not capture selected patches: {}",
                terminal_text(&String::from_utf8_lossy(&errors))
            );
            Ok::<_, anyhow::Error>(())
        };
        tokio::time::timeout(Duration::from_secs(120), operation)
            .await
            .context("Reading patches timed out; no changes were committed")??;
        Ok(())
    }
}
fn validate_trees(base: &str, tree: &str) -> Result<()> {
    ensure!(
        [base, tree]
            .iter()
            .all(|value| crate::model::is_object_id(value)),
        "Invalid snapshot tree identity"
    );
    Ok(())
}
fn patch_locations(mut file: File) -> Result<Vec<PatchLocation>> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file);
    let mut records = Vec::new();
    loop {
        let mut header = Vec::new();
        reader.by_ref().take(4097).read_until(0, &mut header)?;
        if header == [0] {
            break;
        }
        ensure!(
            header.len() <= 4096 && header.last() == Some(&0),
            "Malformed raw patch header"
        );
        let fields: Vec<_> = header[..header.len() - 1]
            .split(|byte| *byte == b' ')
            .collect();
        ensure!(
            fields.len() == 5 && fields[0].starts_with(b":"),
            "Malformed raw patch metadata"
        );
        let kind = fields[4];
        ensure!(
            matches!(kind, b"A" | b"M" | b"D" | b"T"),
            "Unexpected patch change type"
        );
        let mut path = Vec::new();
        reader
            .by_ref()
            .take(48 * 1024 + 1)
            .read_until(0, &mut path)?;
        ensure!(path.last() == Some(&0), "Missing patch filename terminator");
        path.pop();
        records.push((RepoPath::new(path)?, char::from(kind[0])));
    }
    let mut offset = reader.stream_position()?;
    let mut starts = Vec::new();
    let mut matched = Some(0);
    let prefix = b"diff --git ";
    let mut buffer = [0; 65536];
    loop {
        let size = reader.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        for (index, &byte) in buffer[..size].iter().enumerate() {
            if let Some(count) = matched {
                if byte == prefix[count] {
                    if count + 1 == prefix.len() {
                        starts.push(offset + index as u64 + 1 - prefix.len() as u64);
                        matched = None;
                    } else {
                        matched = Some(count + 1);
                    }
                } else {
                    matched = None;
                }
            }
            if byte == b'\n' {
                matched = Some(0);
            }
        }
        offset += size as u64;
    }
    ensure!(
        starts.len() == records.len(),
        "Patch headers do not match raw path metadata"
    );
    starts.push(offset);
    Ok(records
        .into_iter()
        .enumerate()
        .map(|(index, (path, change))| PatchLocation {
            path,
            change,
            offset: starts[index],
            bytes: starts[index + 1] - starts[index],
        })
        .collect())
}
