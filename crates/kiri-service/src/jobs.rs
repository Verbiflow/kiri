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
