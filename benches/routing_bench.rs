//! 路由匹配基准测试（spec §5.3）
//!
//! 测试精确/前缀/正则三种匹配模式的 p50/p99 延迟。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rapidgate::core::config::route::{AuthConfig, MatchRule, RouteConfig, UpstreamRef};
use rapidgate::core::routing::RouteTable;

fn make_test_table() -> RouteTable {
    let routes = vec![
        RouteConfig {
            name: "exact-chat".to_string(),
            match_rule: MatchRule {
                method: "POST".to_string(),
                path: "/v1/chat/completions".to_string(),
                host: None,
                headers: vec![],
                query: vec![],
                cookies: vec![],
            },
            upstream: Some(UpstreamRef {
                id: "openai".to_string(),
            }),
            auth: AuthConfig::default(),
            rate_limit: None,
            canary: None,
        },
        RouteConfig {
            name: "prefix-models".to_string(),
            match_rule: MatchRule {
                method: "GET".to_string(),
                path: "/v1/models/".to_string(),
                host: None,
                headers: vec![],
                query: vec![],
                cookies: vec![],
            },
            upstream: Some(UpstreamRef {
                id: "openai".to_string(),
            }),
            auth: AuthConfig::default(),
            rate_limit: None,
            canary: None,
        },
        RouteConfig {
            name: "regex-users".to_string(),
            match_rule: MatchRule {
                method: "GET".to_string(),
                path: "~^/v1/users/\\d+/profile$".to_string(),
                host: None,
                headers: vec![],
                query: vec![],
                cookies: vec![],
            },
            upstream: Some(UpstreamRef {
                id: "openai".to_string(),
            }),
            auth: AuthConfig::default(),
            rate_limit: None,
            canary: None,
        },
    ];
    RouteTable::new(routes).unwrap()
}

fn bench_exact_match(c: &mut Criterion) {
    let table = make_test_table();
    c.bench_function("exact_match", |b| {
        b.iter(|| {
            table.match_request(
                black_box("POST"),
                black_box("/v1/chat/completions"),
                &[],
                &[],
            )
        })
    });
}

fn bench_prefix_match(c: &mut Criterion) {
    let table = make_test_table();
    c.bench_function("prefix_match", |b| {
        b.iter(|| table.match_request(black_box("GET"), black_box("/v1/models/gpt-4"), &[], &[]))
    });
}

fn bench_regex_match(c: &mut Criterion) {
    let table = make_test_table();
    c.bench_function("regex_match", |b| {
        b.iter(|| {
            table.match_request(
                black_box("GET"),
                black_box("/v1/users/12345/profile"),
                &[],
                &[],
            )
        })
    });
}

criterion_group!(
    benches,
    bench_exact_match,
    bench_prefix_match,
    bench_regex_match
);
criterion_main!(benches);
