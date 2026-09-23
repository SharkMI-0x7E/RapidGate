//! 端到端流程测试（spec checklist 覆盖缺口）
//!
//! 全部通过 mock 上游(127.0.0.1)+ spawn_app 真实 HTTP 调用，验证：
//! - 路由级鉴权(Bearer)：无/错/对 三种凭据
//! - 路由级限流(token_bucket rps=1/burst=1)：第一次放行、第二次 429
//! - embeddings 独立端点路径（返回与 chat 不同的内容）
//! - SSE 流式透传（标准 data: 行、无双重包装、含 [DONE]）
//! - admin API 鉴权（RGD_ADMIN_TOKEN 401/200）
//! - /healthz 与 /metrics
//! - v1 + v2 配置并存反序列化（canary / cookies / max_retries）

#[path = "common/mod.rs"]
mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde_json::{json, Value};

use rapidgate::core::config::gateway::SsrfConfig;
use rapidgate::core::config::route::{
    AuthConfig, AuthKind, MatchRule, RateLimitConfig, RouteConfig, UpstreamRef,
};
use rapidgate::core::config::upstream::{LoadBalancer, UpstreamConfig};
use rapidgate::core::routing::{RouteTable, Router};
use rapidgate::service::state::AppState;
use tokio::net::TcpListener;

// ==================== mock 上游 ====================

/// 起一个综合 mock 上游：同时提供 /v1/chat/completions（非流式 + 流式）与 /v1/embeddings
async fn spawn_mock_upstream() -> SocketAddr {
    use axum::routing::post;
    use axum::Router;

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_handler))
        .route("/v1/embeddings", post(embeddings_handler));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// chat 端点：请求体带 stream=true 时返回 SSE 流，否则返回标准 JSON
async fn chat_handler(req: axum::extract::Request) -> axum::response::Response {
    let bytes = axum::body::to_bytes(req.into_body(), 1 << 20)
        .await
        .unwrap_or_default();
    let is_stream = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);

    if is_stream {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"index\":0}]}\n\n",
            "data: [DONE]\n\n",
        );
        (
            StatusCode::OK,
            [("content-type", "text/event-stream")],
            body,
        )
            .into_response()
    } else {
        (
            StatusCode::OK,
            axum::Json(json!({
                "id": "mock-chat",
                "object": "chat.completion",
                "model": "gpt-4",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi from mock"}}]
            })),
        )
            .into_response()
    }
}

/// embeddings 端点：返回 OpenAI 兼容且内容与 chat 明显不同的响应
async fn embeddings_handler() -> axum::response::Response {
    (
        StatusCode::OK,
        axum::Json(json!({
            "object": "list",
            "data": [{"embedding": [0.125, 0.5, -0.25], "index": 0}],
            "model": "text-embedding-3-small"
        })),
    )
        .into_response()
}

// ==================== state 构造 helper ====================

fn make_state(
    routes: Vec<RouteConfig>,
    upstreams: Vec<UpstreamConfig>,
    rps: u32,
    burst: u32,
) -> AppState {
    let table = RouteTable::new(routes).expect("route table");
    let (state, _rx) = AppState::new(
        Router::new(table),
        upstreams,
        RateLimitConfig {
            algorithm: "token_bucket".into(),
            rps,
            burst,
        },
        PathBuf::from("./config"),
        1 << 20,
        5000,
        // 测试放行回环上游，避免 SSRF 拦截（SSRF 本身由 tests/ssrf.rs 覆盖）
        SsrfConfig {
            enabled: false,
            allow_list: vec![], // enabled=false 时 allow_list 不参与判断
        },
    );
    state
}

