use anyhow::Result;
use kiri_core::{model::DiffSide, repo::Repository};
use serde_json::json;
use std::{path::Path, time::Instant};

pub async fn run(path: &Path, runs: u16, machine: bool) -> Result<()> {
    let mut open = Vec::new();
    let mut status = Vec::new();
    let mut diff = Vec::new();
    let mut total = Vec::new();
    let mut files = 0;
    let mut preview_bytes = 0;
    for _ in 0..runs {
        let started = Instant::now();
        let repo = Repository::open(path).await?;
        open.push(started.elapsed().as_secs_f64() * 1000.0);
        let phase = Instant::now();
        let snapshot = repo.status().await?;
        status.push(phase.elapsed().as_secs_f64() * 1000.0);
        files = snapshot.files.len();
        let phase = Instant::now();
        if let Some(file) = snapshot.files.first() {
            let side = if file.worktree.is_some() {
                DiffSide::Worktree
            } else {
                DiffSide::Staged
            };
            let document = repo.diff(file, side, false).await?;
            preview_bytes = document.raw.len();
        }
        diff.push(phase.elapsed().as_secs_f64() * 1000.0);
        total.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let report = json!({"runs":runs,"changed_files":files,"first_preview_bytes":preview_bytes,
        "open_ms":distribution(open),"status_ms":distribution(status),"first_diff_ms":distribution(diff),"total_ms":distribution(total)});
    if machine {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{files} changed files, {runs} runs, selected-file preview only\n");
        for key in ["open_ms", "status_ms", "first_diff_ms", "total_ms"] {
            println!(
                "{key:18} median {:8.2} ms   p95 {:8.2} ms",
                report[key]["median"].as_f64().unwrap_or_default(),
                report[key]["p95"].as_f64().unwrap_or_default()
            );
        }
        println!("\nNo repository-wide patch, line count, or AI request was generated.");
    }
    Ok(())
}

fn distribution(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let at = |p: f64| {
        values
            .get(((values.len() as f64 * p).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or_default()
    };
    json!({"median":at(0.5),"p95":at(0.95),"max":values.last()})
}
