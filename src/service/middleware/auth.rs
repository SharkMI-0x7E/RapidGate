//! 鉴权中间件（spec §5.4）
//!
//! 从请求提取 `Authorization` header → 判断类型 → 调用对应 Authenticator。
//! 校验失败返回 401 + JSON `unauthorized`，**不**区分"key 不存在" vs "key 错误"。

use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::core::config::route::{AuthKind, RouteConfig};

/// 鉴权中间件（预留脚手架，真实校验由 handler 层按 route.auth 调用 [`check_route_auth`]）
pub async fn auth_middleware(req: Request, next: axum::middleware::Next) -> Response {
    next.run(req).await
}

/// 按路由鉴权配置校验请求凭据。
///
/// - `AuthKind::None` → 直接放行
/// - `AuthKind::Bearer` → 校验 `Authorization: Bearer <key>` 且 key 在 `keys` 内
/// - `AuthKind::ApiKey` → 校验 `Authorization: ApiKey <key>` 或裸 key，且 key 在 `keys` 内
/// - `keys` 为空且 kind!=None → 视为配置错误（500，不裸放行）
///
/// 校验失败返回 401（body 统一为 `{"error":{"code":"unauthorized",...}}`），
/// 不区分"key 不存在" vs "key 错误"，避免枚举泄露。
// 返回的 Err 是 HTTP Response 本身（必须直接返回给客户端），体积超 lint 阈值：
// 用 Box 会迫使所有调用点多一层解包而不带来实际收益，故允许该 lint。
#[allow(clippy::result_large_err)]
pub fn check_route_auth(route: &RouteConfig, req: &Request) -> Result<(), Response> {
    match route.auth.kind {
        AuthKind::None => Ok(()),
        AuthKind::Bearer => {
            if route.auth.keys.is_empty() {
                return Err(auth_config_error(route));
            }
            let provided = match extract_credential(req) {
                Some((AuthType::Bearer, key)) => key,
                _ => return Err(unauthorized_response()),
            };
            if route
                .auth
                .keys
                .iter()
                .any(|expected| const_eq(&provided, expected))
            {
                Ok(())
            } else {
                Err(unauthorized_response())
            }
        }
        AuthKind::ApiKey => {
            if route.auth.keys.is_empty() {
                return Err(auth_config_error(route));
            }
            let provided = match extract_credential(req) {
                Some((_, key)) => key,
                None => return Err(unauthorized_response()),
            };
            if route
                .auth
                .keys
                .iter()
                .any(|expected| const_eq(&provided, expected))
            {
                Ok(())
            } else {
                Err(unauthorized_response())
            }
        }
    }
}

