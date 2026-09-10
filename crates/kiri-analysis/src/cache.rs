use crate::{AnalysisCache, AnalysisNode};
use anyhow::Result;
use futures::future::BoxFuture;
use kiri_core::storage::{Store, atomic_write, read_json};
use std::path::PathBuf;

pub struct FileCache {
    root: PathBuf,
}
impl FileCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}
impl AnalysisCache for FileCache {
    fn load<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<AnalysisNode>>> {
        Box::pin(async move {
            anyhow::ensure!(
                id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid analysis cache key"
            );
            let path = self.root.join(format!("{id}.json"));
            tokio::task::spawn_blocking(move || read_json(&path)).await?
        })
    }
    fn save<'a>(&'a self, node: &'a AnalysisNode) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            anyhow::ensure!(
                node.id.len() == 64 && node.id.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "Invalid analysis cache key"
            );
            let node = node.clone();
            let store = Store::at(self.root.clone());
            tokio::task::spawn_blocking(move || {
                store.prepare()?;
                atomic_write(
                    &store.path(&format!("{}.json", node.id)),
                    &serde_json::to_vec(&node)?,
                )
            })
            .await?
        })
    }
}
#[derive(Default)]
pub struct MemoryCache {
    nodes: tokio::sync::Mutex<std::collections::VecDeque<AnalysisNode>>,
}
impl AnalysisCache for MemoryCache {
    fn load<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<Option<AnalysisNode>>> {
        Box::pin(async move {
            Ok(self
                .nodes
                .lock()
                .await
                .iter()
                .find(|node| node.id == id)
                .cloned())
        })
    }
    fn save<'a>(&'a self, node: &'a AnalysisNode) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut nodes = self.nodes.lock().await;
            nodes.retain(|old| old.id != node.id);
            nodes.push_front(node.clone());
            while nodes.len() > 1024 {
                nodes.pop_back();
            }
            Ok(())
        })
    }
}

pub struct NoCache;
impl AnalysisCache for NoCache {
    fn load<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<Option<AnalysisNode>>> {
        Box::pin(async { Ok(None) })
    }
    fn save<'a>(&'a self, _node: &'a AnalysisNode) -> BoxFuture<'a, Result<()>> {
        Box::pin(async { Ok(()) })
    }
}
