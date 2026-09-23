//! RouteConfig + RouteMatch（spec §4.2 / §6.2）

use std::collections::HashMap;

use serde::de::Deserializer;
use serde::Deserialize;

use crate::core::config::upstream::UpstreamId;

/// 单条路由配置
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    pub name: String,
    #[serde(rename = "match")]
    pub match_rule: MatchRule,
    /// 直连上游（直连与 canary 二选一）
    #[serde(default)]
    pub upstream: Option<UpstreamRef>,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub rate_limit: Option<RateLimitConfig>,
    /// 灰度路由配置（v2.yaml canary 段）
    #[serde(default)]
    pub canary: Option<CanaryConfig>,
}

/// 路由匹配条件（spec §4.3）
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchRule {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub host: Option<String>,
    /// Header 匹配；v2.yaml 用 map 语法（`x-provider: value`），也兼容列表形式
    #[serde(default, deserialize_with = "deserialize_header_map")]
    pub headers: Vec<HeaderMatch>,
    #[serde(default)]
    pub query: Vec<QueryMatch>,
    /// Cookie 匹配；v2.yaml 用 map 语法（`provider_group: value`），也兼容列表形式
    #[serde(default, deserialize_with = "deserialize_cookie_map")]
    pub cookies: Vec<CookieMatch>,
}

/// Header 匹配
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderMatch {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub regex: bool,
}

/// Cookie 匹配条件（v2.yaml 用 map 语法：cookies: {name: value}）
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CookieMatch {
    pub name: String,
    pub value: String,
}

/// 灰度路由配置（v2.yaml canary 段）
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanaryConfig {
    pub strategy: String,
    pub targets: Vec<CanaryTarget>,
}

/// 灰度目标：按权重分发到指定上游
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanaryTarget {
    pub upstream_id: UpstreamId,
    pub weight: u32,
}

// -------------------- 兼容 map / list 两种形式的反序列化 --------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum HeaderMatchRepr {
    Map(HashMap<String, String>),
    List(Vec<HeaderMatch>),
}

/// 兼容两种 header 语法：`headers: {k: v}`（map）或 `headers: [{name, value}]`（list）
fn deserialize_header_map<'de, D>(deserializer: D) -> Result<Vec<HeaderMatch>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match HeaderMatchRepr::deserialize(deserializer)? {
        HeaderMatchRepr::Map(map) => map
            .into_iter()
            .map(|(name, value)| HeaderMatch {
                name,
                value,
                regex: false,
            })
            .collect(),
        HeaderMatchRepr::List(list) => list,
    })
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CookieMatchRepr {
    Map(HashMap<String, String>),
    List(Vec<CookieMatch>),
}

/// 兼容两种 cookie 语法：`cookies: {k: v}`（map）或 `cookies: [{name, value}]`（list）
fn deserialize_cookie_map<'de, D>(deserializer: D) -> Result<Vec<CookieMatch>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match CookieMatchRepr::deserialize(deserializer)? {
        CookieMatchRepr::Map(map) => map
            .into_iter()
            .map(|(name, value)| CookieMatch { name, value })
            .collect(),
        CookieMatchRepr::List(list) => list,
    })
}

/// Query 参数匹配
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QueryMatch {
    pub name: String,
    pub value: String,
}

/// 路由 → 上游引用
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpstreamRef {
    pub id: UpstreamId,
}

/// 鉴权配置（阶段一仅支持 bearer / apikey）
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    #[serde(rename = "type", default)]
    pub kind: AuthKind,
    /// 允许的 API Key / Bearer token 列表；为空 + kind!=none 视为配置错误（拒绝请求）
    #[serde(default)]
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    #[default]
    None,
    Bearer,
    ApiKey,
}

/// 速率限制（默认从 `defaults.rate_limit` 继承）
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitConfig {
    pub algorithm: String,
    pub rps: u32,
    pub burst: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_headers_map_form() {
        let rule: MatchRule =
            serde_yaml::from_str("method: POST\npath: /x\nheaders:\n  x-provider: anthropic\n")
                .unwrap();
        assert_eq!(rule.headers.len(), 1);
        assert_eq!(rule.headers[0].name, "x-provider");
        assert_eq!(rule.headers[0].value, "anthropic");
        assert!(!rule.headers[0].regex);
    }

    #[test]
    fn deserialize_headers_list_form() {
        let rule: MatchRule = serde_yaml::from_str(
            "method: POST\npath: /x\nheaders:\n  - name: x-provider\n    value: openai\n",
        )
        .unwrap();
        assert_eq!(rule.headers.len(), 1);
        assert_eq!(rule.headers[0].name, "x-provider");
        assert_eq!(rule.headers[0].value, "openai");
    }

    #[test]
    fn deserialize_cookies_map_form() {
        let rule: MatchRule =
            serde_yaml::from_str("method: POST\npath: /x\ncookies:\n  provider_group: beta\n")
                .unwrap();
        assert_eq!(rule.cookies.len(), 1);
        assert_eq!(rule.cookies[0].name, "provider_group");
        assert_eq!(rule.cookies[0].value, "beta");
    }

    #[test]
    fn deserialize_route_with_canary() {
        let route: RouteConfig = serde_yaml::from_str(
            "name: canary\nauth:\n  type: none\nmatch:\n  method: POST\n  path: /v2/x\ncanary:\n  strategy: weight\n  targets:\n    - upstream_id: openai\n      weight: 60\n    - upstream_id: anthropic\n      weight: 40\n",
        )
        .unwrap();
        assert!(route.upstream.is_none());
        let canary = route.canary.unwrap();
        assert_eq!(canary.strategy, "weight");
        assert_eq!(canary.targets.len(), 2);
        assert_eq!(canary.targets[0].upstream_id, "openai");
        assert_eq!(canary.targets[0].weight, 60);
    }

    #[test]
    fn deserialize_route_with_max_retries_upstream() {
        let cfg: crate::core::config::upstream::UpstreamConfig = serde_yaml::from_str(
            "id: openai\nprovider: openai\nbase_url: https://api.openai.com\napi_key: sk-0123456789abcdef\nmax_retries: 3\n",
        )
        .unwrap();
        assert_eq!(cfg.max_retries, Some(3));
    }
}