/// 构造一个上游 + 指向它的 chat/embeddings 路由
fn chat_embeddings_state(mock: SocketAddr, rps: u32, burst: u32) -> AppState {
    let upstream = UpstreamConfig {
        id: "mock".into(),
        provider: "openai".into(),
        base_url: format!("http://{mock}"),
        api_key: "sk-0123456789abcdef".into(),
        load_balancer: LoadBalancer::RoundRobin,
        models: vec![],
        timeout_ms: Some(5000),
        pool: None,
        max_retries: None,
    };

    let chat_route = RouteConfig {
        name: "chat-route".into(),
        match_rule: MatchRule {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            host: None,
            headers: vec![],
            query: vec![],
            cookies: vec![],
        },
        upstream: Some(UpstreamRef { id: "mock".into() }),
        auth: AuthConfig {
            kind: AuthKind::None,
            keys: vec![],
        },
        rate_limit: None,
        canary: None,
    };

    let emb_route = RouteConfig {
        name: "emb-route".into(),
        match_rule: MatchRule {
            method: "POST".into(),
            path: "/v1/embeddings".into(),
            host: None,
            headers: vec![],
            query: vec![],
            cookies: vec![],
        },
        upstream: Some(UpstreamRef { id: "mock".into() }),
        auth: AuthConfig {
            kind: AuthKind::None,
            keys: vec![],
        },
        rate_limit: None,
        canary: None,
    };

    make_state(vec![chat_route, emb_route], vec![upstream], rps, burst)
}

// ==================== 1. 鉴权 ====================

fn auth_state(mock: SocketAddr) -> AppState {
    let upstream = UpstreamConfig {
        id: "mock".into(),
        provider: "openai".into(),
        base_url: format!("http://{mock}"),
        api_key: "sk-0123456789abcdef".into(),
        load_balancer: LoadBalancer::RoundRobin,
        models: vec![],
        timeout_ms: Some(5000),
        pool: None,
        max_retries: None,
    };
    let route = RouteConfig {
        name: "auth-route".into(),
        match_rule: MatchRule {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            host: None,
            headers: vec![],
            query: vec![],
            cookies: vec![],
        },
        upstream: Some(UpstreamRef { id: "mock".into() }),
        auth: AuthConfig {
            kind: AuthKind::Bearer,
            keys: vec!["secret-token".into()],
        },
        rate_limit: None,
        canary: None,
    };
    make_state(vec![route], vec![upstream], 100, 200)
}

async fn post_chat(
    addr: SocketAddr,
    auth: Option<&str>,
    forward_client: Option<&str>,
) -> reqwest::Response {
    let mut req = reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false
        }));
    if let Some(a) = auth {
        req = req.header("Authorization", format!("Bearer {a}"));
    }
    if let Some(ip) = forward_client {
        req = req.header("X-Forwarded-For", ip);
    }
    req.send().await.expect("send request")
}

#[tokio::test]
async fn auth_missing_header_rejected() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(auth_state(mock)).await;
    let resp = post_chat(app.addr, None, None).await;
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "缺少 Authorization 应 401"
    );
}

#[tokio::test]
async fn auth_wrong_key_rejected() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(auth_state(mock)).await;
    let resp = post_chat(app.addr, Some("wrong-key"), None).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "错误 key 应 401");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn auth_valid_key_proxies_to_upstream() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(auth_state(mock)).await;
    let resp = post_chat(app.addr, Some("secret-token"), None).await;
    assert_eq!(resp.status(), StatusCode::OK, "正确 key 应转发成功");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["choices"][0]["message"]["content"], "hi from mock");
}

// ==================== 2. 限流 ====================

#[tokio::test]
async fn rate_limit_first_allowed_second_limited() {
    let mock = spawn_mock_upstream().await;
    // 路由级限流 rps=1 burst=1；默认限流配高些，避免干扰
    let state = chat_embeddings_state(mock, 1, 1);
    // 给限流测试单独起一个 state，避免共享默认 limiter 干扰其他测试
    let app = common::spawn_app(state).await;

    // 同一 client key（X-Forwarded-For），第一次应放行
    let first = post_chat(app.addr, None, Some("1.2.3.4")).await;
    assert_ne!(
        first.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "第一次不应 429"
    );

    // 第二次应 429
    let second = post_chat(app.addr, None, Some("1.2.3.4")).await;
    assert_eq!(
        second.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "第二次应被限流"
    );
    let body: Value = second.json().await.expect("json");
    assert_eq!(body["error"]["code"], "rate_limited");
}

