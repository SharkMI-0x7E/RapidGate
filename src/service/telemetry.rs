//! service/telemetry — tracing 初始化 + 敏感 Header 脱敏 + 慢请求日志（spec §5）

use std::sync::OnceLock;

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use crate::core::observability::{OtelConfig, OtelInitializer};

static INIT: OnceLock<()> = OnceLock::new();

/// 初始化全局 tracing subscriber；幂等。
///
/// OpenTelemetry 默认 noop/关闭：仅当环境变量 `RGD_OTEL_ENABLED=1` 时才初始化 OTLP
/// （避免无 collector 时阻塞启动）。初始化失败只降级为日志告警，不影响网关运行。
pub fn init() {
    INIT.get_or_init(|| {
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,rapidgate=debug"));

        let fmt_layer = fmt::layer()
            .with_target(true)
            .with_thread_ids(false)
            .with_line_number(false);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();

        // RGD_OTEL_ENABLED=1 时启用 OTLP tracing 导出（best-effort）
        if std::env::var("RGD_OTEL_ENABLED").as_deref() == Ok("1") {
            let cfg = OtelConfig::default();
            match OtelInitializer::init_tracing(&cfg) {
                Ok(_) => tracing::info!(
                    endpoint = %cfg.endpoint,
                    service = %cfg.service_name,
                    "opentelemetry tracing enabled"
                ),
                Err(e) => tracing::warn!(error = %e, "opentelemetry init failed (otel disabled)"),
            }
        }
    });
}

/// 敏感 Header 名称列表
const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "x-api-key",
    "cookie",
    "set-cookie",
    "proxy-authorization",
];

/// 脱敏 Header 值
pub fn redact_header(name: &str, value: &str) -> String {
    if SENSITIVE_HEADERS
        .iter()
        .any(|s| s.eq_ignore_ascii_case(name))
    {
        "***".to_string()
    } else {
        value.to_string()
    }
}

/// 检查是否为慢请求并记录 warn 日志
pub fn check_slow_request(latency_ms: u64, threshold_ms: u64, method: &str, path: &str) {
    if latency_ms > threshold_ms {
        tracing::warn!(
            method = %method,
            path = %path,
            latency_ms = latency_ms,
            threshold_ms = threshold_ms,
            "slow request detected"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init();
        init();
    }

    #[test]
    fn redacts_authorization() {
        assert_eq!(redact_header("Authorization", "Bearer sk-xxx"), "***");
    }

    #[test]
    fn redacts_x_api_key() {
        assert_eq!(redact_header("X-Api-Key", "my-secret-key"), "***");
    }

    #[test]
    fn preserves_normal_header() {
        assert_eq!(
            redact_header("Content-Type", "application/json"),
            "application/json"
        );
    }
}
