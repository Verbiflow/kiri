use anyhow::Result;
use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::Mutex;

type Gate = Weak<Mutex<()>>;
/// Enough for the selected file, its prefetched neighbours, and recent revisits. Documents are
/// bounded by the preview budget, so the worst case stays under 8 MiB per repository.
const CAPACITY: usize = 16;
pub(crate) struct ReadCache<K, V> {
    values: Mutex<VecDeque<(K, u64, Arc<V>)>>,
    reads: Mutex<HashMap<(K, u64), Gate>>,
}
impl<K, V> Default for ReadCache<K, V> {
    fn default() -> Self {
        Self {
            values: Mutex::new(VecDeque::new()),
            reads: Mutex::new(HashMap::new()),
        }
    }
}
impl<K: Clone + Eq + Hash, V> ReadCache<K, V> {
    pub async fn get(
        &self,
        key: K,
        revision: &AtomicU64,
        load: impl Future<Output = Result<V>>,
    ) -> Result<Arc<V>> {
        let generation = revision.load(Ordering::Acquire);
        let gate = {
            let mut reads = self.reads.lock().await;
            reads.retain(|_, value| value.strong_count() > 0);
            let lookup = (key.clone(), generation);
            if let Some(gate) = reads.get(&lookup).and_then(Weak::upgrade) {
                gate
            } else {
                let gate = Arc::new(Mutex::new(()));
                reads.insert(lookup, Arc::downgrade(&gate));
                gate
            }
        };
        let _guard = gate.lock().await;
        {
            let mut values = self.values.lock().await;
            if let Some(index) = values
                .iter()
                .position(|(cached, epoch, _)| cached == &key && *epoch == generation)
                && let Some(entry) = values.remove(index)
            {
                let value = entry.2.clone();
                values.push_front(entry);
                return Ok(value);
            }
        }
        let value = Arc::new(load.await?);
        if revision.load(Ordering::Acquire) == generation {
            let mut values = self.values.lock().await;
            values.retain(|(_, epoch, _)| *epoch == generation);
            values.push_front((key, generation, value.clone()));
            while values.len() > CAPACITY {
                values.pop_back();
            }
        }
        Ok(value)
    }
}
