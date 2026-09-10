import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


def main():
    root = Path(tempfile.mkdtemp(prefix="kiri-demo-")) / "harbor"
    root.mkdir()
    env = dict(os.environ, GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL="/dev/null", GIT_AUTHOR_NAME="Kiri Demo", GIT_AUTHOR_EMAIL="demo@example.invalid", GIT_COMMITTER_NAME="Kiri Demo", GIT_COMMITTER_EMAIL="demo@example.invalid")

    def git(*args):
        subprocess.run(["git", "-C", str(root), *args], env=env, check=True, capture_output=True)

    def put(name, text):
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    git("init", "--quiet", "--initial-branch=search-preview")
    put("Cargo.toml", '[package]\nname = "harbor"\nversion = "0.1.0"\nedition = "2024"\n')
    put("src/lib.rs", "pub mod search;\npub mod workspace;\n")
    put("src/search.rs", '''use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub path: PathBuf,
    pub score: usize,
}

pub fn search(query: &str, paths: &[PathBuf]) -> Vec<SearchResult> {
    paths.iter()
        .filter(|path| path.to_string_lossy().contains(query))
        .map(|path| SearchResult { path: path.clone(), score: 0 })
        .collect()
}
''')
    put("src/workspace.rs", '''use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub name: String,
    pub root: PathBuf,
}
''')
    put("tests/search.rs", '''use harbor::search::search;
use std::path::PathBuf;

#[test]
fn finds_matching_paths() {
    let paths = vec![PathBuf::from("src/main.rs")];
    assert_eq!(search("main", &paths).len(), 1);
}
''')
    git("add", ".")
    git("commit", "--quiet", "-m", "feat: add local project search")
    put("src/search.rs", '''use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct SearchResult {
    pub path: PathBuf,
    pub score: usize,
}

pub fn search(query: &str, paths: &[PathBuf]) -> Vec<SearchResult> {
    let query = query.trim().to_ascii_lowercase();
    if query.is_empty() {
        return Vec::new();
    }

    let mut results: Vec<_> = paths.iter()
        .filter_map(|path| {
            let name = path.to_string_lossy().to_ascii_lowercase();
            let position = name.find(&query)?;
            let score = if position == 0 { 100 } else { 50 };
            Some(SearchResult { path: path.clone(), score })
        })
        .collect();

    results.sort_by(|a, b| b.score.cmp(&a.score).then(a.path.cmp(&b.path)));
    results.truncate(50);
    results
}
''')
    put("src/workspace.rs", '''use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Workspace {
    pub name: String,
    pub root: PathBuf,
}

impl Workspace {
    pub fn open(root: PathBuf) -> std::io::Result<Self> {
        let root = root.canonicalize()?;
        let name = root.file_name().unwrap_or_default().to_string_lossy().into_owned();
        Ok(Self { name, root })
    }
}
''')
    put("tests/search.rs", '''use harbor::search::search;
use std::path::PathBuf;

#[test]
fn search_is_case_insensitive() {
    let paths = vec![PathBuf::from("src/Main.rs")];
    assert_eq!(search("MAIN", &paths).len(), 1);
}

#[test]
fn empty_queries_do_not_list_every_file() {
    let paths = vec![PathBuf::from("src/main.rs")];
    assert!(search("  ", &paths).is_empty());
}
''')
    put("tests/workspace.rs", '''use harbor::workspace::Workspace;
use std::path::PathBuf;

#[test]
fn missing_workspaces_return_an_error() {
    assert!(Workspace::open(PathBuf::from("/missing/harbor-workspace")).is_err());
}
''')
    git("add", "tests/search.rs")
    if len(sys.argv) == 3 and sys.argv[1] == "--run":
        binary = str(Path(sys.argv[2]).resolve())
        env["KIRI_CONFIG_DIR"] = str(root / ".git/kiri-demo-config")
        os.chdir(root)
        sys.stdout.write("\x1b]0;Kiri | Harbor\x07")
        sys.stdout.flush()
        os.execvpe(binary, [binary, "-C", str(root)], env)
    print(json.dumps({"repository": str(root), "config": str(root / ".git/kiri-demo-config")}, indent=2))


if __name__ == "__main__":
    main()
