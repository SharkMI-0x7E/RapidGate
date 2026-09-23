//! 限流中间件（spec §5.4）
//!
//! 从请求提取 key → 调用 RateLimiter::check → 超限返回 429。

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::core::config::route::RouteConfig;
use crate::core::ratelimit::sliding_window::SlidingWindow;
use crate::core::ratelimit::token_bucket::TokenBucket;
use crate::core::ratelimit::{Decision, RateLimiter};
use crate::service::state::AppState;

/// 限流中间件（预留脚手架，真实限流由 handler 层按 route 配置调用 [`check_route_rate_limit`]）
pub async fn ratelimit_middleware(req: Request, next: axum::middleware::Next) -> Response {
    next.run(req).await
}

/// 对单个请求执行路由级限流。
///
/// limiter 以 `route 名 + algorithm` 为 key 缓存在 `state.limiters`（Moka），
/// 无则按配置 `algorithm`（token_bucket / sliding_window）创建并缓存。
/// 超限返回 429 `{"error":{"code":"rate_limited",...}}`。
// 返回的 Err 是 HTTP Response 本身（必须直接返回给客户端），体积超 lint 阈值：
// 与 auth::check_route_auth 同理，用 Box 会迫使调用点多一层解包而无实际收益，故允许该 lint。
#[allow(clippy::result_large_err)]
pub async fn check_route_rate_limit(
    state: &Arc<AppState>,
    route: &RouteConfig,
    client_key: &str,
) -> Result<(), Response> {
    let cfg = route
        .rate_limit
        .clone()
        .unwrap_or_else(|| state.default_rate_limit.clone());
    let limiter_key = format!("{}:{}", route.name, cfg.algorithm);

    let limiter = match state.limiters.get(&limiter_key).await {
        Some(l) => l,
        None => {
            let created: Arc<dyn RateLimiter> = match cfg.algorithm.as_str() {
                "sliding_window" => Arc::new(SlidingWindow::new(cfg.rps, cfg.burst)),
                _ => Arc::new(TokenBucket::new(cfg.rps, cfg.burst)),
            };
            state
                .limiters
                .insert(limiter_key.clone(), created.clone())
                .await;
            created
        }
    };

    match limiter.check(&client_key.to_string()).await.map_err(|e| {
        tracing::error!(
            route = %route.name,
            algorithm = %cfg.algorithm,
            error = %e,
            "rate limiter check failed"
        );
        e
    }) {
        Ok(Decision::Allow) => Ok(()),
        Ok(Decision::Deny) => {
            tracing::warn!(route = %route.name, key = %client_key, "rate limit exceeded");
            state.metrics.record_ratelimit(client_key);
            Err(rate_limited_response())
        }
        // limiter 自身异常：保守放行，避免单点导致整体不可用
        Err(_) => Ok(()),
    }
}

/// 构造 429 限流响应
pub fn rate_limited_response() -> Response {
    let body = json!({
        "error": {
            "code": "rate_limited",
            "message": "rate limit exceeded",
        }
    });
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// 从请求提取限流 key（优先 X-Forwarded-For，其次 fallback）。
///
/// 若 X-Forwarded-For 缺失则回退到 `unknown`——真实部署建议在前置 LB 上填充该头。
/// 集成测试需显式传 X-Forwarded-For 以区分不同客户端并触发 429。
pub fn extract_limit_key(req: &Request) -> String {
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(ip) = forwarded.to_str() {
            let first = ip.split(',').next().map(str::trim).unwrap_or("");
            if !first.is_empty() {
                return first.to_string();
            }
        }
    }
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    #[test]
    fn extract_key_from_forwarded_for() {
        let req = HttpRequest::builder()
            .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
            .body(Body::empty())
            .unwrap();
        assert_eq!(extract_limit_key(&req), "1.2.3.4");
    }

    #[test]
    fn extract_key_fallback() {
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        assert_eq!(extract_limit_key(&req), "unknown");
    }

    // ---- check_route_rate_limit ----

    use crate::core::config::route::{AuthConfig, AuthKind, RouteConfig};
    use crate::core::routing::{RouteTable, Router};
    use std::path::PathBuf;

    fn test_route() -> RouteConfig {
        RouteConfig {
            name: "rl-route".into(),
            match_rule: crate::core::config::route::MatchRule {
                method: "POST".into(),
                path: "/v1/x".into(),
                host: None,
                headers: vec![],
                query: vec![],
                cookies: vec![],
            },
            upstream: None,
            auth: AuthConfig {
                kind: AuthKind::None,
                keys: vec![],
            },
            rate_limit: None,
            canary: None,
        }
    }

    fn test_state() -> Arc<AppState> {
        let (state, _rx) = AppState::new(
            Router::new(RouteTable::empty()),
            vec![],
            crate::core::config::route::RateLimitConfig {
                algorithm: "token_bucket".into(),
                rps: 1,
                burst: 1,
            },
            PathBuf::from("./config"),
            1024,
            1000,
            crate::core::config::gateway::SsrfConfig::default(),
        );
        Arc::new(state)
    }

    #[tokio::test]
    async fn first_request_allowed_then_rate_limited() {
        let state = test_state();
        let route = test_route();
        // token_bucket rps=1 burst=1：同一 client key 第一次放行，第二次 429
        assert!(check_route_rate_limit(&state, &route, "client-1")
            .await
            .is_ok());
        let denied = check_route_rate_limit(&state, &route, "client-1")
            .await
            .unwrap_err();
        assert_eq!(denied.status(), StatusCode::TOO_MANY_REQUESTS);
        // 不同 client 不受影响
        assert!(check_route_rate_limit(&state, &route, "client-2")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn rate_limited_body_format() {
        let state = test_state();
        let route = test_route();
        let _ = check_route_rate_limit(&state, &route, "c1").await;
        let res = check_route_rate_limit(&state, &route, "c1")
            .await
            .unwrap_err();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        use axum::body::to_bytes;
        let body = to_bytes(res.into_body(), 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "rate_limited");
    }
}
