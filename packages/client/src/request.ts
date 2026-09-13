export type Command =
  | {
      method: 'hello';
      schema_hash?: string | null;
      version: number;
    }
  | {
      method: 'open';
      path: string;
    }
  | {
      method: 'close';
      repo: number;
    }
  | {
      fresh: boolean;
      method: 'status';
      repo: number;
      /**
       * Revision the caller already holds. When the repository still reports that revision,
       * the reply is `unchanged` instead of the full inventory.
       */
      since?: number | null;
    }
  | {
      large: boolean;
      method: 'preview';
      path: RepoPath;
      repo: number;
      side: DiffSide;
    }
  | {
      comparison: Comparison;
      method: 'compare';
      path: RepoPath;
      repo: number;
    }
  | {
      limit: number;
      method: 'log';
      repo: number;
    }
  | {
      method: 'commit_files';
      oid: string;
      repo: number;
    }
  | {
      method: 'files';
      repo: number;
    }
  | {
      case_sensitive: boolean;
      method: 'search';
      regex: boolean;
      repo: number;
      term: string;
      whole_word: boolean;
    }
  | {
      method: 'operation';
      repo: number;
    }
  | {
      method: 'fence';
      repo: number;
    }
  | {
      method: 'stage_all';
      repo: number;
      side: DiffSide;
    }
  | {
      amend: boolean;
      message: string;
      method: 'commit_message';
      repo: number;
    }
  | {
      branch: string;
      method: 'push';
      repo: number;
    }
  | {
      method: 'stage';
      paths: RepoPath[];
      repo: number;
      side: DiffSide;
    }
  | {
      method: 'prepare';
      options: AnalysisOptions;
      paths?: RepoPath[] | null;
      repo: number;
      scope?: 'staged' | 'worktree' | 'auto';
    }
  | {
      cache_identity?: string | null;
      instructions: string;
      kind: ProposalKind;
      method: 'propose';
      model: string;
      prepared: number;
    }
  | {
      method: 'inspect';
      prepared: number;
      request: Inspection;
    }
  | {
      method: 'manifest';
      prepared: number;
    }
  | {
      method: 'release';
      prepared: number;
    }
  | {
      draft: CommitDraft;
      method: 'commit';
      repo: number;
    }
  | {
      method: 'apply_plan';
      plan: CommitPlan;
      repo: number;
    }
  | {
      method: 'cancel';
      request: number;
    }
  | {
      call: number;
      method: 'model_result';
      result: ModelResult;
    };
export type RepoPath = number[];
export type DiffSide = 'worktree' | 'staged';
export type Comparison =
  | {
      kind: 'head_to_index';
    }
  | {
      kind: 'index_to_worktree';
    }
  | {
      kind: 'head_to_worktree';
    }
  | {
      kind: 'commit';
      oid: string;
    };
export type ProposalKind = 'draft' | 'plan';
export type Inspection =
  | {
      id: string;
      kind: 'source';
      limit: number;
      offset: number;
    }
  | {
      id: string;
      kind: 'node';
      limit: number;
      offset: number;
    }
  | {
      id: string;
      kind: 'references';
      limit: number;
      offset: number;
    }
  | {
      kind: 'inventory';
      limit: number;
      offset: number;
    };
export type ReviewSnapshot =
  | {
      snapshot: StagedSnapshot;
      source: 'staged';
    }
  | {
      snapshot: WorktreeSnapshot;
      source: 'worktree';
    };
export type ModelResult =
  | {
      kind: 'text';
      text: string;
    }
  | {
      code?: string | null;
      kind: 'error';
      message: string;
    };

export interface Request {
  command: Command;
  id: number;
}
export interface AnalysisOptions {
  cache?: boolean;
  chunk_bytes?: number;
  concurrency?: number;
  fan_in?: number;
  inspection_rounds?: number;
  max_calls?: number;
  mode?: 'fast' | 'deep';
  worker_model?: string | null;
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