// ==================== 3. embeddings 路径 ====================

#[tokio::test]
async fn embeddings_returns_distinct_mock_payload() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(chat_embeddings_state(mock, 100, 200)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/embeddings", app.addr))
        .json(&json!({
            "model": "text-embedding-3-small",
            "input": ["hello"],
            "stream": false
        }))
        .send()
        .await
        .expect("send embeddings request");

    assert_eq!(resp.status(), StatusCode::OK, "embeddings 应转发成功");
    let body: Value = resp.json().await.expect("json");
    // 必须是 embeddings 的独特响应（object=list），而不是 chat 响应（object=chat.completion）
    assert_eq!(body["object"], "list", "返回体应来自 embeddings 而非 chat");
    let embedding = &body["data"][0]["embedding"];
    assert!(embedding.is_array(), "embedding 应是数组");
    assert_eq!(embedding[0].as_f64(), Some(0.125));
}

// ==================== 4. SSE 流式 ====================

#[tokio::test]
async fn stream_sse_tunneled_without_double_wrapping() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(chat_embeddings_state(mock, 100, 200)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", app.addr))
        .json(&json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true
        }))
        .send()
        .await
        .expect("send stream request");

    assert_eq!(resp.status(), StatusCode::OK, "流式请求应 200");
    let ct = resp
        .headers()
        .get("content-type")
        .expect("content-type")
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.contains("text/event-stream"),
        "Content-Type 应为 text/event-stream，实际: {ct}"
    );

    let text = resp.text().await.expect("read sse body");
    assert!(text.contains("data: [DONE]"), "应包含结束标记 data: [DONE]");

    // 不允许双重包装：data: 后不能再紧跟 data:
    assert!(
        !text.contains("data: data:"),
        "SSE 事件被双重包装（data: data:），body: {text}"
    );

    // 每一行 data 事件都应以 data: 开头，且内容为合法 JSON（或 [DONE]）
    for event in text.split("\n\n") {
        let event = event.trim();
        if event.is_empty() {
            continue;
        }
        assert!(
            event.starts_with("data: "),
            "SSE 行应以 'data: ' 开头，实际: {event:?}"
        );
        let payload = event.trim_start_matches("data: ").trim();
        if payload == "[DONE]" {
            continue;
        }
        serde_json::from_str::<Value>(payload)
            .unwrap_or_else(|e| panic!("SSE payload 应为合法 JSON: {payload} ({e})"));
    }
}

// ==================== 5. admin API 鉴权 ====================

