use crate::{model::terminal_text, repo::Repository, storage::Store};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    pub root: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspaces {
    pub entries: Vec<Workspace>,
}

impl Workspaces {
    pub fn load(store: &Store) -> Result<Self> {
        store.load("workspaces.json")
    }

    pub async fn add(store: &Store, path: &Path, name: Option<String>) -> Result<Workspace> {
        let repo = Repository::open(path).await?;
        let root = repo.root().to_path_buf();
        let label = name.unwrap_or_else(|| {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let workspace = Workspace {
            root,
            name: terminal_text(&label),
        };
        store.update::<Self, _>("workspaces.json", |registry| {
            if let Some(existing) = registry
                .entries
                .iter_mut()
                .find(|w| w.root == workspace.root)
            {
                existing.name.clone_from(&workspace.name);
            } else {
                registry.entries.push(workspace.clone());
            }
            Ok(())
        })?;
        Ok(workspace)
    }

    pub fn remove(store: &Store, root: &Path) -> Result<()> {
        store.update::<Self, _>("workspaces.json", |registry| {
            registry.entries.retain(|w| w.root != root);
            Ok(())
        })
    }
}
