//! service/server — axum::Router 组装 + graceful shutdown + body limit + banner（spec §5.6）

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use tower_http::trace::TraceLayer;

use crate::service::handler;
use crate::service::middleware::trace::request_id;
use crate::service::state::AppState;

/// 未知路径 → 结构化 404 JSON
async fn fallback_404(req: Request) -> Response {
    let path = req.uri().path().to_string();
    let body = json!({
        "error": {
            "code": "route_not_found",
            "message": path,
        }
    });
    (
        StatusCode::NOT_FOUND,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// 请求体大小限制中间件，阈值从 `AppState.max_body_bytes` 读取
async fn body_size_limit(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    // 从 Content-Length header 检查（超过阈值直接拒绝）
    if let Some(content_length) = req.headers().get("content-length") {
        if let Ok(len) = content_length.to_str().unwrap_or("0").parse::<usize>() {
            let max = state.max_body_bytes;
            if len > max {
                let body = json!({
                    "error": {
                        "code": "payload_too_large",
                        "message": format!("request body {len} bytes exceeds limit {max} bytes"),
                    }
                });
                return (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    [(CONTENT_TYPE, "application/json")],
                    body.to_string(),
                )
                    .into_response();
            }
        }
    }
    next.run(req).await
}

/// GET /metrics — 暴露 Prometheus 文本格式指标（放行，不鉴权不限流）
async fn metrics_handler(State(state): State<Arc<AppState>>) -> Response {
    let body = state.metrics.export();
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/plain; version=0.0.4")],
        body,
    )
        .into_response()
}

/// 组装顶层 axum::Router（主服务端口）
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(handler::chat_completions))
        .route("/v1/embeddings", post(handler::embeddings))
        .route("/v1/models", get(handler::list_models))
        .route("/healthz", get(handler::healthz))
        .route("/readyz", get(handler::readyz))
        .route("/metrics", get(metrics_handler))
        .fallback(fallback_404)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            body_size_limit,
        ))
        .layer(axum::middleware::from_fn(
            crate::service::middleware::recovery::recovery,
        ))
        .layer(axum::middleware::from_fn(request_id))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// 打印启动横幅
pub fn print_banner(listen: &str, config_dir: &str, route_count: usize, upstream_count: usize) {
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        listen = %listen,
        config_dir = %config_dir,
        routes = route_count,
        upstreams = upstream_count,
        "RapidGate starting"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use http::StatusCode;
    use tower::ServiceExt;

    fn empty_state() -> Arc<AppState> {
        use crate::core::config::gateway::SsrfConfig;
        use crate::core::config::route::RateLimitConfig;
        use crate::core::routing::Router;
        let (state, _rx) = AppState::new(
            Router::default(),
            vec![],
            RateLimitConfig {
                algorithm: "token_bucket".into(),
                rps: 1,
                burst: 1,
            },
            std::path::PathBuf::from("./config"),
            1024,
            1000,
            SsrfConfig::default(),
        );
        Arc::new(state)
    }

    #[tokio::test]
    async fn healthz_works() {
        let app = router(empty_state());
        let resp = app
            .oneshot(HttpRequest::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_exposed() {
        let state = empty_state();
        let app = router(state.clone());
        let resp = app
            .oneshot(HttpRequest::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // 端点可用即可；具体指标内容由 core::observability::metrics 单测覆盖
        use axum::body::to_bytes;
        let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("# HELP") || text.is_empty());
    }
}
