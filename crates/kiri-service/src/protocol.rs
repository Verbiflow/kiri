use kiri_analysis::{
    AnalysisOptions, EvidencePage, Inspection,
    progress::Progress,
    proposal::{CommitDraft, CommitPlan},
};
use kiri_core::model::{DiffSide, RepoPath, RepoStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SourceUnit {
    pub id: String,
    pub bytes: usize,
    pub sources: Vec<kiri_analysis::SourceRef>,
}

pub const VERSION: u32 = 2;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

pub fn schema() -> anyhow::Result<Value> {
    let mut document = canonical(
        serde_json::json!({"version":VERSION,"request":schemars::schema_for!(Request),"frame":schemars::schema_for!(Frame)}),
    );
    let hash = kiri_core::model::digest(&serde_json::to_vec(&document)?);
    document["schema_hash"] = Value::String(hash);
    Ok(canonical(document))
}
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        value => value,
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct RepoId(pub u32);
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u32,
    pub command: Command,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Hello {
        version: u32,
        #[serde(default)]
        schema_hash: Option<String>,
    },
    Open {
        path: String,
    },
    Close {
        repo: RepoId,
    },
    Status {
        repo: RepoId,
        fresh: bool,
        /// Revision the caller already holds. When the repository still reports that revision,
        /// the reply is `unchanged` instead of the full inventory.
        #[serde(default)]
        since: Option<u64>,
    },
    Preview {
        repo: RepoId,
        path: RepoPath,
        side: DiffSide,
        large: bool,
    },
    Compare {
        repo: RepoId,
        path: RepoPath,
        comparison: kiri_core::workbench::Comparison,
    },
    Log {
        repo: RepoId,
        limit: usize,
    },
    CommitFiles {
        repo: RepoId,
        oid: String,
    },
    Files {
        repo: RepoId,
    },
    Search {
        repo: RepoId,
        term: String,
        case_sensitive: bool,
        whole_word: bool,
        regex: bool,
    },
    Operation {
        repo: RepoId,
    },
    Fence {
        repo: RepoId,
    },
    StageAll {
        repo: RepoId,
        side: DiffSide,
    },
    CommitMessage {
        repo: RepoId,
        message: String,
        amend: bool,
    },
    Push {
        repo: RepoId,
        branch: String,
    },
    Stage {
        repo: RepoId,
        paths: Vec<RepoPath>,
        side: DiffSide,
    },
    Prepare {
        repo: RepoId,
        #[serde(default)]
        scope: kiri_core::review::CaptureScope,
        paths: Option<Vec<RepoPath>>,
        options: AnalysisOptions,
    },
    Propose {
        prepared: u32,
        model: String,
        #[serde(default)]
        cache_identity: Option<String>,
        kind: ProposalKind,
        instructions: String,
    },
    Inspect {
        prepared: u32,
        request: Inspection,
    },
    Manifest {
        prepared: u32,
    },
    Release {
        prepared: u32,
    },
    Commit {
        repo: RepoId,
        draft: CommitDraft,
    },
    Cancel {
        request: u32,
    },
    ModelResult {
        call: u32,
        result: ModelResult,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Draft,
    Plan,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelResult {
    Text {
        text: String,
    },
    Error {
        message: String,
        #[serde(default)]
        code: Option<String>,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResultValue {
    Manifest {
        source: String,
        files: Vec<kiri_analysis::EvidenceFile>,
        units: Vec<SourceUnit>,
        roots: Vec<String>,
    },
    NotRepository,
    Hello {
        version: u32,
        capabilities: Vec<String>,
    },
    Opened {
        repo: RepoId,
        root: String,
    },
    Status {
        revision: u64,
        status: RepoStatus,
    },
    Unchanged {
        revision: u64,
    },
    Preview {
        patch: Vec<u8>,
        partial: bool,
        binary: bool,
        notice: Option<String>,
    },
    Prepared {
        prepared: u32,
        files: usize,
        bytes: u64,
        chunks: usize,
        estimated_calls: usize,
    },
    Draft {
        draft: CommitDraft,
    },
    Plan {
        plan: CommitPlan,
    },
    Evidence {
        page: EvidencePage,
    },
    Compared {
        preview: kiri_core::workbench::Preview,
    },
    History {
        entries: Vec<kiri_core::workbench::CommitEntry>,
    },
    CommitFiles {
        files: Vec<kiri_core::workbench::CommitFile>,
    },
    Files {
        paths: Vec<RepoPath>,
    },
    Search {
        lines: Vec<String>,
    },
    Operation {
        operation: Option<String>,
    },
    Committed {
        oid: String,
    },
    Ok,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Reply {
        id: u32,
        result: ResultValue,
    },
    Error {
        id: u32,
        code: ErrorCode,
        message: String,
    },
    Progress {
        request: u32,
        event: Progress,
    },
    ModelCall {
        request: u32,
        call: u32,
        model: String,
        system: String,
        input: String,
        schema: Value,
    },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    ProtocolMismatch,
    NotFound,
    Busy,
    Cancelled,
    OperationFailed,
    OutcomeUnknown,
}
