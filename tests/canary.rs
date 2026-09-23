//! 灰度路由集成测试（spec §2 [S3]）
//!
//! 覆盖灰度权重 / Header / Cookie 策略，以及会话黏性。

use std::collections::HashMap;
use std::sync::Arc;

use rapidgate::core::canary::policy::{CanaryPolicy, CookiePolicy, HeaderPolicy, WeightPolicy};
use rapidgate::core::canary::sticky::StickySession;
use rapidgate::core::config::route::{AuthConfig, MatchRule, RouteConfig, UpstreamRef};
use rapidgate::core::config::upstream::{LoadBalancer, UpstreamConfig};
use rapidgate::core::routing::CanaryRouter;

// -------------------- 辅助构造 --------------------

fn make_route() -> RouteConfig {
    RouteConfig {
        name: "canary-test".to_string(),
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

fn make_upstreams(count: usize) -> Vec<UpstreamConfig> {
    (0..count)
        .map(|i| UpstreamConfig {
            id: format!("upstream-{i}"),
            provider: "openai".to_string(),
            base_url: "https://api.example.com".to_string(),
            api_key: "sk-test-key-1234567890abcdef".to_string(),
            load_balancer: LoadBalancer::default(),
            models: vec![],
            timeout_ms: None,
            pool: None,
            max_retries: None,
        })
        .collect()
}

// -------------------- 权重策略 --------------------

#[test]
fn weight_policy_zero_upstreams_returns_zero() {
    let policy = WeightPolicy::new(50);
    let headers = HashMap::new();
    let cookies = HashMap::new();
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 0);
    assert_eq!(idx, 0);
}

#[test]
fn weight_policy_single_upstream_always_zero() {
    let policy = WeightPolicy::new(50);
    let headers = HashMap::new();
    let cookies = HashMap::new();
    for _ in 0..10 {
        let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 1);
        assert_eq!(idx, 0);
    }
}

#[test]
fn weight_policy_full_weight_to_primary() {
    // primary_weight = 100 时，所有流量应走主 upstream
    let policy = WeightPolicy::new(100);
    let headers = HashMap::new();
    let cookies = HashMap::new();
    for _ in 0..50 {
        let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
        assert_eq!(idx, 0);
    }
}

#[test]
fn weight_policy_zero_weight_to_others() {
    // primary_weight = 0 时，所有流量应走其他 upstream
    let policy = WeightPolicy::new(0);
    let headers = HashMap::new();
    let cookies = HashMap::new();
    for _ in 0..50 {
        let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
        assert!((1..3).contains(&idx), "idx {idx} out of range");
    }
}

#[test]
fn weight_policy_distribution_roughly_matches_ratio() {
    // 权重 70:30，跑 1000 次，主 upstream 被选中的次数应在 600~800 之间
    let policy = WeightPolicy::new(70);
    let headers = HashMap::new();
    let cookies = HashMap::new();
    let trials = 1000;
    let mut primary_count = 0u32;
    for _ in 0..trials {
        let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 2);
        if idx == 0 {
            primary_count += 1;
        }
    }
    assert!(
        primary_count > 600 && primary_count < 800,
        "primary_count={primary_count} out of expected range"
    );
}

// -------------------- Header 策略 --------------------

#[test]
fn header_policy_matches_exact_value() {
    let policy = HeaderPolicy {
        header_name: "x-canary".to_string(),
        header_value: "true".to_string(),
        target_index: 1,
    };
    let mut headers = HashMap::new();
    headers.insert("x-canary".to_string(), "true".to_string());
    let cookies = HashMap::new();
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 1);
}

#[test]
fn header_policy_falls_back_when_no_match() {
    let policy = HeaderPolicy {
        header_name: "x-canary".to_string(),
        header_value: "true".to_string(),
        target_index: 1,
    };
    let headers = HashMap::new();
    let cookies = HashMap::new();
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 0);
}

