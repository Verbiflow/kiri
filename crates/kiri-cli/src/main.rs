mod args;
mod bench;
mod settings;

use anyhow::{Context, Result, bail};
use args::{Args, Command};
use clap::Parser;
use kiri_ai::{
    provider::AiClient,
    workflow::{self, CommitDraft, CommitPlan},
};
use kiri_core::{
    model::{DiffSide, RepoPath, terminal_text},
    repo::Repository,
    storage::Store,
};
use std::{io::IsTerminal, path::PathBuf};

#[tokio::main]
async fn main() {
    if let Err(error) = run(Args::parse()).await {
        eprintln!("kiri: {}", terminal_text(&format!("{error:#}")));
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<()> {
    let store = Store::discover()?;
    let command = args.command.unwrap_or(Command::Tui);
    match command {
        Command::Tui => return tui(args.repo, store, DiffSide::Worktree, args.color).await,
        Command::Provider { command } => return settings::provider(&store, command).await,
        Command::Workspace { command } => return settings::workspace(&store, command).await,
        Command::Bench { runs, json } => return bench::run(&args.repo, runs, json).await,
        Command::Diff {
            path: None, staged, ..
        } => return tui(args.repo, store, side(staged), args.color).await,
        _ => {}
    }
    let repo = Repository::open(&args.repo).await?;
    match command {
        Command::Fetch | Command::Pull { .. } | Command::Push { .. } => {
            use kiri_core::sync::SyncAction;
            let action = match command {
                Command::Fetch => SyncAction::Fetch,
                Command::Pull { yes: true } => SyncAction::Pull,
                Command::Push { yes: true } => SyncAction::Push,
                _ => bail!("Review the branch and add --yes to approve pull or push."),
            };
            let target = kiri_core::sync::SyncTarget::from(&repo.status().await?);
            repo.sync(action, Some(&target)).await?;
            let status = repo.status().await?;
            println!(
                "{} complete. {} incoming, {} outgoing.",
                action.label(),
                status.behind,
                status.ahead
            );
        }
        Command::Branch { name, json } => {
            if let Some(name) = name {
                repo.switch_branch(&name).await?;
                println!("Switched to {}", terminal_text(&name));
            } else {
                let branches = repo.branches().await?;
                if json {
                    println!("{}", serde_json::to_string_pretty(&branches)?);
                } else {
                    for branch in branches {
                        println!(
                            "{} {:30} {}",
                            if branch.current { ">" } else { " " },
                            terminal_text(&branch.name),
                            terminal_text(&branch.upstream)
                        );
                    }
                }
            }
        }
        Command::Log { limit, json } => {
            let history = repo.history(limit).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&history)?);
            } else {
                for entry in history {
                    println!(
                        "{}  {}  {} · {}",
                        entry.short_oid,
                        terminal_text(&entry.subject),
                        terminal_text(&entry.author),
                        terminal_text(&entry.age)
                    );
                }
            }
        }
        Command::Status { json } => {
            let status = repo.status().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "{}  {} changed files  |  {} incoming · {} outgoing · cached refs",
                    terminal_text(&status.branch),
                    status.files.len(),
                    status.behind,
                    status.ahead
                );
                for file in status.files {
                    println!(
                        "{}{} {}",
                        file.staged.map(|k| k.letter()).unwrap_or(' '),
                        file.worktree.map(|k| k.letter()).unwrap_or(' '),
                        file.path
                    );
                }
            }
        }
        Command::Diff {
            path: Some(path),
            staged,
            large,
            json,
        } => {
            let path = RepoPath::from_path(&path)?;
            let file = repo
                .status()
                .await?
                .files
                .into_iter()
                .find(|f| f.path == path)
                .context("This file has no changes. Paths are relative to the repository root.")?;
            let doc = repo.diff(&file, side(staged), large).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"path":file.path,"patch":String::from_utf8_lossy(&doc.raw),"truncated":doc.truncated,"binary":doc.binary,"notice":doc.notice,"hunks":doc.hunks.len()})
                    )?
                );
            } else {
                if let Some(notice) = doc.notice {
                    println!("{notice}");
                }
                for line in String::from_utf8_lossy(&doc.raw).lines() {
                    println!("{}", terminal_text(line));
                }
                if doc.truncated {
                    eprintln!(
                        "Preview capped at 512 KiB or 20,000 lines. Use Git directly for the complete patch."
                    );
                }
            }
        }
        Command::Stage { paths, hunk } => {
            change_index(&repo, paths, hunk, DiffSide::Worktree).await?
        }
        Command::Unstage { paths, hunk } => {
            change_index(&repo, paths, hunk, DiffSide::Staged).await?
        }
        Command::Draft { analysis, json } => {
            let (client, prepared) = prepare_analysis(&repo, &store, analysis).await?;
            let draft = workflow::draft_prepared(&repo, &client, prepared).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&draft)?);
            } else {
                println!("{}", draft.message);
                for warning in draft.warnings {
                    eprintln!("{warning}");
                }
            }
        }
        Command::Commit {
            draft,
            message,
            ai,
            yes,
            json,
        } => {
            if !yes {
                bail!("Commit requires --yes. Review a message with `kiri draft` first.");
            }
            let draft = if let Some(path) = draft {
                read_input::<CommitDraft>(&path)?
            } else if ai {
                workflow::draft(&repo, &AiClient::configured(&store)?, None).await?
            } else {
                let mut draft = CommitDraft::manual(&repo, None).await?;
                draft.message = message.context("Supply --draft, --message, or --ai")?;
                draft
            };
            let oid = draft.commit(&repo).await?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({"commit":oid,"message":draft.message})
                );
            } else {
                println!(
                    "Committed {}  {}",
                    &oid[..oid.len().min(12)],
                    terminal_text(draft.message.lines().next().unwrap_or_default())
                );
            }
        }
        Command::Plan { analysis, json } => {
            let (client, prepared) = prepare_analysis(&repo, &store, analysis).await?;
            let plan = workflow::plan_prepared(&repo, &client, prepared).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&plan)?);
            } else {
                for (index, group) in plan.groups.iter().enumerate() {
                    println!(
                        "\n{}. {}\n   {}",
                        index + 1,
                        terminal_text(&group.message),
                        terminal_text(&group.reason)
                    );
                    for file in plan.files.iter().filter(|f| group.files.contains(&f.id)) {
                        println!("   {}", file.path);
                    }
                }
                for warning in plan.warnings {
                    eprintln!("{warning}");
                }
                println!(
                    "\nNo commits created. Save with `kiri plan --json > plan.json`, review, then `kiri apply plan.json --yes`."
                );
            }
        }
        Command::Apply { plan, yes, json } => {
            if !yes {
                bail!("Review the saved plan, then add --yes to create its commits.");
            }
            let plan: CommitPlan = read_input(&plan)?;
            let commits = plan
                .apply(&repo, |index, oid| {
                    eprintln!("Created group {}: {}", index + 1, oid)
                })
                .await?;
            if json {
                println!("{}", serde_json::json!({"commits":commits}));
            } else {
                println!(
                    "Created {} commits. Working files were preserved.",
                    commits.len()
                );
            }
        }
        Command::Tui
        | Command::Provider { .. }
        | Command::Workspace { .. }
        | Command::Bench { .. }
        | Command::Diff { path: None, .. } => unreachable!(),
    }
    Ok(())
}

