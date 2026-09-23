//! UpstreamConfig + LoadBalancer（spec §4.2）

use serde::Deserialize;

/// 上游 ID 字符串（kebab-case，spec §9.4）
pub type UpstreamId = String;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamConfig {
    pub id: UpstreamId,
    pub provider: String,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub load_balancer: LoadBalancer,
    #[serde(default)]
    pub models: Vec<String>,
    /// Per-upstream request timeout in milliseconds (optional, falls back to global)
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Per-upstream pool configuration (optional, falls back to global)
    #[serde(default)]
    pub pool: Option<UpstreamPoolConfig>,
    /// 上游重试次数（v2.yaml 保留解析，阶段内不实现重试逻辑）
    #[serde(default)]
    pub max_retries: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamPoolConfig {
    /// Idle timeout for connections in seconds
    #[serde(default = "default_pool_idle_timeout")]
    pub idle_timeout_secs: u64,
    /// Max idle connections per host
    #[serde(default = "default_pool_max_idle_per_host")]
    pub max_idle_per_host: usize,
}

fn default_pool_idle_timeout() -> u64 {
    90
}

fn default_pool_max_idle_per_host() -> usize {
    10
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancer {
    #[default]
    RoundRobin,
    Random,
    // 一致性哈希留 [S2]
}
