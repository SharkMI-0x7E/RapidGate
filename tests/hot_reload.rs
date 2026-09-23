//! 配置热重载集成测试（spec §5.5）
//!
//! 验证 `reload()` 在配置加载失败（config 目录缺失/占位符缺失）时
//! **保留旧配置**，即 ArcSwap 整体替换只在成功时发生。

use std::path::PathBuf;
use std::sync::Arc;

use rapidgate::core::config::gateway::SsrfConfig;
use rapidgate::core::config::route::{
    AuthConfig, AuthKind, MatchRule, RateLimitConfig, RouteConfig, UpstreamRef,
};
use rapidgate::core::routing::{RouteTable, Router};
use rapidgate::service::hot_reload;
use rapidgate::service::state::AppState;

fn state_with_one_route() -> Arc<AppState> {
    let route = RouteConfig {
        name: "keep-me".into(),
        match_rule: MatchRule {
            method: "GET".into(),
            path: "/v1/models".into(),
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
        vec![],
        RateLimitConfig {
            algorithm: "token_bucket".into(),
            rps: 1,
            burst: 1,
        },
        PathBuf::from("./config"),
        1024,
        1000,
        SsrfConfig::default(),
    );
    Arc::new(state)
}

/// 指向一个不存在 default.yaml 的临时目录，迫使 reload 加载失败 → 应保留旧配置
#[tokio::test]
async fn reload_keeps_old_config_when_load_fails() {
    let dir = std::env::temp_dir().join(format!("rg_hot_reload_{}", std::process::id()));
    // 故意不创建 default.yaml，仅保证目录存在可被 resolve_paths 走到
    std::fs::create_dir_all(&dir).expect("create temp dir");

    std::env::set_var("RGD_CONFIG_DIR", &dir);
    let state = state_with_one_route();
    let before = state.route_table.snapshot().len();
    assert_eq!(before, 1, "初始应有 1 条路由");

    hot_reload::reload(&state).await;

    // 加载失败（临时目录无 default.yaml）→ 旧配置保留
    let after = state.route_table.snapshot().len();
    assert_eq!(
        after, before,
        "配置加载失败后旧路由表应被保留（RGD_CONFIG_DIR={dir:?}）"
    );

    std::env::remove_var("RGD_CONFIG_DIR");
    std::fs::remove_dir_all(&dir).ok();
}

/// 配置目录缺失时 resolve_paths 返回 Err（不 panic），reload 因此保留旧表
#[test]
fn resolve_paths_missing_dir_errors_gracefully() {
    let missing = std::env::temp_dir().join(format!("rg_missing_{}", std::process::id()));
    std::env::set_var("RGD_CONFIG_DIR", &missing);
    let err = rapidgate::service::config_loader::resolve_paths();
    std::env::remove_var("RGD_CONFIG_DIR");
    assert!(err.is_err(), "缺失 config 目录应返回 Err 而非 panic");
}
