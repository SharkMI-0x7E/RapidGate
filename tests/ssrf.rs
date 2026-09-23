//! SSRF 防护端到端测试（spec §8）
//!
//! 起本地 mock 上游（127.0.0.1），验证：
//! - `ssrf.enabled=true` 且 `allow_list` 包含回环 host → 转发成功；
//! - `ssrf.enabled=true` 且不豁免 → 回环地址被拒（400 bad_request）。

#[path = "common/mod.rs"]
mod common;

use std::net::SocketAddr;
use std::path::PathBuf;

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

/// 启动 mock 上游，监听 127.0.0.1:0，返回固定的 OpenAI 兼容响应
async fn spawn_mock_upstream() -> SocketAddr {
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        axum::routing::post(|| async {
            (
                StatusCode::OK,
                axum::Json(json!({
                    "id": "mock-id",
                    "object": "chat.completion",
                    "model": "gpt-4",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "hi from mock"},
                        "finish_reason": "stop"
                    }]
                })),
            )
                .into_response()
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

fn make_state(mock_addr: SocketAddr, ssrf_enabled: bool, allow_list: Vec<String>) -> AppState {
    let upstream = UpstreamConfig {
        id: "mock".into(),
        provider: "openai".into(),
        base_url: format!("http://{mock_addr}"),
        api_key: "sk-0123456789abcdef".into(),
        load_balancer: LoadBalancer::RoundRobin,
        models: vec![],
        timeout_ms: Some(5000),
        pool: None,
        max_retries: None,
    };
    let route = RouteConfig {
        name: "mock-route".into(),
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
    let table = RouteTable::new(vec![route]).expect("route table");
    let (state, _rx) = AppState::new(
        Router::new(table),
        vec![upstream],
        RateLimitConfig {
            algorithm: "token_bucket".into(),
            rps: 10,
            burst: 20,
        },
        PathBuf::from("./config"),
        1024,
        5000,
        SsrfConfig {
            enabled: ssrf_enabled,
            allow_list,
        },
    );
    state
}

async fn send_chat(addr: SocketAddr) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{addr}/v1/chat/completions"))
        .header("Content-Type", "application/json")
        .json(&json!({
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": false
        }))
        .send()
        .await
        .expect("send request")
}

#[tokio::test]
async fn ssrf_allow_list_allows_loopback_upstream() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(make_state(mock, true, vec!["127.0.0.1".into()])).await;

    let resp = send_chat(app.addr).await;
    assert_eq!(resp.status(), 200, "allow_list 豁免回环 host 应转发成功");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["choices"][0]["message"]["content"], "hi from mock");
}

#[tokio::test]
async fn ssrf_enabled_blocks_loopback_upstream() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(make_state(mock, true, vec![])).await;

    let resp = send_chat(app.addr).await;
    assert_eq!(resp.status(), 400, "回环地址未豁免应被 SSRF 拦截");
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn ssrf_disabled_allows_loopback_upstream() {
    let mock = spawn_mock_upstream().await;
    let app = common::spawn_app(make_state(mock, false, vec![])).await;

    let resp = send_chat(app.addr).await;
    assert_eq!(resp.status(), 200, "ssrf 关闭时应纯放行");
}
