//! Admin API routes (spec §5.7)
//!
//! - `GET /admin/routes`    — current route table snapshot
//! - `GET /admin/upstreams` — upstream configs (api_key redacted)
//! - `GET /admin/limits`    — rate limiter status
//! - `GET /admin/config`    — gateway config dump (sensitive fields redacted)
//!
//! All admin operations emit structured audit logs via `tracing::info!`.

use std::sync::Arc;

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;

use crate::service::admin::auth::admin_auth;
use crate::service::state::AppState;

/// Build the admin router with all admin endpoints and auth middleware.
pub fn admin_routes(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/admin/routes", get(get_routes))
        .route("/admin/upstreams", get(get_upstreams))
        .route("/admin/limits", get(get_limits))
        .route("/admin/config", get(get_config))
        .layer(from_fn(admin_auth))
        .with_state(state)
}

/// GET /admin/routes — returns the current route table snapshot.
///
/// Each entry contains the route name, HTTP method, and path.
async fn get_routes(State(state): State<Arc<AppState>>) -> Response {
    let table = state.route_table.snapshot();
    let route_count = table.routes.len();

    let routes: Vec<serde_json::Value> = table
        .routes
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "method": r.match_rule.method,
                "path": r.match_rule.path,
                "upstream": r.upstream.as_ref().map(|u| u.id.as_str()),
            })
        })
        .collect();

    tracing::info!(
        endpoint = "routes",
        route_count = route_count,
        "admin API: list routes"
    );

    json_response(StatusCode::OK, json!({ "routes": routes }))
}

/// GET /admin/upstreams — returns upstream configurations.
///
/// The `api_key` field is always redacted.
async fn get_upstreams(State(state): State<Arc<AppState>>) -> Response {
    let upstream_count = state.upstream_configs.len();

    let upstreams: Vec<serde_json::Value> = state
        .upstream_configs
        .iter()
        .map(|u| {
            json!({
                "id": u.id,
                "provider": u.provider,
                "base_url": u.base_url,
                "api_key": "***REDACTED***",
                "load_balancer": format!("{:?}", u.load_balancer),
                "models": u.models,
            })
        })
        .collect();

    tracing::info!(
        endpoint = "upstreams",
        upstream_count = upstream_count,
        "admin API: list upstreams"
    );

    json_response(StatusCode::OK, json!({ "upstreams": upstreams }))
}

/// GET /admin/limits — returns rate limiter cache status.
///
/// Reports the number of active limiter entries in the Moka cache.
async fn get_limits(State(state): State<Arc<AppState>>) -> Response {
    let entry_count = state.limiters.entry_count();
    let default_limit = &state.default_rate_limit;

    tracing::info!(
        endpoint = "limits",
        active_limiters = entry_count,
        "admin API: list rate limiters"
    );

    json_response(
        StatusCode::OK,
        json!({
            "active_limiters": entry_count,
            "default": {
                "algorithm": default_limit.algorithm,
                "rps": default_limit.rps,
                "burst": default_limit.burst,
            }
        }),
    )
}

/// GET /admin/config — returns gateway configuration with sensitive fields redacted.
async fn get_config(State(state): State<Arc<AppState>>) -> Response {
    let config = json!({
        "config_dir": state.config_dir.display().to_string(),
        "max_body_bytes": state.max_body_bytes,
        "request_timeout_ms": state.request_timeout_ms,
        "upstream_count": state.upstream_configs.len(),
        "route_count": state.route_table.snapshot().len(),
        "default_rate_limit": {
            "algorithm": state.default_rate_limit.algorithm,
            "rps": state.default_rate_limit.rps,
            "burst": state.default_rate_limit.burst,
        },
    });

    tracing::info!(
        endpoint = "config",
        upstream_count = state.upstream_configs.len(),
        route_count = state.route_table.snapshot().len(),
        "admin API: dump config"
    );

    json_response(StatusCode::OK, config)
}

/// Helper: build a JSON response with the given status code and body.
fn json_response(status: StatusCode, body: serde_json::Value) -> Response {
    (
        status,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}