#[tokio::test]
async fn admin_requires_token_and_lists_routes() {
    std::env::set_var("RGD_ADMIN_TOKEN", "admin-secret-token");

    let state = Arc::new(common::empty_state());
    let router = rapidgate::service::admin::routes::admin_routes(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind admin");
    let addr = listener.local_addr().expect("admin addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    // 无凭据 -> 401
    let no_auth = reqwest::Client::new()
        .get(format!("http://{addr}/admin/routes"))
        .send()
        .await
        .expect("admin no auth");
    assert_eq!(
        no_auth.status(),
        StatusCode::UNAUTHORIZED,
        "无 admin key 应 401"
    );

    // 错误凭据 -> 401
    let wrong = reqwest::Client::new()
        .get(format!("http://{addr}/admin/routes"))
        .header("Authorization", "Bearer nope")
        .send()
        .await
        .expect("admin wrong key");
    assert_eq!(
        wrong.status(),
        StatusCode::UNAUTHORIZED,
        "错误 admin key 应 401"
    );

    // 正确凭据 -> 200 + routes 字段
    let ok = reqwest::Client::new()
        .get(format!("http://{addr}/admin/routes"))
        .header("Authorization", "Bearer admin-secret-token")
        .send()
        .await
        .expect("admin ok");
    assert_eq!(ok.status(), StatusCode::OK, "正确 admin key 应 200");
    let body: Value = ok.json().await.expect("admin json");
    assert!(
        body["routes"].is_array(),
        "/admin/routes 应返回 routes 数组"
    );

    std::env::remove_var("RGD_ADMIN_TOKEN");
}

// ==================== 6. /metrics 与 /healthz ====================

#[tokio::test]
async fn healthz_returns_200() {
    let app = common::spawn_app(common::empty_state()).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/healthz", app.addr))
        .send()
        .await
        .expect("healthz");
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_exposes_prometheus_text() {
    let app = common::spawn_app(common::empty_state()).await;
    let resp = reqwest::Client::new()
        .get(format!("http://{}/metrics", app.addr))
        .send()
        .await
        .expect("metrics");
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        ct.contains("text/plain"),
        "metrics Content-Type 应为 text/plain，实际: {ct}"
    );

    // 注：当前生产路径尚未在 handler 中调用 Metrics::record_request，默认 body 可能为空，
    // 这是测试暴露的已知缺口（见报告）；此处只校验端点可用 + 协议正确，不强断言 body 内容。
    let _text = resp.text().await.expect("metrics body");
}

// ==================== 7. v1 + v2 配置并存 ====================

#[derive(serde::Deserialize)]
struct RoutesFile {
    routes: Vec<RouteConfig>,
}

#[derive(serde::Deserialize)]
struct UpstreamsFile {
    upstreams: Vec<UpstreamConfig>,
}

const V1_YAML: &str = include_str!("../config/routes/v1.yaml");
const V2_YAML: &str = include_str!("../config/routes/v2.yaml");

#[test]
fn v1_and_v2_routes_coexist_and_deserialize() {
    let v1: RoutesFile = serde_yaml::from_str(V1_YAML).expect("v1.yaml must parse");
    let v2: RoutesFile = serde_yaml::from_str(V2_YAML).expect("v2.yaml must parse");

    // v1 提供 3 条，v2 提供 7 条
    assert_eq!(v1.routes.len(), 3, "v1.yaml 应有 3 条路由");
    assert_eq!(v2.routes.len(), 7, "v2.yaml 应有 7 条路由");

    let mut all = Vec::new();
    all.extend(v1.routes);
    all.extend(v2.routes);

    // 合并后不重复、能编译成路由表（v2 覆盖同名 /v1/... 路径不冲突）
    let table = RouteTable::new(all).expect("v1+v2 合并路由表应可编译");
    assert_eq!(table.len(), 10);

    // v2 的 canary 段与 cookies map 被正确解析
    let by_name = |name: &str| {
        table
            .routes
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("missing route {name}"))
    };

    let canary = by_name("v2-chat-completions");
    let canary_cfg = canary.canary.as_ref().expect("应为 canary 路由");
    assert_eq!(canary_cfg.strategy, "weight");
    let total: u32 = canary_cfg.targets.iter().map(|t| t.weight).sum();
    assert_eq!(total, 100);
    assert!(canary.upstream.is_none(), "canary 路由不应有直连 upstream");

    let cookie_route = by_name("v2-chat-cookie-routing");
    assert_eq!(cookie_route.match_rule.cookies.len(), 1);
    assert_eq!(cookie_route.match_rule.cookies[0].name, "provider_group");
    assert_eq!(cookie_route.match_rule.cookies[0].value, "beta");
    assert!(cookie_route.canary.is_some());
}

#[test]
fn v2_upstreams_preserve_max_retries_and_multiple_providers() {
    let v2: UpstreamsFile = serde_yaml::from_str(V2_YAML).expect("v2.yaml upstreams must parse");
    assert_eq!(
        v2.upstreams.len(),
        4,
        "v2 应有 4 个上游（openai/anthropic/gemini/local）"
    );

    let openai = v2
        .upstreams
        .iter()
        .find(|u| u.id == "openai-v2")
        .expect("openai-v2 upstream");
    assert_eq!(openai.max_retries, Some(3), "openai-v2 max_retries 应为 3");

    let anthropic = v2
        .upstreams
        .iter()
        .find(|u| u.id == "anthropic-v2")
        .expect("anthropic-v2");
    assert_eq!(anthropic.provider, "anthropic");
}
