export type Frame =
  | {
      id: number;
      result: ResultValue;
      type: 'reply';
      [k: string]: unknown;
    }
  | {
      code: ErrorCode;
      id: number;
      message: string;
      type: 'error';
      [k: string]: unknown;
    }
  | {
      event: Progress;
      request: number;
      type: 'progress';
      [k: string]: unknown;
    }
  | {
      call: number;
      input: string;
      model: string;
      request: number;
      schema: unknown;
      system: string;
      type: 'model_call';
      [k: string]: unknown;
    };
export type ResultValue =
  | {
      files: EvidenceFile[];
      kind: 'manifest';
      roots: string[];
      source: string;
      units: SourceUnit[];
      [k: string]: unknown;
    }
  | {
      kind: 'not_repository';
      [k: string]: unknown;
    }
  | {
      capabilities: string[];
      kind: 'hello';
      version: number;
      [k: string]: unknown;
    }
  | {
      kind: 'opened';
      repo: number;
      root: string;
      [k: string]: unknown;
    }
  | {
      kind: 'status';
      revision: number;
      status: RepoStatus;
      [k: string]: unknown;
    }
  | {
      kind: 'unchanged';
      revision: number;
      [k: string]: unknown;
    }
  | {
      binary: boolean;
      kind: 'preview';
      notice?: string | null;
      partial: boolean;
      patch: number[];
      [k: string]: unknown;
    }
  | {
      bytes: number;
      chunks: number;
      estimated_calls: number;
      files: number;
      kind: 'prepared';
      prepared: number;
      [k: string]: unknown;
    }
  | {
      draft: CommitDraft;
      kind: 'draft';
      [k: string]: unknown;
    }
  | {
      kind: 'plan';
      plan: CommitPlan;
      [k: string]: unknown;
    }
  | {
      kind: 'evidence';
      page: EvidencePage;
      [k: string]: unknown;
    }
  | {
      kind: 'compared';
      preview: Preview;
      [k: string]: unknown;
    }
  | {
      entries: CommitEntry[];
      kind: 'history';
      [k: string]: unknown;
    }
  | {
      files: CommitFile[];
      kind: 'commit_files';
      [k: string]: unknown;
    }
  | {
      kind: 'files';
      paths: RepoPath[];
      [k: string]: unknown;
    }
  | {
      kind: 'search';
      lines: string[];
      [k: string]: unknown;
    }
  | {
      kind: 'operation';
      operation?: string | null;
      [k: string]: unknown;
    }
  | {
      kind: 'committed';
      oid: string;
      [k: string]: unknown;
    }
  | {
      commits: string[];
      kind: 'applied';
      [k: string]: unknown;
    }
  | {
      kind: 'ok';
      [k: string]: unknown;
    };
export type RepoPath = number[];
export type ChangeKind =
  'modified' | 'added' | 'deleted' | 'renamed' | 'copied' | 'type_changed' | 'unmerged' | 'untracked';
export type ReviewSnapshot =
  | {
      snapshot: StagedSnapshot;
      source: 'staged';
    }
  | {
      snapshot: WorktreeSnapshot;
      source: 'worktree';
    };
export type Preview =
  | {
      after?: string | null;
      before?: string | null;
      kind: 'files';
      [k: string]: unknown;
    }
  | {
      kind: 'binary';
      [k: string]: unknown;
    }
  | {
      kind: 'patch';
      limited: boolean;
      patch: string;
      [k: string]: unknown;
    }
  | {
      kind: 'unavailable';
      reason: string;
      [k: string]: unknown;
    };
export type ErrorCode =
  'invalid_request' | 'protocol_mismatch' | 'not_found' | 'busy' | 'cancelled' | 'operation_failed' | 'outcome_unknown';
export type Progress =
  | {
      done: number;
      phase: 'reading';
      total: number;
      [k: string]: unknown;
    }
  | {
      bytes: number;
      done: number;
      phase: 'indexing';
      total: number;
      [k: string]: unknown;
    }
  | {
      cached: number;
      done: number;
      phase: 'analyzing';
      total: number;
      [k: string]: unknown;
    }
  | {
      done: number;
      level: number;
      phase: 'reducing';
      total: number;
      [k: string]: unknown;
    }
  | {
      attempt: number;
      delay_ms: number;
      phase: 'retrying';
      [k: string]: unknown;
    }
  | {
      phase: 'inspecting';
      round: number;
      sources: number;
      [k: string]: unknown;
    }
  | {
      phase: 'drafting';
      [k: string]: unknown;
    };

export interface EvidenceFile {
  bytes: number;
  change: string;
  path: RepoPath;
  withheld?: string | null;
  [k: string]: unknown;
}
export interface SourceUnit {
  bytes: number;
  id: string;
  sources: SourceRef[];
  [k: string]: unknown;
}
export interface SourceRef {
  end: number;
  file: number;
  start: number;
  [k: string]: unknown;
}
export interface RepoStatus {
  ahead: number;
  behind: number;
  branch: string;
  files: FileChange[];
  head?: string | null;
  upstream?: string | null;
  [k: string]: unknown;
}
export interface FileChange {
  /**
   * Blob recorded in HEAD for this path, when Git reported one.
   */
  head_oid?: string | null;
  /**
   * Blob recorded in the index for this path, when Git reported one.
   */
  index_oid?: string | null;
  original_path?: RepoPath | null;
  path: RepoPath;
  staged?: ChangeKind | null;
  submodule: boolean;
  worktree?: ChangeKind | null;
  [k: string]: unknown;
}
export interface CommitDraft {
  analysis?: AnalysisReport | null;
  message: string;
  paths?: RepoPath[] | null;
  repository: string;
  snapshot: ReviewSnapshot;
  warnings: string[];
}
export interface AnalysisReport {
  bytes: number;
  cache_hits: number;
  files: number;
  model_calls: number;
  reduction_levels: number;
  unique_chunks: number;
  [k: string]: unknown;
}
export interface StagedSnapshot {
  head?: string | null;
  head_ref?: string | null;
  index_digest: string;
  tree: string;
}
export interface WorktreeSnapshot {
  files: FileStamp[];
  head?: string | null;
  head_ref?: string | null;
  index_digest?: string | null;
}
export interface FileStamp {
  digest?: string | null;
  mode: number;
  path: RepoPath;
}
export interface CommitPlan {
  files: PlanFile[];
  groups: CommitGroup[];
  repository: string;
  /**
   * Staged plans commit from a frozen index; working-tree plans stage each group's captured
   * files right before its commit, so nothing touches the index until the plan is applied.
   */
  snapshot:
    | {
        snapshot: StagedSnapshot;
        source: 'staged';
      }
    | {
        snapshot: WorktreeSnapshot;
        source: 'worktree';
      };
  warnings: string[];
}
export interface PlanFile {
  id: string;
  path: RepoPath;
}
export interface CommitGroup {
  files: string[];
  message: string;
  reason: string;
}
export interface EvidencePage {
  data: string;
  encoding: string;
  id: string;
  next_offset?: number | null;
  offset: number;
  total_bytes: number;
  [k: string]: unknown;
}
export interface CommitEntry {
  author: string;
  date: string;
  oid: string;
  short_oid: string;
  subject: string;
  [k: string]: unknown;
}
export interface CommitFile {
  kind: ChangeKind;
  path: RepoPath;
  [k: string]: unknown;
}
