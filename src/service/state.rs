//! AppState — 跨请求共享状态（spec §5.2）
//!
//! 全部 Arc 共享，通过 `axum::extract::State` 注入 handler。

use std::path::PathBuf;
use std::sync::Arc;

use moka::future::Cache;
use tokio::sync::mpsc;

use crate::core::audit::AuditEvent;
use crate::core::config::gateway::SsrfConfig;
use crate::core::config::route::RateLimitConfig;
use crate::core::config::upstream::{UpstreamConfig, UpstreamId};
use crate::core::observability::Metrics;
use crate::core::routing::Router;
use crate::service::providers::ProviderFactory;
use crate::service::upstream_pool::UpstreamPool;

pub type UpstreamCache = Cache<UpstreamId, Arc<reqwest::Client>>;
pub type LimiterCache = Cache<String, Arc<dyn crate::core::ratelimit::RateLimiter>>;

pub struct AppState {
    pub route_table: Router,
    pub upstreams: UpstreamCache,
    pub limiters: LimiterCache,
    pub audit_tx: mpsc::UnboundedSender<AuditEvent>,
    pub config_dir: PathBuf,
    pub max_body_bytes: usize,
    pub request_timeout_ms: u64,
    pub upstream_configs: Vec<UpstreamConfig>,
    pub default_rate_limit: RateLimitConfig,
    pub provider_factory: ProviderFactory,
    /// Prometheus 指标收集器（/metrics 端点导出）
    pub metrics: Metrics,
    /// 上游连接池 + SSRF 防护（转发路径唯一出口）
    pub upstream_pool: UpstreamPool,
}

impl AppState {
    /// 构造共享状态；返回 (AppState, 审计事件接收端)，启动方负责消费审计事件。
    pub fn new(
        route_table: Router,
        upstream_configs: Vec<UpstreamConfig>,
        default_rate_limit: RateLimitConfig,
        config_dir: PathBuf,
        max_body_bytes: usize,
        request_timeout_ms: u64,
        ssrf: SsrfConfig,
    ) -> (Self, mpsc::UnboundedReceiver<AuditEvent>) {
        let upstreams: UpstreamCache = Cache::builder().max_capacity(1024).build();
        let limiters: LimiterCache = Cache::builder().max_capacity(1024).build();
        let (audit_tx, audit_rx) = mpsc::unbounded_channel();
        let upstream_pool = UpstreamPool::new(
            ssrf.enabled,
            ssrf.allow_list,
            request_timeout_ms,
            max_body_bytes,
        );
        let state = Self {
            route_table,
            upstreams,
            limiters,
            audit_tx,
            config_dir,
            max_body_bytes,
            request_timeout_ms,
            upstream_configs,
            default_rate_limit,
            provider_factory: ProviderFactory,
            metrics: Metrics::default(),
            upstream_pool,
        };
        (state, audit_rx)
    }
}
