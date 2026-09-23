//! Prometheus 指标导出（spec §4.6）
//!
//! 提供请求数、延迟、错误率等指标的 Prometheus 导出能力。

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, Opts, Registry};

/// 指标收集器
pub struct Metrics {
    registry: Registry,
    /// 请求总数（按路由、方法、状态码分类）
    requests_total: IntCounterVec,
    /// 请求延迟分布（按路由分类）
    request_duration: HistogramVec,
    /// 上游请求总数（按 upstream 名称分类）
    upstream_requests_total: IntCounterVec,
    /// 上游请求延迟（按 upstream 名称分类）
    upstream_duration: HistogramVec,
    /// 限流触发次数（按 key 分类）
    ratelimit_triggered: IntCounterVec,
    /// 熔断器状态变化次数（按 upstream 名称分类）
    breaker_state_changes: IntCounterVec,
}

impl Metrics {
    /// 创建新的指标收集器
    pub fn new() -> Result<Self, prometheus::Error> {
        let registry = Registry::new();

        let requests_total = IntCounterVec::new(
            Opts::new("rapidgate_requests_total", "Total number of requests"),
            &["route", "method", "status"],
        )?;

        let request_duration = HistogramVec::new(
            HistogramOpts::new(
                "rapidgate_request_duration_seconds",
                "Request duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
            &["route"],
        )?;

        let upstream_requests_total = IntCounterVec::new(
            Opts::new(
                "rapidgate_upstream_requests_total",
                "Total number of upstream requests",
            ),
            &["upstream", "status"],
        )?;

        let upstream_duration = HistogramVec::new(
            HistogramOpts::new(
                "rapidgate_upstream_duration_seconds",
                "Upstream request duration in seconds",
            )
            .buckets(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]),
            &["upstream"],
        )?;

        let ratelimit_triggered = IntCounterVec::new(
            Opts::new(
                "rapidgate_ratelimit_triggered_total",
                "Total number of rate limit triggers",
            ),
            &["key"],
        )?;

        let breaker_state_changes = IntCounterVec::new(
            Opts::new(
                "rapidgate_breaker_state_changes_total",
                "Total number of breaker state changes",
            ),
            &["upstream", "from_state", "to_state"],
        )?;

        registry.register(Box::new(requests_total.clone()))?;
        registry.register(Box::new(request_duration.clone()))?;
        registry.register(Box::new(upstream_requests_total.clone()))?;
        registry.register(Box::new(upstream_duration.clone()))?;
        registry.register(Box::new(ratelimit_triggered.clone()))?;
        registry.register(Box::new(breaker_state_changes.clone()))?;

        Ok(Self {
            registry,
            requests_total,
            request_duration,
            upstream_requests_total,
            upstream_duration,
            ratelimit_triggered,
            breaker_state_changes,
        })
    }

    /// 记录请求
    pub fn record_request(&self, route: &str, method: &str, status: u16, duration_secs: f64) {
        let status_str = status.to_string();
        self.requests_total
            .with_label_values(&[route, method, &status_str])
            .inc();
        self.request_duration
            .with_label_values(&[route])
            .observe(duration_secs);
    }

    /// 记录上游请求
    pub fn record_upstream_request(&self, upstream: &str, status: u16, duration_secs: f64) {
        let status_str = status.to_string();
        self.upstream_requests_total
            .with_label_values(&[upstream, &status_str])
            .inc();
        self.upstream_duration
            .with_label_values(&[upstream])
            .observe(duration_secs);
    }

    /// 记录限流触发
    pub fn record_ratelimit(&self, key: &str) {
        self.ratelimit_triggered.with_label_values(&[key]).inc();
    }

    /// 记录熔断器状态变化
    pub fn record_breaker_change(&self, upstream: &str, from_state: &str, to_state: &str) {
        self.breaker_state_changes
            .with_label_values(&[upstream, from_state, to_state])
            .inc();
    }

    /// 导出 Prometheus 格式文本
    pub fn export(&self) -> String {
        prometheus::TextEncoder::new()
            .encode_to_string(&self.registry.gather())
            // encode 只会在编码器内存分配失败时异常，这里用空串兜底
            .unwrap_or_default()
    }

    /// 获取 Registry 引用（用于自定义指标注册）
    pub fn registry(&self) -> &Registry {
        &self.registry
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new().expect("failed to create metrics")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_creation() {
        let metrics = Metrics::new().unwrap();
        // Record some data to ensure metrics are registered
        metrics.record_request("/test", "GET", 200, 0.1);
        let output = metrics.export();
        assert!(output.contains("rapidgate_requests_total"));
        assert!(output.contains("rapidgate_request_duration_seconds"));
    }

    #[test]
    fn test_record_request() {
        let metrics = Metrics::new().unwrap();
        metrics.record_request("/v1/chat", "POST", 200, 0.123);
        let output = metrics.export();
        assert!(output.contains("rapidgate_requests_total"));
    }

    #[test]
    fn test_record_upstream_request() {
        let metrics = Metrics::new().unwrap();
        metrics.record_upstream_request("openai", 200, 0.456);
        let output = metrics.export();
        assert!(output.contains("rapidgate_upstream_requests_total"));
    }

    #[test]
    fn test_record_ratelimit() {
        let metrics = Metrics::new().unwrap();
        metrics.record_ratelimit("user_123");
        let output = metrics.export();
        assert!(output.contains("rapidgate_ratelimit_triggered_total"));
    }

    #[test]
    fn test_record_breaker_change() {
        let metrics = Metrics::new().unwrap();
        metrics.record_breaker_change("openai", "closed", "open");
        let output = metrics.export();
        assert!(output.contains("rapidgate_breaker_state_changes_total"));
    }
}
