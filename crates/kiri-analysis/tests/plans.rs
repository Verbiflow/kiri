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
        }
        .into(),
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

mod worktree {
    use anyhow::{Result, bail};
    use kiri_analysis::proposal::{CommitGroup, CommitPlan, PlanFile};
    use kiri_core::{model::RepoPath, repo::Repository, review::CaptureScope};
    use std::{fs, path::Path, process::Command};

    fn git(root: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Kiri Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Kiri Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .output()?;
        if !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr));
        }
        Ok(String::from_utf8(output.stdout)?)
    }

    async fn plan(repo: &Repository, groups: &[&[&str]]) -> Result<CommitPlan> {
        let captured = repo.capture_changes(CaptureScope::Worktree).await?;
        let files: Vec<PlanFile> = captured
            .evidence
            .changes()
            .await?
            .into_iter()
            .map(|(path, _)| PlanFile {
                id: path.id(),
                path,
            })
            .collect();
        let groups = groups
            .iter()
            .enumerate()
            .map(|(index, names)| {
                Ok(CommitGroup {
                    message: format!("feat: group {}", index + 1),
                    reason: "test boundary".into(),
                    files: names
                        .iter()
                        .map(|name| RepoPath::new(name.as_bytes().to_vec()).map(|path| path.id()))
                        .collect::<Result<_>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(CommitPlan {
            repository: repo.root().to_path_buf(),
            snapshot: captured.snapshot,
            files,
            groups,
            warnings: Vec::new(),
        })
    }

    #[tokio::test]
    async fn working_tree_plans_stage_each_group_and_preserve_the_rest() -> Result<()> {
        let temp = tempfile::tempdir()?;
        git(temp.path(), &["init", "-q"])?;
        fs::create_dir_all(temp.path().join("src"))?;
        fs::write(temp.path().join("src/lib.rs"), "fn lib() {}\n")?;
        fs::write(temp.path().join("README.md"), "# demo\n")?;
        fs::write(temp.path().join("notes.txt"), "keep\n")?;
        git(temp.path(), &["add", "."])?;
        git(temp.path(), &["commit", "-qm", "initial"])?;
        fs::write(temp.path().join("src/lib.rs"), "fn lib() { changed() }\n")?;
        fs::write(temp.path().join("src/new.rs"), "fn new() {}\n")?;
        fs::write(temp.path().join("README.md"), "# demo\n\nDocs.\n")?;
        fs::write(temp.path().join("notes.txt"), "keep\nscratch\n")?;
        let repo = Repository::open(temp.path()).await?;
        let plan = plan(
            &repo,
            &[
                &["src/lib.rs", "src/new.rs"],
                &["README.md"],
                &["notes.txt"],
            ],
        )
        .await?;
        // A plan never covers files outside its groups; drop notes.txt from this one so it
        // stays a working change.
        let mut plan = plan;
        plan.groups.pop();
        plan.files.retain(|file| file.path.bytes() != b"notes.txt");
        plan.validate()?;
        assert!(git(temp.path(), &["diff", "--cached", "--name-only"])?.is_empty());
        let mut progress = Vec::new();
        let commits = plan
            .apply(&repo, |index, oid| progress.push((index, oid.to_owned())))
            .await?;
        assert_eq!(commits.len(), 2);
        assert_eq!(progress.len(), 2);
        let first = git(
            temp.path(),
            &["show", "--format=", "--name-only", &commits[0]],
        )?;
        assert_eq!(
            first.lines().collect::<Vec<_>>(),
            vec!["src/lib.rs", "src/new.rs"]
        );
        let second = git(
            temp.path(),
            &["show", "--format=", "--name-only", &commits[1]],
        )?;
        assert_eq!(second.trim(), "README.md");
        assert_eq!(git(temp.path(), &["rev-parse", "HEAD"])?.trim(), commits[1]);
        assert_eq!(
            git(temp.path(), &["status", "--porcelain"])?.trim(),
            "M notes.txt"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("notes.txt"))?,
            "keep\nscratch\n"
        );
        Ok(())
    }

    #[tokio::test]
    async fn working_tree_plans_refuse_files_edited_after_review() -> Result<()> {
        let temp = tempfile::tempdir()?;
        git(temp.path(), &["init", "-q"])?;
        fs::write(temp.path().join("a.txt"), "a\n")?;
        fs::write(temp.path().join("b.txt"), "b\n")?;
        git(temp.path(), &["add", "."])?;
        git(temp.path(), &["commit", "-qm", "initial"])?;
        fs::write(temp.path().join("a.txt"), "a2\n")?;
        fs::write(temp.path().join("b.txt"), "b2\n")?;
        let repo = Repository::open(temp.path()).await?;
        let plan = plan(&repo, &[&["a.txt"], &["b.txt"]]).await?;
        fs::write(temp.path().join("b.txt"), "b3\n")?;
        let before = git(temp.path(), &["rev-parse", "HEAD"])?;
        assert!(plan.apply(&repo, |_, _| {}).await.is_err());
        assert_eq!(git(temp.path(), &["rev-parse", "HEAD"])?, before);
        assert!(git(temp.path(), &["diff", "--cached", "--name-only"])?.is_empty());
        assert_eq!(fs::read_to_string(temp.path().join("b.txt"))?, "b3\n");
        Ok(())
    }
}
