//! 滑动窗口限流（spec §4.5）

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::core::error::CoreError;
use crate::core::ratelimit::local_store::LocalStore;
use crate::core::ratelimit::{Decision, LimitKey, RateLimiter};

pub struct SlidingWindow {
    window: Duration,
    max_requests: usize,
    store: LocalStore<LimitKey, VecDeque<Instant>>,
}

impl SlidingWindow {
    pub fn new(rps: u32, _burst: u32) -> Self {
        // rps 决定窗口长度，窗口内允许 rps 次
        Self {
            window: Duration::from_secs(1)
                .checked_div(rps.max(1))
                .unwrap_or(Duration::from_secs(1)),
            max_requests: rps as usize,
            store: LocalStore::new(10_000, Duration::from_secs(600)),
        }
    }
}

#[async_trait]
impl RateLimiter for SlidingWindow {
    async fn check(&self, key: &LimitKey) -> Result<Decision, CoreError> {
        let now = Instant::now();
        let window = self.window;
        let max_requests = self.max_requests;
        let mut entry = self
            .store
            .update(key.clone(), VecDeque::new(), |mut entry| {
                // 弹出窗口外的旧记录
                while let Some(&front) = entry.front() {
                    if now.duration_since(front) > window {
                        entry.pop_front();
                    } else {
                        break;
                    }
                }
                entry
            })
            .await;

        if entry.len() < max_requests {
            entry.push_back(now);
            self.store.insert(key.clone(), entry).await;
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
    async fn allows_up_to_rps() {
        let lim = SlidingWindow::new(3, 3);
        for _ in 0..3 {
            assert_eq!(lim.check(&"u1".into()).await.unwrap(), Decision::Allow);
        }
        assert_eq!(lim.check(&"u1".into()).await.unwrap(), Decision::Deny);
    }
}