async fn change_index(
    repo: &Repository,
    paths: Vec<PathBuf>,
    hunk: Option<usize>,
    side: DiffSide,
) -> Result<()> {
    let paths: Vec<_> = paths
        .iter()
        .map(|p| RepoPath::from_path(p))
        .collect::<Result<_>>()?;
    if let Some(hunk) = hunk {
        if paths.len() != 1 || hunk == 0 {
            bail!("--hunk requires exactly one file and a 1-based hunk number");
        }
        let file = repo
            .status()
            .await?
            .files
            .into_iter()
            .find(|f| f.path == paths[0])
            .context("File has no changes")?;
        let doc = repo.diff(&file, side, false).await?;
        repo.stage_hunk(&file, side, &doc, hunk - 1).await?;
    } else {
        match side {
            DiffSide::Worktree => repo.stage(&paths).await?,
            DiffSide::Staged => repo.unstage(&paths).await?,
        }
    }
    println!(
        "{} selection. Working files were preserved.",
        if side == DiffSide::Worktree {
            "Staged"
        } else {
            "Unstaged"
        }
    );
    Ok(())
}

async fn prepare_analysis(
    repo: &Repository,
    store: &Store,
    args: args::AnalysisArgs,
) -> Result<(
    AiClient,
    std::sync::Arc<kiri_ai::analysis::PreparedAnalysis>,
)> {
    let client = AiClient::configured(store)?;
    let mut options = kiri_ai::config::Settings::load(store)?.analysis;
    if let Some(concurrency) = args.concurrency {
        options.concurrency = usize::from(concurrency);
    }
    if let Some(mode) = args.mode {
        options.mode = mode;
    }
    if let Some(max_calls) = args.max_calls {
        options.max_calls = max_calls as usize;
    }
    if let Some(model) = args.worker_model {
        options.worker_model = Some(model);
    }
    if args.no_cache {
        options.cache = false;
    }
    let paths: Vec<_> = args
        .paths
        .iter()
        .map(|path| RepoPath::from_path(path))
        .collect::<Result<_>>()?;
    let scope = if paths.is_empty() {
        None
    } else {
        Some(paths.as_slice())
    };
    let prepared = kiri_ai::analysis::prepare(repo, scope, options, client.observer()).await?;
    Ok((client, std::sync::Arc::new(prepared)))
}

fn read_input<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Result<T> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        bail!("Draft or plan exceeds 16 MiB");
    }
    serde_json::from_slice(&bytes).context("Invalid draft or plan JSON")
}

fn side(staged: bool) -> DiffSide {
    if staged {
        DiffSide::Staged
    } else {
        DiffSide::Worktree
    }
}

async fn tui(
    path: PathBuf,
    store: Store,
    side: DiffSide,
    color: kiri_tui::ColorMode,
) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!(
            "The TUI needs a terminal. Use `kiri status --json`, `kiri diff <file>`, or `kiri --help` for non-interactive commands."
        );
    }
    kiri_tui::run(path, store, side, color).await
}
