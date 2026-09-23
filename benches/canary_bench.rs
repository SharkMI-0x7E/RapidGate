//! 灰度路由性能基准测试
//!
//! 测试阶段三新增的灰度路由策略性能：
//! - WeightPolicy：按权重随机选择
//! - HeaderPolicy：根据请求头匹配
//! - CookiePolicy：根据 Cookie 匹配
//! - StickySession：会话黏性
//! - CanaryRouter：完整决策流程

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rapidgate::core::canary::policy::{CanaryPolicy, CookiePolicy, HeaderPolicy, WeightPolicy};
use rapidgate::core::canary::sticky::StickySession;
use rapidgate::core::config::route::{AuthConfig, MatchRule, RouteConfig, UpstreamRef};
use rapidgate::core::config::upstream::UpstreamConfig;
use rapidgate::core::routing::CanaryRouter;
use std::collections::HashMap;
use std::sync::Arc;

/// 创建测试用的 upstream 列表
fn make_test_upstreams() -> Vec<UpstreamConfig> {
    vec![
        UpstreamConfig {
            id: "stable".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: "test-key".to_string(),
            load_balancer: Default::default(),
            models: vec![],
            timeout_ms: None,
            pool: None,
            max_retries: None,
        },
        UpstreamConfig {
            id: "canary".to_string(),
            provider: "openai".to_string(),
            base_url: "https://api.openai.com".to_string(),
            api_key: "test-key".to_string(),
            load_balancer: Default::default(),
            models: vec![],
            timeout_ms: None,
            pool: None,
            max_retries: None,
        },
        UpstreamConfig {
            id: "backup".to_string(),
            provider: "anthropic".to_string(),
            base_url: "https://api.anthropic.com".to_string(),
            api_key: "test-key".to_string(),
            load_balancer: Default::default(),
            models: vec![],
            timeout_ms: None,
            pool: None,
            max_retries: None,
        },
    ]
}

/// 创建测试用的路由配置
fn make_test_route() -> RouteConfig {
    RouteConfig {
        name: "test-route".to_string(),
        match_rule: MatchRule {
            method: "POST".to_string(),
            path: "/v1/chat/completions".to_string(),
            host: None,
            headers: vec![],
            query: vec![],
            cookies: vec![],
        },
        upstream: Some(UpstreamRef {
            id: "test-upstream".to_string(),
        }),
        auth: AuthConfig::default(),
        rate_limit: None,
        canary: None,
    }
}

/// 测试 WeightPolicy 性能
fn bench_weight_policy(c: &mut Criterion) {
    let policy = WeightPolicy::new(80);
    let headers = HashMap::new();
    let cookies = HashMap::new();
    let client_ip = "192.168.1.100";
    let upstream_count = 3;

    c.bench_function("weight_policy_select", |b| {
        b.iter(|| {
            policy.select_upstream(
                black_box(&headers),
                black_box(&cookies),
                black_box(client_ip),
                black_box(upstream_count),
            )
        })
    });
}

/// 测试 HeaderPolicy 性能（匹配场景）
fn bench_header_policy_match(c: &mut Criterion) {
    let policy = HeaderPolicy {
        header_name: "x-canary-version".to_string(),
        header_value: "v2".to_string(),
        target_index: 1,
    };

    let mut headers = HashMap::new();
    headers.insert("x-canary-version".to_string(), "v2".to_string());
    let cookies = HashMap::new();
    let client_ip = "192.168.1.100";
    let upstream_count = 3;

    c.bench_function("header_policy_match", |b| {
        b.iter(|| {
            policy.select_upstream(
                black_box(&headers),
                black_box(&cookies),
                black_box(client_ip),
                black_box(upstream_count),
            )
        })
    });
}

/// 测试 HeaderPolicy 性能（不匹配场景）
fn bench_header_policy_no_match(c: &mut Criterion) {
    let policy = HeaderPolicy {
        header_name: "x-canary-version".to_string(),
        header_value: "v2".to_string(),
        target_index: 1,
    };

    let headers = HashMap::new();
    let cookies = HashMap::new();
    let client_ip = "192.168.1.100";
    let upstream_count = 3;

    c.bench_function("header_policy_no_match", |b| {
        b.iter(|| {
            policy.select_upstream(
                black_box(&headers),
                black_box(&cookies),
                black_box(client_ip),
                black_box(upstream_count),
            )
        })
    });
}

