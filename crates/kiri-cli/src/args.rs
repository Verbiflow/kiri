use clap::{Parser, Subcommand};
use kiri_ai::config::Provider;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kiri",
    version,
    about = "Fast Git review. Thoughtful commits.",
    after_help = "Run without a command to open the TUI. AI only runs when requested; commits require --yes or an explicit confirmation in the TUI."
)]
pub struct Args {
    #[arg(
        short = 'C',
        long,
        global = true,
        default_value = ".",
        help = "Repository or a directory inside it"
    )]
    pub repo: PathBuf,
    #[arg(
        long,
        global = true,
        default_value = "always",
        value_name = "WHEN",
        help = "TUI colors are on by default. Use auto to honor NO_COLOR, or never for monochrome."
    )]
    pub color: kiri_tui::ColorMode,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Open the interactive workspace")]
    Tui,
    #[command(about = "List staged and working-tree changes without computing patches")]
    Status {
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Update remote-tracking refs without changing working files")]
    Fetch,
    #[command(about = "Pull the current upstream, fast-forward only; never auto-stash")]
    Pull {
        #[arg(long)]
        yes: bool,
    },
    #[command(about = "Push this branch to its configured upstream, never force-push")]
    Push {
        #[arg(long)]
        yes: bool,
    },
    #[command(about = "List local branches or switch to one")]
    Branch {
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Show recent commit history")]
    Log {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Inspect one file, or open the diff browser")]
    Diff {
        path: Option<PathBuf>,
        #[arg(long)]
        staged: bool,
        #[arg(long)]
        large: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Stage exactly the selected files or one hunk")]
    Stage {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long, help = "1-based hunk number; requires one file")]
        hunk: Option<usize>,
    },
    #[command(about = "Unstage selected files or one hunk; never discard working files")]
    Unstage {
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        #[arg(long)]
        hunk: Option<usize>,
    },
    #[command(about = "Ask AI for a commit message for the current staged snapshot")]
    Draft {
        #[command(flatten)]
        analysis: AnalysisArgs,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Commit an approved draft, a message, or a fresh AI message")]
    Commit {
        #[arg(long, conflicts_with_all = ["message", "ai"])]
        draft: Option<PathBuf>,
        #[arg(short, long, conflicts_with = "ai")]
        message: Option<String>,
        #[arg(long)]
        ai: bool,
        #[arg(long, help = "Approve creating this commit")]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Propose cohesive, file-level commits from staged changes; does not commit")]
    Plan {
        #[command(flatten)]
        analysis: AnalysisArgs,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Create the reviewed commits from a saved JSON plan")]
    Apply {
        plan: PathBuf,
        #[arg(long)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Connect, select, or inspect AI providers")]
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
    #[command(about = "Manage saved project workspaces")]
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    #[command(about = "Read-only benchmark of discovery, status, and first-file preview")]
    Bench {
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u16).range(1..101))]
        runs: u16,
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Args)]
pub struct AnalysisArgs {
    #[arg(
        long,
        help = "fast: complete coverage with direct synthesis; deep: optional source inspection and deeper reasoning where supported"
    )]
    pub mode: Option<kiri_ai::analysis::AnalysisMode>,
    #[arg(help = "Only these staged files or folders; omit to analyze all staged changes")]
    pub paths: Vec<PathBuf>,
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..33))]
    pub concurrency: Option<u16>,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub max_calls: Option<u32>,
    #[arg(
        long,
        help = "Use this model for chunk analysis; the selected model combines the results"
    )]
    pub worker_model: Option<String>,
    #[arg(
        long,
        help = "Regenerate chunk summaries instead of reusing cached results"
    )]
    pub no_cache: bool,
}

#[derive(Subcommand)]
pub enum ProviderCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Connect {
        provider: Provider,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        endpoint: Option<String>,
        #[arg(long)]
        region: Option<String>,
        #[arg(long)]
        aws_profile: Option<String>,
        #[arg(
            long,
            help = "Read the API key from stdin, never from a command-line argument"
        )]
        key_stdin: bool,
    },
    Use {
        provider: Provider,
        #[arg(long)]
        model: Option<String>,
    },
    Login {
        provider: Provider,
    },
}

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    List {
        #[arg(long)]
        json: bool,
    },
    Add {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        path: PathBuf,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use kiri_tui::ColorMode;

    #[test]
    fn default_launch_enables_tui_colors_even_with_no_color() -> anyhow::Result<()> {
        for arguments in [
            &["kiri"][..],
            &["kiri", "-C", "/project"][..],
            &["kiri", "tui"][..],
            &["kiri", "diff", "--staged"][..],
        ] {
            let args = Args::try_parse_from(arguments)?;
            assert_eq!(args.color, ColorMode::Always);
            assert!(args.color.enabled(Some("1")));
        }
        Ok(())
    }

    #[test]
    fn explicit_color_policy_is_preserved() -> anyhow::Result<()> {
        for (flag, expected) in [("auto", ColorMode::Auto), ("never", ColorMode::Never)] {
            let args = Args::try_parse_from(["kiri", "--color", flag])?;
            assert_eq!(args.color, expected);
            assert!(!args.color.enabled(Some("1")));
        }
        Ok(())
    }
}
