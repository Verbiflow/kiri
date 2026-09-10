use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum Progress {
    Reading {
        done: usize,
        total: usize,
    },
    Indexing {
        done: usize,
        total: usize,
        bytes: u64,
    },
    Analyzing {
        done: usize,
        total: usize,
        cached: usize,
    },
    Reducing {
        level: usize,
        done: usize,
        total: usize,
    },
    Retrying {
        attempt: usize,
        delay_ms: u64,
    },
    Inspecting {
        round: usize,
        sources: usize,
    },
    Drafting,
}
impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Reading { done, total } => {
                write!(
                    f,
                    "Capturing local patches: {done}/{total} files · no model calls"
                )
            }
            Self::Indexing { done, total, bytes } => write!(
                f,
                "Organizing local evidence: {done}/{total} files · {:.1} MiB",
                *bytes as f64 / 1048576.0
            ),
            Self::Analyzing {
                done,
                total,
                cached,
            } => write!(f, "Analyzing chunks: {done}/{total} · {cached} cached"),
            Self::Reducing { level, done, total } => {
                write!(f, "Combining summaries · level {level}: {done}/{total}")
            }
            Self::Retrying { attempt, delay_ms } => write!(
                f,
                "Provider temporarily unavailable · retry {attempt} in {:.1}s",
                *delay_ms as f64 / 1000.0
            ),
            Self::Inspecting { round, sources } => write!(
                f,
                "Inspecting original evidence · round {round} · {sources} sources"
            ),
            Self::Drafting => f.write_str("Writing the final commit proposal"),
        }
    }
}
pub type Observer = Arc<dyn Fn(Progress) + Send + Sync>;
pub fn silent() -> Observer {
    Arc::new(|_| {})
}