#[test]
fn header_policy_falls_back_on_wrong_value() {
    let policy = HeaderPolicy {
        header_name: "x-canary".to_string(),
        header_value: "true".to_string(),
        target_index: 1,
    };
    let mut headers = HashMap::new();
    headers.insert("x-canary".to_string(), "false".to_string());
    let cookies = HashMap::new();
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 0);
}

#[test]
fn header_policy_target_out_of_range_falls_back() {
    let policy = HeaderPolicy {
        header_name: "x-canary".to_string(),
        header_value: "true".to_string(),
        target_index: 5, // 超出 upstream 数量
    };
    let mut headers = HashMap::new();
    headers.insert("x-canary".to_string(), "true".to_string());
    let cookies = HashMap::new();
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 0);
}

// -------------------- Cookie 策略 --------------------

#[test]
fn cookie_policy_matches_exact_value() {
    let policy = CookiePolicy {
        cookie_name: "group".to_string(),
        cookie_value: "beta".to_string(),
        target_index: 2,
    };
    let headers = HashMap::new();
    let mut cookies = HashMap::new();
    cookies.insert("group".to_string(), "beta".to_string());
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 2);
}

#[test]
fn cookie_policy_falls_back_when_no_match() {
    let policy = CookiePolicy {
        cookie_name: "group".to_string(),
        cookie_value: "beta".to_string(),
        target_index: 2,
    };
    let headers = HashMap::new();
    let cookies = HashMap::new();
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 0);
}

#[test]
fn cookie_policy_target_out_of_range_falls_back() {
    let policy = CookiePolicy {
        cookie_name: "group".to_string(),
        cookie_value: "beta".to_string(),
        target_index: 10,
    };
    let headers = HashMap::new();
    let mut cookies = HashMap::new();
    cookies.insert("group".to_string(), "beta".to_string());
    let idx = policy.select_upstream(&headers, &cookies, "127.0.0.1", 3);
    assert_eq!(idx, 0);
}

// -------------------- CanaryRouter 集成 --------------------

#[test]
fn canary_router_with_weight_policy_selects_upstream() {
    let policy = Arc::new(WeightPolicy::new(80));
    let router = CanaryRouter::new(policy, None);
    let route = make_route();
    let upstreams = make_upstreams(3);
    let headers = HashMap::new();
    let cookies = HashMap::new();

    let result = router.select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams);
    assert!(result.is_ok());
    let selected = result.unwrap();
    assert!(selected.id.starts_with("upstream-"));
}

#[test]
fn canary_router_with_header_policy_routes_correctly() {
    let policy = Arc::new(HeaderPolicy {
        header_name: "x-version".to_string(),
        header_value: "v2".to_string(),
        target_index: 1,
    });
    let router = CanaryRouter::new(policy, None);
    let route = make_route();
    let upstreams = make_upstreams(2);

    // 带匹配 header
    let mut headers = HashMap::new();
    headers.insert("x-version".to_string(), "v2".to_string());
    let cookies = HashMap::new();
    let selected = router
        .select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams)
        .unwrap();
    assert_eq!(selected.id, "upstream-1");

    // 不带匹配 header
    let headers = HashMap::new();
    let selected = router
        .select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams)
        .unwrap();
    assert_eq!(selected.id, "upstream-0");
}

#[test]
fn canary_router_with_cookie_policy_routes_correctly() {
    let policy = Arc::new(CookiePolicy {
        cookie_name: "env".to_string(),
        cookie_value: "staging".to_string(),
        target_index: 1,
    });
    let router = CanaryRouter::new(policy, None);
    let route = make_route();
    let upstreams = make_upstreams(2);
    let headers = HashMap::new();

    // 带匹配 cookie
    let mut cookies = HashMap::new();
    cookies.insert("env".to_string(), "staging".to_string());
    let selected = router
        .select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams)
        .unwrap();
    assert_eq!(selected.id, "upstream-1");

    // 不带匹配 cookie
    let cookies = HashMap::new();
    let selected = router
        .select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams)
        .unwrap();
    assert_eq!(selected.id, "upstream-0");
}