/// 常量时间字符串比较，避免时序侧信道
fn const_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// 配置错误（keys 为空）：返回 500 + 日志告警，禁止裸放行
fn auth_config_error(route: &RouteConfig) -> Response {
    tracing::warn!(
        route = %route.name,
        kind = ?route.auth.kind,
        "route auth requires at least one key but none configured"
    );
    let body = json!({
        "error": {
            "code": "internal_error",
            "message": "auth misconfiguration: no keys configured",
        }
    });
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// 从 Authorization header 提取凭据
pub fn extract_credential(req: &Request) -> Option<(AuthType, String)> {
    let auth_header = req.headers().get("authorization")?;
    let value = auth_header.to_str().ok()?;

    if let Some(token) = value.strip_prefix("Bearer ") {
        Some((AuthType::Bearer, token.trim().to_string()))
    } else if let Some(key) = value.strip_prefix("ApiKey ") {
        Some((AuthType::ApiKey, key.trim().to_string()))
    } else {
        // 无 prefix 时当作 API Key
        Some((AuthType::ApiKey, value.trim().to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthType {
    Bearer,
    ApiKey,
}

/// 构造 401 未授权响应
pub fn unauthorized_response() -> Response {
    let body = json!({
        "error": {
            "code": "unauthorized",
            "message": "invalid or missing credentials",
        }
    });
    (
        StatusCode::UNAUTHORIZED,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;

    #[test]
    fn extract_bearer() {
        let req = HttpRequest::builder()
            .header("authorization", "Bearer my-token")
            .body(Body::empty())
            .unwrap();
        let (auth_type, cred) = extract_credential(&req).unwrap();
        assert_eq!(auth_type, AuthType::Bearer);
        assert_eq!(cred, "my-token");
    }

    #[test]
    fn extract_api_key() {
        let req = HttpRequest::builder()
            .header("authorization", "ApiKey sk-test-key")
            .body(Body::empty())
            .unwrap();
        let (auth_type, cred) = extract_credential(&req).unwrap();
        assert_eq!(auth_type, AuthType::ApiKey);
        assert_eq!(cred, "sk-test-key");
    }

    #[test]
    fn missing_header_returns_none() {
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        assert!(extract_credential(&req).is_none());
    }

    // ---- check_route_auth ----

    use crate::core::config::route::{AuthConfig, AuthKind, RouteConfig};

    /// 构造鉴权路由：给定 kind 与 keys
    fn auth_route(kind: AuthKind, keys: Vec<&str>) -> RouteConfig {
        RouteConfig {
            name: "test-route".into(),
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
                kind,
                keys: keys.into_iter().map(String::from).collect(),
            },
            rate_limit: None,
            canary: None,
        }
    }

    fn with_header(key: &str, value: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .header(key, value)
            .body(Body::empty())
            .unwrap()
    }

    fn status_of(res: &Response) -> StatusCode {
        res.status()
    }

    #[test]
    fn bearer_accepts_valid_key() {
        let route = auth_route(AuthKind::Bearer, vec!["secret-token"]);
        let req = with_header("authorization", "Bearer secret-token");
        assert!(check_route_auth(&route, &req).is_ok());
    }

    #[test]
    fn bearer_rejects_wrong_key() {
        let route = auth_route(AuthKind::Bearer, vec!["secret-token"]);
        let req = with_header("authorization", "Bearer wrong-key");
        assert_eq!(
            status_of(&check_route_auth(&route, &req).unwrap_err()),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn bearer_rejects_missing_header() {
        let route = auth_route(AuthKind::Bearer, vec!["secret-token"]);
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        assert_eq!(
            status_of(&check_route_auth(&route, &req).unwrap_err()),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn unauthorized_body_format() {
        let route = auth_route(AuthKind::Bearer, vec!["secret-token"]);
        let req = with_header("authorization", "Bearer nope");
        let res = check_route_auth(&route, &req).unwrap_err();
        assert_eq!(status_of(&res), StatusCode::UNAUTHORIZED);
        use axum::body::to_bytes;
        let body = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(to_bytes(res.into_body(), 1024))
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "unauthorized");
        assert_eq!(v["error"]["message"], "invalid or missing credentials");
    }

    #[test]
    fn apikey_accepts_matching_key() {
        let route = auth_route(AuthKind::ApiKey, vec!["sk-abc"]);
        let req = with_header("authorization", "sk-abc"); // 裸 key
        assert!(check_route_auth(&route, &req).is_ok());
        let req2 = with_header("authorization", "ApiKey sk-abc");
        assert!(check_route_auth(&route, &req2).is_ok());
    }

    #[test]
    fn apikey_rejects_unknown_key() {
        let route = auth_route(AuthKind::ApiKey, vec!["sk-abc"]);
        let req = with_header("authorization", "ApiKey sk-xyz");
        assert_eq!(
            status_of(&check_route_auth(&route, &req).unwrap_err()),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn none_passes_without_auth() {
        let route = auth_route(AuthKind::None, vec![]);
        let req = HttpRequest::builder().body(Body::empty()).unwrap();
        assert!(check_route_auth(&route, &req).is_ok());
    }

    #[test]
    fn empty_keys_returns_config_error() {
        let route = auth_route(AuthKind::Bearer, vec![]);
        let req = with_header("authorization", "Bearer anything");
        assert_eq!(
            status_of(&check_route_auth(&route, &req).unwrap_err()),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
