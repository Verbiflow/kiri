use std::{collections::HashMap, hash::Hash};
use tokio::task::JoinHandle;

pub struct TaskGroup<K> {
    tasks: HashMap<K, JoinHandle<()>>,
}
impl<K: Eq + Hash> Default for TaskGroup<K> {
    fn default() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }
}
impl<K: Eq + Hash> TaskGroup<K> {
    pub fn contains(&self, key: &K) -> bool {
        self.tasks.get(key).is_some_and(|task| !task.is_finished())
    }
    pub fn replace(&mut self, key: K, future: impl Future<Output = ()> + Send + 'static) {
        self.cancel(&key);
        self.tasks.retain(|_, task| !task.is_finished());
        self.tasks.insert(key, tokio::spawn(future));
    }
    pub fn cancel(&mut self, key: &K) {
        if let Some(task) = self.tasks.remove(key) {
            task.abort();
        }
    }
    pub async fn stop(&mut self, key: &K) -> bool {
        let Some(task) = self.tasks.remove(key) else {
            return false;
        };
        task.abort();
        let _ = task.await;
        true
    }
    pub fn forget(&mut self, key: &K) {
        self.tasks.remove(key);
    }
    /// Cancel every task whose key fails `keep`, leaving the others running.
    pub fn retain(&mut self, mut keep: impl FnMut(&K) -> bool) {
        self.tasks.retain(|key, task| {
            let wanted = keep(key);
            if !wanted {
                task.abort();
            }
            wanted
        });
    }
    pub fn len(&self) -> usize {
        self.tasks
            .values()
            .filter(|task| !task.is_finished())
            .count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
impl<K> Drop for TaskGroup<K> {
    fn drop(&mut self) {
        for task in self.tasks.values() {
            task.abort();
        }
    }
}

/// A spawned task that is cancelled when its handle is dropped, so overlapped reads never
/// outlive the request that started them.
pub struct AbortOnDrop<T>(JoinHandle<T>);
impl<T: Send + 'static> AbortOnDrop<T> {
    pub fn spawn(future: impl Future<Output = T> + Send + 'static) -> Self {
        Self(tokio::spawn(future))
    }
}
impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, tokio::task::JoinError>;
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(cx)
    }
}
impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
