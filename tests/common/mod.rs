//! 集成测试公共工具

use std::net::SocketAddr;
use std::sync::Arc;

use rapidgate::core::config::route::RateLimitConfig;
use rapidgate::core::routing::{RouteTable, Router};
use rapidgate::service::config_loader::LoadedConfig;
use rapidgate::service::state::AppState;
use tokio::net::TcpListener;

pub struct TestApp {
    pub addr: SocketAddr,
    #[allow(dead_code)]
    pub state: Arc<AppState>,
    pub _handle: tokio::task::JoinHandle<()>,
}

/// 启动测试 app，绑定到 127.0.0.1:0
pub async fn spawn_app(state: AppState) -> TestApp {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let state = Arc::new(state);
    let app = rapidgate::service::server::router(state.clone());
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    TestApp {
        addr,
        state,
        _handle: handle,
    }
}

/// 构造空配置 AppState（用于无上游路由测试）
// 各测试文件独立编译，此 helper 只被部分测试引用；允许 unused 以免 ssrf.rs 单独编译时报 lint
#[allow(dead_code)]
pub fn empty_state() -> AppState {
    AppState::new(
        Router::new(RouteTable::empty()),
        vec![],
        RateLimitConfig {
            algorithm: "token_bucket".into(),
            rps: 1,
            burst: 1,
        },
        std::path::PathBuf::from("./config"),
        1024,
        1000,
        rapidgate::core::config::gateway::SsrfConfig::default(),
    )
    .0
}

/// 构造最小可用的 LoadedConfig（仅 healthz 路径可达）
/// 阶段二/三的认证/限流集成测试会用到，先保留脚手架
#[allow(dead_code)]
pub fn minimal_loaded() -> Arc<LoadedConfig> {
    use rapidgate::core::config::gateway::{
        BreakerDefaults, Defaults, GatewayConfig, LoggingConfig, SsrfConfig, UpstreamAllowlist,
    };
    use rapidgate::core::config::route::RateLimitConfig;
    Arc::new(LoadedConfig {
        gateway: GatewayConfig {
            listen: "127.0.0.1:0".into(),
            admin_listen: "127.0.0.1:9090".into(),
            request_timeout_ms: 1000,
            max_body_bytes: 1024,
            shutdown_timeout_ms: 1000,
            logging: LoggingConfig {
                level: "info".into(),
                format: "pretty".into(),
            },
            upstream_allowlist: UpstreamAllowlist {
                hosts: vec!["127.0.0.1".into()],
            },
            defaults: Defaults {
                rate_limit: RateLimitConfig {
                    algorithm: "token_bucket".into(),
                    rps: 1,
                    burst: 1,
                },
                breaker: BreakerDefaults {
                    failure_threshold: 5,
                    open_duration_ms: 1000,
                },
            },
            upstreams: vec![],
            ssrf: SsrfConfig::default(),
        },
        routes: vec![],
    })
}