#[test]
fn canary_router_empty_upstreams_returns_error() {
    let policy = Arc::new(WeightPolicy::new(50));
    let router = CanaryRouter::new(policy, None);
    let route = make_route();
    let upstreams: Vec<UpstreamConfig> = vec![];
    let headers = HashMap::new();
    let cookies = HashMap::new();

    let result = router.select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams);
    assert!(result.is_err());
}

// -------------------- 会话黏性 --------------------

#[test]
fn sticky_session_consistent_upstream_selection() {
    let sticky = StickySession::new("session_id".to_string(), 3600);
    let session_id = "user-abc-123-def";

    // 同一 session_id 应始终选择相同 upstream
    let selections: Vec<usize> = (0..20)
        .map(|_| sticky.select_upstream(session_id, 4))
        .collect();
    let first = selections[0];
    assert!(selections.iter().all(|&s| s == first));
}

#[test]
fn sticky_session_different_sessions_may_differ() {
    let sticky = StickySession::new("session_id".to_string(), 3600);

    // 不同 session_id 在大量样本下应产生至少两种不同选择
    let selections: Vec<usize> = (0..100)
        .map(|i| sticky.select_upstream(&format!("session-{i}"), 4))
        .collect();
    let unique: std::collections::HashSet<_> = selections.iter().collect();
    assert!(unique.len() > 1, "expected distribution across upstreams");
}

#[test]
fn sticky_session_extract_from_cookies() {
    let sticky = StickySession::new("sid".to_string(), 3600);
    let mut cookies = HashMap::new();
    cookies.insert("sid".to_string(), "token-xyz".to_string());

    assert_eq!(
        sticky.extract_session_id(&cookies),
        Some("token-xyz".to_string())
    );
}

#[test]
fn sticky_session_extract_missing_cookie() {
    let sticky = StickySession::new("sid".to_string(), 3600);
    let cookies = HashMap::new();
    assert_eq!(sticky.extract_session_id(&cookies), None);
}

#[test]
fn sticky_session_expiration() {
    let sticky = StickySession::new("sid".to_string(), 10);

    // 刚创建的会话不应过期
    assert!(!sticky.is_expired(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    ));

    // 100 秒前创建的会话应已过期（TTL = 10）
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(sticky.is_expired(now.saturating_sub(100)));
}

#[test]
fn sticky_session_generate_unique_ids() {
    let sticky = StickySession::new("sid".to_string(), 3600);
    let id1 = sticky.generate_session_id("192.168.1.1");
    let id2 = sticky.generate_session_id("192.168.1.1");

    // 包含时间戳和随机数，两次生成应不同
    assert_ne!(id1, id2);
    assert_eq!(id1.len(), 16);
    assert_eq!(id2.len(), 16);
}

#[test]
fn canary_router_sticky_overrides_policy() {
    // 即使策略指向特定 upstream，会话黏性应优先
    let policy = Arc::new(HeaderPolicy {
        header_name: "x-canary".to_string(),
        header_value: "true".to_string(),
        target_index: 1,
    });
    let sticky = StickySession::new("sid".to_string(), 3600);
    let router = CanaryRouter::new(policy, Some(sticky.clone()));
    let route = make_route();
    let upstreams = make_upstreams(3);

    // 带会话 cookie 时，应忽略 header 策略
    let headers = HashMap::new();
    let mut cookies = HashMap::new();
    cookies.insert("sid".to_string(), "some-session".to_string());

    let selected = router
        .select_upstream(&route, &headers, &cookies, "127.0.0.1", &upstreams)
        .unwrap();

    // 会话黏性基于 hash 选择，应稳定
    let expected_idx = sticky.select_upstream("some-session", 3);
    assert_eq!(selected.id, format!("upstream-{expected_idx}"));
}
