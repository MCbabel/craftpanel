use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::model::Id;

type Answer = Pin<Box<dyn Future<Output = HashSet<Id>> + Send>>;

#[derive(Clone)]
pub struct LiveServers(Arc<dyn Fn() -> Answer + Send + Sync>);

impl LiveServers {
    pub fn none() -> Self {
        Self::fixed([])
    }

    pub fn fixed(ids: impl IntoIterator<Item = Id>) -> Self {
        let ids: HashSet<Id> = ids.into_iter().collect();
        Self::from_fn(move || {
            let ids = ids.clone();
            async move { ids }
        })
    }

    pub fn from_fn<F, Fut>(read: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HashSet<Id>> + Send + 'static,
    {
        Self(Arc::new(move || Box::pin(read())))
    }

    pub async fn ids(&self) -> HashSet<Id> {
        (self.0)().await
    }

    pub async fn among(&self, servers: &[Id]) -> HashSet<Id> {
        let live = self.ids().await;
        servers.iter().copied().filter(|id| live.contains(id)).collect()
    }
}

impl std::fmt::Debug for LiveServers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveServers")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn without_a_reader_nothing_is_running() {
        assert!(LiveServers::none().ids().await.is_empty());
    }

    #[tokio::test]
    async fn a_reader_is_asked_again_on_every_question() {
        let asked = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = Arc::clone(&asked);
        let live = LiveServers::from_fn(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            async { HashSet::new() }
        });

        live.ids().await;
        live.ids().await;
        assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 2, "no stale snapshot");
    }

    #[tokio::test]
    async fn among_keeps_only_the_servers_that_were_asked_about() {
        let running = Id::new();
        let stopped = Id::new();
        let elsewhere = Id::new();
        let live = LiveServers::fixed([running, elsewhere]);

        let found = live.among(&[running, stopped]).await;
        assert_eq!(found, HashSet::from([running]));
    }
}
