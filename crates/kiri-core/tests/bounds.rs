use anyhow::Result;
use kiri_core::{diff::DiffDocument, process, storage::Store};
use serde_json::Value;
use std::{
    fs,
    time::{Duration, Instant},
};
use tokio::process::Command;

#[tokio::test]
async fn output_budget_stops_a_producer_instead_of_buffering_it() -> Result<()> {
    let command = Command::new("yes");
    let output = process::run(command, None, 4096, Duration::from_secs(2)).await?;
    assert!(output.truncated);
    assert_eq!(output.stdout.len(), 4096);
    Ok(())
}

#[tokio::test]
async fn mutation_log_limits_preserve_exit_status_and_drain_both_streams() -> Result<()> {
    for code in [0, 7] {
        let mut command = Command::new("sh");
        command.args(["-c", &format!("i=0; while [ $i -lt 12000 ]; do printf 'stdout progress line\\n'; printf 'stderr progress line\\n' >&2; i=$((i+1)); done; exit {code}")]);
        let output = process::run_mutation(command, None, Duration::from_secs(10)).await?;
        assert_eq!(output.status.code(), Some(code));
        assert!(output.logs_abbreviated);
        assert_eq!(output.stdout_excerpt.len(), 65536);
        assert_eq!(output.stderr_excerpt.len(), 65536);
    }
    Ok(())
}

#[tokio::test]
async fn mutation_timeout_is_an_unknown_outcome_not_an_asserted_failure() {
    let mut command = Command::new("sleep");
    command.arg("30");
    let error = match process::run_mutation(command, None, Duration::from_millis(50)).await {
        Ok(_) => panic!("Expected timeout"),
        Err(error) => error,
    };
    assert!(
        error
            .downcast_ref::<process::MutationCompletionUnknown>()
            .is_some()
    );
}

#[tokio::test]
async fn timeout_cancels_a_slow_process() {
    let mut command = Command::new("sleep");
    command.arg("30");
    let started = Instant::now();
    assert!(
        process::run(command, None, 4096, Duration::from_millis(50))
            .await
            .is_err()
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn line_budget_prevents_pathological_small_line_allocations() {
    let document = DiffDocument::parse(vec![b'\n'; 1_000_000], false);
    assert!(document.truncated);
    assert_eq!(document.lines.len(), 20000);
    assert_eq!(document.raw.len(), 20000);
}

#[test]
fn truncated_minified_files_keep_a_readable_prefix() {
    let mut raw = b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -0,0 +1 @@\n+minified=".to_vec();
    raw.extend(vec![b'x'; 4096]);
    let document = DiffDocument::parse(raw, true);
    assert!(
        document
            .lines
            .last()
            .is_some_and(|line| line.kind == kiri_core::diff::LineKind::Added)
    );
    assert!(document.hunk_patch(0).is_err());
}

#[test]
fn diagnostic_lines_remain_readable_without_allowing_terminal_control_sequences() {
    let message = kiri_core::model::terminal_message("first line\nsecond line\u{1b}[2J\u{202e}");
    assert_eq!(message.lines().count(), 2);
    assert!(!message.contains("\\n"));
    assert!(!message.contains('\u{1b}'));
    assert!(!message.contains('\u{202e}'));
    assert_eq!(
        kiri_core::model::terminal_text("filename\npart"),
        "filename\\npart"
    );
}

#[test]
fn corrupt_state_is_not_silently_replaced() -> Result<()> {
    let temp = tempfile::TempDir::new()?;
    let store = Store::at(temp.path());
    fs::write(store.path("settings.json"), "not-json")?;
    assert!(
        store
            .update::<Value, _>("settings.json", |_| Ok(()))
            .is_err()
    );
    assert_eq!(fs::read_to_string(store.path("settings.json"))?, "not-json");
    Ok(())
}

#[cfg(unix)]
#[test]
fn persisted_credentials_are_owner_only() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::TempDir::new()?;
    let store = Store::at(temp.path());
    store.update::<Value, _>("credentials.json", |value| {
        *value = serde_json::json!({"placeholder":"test-only"});
        Ok(())
    })?;
    assert_eq!(
        fs::metadata(store.path("credentials.json"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    Ok(())
}
