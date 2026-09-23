//! 进程内 Moka 限流状态存储（spec §4.5）
//!
//! 阶段一提供通用 Moka 缓存；
//! 阶段二 [S2+] 作为本地（单机）限流模式下 TokenBucket / SlidingWindow 的
//! 状态持久化后端 —— “local 模式默认使用 LocalStore”。
//!
//! 相比各限流器内部的 `Mutex<HashMap>`，这里用 Moka 提供：
//! - 带 TTL 的状态（自然清理长时间不活跃的 key）
//! - 容量上限，防止恶意随机 key 撑爆内存
//! - 并发安全的 get/insert，无需逐 key 加锁

use std::time::Duration;

use moka::future::Cache;

/// 本地（进程内）限流状态存储
pub struct LocalStore<K, V>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + Clone + 'static,
{
    inner: Cache<K, V>,
}

impl<K, V> LocalStore<K, V>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + Clone + 'static,
{
    /// 创建一个本地限流状态存储
    ///
    /// - `max_capacity`：最多缓存的 key 数
    /// - `ttl`：每条状态的有效期（过期自动清理）
    pub fn new(max_capacity: u64, ttl: Duration) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .time_to_live(ttl)
            .build();
        Self { inner }
    }

    /// 读取某个 key 的状态
    pub async fn get(&self, key: &K) -> Option<V> {
        self.inner.get(key).await
    }

    /// 写入某个 key 的状态
    pub async fn insert(&self, key: K, value: V) {
        self.inner.insert(key, value).await;
    }

    /// 读取 key 的状态，若不存在则以 `init` 初始化并写入
    pub async fn get_or_insert(&self, key: K, init: V) -> V {
        if let Some(v) = self.inner.get(&key).await {
            return v;
        }
        self.inner.insert(key, init.clone()).await;
        init
    }

    /// 原子读取-更新-写回：用 `update` 闭包把旧值转为新值写入。
    /// 若 key 不存在，先插入 `init` 再应用闭包。
    pub async fn update(&self, key: K, init: V, update: impl FnOnce(V) -> V) -> V {
        let mut value = self.get_or_insert(key.clone(), init).await;
        value = update(value);
        self.inner.insert(key, value.clone()).await;
        value
    }

    /// 删除某个 key 的状态
    pub async fn invalidate(&self, key: &K) {
        self.inner.invalidate(key).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn get_returns_inserted_value() {
        let store: LocalStore<String, u64> = LocalStore::new(100, Duration::from_secs(60));
        store.insert("user:1".to_string(), 3).await;
        assert_eq!(store.get(&"user:1".to_string()).await, Some(3));
        assert!(store.get(&"user:2".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn get_or_insert_initializes_missing_key() {
        let store: LocalStore<String, u64> = LocalStore::new(100, Duration::from_secs(60));
        let v = store.get_or_insert("key:1".to_string(), 7).await;
        assert_eq!(v, 7);
        // 第二次读取返回已有值，不覆盖
        assert_eq!(store.get_or_insert("key:1".to_string(), 99).await, 7);
    }

    #[tokio::test]
    async fn update_applies_closure() {
        let store: LocalStore<String, u64> = LocalStore::new(100, Duration::from_secs(60));
        let init = 1u64;
        let n = store.update("key:a".to_string(), init, |c| c + 1).await;
        assert_eq!(n, 2);
        let n = store.update("key:a".to_string(), init, |c| c + 1).await;
        assert_eq!(n, 3);
    }

    #[tokio::test]
    async fn invalidate_removes_key() {
        let store: LocalStore<String, u64> = LocalStore::new(100, Duration::from_secs(60));
        store.insert("key:x".to_string(), 5).await;
        let k = "key:x".to_string();
        store.invalidate(&k).await;
        assert!(store.get(&k).await.is_none());
    }
}
