//! GatewayConfig — 网关总配置（spec §6.1）

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayConfig {
    pub listen: String,
    /// Admin 独立监听地址（/admin/*、/metrics）；默认 127.0.0.1:9090
    #[serde(default = "default_admin_listen")]
    pub admin_listen: String,
    pub request_timeout_ms: u64,
    pub max_body_bytes: usize,
    pub shutdown_timeout_ms: u64,
    pub logging: LoggingConfig,
    pub upstream_allowlist: UpstreamAllowlist,
    pub defaults: Defaults,
    pub upstreams: Vec<crate::core::config::upstream::UpstreamConfig>,
    /// SSRF 防护配置（默认严格启用）
    #[serde(default)]
    pub ssrf: SsrfConfig,
}

pub fn default_admin_listen() -> String {
    "127.0.0.1:9090".into()
}

/// SSRF 防护配置（spec §8）
///
/// `enabled=true`（默认）时：DNS 解析目标 base_url 的 host，拦截回环/私有/链路本地 IP。
/// `allow_list` 里的主机名可精确豁免（仅匹配 base_url 的 host）。
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SsrfConfig {
    /// 默认严格启用
    #[serde(default = "default_ssrf_enabled")]
    pub enabled: bool,
    /// 显式豁免的主机名列表（精确匹配 base_url 的 host）
    #[serde(default)]
    pub allow_list: Vec<String>,
}

fn default_ssrf_enabled() -> bool {
    true
}

impl Default for SsrfConfig {
    fn default() -> Self {
        Self {
            enabled: default_ssrf_enabled(),
            allow_list: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamAllowlist {
    pub hosts: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub rate_limit: crate::core::config::route::RateLimitConfig,
    pub breaker: BreakerDefaults,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakerDefaults {
    pub failure_threshold: u32,
    pub open_duration_ms: u64,
}
