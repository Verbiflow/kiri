use anyhow::Result;
use kiri_analysis::proposal::{CommitGroup, CommitPlan, PlanFile};
use kiri_core::{commit::StagedSnapshot, model::RepoPath};

#[test]
fn plans_require_exact_file_coverage_without_unknown_or_duplicate_ids() -> Result<()> {
    let path = RepoPath::new(b"a.txt".to_vec())?;
    let mut plan = CommitPlan {
        repository: Default::default(),
        snapshot: StagedSnapshot {
            head: None,
            head_ref: None,
            tree: "tree".into(),
            index_digest: "index".into(),
        },
        files: vec![PlanFile {
            id: path.id(),
            path,
        }],
        groups: vec![CommitGroup {
            message: "fix: preserve changes".into(),
            reason: "related".into(),
            files: Vec::new(),
        }],
        warnings: Vec::new(),
    };
    assert!(plan.validate().is_err());
    plan.groups[0].files.push(plan.files[0].id.clone());
    plan.validate()?;
    plan.groups[0].files.push(plan.files[0].id.clone());
    assert!(plan.validate().is_err());
    plan.groups[0].files = vec!["invented".into()];
    assert!(plan.validate().is_err());
    Ok(())
}
