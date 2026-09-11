use crate::model::{FileChange, RepoPath, terminal_text};
use std::{
    collections::{BTreeMap, HashSet},
    ops::Range,
};

#[derive(Clone, Debug)]
pub enum Entry {
    Folder { path: RepoPath, files: Range<usize> },
    File { index: usize },
}

#[derive(Clone, Debug)]
pub struct TreeNode {
    pub entry: Entry,
    pub name: String,
    pub depth: usize,
    pub parent: Option<usize>,
    pub subtree_end: usize,
    pub staged: usize,
    pub working: usize,
}

impl TreeNode {
    pub fn path<'a>(&'a self, files: &'a [FileChange]) -> &'a RepoPath {
        match &self.entry {
            Entry::Folder { path, .. } => path,
            Entry::File { index } => &files[*index].path,
        }
    }
    pub fn is_folder(&self) -> bool {
        matches!(self.entry, Entry::Folder { .. })
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChangeTree {
    pub nodes: Vec<TreeNode>,
    files: Vec<usize>,
}

#[derive(Default)]
struct Directory {
    directories: BTreeMap<Vec<u8>, Directory>,
    files: BTreeMap<Vec<u8>, usize>,
}

impl ChangeTree {
    pub fn build(changes: &[FileChange], included: &[usize]) -> Self {
        let mut root = Directory::default();
        for &index in included {
            let components: Vec<_> = changes[index]
                .path
                .bytes()
                .split(|b| *b == b'/')
                .filter(|part| !part.is_empty())
                .collect();
            let Some((name, parents)) = components.split_last() else {
                continue;
            };
            let mut directory = &mut root;
            for part in parents {
                directory = directory.directories.entry(part.to_vec()).or_default();
            }
            directory.files.insert(name.to_vec(), index);
        }
        let mut tree = Self::default();
        tree.flatten(root, &[], None, 0, changes);
        tree
    }

    fn flatten(
        &mut self,
        directory: Directory,
        prefix: &[u8],
        parent: Option<usize>,
        depth: usize,
        changes: &[FileChange],
    ) {
        for (name, child) in directory.directories {
            let mut path = prefix.to_vec();
            path.extend_from_slice(&name);
            let Ok(folder) = RepoPath::new(path.clone()) else {
                continue;
            };
            let index = self.nodes.len();
            let start = self.files.len();
            self.nodes.push(TreeNode {
                entry: Entry::Folder {
                    path: folder,
                    files: start..start,
                },
                name: terminal_text(&String::from_utf8_lossy(&name)),
                depth,
                parent,
                subtree_end: 0,
                staged: 0,
                working: 0,
            });
            path.push(b'/');
            self.flatten(child, &path, Some(index), depth + 1, changes);
            let end = self.nodes.len();
            let members = &self.files[start..];
            let staged = members
                .iter()
                .filter(|&&i| changes[i].staged.is_some())
                .count();
            let working = members
                .iter()
                .filter(|&&i| changes[i].worktree.is_some())
                .count();
            let node = &mut self.nodes[index];
            node.subtree_end = end;
            node.staged = staged;
            node.working = working;
            if let Entry::Folder { files, .. } = &mut node.entry {
                files.end = self.files.len();
            }
        }
        for (name, index) in directory.files {
            self.nodes.push(TreeNode {
                entry: Entry::File { index },
                name: terminal_text(&String::from_utf8_lossy(&name)),
                depth,
                parent,
                subtree_end: self.nodes.len() + 1,
                staged: usize::from(changes[index].staged.is_some()),
                working: usize::from(changes[index].worktree.is_some()),
            });
            self.files.push(index);
        }
    }

    pub fn members(&self, node: usize) -> &[usize] {
        match &self.nodes[node].entry {
            Entry::Folder { files, .. } => &self.files[files.clone()],
            Entry::File { index } => std::slice::from_ref(index),
        }
    }

    pub fn visible(&self, collapsed: &HashSet<RepoPath>, reveal_matches: bool) -> Vec<usize> {
        let mut visible = Vec::new();
        let mut index = 0;
        while let Some(node) = self.nodes.get(index) {
            visible.push(index);
            index = match &node.entry {
                Entry::Folder { path, .. } if !reveal_matches && collapsed.contains(path) => {
                    node.subtree_end
                }
                _ => index + 1,
            };
        }
        visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChangeKind;
    use anyhow::Result;

    fn changes(paths: &[&[u8]]) -> Result<Vec<FileChange>> {
        paths
            .iter()
            .map(|path| {
                Ok(FileChange {
                    path: RepoPath::new(path.to_vec())?,
                    original_path: None,
                    staged: None,
                    worktree: Some(ChangeKind::Modified),
                    submodule: false,
                    head_oid: None,
                    index_oid: None,
                })
            })
            .collect()
    }

    #[test]
    fn folders_own_exact_descendants_not_similar_prefixes() -> Result<()> {
        let files = changes(&[
            b"src/a.rs",
            b"src/deep/b.rs",
            b"src-other/c.rs",
            b"outside.rs",
        ])?;
        let tree = ChangeTree::build(&files, &[0, 1, 2, 3]);
        let folder = tree
            .nodes
            .iter()
            .position(
                |node| matches!(&node.entry, Entry::Folder { path, .. } if path.bytes() == b"src"),
            )
            .ok_or_else(|| anyhow::anyhow!("folder missing"))?;
        assert_eq!(
            tree.members(folder).iter().copied().collect::<HashSet<_>>(),
            HashSet::from([0, 1])
        );
        let collapsed = HashSet::from([RepoPath::new(b"src".to_vec())?]);
        let visible = tree.visible(&collapsed, false);
        assert!(visible.contains(&folder));
        assert!(
            !visible
                .iter()
                .any(|&i| matches!(tree.nodes[i].entry, Entry::File { index: 0 | 1 }))
        );
        assert!(tree.visible(&collapsed, true).len() > visible.len());
        Ok(())
    }

    #[test]
    fn folder_actions_include_collapsed_children_but_respect_filtering() -> Result<()> {
        let mut files = changes(&[b"api/one.rs", b"api/two.rs", b"api/deep/three.rs"])?;
        files[0].staged = Some(ChangeKind::Modified);
        let tree = ChangeTree::build(&files, &[0, 2]);
        assert_eq!(tree.members(0).len(), 2);
        assert_eq!(tree.nodes[0].staged, 1);
        assert_eq!(tree.nodes[0].working, 2);
        Ok(())
    }

    #[test]
    fn folder_paths_remain_byte_exact() -> Result<()> {
        let files = changes(&[b"dir\xff/file with\nnewline", b"dir\xfe/other"])?;
        let tree = ChangeTree::build(&files, &[0, 1]);
        let paths: HashSet<_> = tree
            .nodes
            .iter()
            .filter_map(|n| match &n.entry {
                Entry::Folder { path, .. } => Some(path.bytes()),
                _ => None,
            })
            .collect();
        assert_eq!(
            paths,
            HashSet::from([b"dir\xff".as_slice(), b"dir\xfe".as_slice()])
        );
        Ok(())
    }
}
