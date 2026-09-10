pub mod cache;
mod corpus;
mod engine;
mod execution;
mod inspect;
pub mod progress;
pub mod proposal;

use anyhow::{Result, bail};
use futures::future::BoxFuture;
use kiri_core::{
    model::RepoPath,
    review::{EvidenceTree, ReviewSnapshot},
};
use progress::Observer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

pub use corpus::{prepare, prepare_captured, prepare_snapshot};
pub use engine::analyze;
pub use inspect::{EvidencePage, Inspection, inspect, synthesize};

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisMode {
    #[default]
    Fast,
    Deep,
}
impl std::fmt::Display for AnalysisMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Fast => "fast",
            Self::Deep => "deep",
        })
    }
}
impl std::str::FromStr for AnalysisMode {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fast" => Ok(Self::Fast),
            "deep" => Ok(Self::Deep),
            _ => Err("Choose fast or deep".into()),
        }
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnalysisOptions {
    pub mode: AnalysisMode,
    pub concurrency: usize,
    pub chunk_bytes: usize,
    pub fan_in: usize,
    pub max_calls: usize,
    pub cache: bool,
    pub worker_model: Option<String>,
    pub inspection_rounds: usize,
}
impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            mode: AnalysisMode::Fast,
            concurrency: 8,
            chunk_bytes: 1_048_576,
            fan_in: 12,
            max_calls: 4096,
            cache: true,
            worker_model: None,
            inspection_rounds: 6,
        }
    }
}
impl AnalysisOptions {
    pub fn inspection_limit(&self) -> usize {
        match self.mode {
            AnalysisMode::Fast => 0,
            AnalysisMode::Deep => self.inspection_rounds,
        }
    }
    pub fn validate(&self) -> Result<()> {
        if !(1..=32).contains(&self.concurrency)
            || !(4096..=1_048_576).contains(&self.chunk_bytes)
            || !(2..=24).contains(&self.fan_in)
            || self.max_calls == 0
            || self.inspection_rounds > 32
        {
            bail!(
                "Invalid analysis limits: concurrency 1–32, chunk bytes 4096–1048576, fan-in 2–24, positive call budget, inspection rounds 0–32"
            );
        }
        if self
            .worker_model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty() || model.len() > 512)
        {
            bail!("Invalid worker model identifier");
        }
        Ok(())
    }
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EvidenceFile {
    pub path: RepoPath,
    pub change: char,
    pub bytes: u64,
    pub withheld: Option<String>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRef {
    pub file: usize,
    pub start: u64,
    pub end: u64,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug)]
pub struct EvidenceUnit {
    pub id: String,
    pub content: PathBuf,
    pub offset: u64,
    pub bytes: usize,
    pub encoded_bytes: usize,
    pub sources: Vec<SourceRef>,
}

#[derive(Clone)]
pub(crate) struct StoredPatch {
    pub content: PathBuf,
    pub offset: u64,
    pub bytes: u64,
}

impl EvidenceUnit {
    pub async fn read(&self) -> Result<Vec<u8>> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::open(&self.content).await?;
        file.seek(std::io::SeekFrom::Start(self.offset)).await?;
        let mut bytes = vec![0; self.bytes];
        file.read_exact(&mut bytes).await?;
        Ok(bytes)
    }
}

#[derive(Clone)]
pub struct PreparedAnalysis {
    pub repository: PathBuf,
    pub snapshot: ReviewSnapshot,
    pub evidence: EvidenceTree,
    pub files: Vec<EvidenceFile>,
    pub units: Vec<EvidenceUnit>,
    pub batches: Vec<Vec<usize>>,
    pub options: AnalysisOptions,
    pub input_bytes: u64,
    pub estimated_calls: usize,
    pub warnings: Vec<String>,
    pub(crate) directory: Arc<tempfile::TempDir>,
    pub(crate) stored: Vec<StoredPatch>,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AnalysisReport {
    pub files: usize,
    pub bytes: u64,
    pub unique_chunks: usize,
    pub model_calls: usize,
    pub cache_hits: usize,
    pub reduction_levels: usize,
}
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisNode {
    pub id: String,
    pub summary: String,
    pub children: Vec<String>,
    pub source_units: usize,
}
pub struct AnalysisResult {
    pub context: String,
    pub report: AnalysisReport,
    pub nodes: Vec<AnalysisNode>,
    pub root_ids: Vec<String>,
}

#[derive(Debug)]
pub struct ContextOverflow;
impl std::fmt::Display for ContextOverflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The model rejected the context size")
    }
}
impl std::error::Error for ContextOverflow {}

pub trait LanguageModel: Send + Sync {
    fn identity(&self) -> String;
    fn remaining_calls(&self) -> Option<usize> {
        None
    }
    fn complete<'a>(
        &'a self,
        system: &'a str,
        input: &'a str,
        schema: Value,
    ) -> BoxFuture<'a, Result<String>>;
}
pub trait AnalysisCache: Send + Sync {
    fn load<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<AnalysisNode>>>;
    fn save<'a>(&'a self, node: &'a AnalysisNode) -> BoxFuture<'a, Result<()>>;
}
pub struct AnalysisRuntime {
    pub model: Arc<dyn LanguageModel>,
    pub worker: Arc<dyn LanguageModel>,
    pub cache: Arc<dyn AnalysisCache>,
    pub observer: Observer,
}
impl PreparedAnalysis {
    pub fn scope_paths(&self) -> Vec<RepoPath> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }
    pub fn overview(&self) -> Value {
        let mut folders = BTreeMap::<String, usize>::new();
        let mut kinds = BTreeMap::<char, usize>::new();
        for file in &self.files {
            let path = file.path.display();
            *folders
                .entry(
                    path.split_once('/')
                        .map(|(dir, _)| dir)
                        .unwrap_or("(root)")
                        .to_owned(),
                )
                .or_default() += 1;
            *kinds.entry(file.change).or_default() += 1;
        }
        serde_json::json!({"file_count":self.files.len(),"patch_bytes":self.input_bytes,"folders":folders,"change_types":kinds,"warnings":self.warnings})
    }
    pub fn storage_path(&self) -> &std::path::Path {
        self.directory.path()
    }
}

pub(crate) fn sensitive_path(path: &str) -> bool {
    path.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        part == ".env"
            || part.starts_with(".env.")
            || matches!(
                part.as_str(),
                "credentials.json" | "secrets.json" | "id_rsa" | "id_ed25519"
            )
            || part.ends_with(".pem")
            || part.ends_with(".key")
    })
}