/// 测试 CookiePolicy 性能
fn bench_cookie_policy(c: &mut Criterion) {
    let policy = CookiePolicy {
        cookie_name: "canary_group".to_string(),
        cookie_value: "beta".to_string(),
        target_index: 2,
    };

    let headers = HashMap::new();
    let mut cookies = HashMap::new();
    cookies.insert("canary_group".to_string(), "beta".to_string());
    let client_ip = "192.168.1.100";
    let upstream_count = 3;

    c.bench_function("cookie_policy_select", |b| {
        b.iter(|| {
            policy.select_upstream(
                black_box(&headers),
                black_box(&cookies),
                black_box(client_ip),
                black_box(upstream_count),
            )
        })
    });
}

/// 测试 StickySession 会话提取性能
fn bench_sticky_session_extract(c: &mut Criterion) {
    let sticky = StickySession::new("session_id".to_string(), 3600);
    let mut cookies = HashMap::new();
    cookies.insert("session_id".to_string(), "abc123def456".to_string());

    c.bench_function("sticky_session_extract", |b| {
        b.iter(|| sticky.extract_session_id(black_box(&cookies)))
    });
}

/// 测试 StickySession upstream 选择性能
fn bench_sticky_session_select(c: &mut Criterion) {
    let sticky = StickySession::new("session_id".to_string(), 3600);
    let session_id = "test_session_abc123";
    let upstream_count = 3;

    c.bench_function("sticky_session_select", |b| {
        b.iter(|| sticky.select_upstream(black_box(session_id), black_box(upstream_count)))
    });
}

/// 测试 StickySession ID 生成性能
fn bench_sticky_session_generate(c: &mut Criterion) {
    let sticky = StickySession::new("session_id".to_string(), 3600);
    let client_ip = "192.168.1.100";

    c.bench_function("sticky_session_generate", |b| {
        b.iter(|| sticky.generate_session_id(black_box(client_ip)))
    });
}

/// 测试 CanaryRouter 完整决策流程（无会话黏性）
fn bench_canary_router_no_sticky(c: &mut Criterion) {
    let policy = Arc::new(WeightPolicy::new(80));
    let router = CanaryRouter::new(policy, None);
    let route = make_test_route();
    let upstreams = make_test_upstreams();
    let headers = HashMap::new();
    let cookies = HashMap::new();
    let client_ip = "192.168.1.100";

    c.bench_function("canary_router_no_sticky", |b| {
        b.iter(|| {
            router.select_upstream(
                black_box(&route),
                black_box(&headers),
                black_box(&cookies),
                black_box(client_ip),
                black_box(&upstreams),
            )
        })
    });
}

/// 测试 CanaryRouter 完整决策流程（有会话黏性）
fn bench_canary_router_with_sticky(c: &mut Criterion) {
    let policy = Arc::new(WeightPolicy::new(80));
    let sticky = StickySession::new("session_id".to_string(), 3600);
    let router = CanaryRouter::new(policy, Some(sticky));
    let route = make_test_route();
    let upstreams = make_test_upstreams();
    let headers = HashMap::new();
    let mut cookies = HashMap::new();
    cookies.insert("session_id".to_string(), "test_session_123".to_string());
    let client_ip = "192.168.1.100";

    c.bench_function("canary_router_with_sticky", |b| {
        b.iter(|| {
            router.select_upstream(
                black_box(&route),
                black_box(&headers),
                black_box(&cookies),
                black_box(client_ip),
                black_box(&upstreams),
            )
        })
    });
}

criterion_group!(
    benches,
    bench_weight_policy,
    bench_header_policy_match,
    bench_header_policy_no_match,
    bench_cookie_policy,
    bench_sticky_session_extract,
    bench_sticky_session_select,
    bench_sticky_session_generate,
    bench_canary_router_no_sticky,
    bench_canary_router_with_sticky,
);
criterion_main!(benches);
