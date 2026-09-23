//! RapidGate 程序入口（spec §5.6）

use std::process::ExitCode;
use std::sync::Arc;

use rapidgate::core::routing::{RouteTable, Router};
use rapidgate::service::config_loader;
use rapidgate::service::state::AppState;
use rapidgate::service::{self};

#[tokio::main]
async fn main() -> ExitCode {
    // 1) 加载 .env（不强制存在）
    let _ = dotenvy::dotenv();

    // 2) 初始化 tracing
    service::telemetry::init();

    // 3) 解析配置路径
    let paths = match config_loader::resolve_paths() {
        Ok(p) => p,
        Err(e) => {
            tracing::error!(error = %e, "resolve_paths failed");
            return ExitCode::from(78);
        }
    };

    // 4) 加载配置
    let cfg = match config_loader::load().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "config load failed");
            return ExitCode::from(1);
        }
    };

    // 5) 编译路由表
    let table = match RouteTable::new(cfg.routes.clone()) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "route table compile failed");
            return ExitCode::from(78);
        }
    };
    let router = Router::new(table);

    // 6) 构造 AppState（同时拿到审计事件接收端）
    let (state, mut audit_rx) = AppState::new(
        router,
        cfg.gateway.upstreams.clone(),
        cfg.gateway.defaults.rate_limit.clone(),
        paths.config_dir,
        cfg.gateway.max_body_bytes,
        cfg.gateway.request_timeout_ms,
        cfg.gateway.ssrf.clone(),
    );
    let state = Arc::new(state);

    // 6.1) 审计事件消费者：异步消费并记录结构化日志
    tokio::spawn(async move {
        while let Some(ev) = audit_rx.recv().await {
            tracing::info!(
                route = %ev.route_id,
                api_key_hash = %ev.api_key_hash,
                status = ev.status,
                latency_ms = ev.latency_ms,
                prompt_tokens = ev.prompt_tokens,
                completion_tokens = ev.completion_tokens,
                "audit event"
            );
        }
    });

    // 7) 启动 Admin（/admin/*、/metrics）独立端口
    {
        let admin_listen = cfg.gateway.admin_listen.clone();
        let admin_router = service::admin::routes::admin_routes(state.clone());
        tokio::spawn(async move {
            match tokio::net::TcpListener::bind(&admin_listen).await {
                Ok(listener) => {
                    tracing::info!(admin_listen = %admin_listen, "admin server starting");
                    if let Err(e) = axum::serve(listener, admin_router).await {
                        tracing::error!(error = %e, "admin server error");
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, admin_listen = %admin_listen, "admin bind failed");
                }
            }
        });
    }

    // 8) 启动主 HTTP 服务 + graceful shutdown
    let listen = cfg.gateway.listen.clone();
    tracing::info!(listen = %listen, "rapidgate starting");

    let app = service::server::router(state);
    let listener = match tokio::net::TcpListener::bind(&listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "bind failed");
            return ExitCode::from(1);
        }
    };

    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown signal received, draining...");
    };

    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
    {
        tracing::error!(error = %e, "server error");
        return ExitCode::from(1);
    }

    tracing::info!("rapidgate stopped cleanly");
    ExitCode::SUCCESS
}
