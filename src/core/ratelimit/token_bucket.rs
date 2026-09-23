//! 令牌桶限流（spec §4.5）

use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::core::error::CoreError;
use crate::core::ratelimit::local_store::LocalStore;
use crate::core::ratelimit::{Decision, LimitKey, RateLimiter};

/// 令牌桶状态
#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// 令牌桶限流器，状态由 `LocalStore`（Moka）持久化 —— local 模式默认后端
pub struct TokenBucket {
    rps: f64,
    burst: f64,
    store: LocalStore<LimitKey, Bucket>,
}

impl TokenBucket {
    pub fn new(rps: u32, burst: u32) -> Self {
        Self {
            rps: rps as f64,
            burst: burst as f64,
            // 每 key 若 10 分钟不活跃自动清理，防止内存被随机 key 撑爆
            store: LocalStore::new(10_000, Duration::from_secs(600)),
        }
    }
}

#[async_trait]
impl RateLimiter for TokenBucket {
    async fn check(&self, key: &LimitKey) -> Result<Decision, CoreError> {
        let now = Instant::now();
        let init = Bucket {
            tokens: self.burst,
            last_refill: now,
        };
        let mut bucket = self
            .store
            .update(key.clone(), init, |mut b| {
                // 补充令牌
                let elapsed = now.duration_since(b.last_refill).as_secs_f64();
                b.tokens = (b.tokens + elapsed * self.rps).min(self.burst);
                b.last_refill = now;
                b
            })
            .await;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            self.store.insert(key.clone(), bucket).await;
            Ok(Decision::Allow)
        } else {
            Ok(Decision::Deny)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_burst() {
        let lim = TokenBucket::new(1, 5);
        for _ in 0..5 {
            assert_eq!(lim.check(&"u1".into()).await.unwrap(), Decision::Allow);
        }
    }

    #[tokio::test]
    async fn denies_over_burst() {
        let lim = TokenBucket::new(1, 2);
        assert_eq!(lim.check(&"u1".into()).await.unwrap(), Decision::Allow);
        assert_eq!(lim.check(&"u1".into()).await.unwrap(), Decision::Allow);
        assert_eq!(lim.check(&"u1".into()).await.unwrap(), Decision::Deny);
    }

    #[tokio::test]
    async fn per_key_isolation() {
        let lim = TokenBucket::new(1, 1);
        assert_eq!(lim.check(&"a".into()).await.unwrap(), Decision::Allow);
        assert_eq!(lim.check(&"a".into()).await.unwrap(), Decision::Deny);
        assert_eq!(lim.check(&"b".into()).await.unwrap(), Decision::Allow);
    }
}
