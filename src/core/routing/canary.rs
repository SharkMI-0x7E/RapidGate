//! 灰度路由集成
//!
//! 将灰度策略集成到路由匹配流程中

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::canary::policy::CanaryPolicy;
use crate::core::canary::sticky::StickySession;
use crate::core::config::route::RouteConfig;
use crate::core::config::upstream::UpstreamConfig;
use crate::core::error::CoreError;

/// 灰度路由决策器
pub struct CanaryRouter {
    /// 灰度策略
    policy: Arc<dyn CanaryPolicy>,
    /// 会话黏性配置
    sticky: Option<StickySession>,
}

impl CanaryRouter {
    /// 创建灰度路由器
    pub fn new(policy: Arc<dyn CanaryPolicy>, sticky: Option<StickySession>) -> Self {
        Self { policy, sticky }
    }

    /// 根据灰度策略选择 upstream
    ///
    /// # Arguments
    /// * `route` - 匹配到的路由配置
    /// * `headers` - 请求头
    /// * `cookies` - Cookie
    /// * `client_ip` - 客户端 IP
    /// * `upstreams` - 可用的 upstream 列表
    ///
    /// # Returns
    /// 选中的 upstream 配置
    pub fn select_upstream<'a>(
        &self,
        _route: &RouteConfig,
        headers: &HashMap<String, String>,
        cookies: &HashMap<String, String>,
        client_ip: &str,
        upstreams: &'a [UpstreamConfig],
    ) -> Result<&'a UpstreamConfig, CoreError> {
        // 如果有会话黏性配置，优先使用
        if let Some(sticky) = &self.sticky {
            if let Some(session_id) = sticky.extract_session_id(cookies) {
                let idx = sticky.select_upstream(&session_id, upstreams.len());
                return upstreams
                    .get(idx)
                    .ok_or_else(|| CoreError::RouteNotFound("no upstream available".to_string()));
            }
        }

        // 使用灰度策略选择
        let idx = self
            .policy
            .select_upstream(headers, cookies, client_ip, upstreams.len());
        upstreams
            .get(idx)
            .ok_or_else(|| CoreError::RouteNotFound("no upstream available".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::canary::policy::WeightPolicy;
    use crate::core::config::route::{AuthConfig, MatchRule, UpstreamRef};

    #[test]
    fn test_canary_router_weight() {
        let policy = Arc::new(WeightPolicy::new(80));
        let router = CanaryRouter::new(policy, None);

        let route = RouteConfig {
            name: "test".to_string(),
            match_rule: MatchRule {
                method: "POST".to_string(),
                path: "/v1/chat".to_string(),
                host: None,
                headers: vec![],
                query: vec![],
                cookies: vec![],
            },
            upstream: Some(UpstreamRef {
                id: "test-upstream".to_string(),
            }),
            auth: AuthConfig::default(),
            rate_limit: None,
            canary: None,
        };

        let upstreams = vec![
            UpstreamConfig {
                id: "stable".to_string(),
                provider: "openai".to_string(),
                base_url: "https://api.openai.com".to_string(),
                api_key: "test-key".to_string(),
                load_balancer: Default::default(),
                models: vec![],
                timeout_ms: None,
                pool: None,
                max_retries: None,
            },
            UpstreamConfig {
                id: "canary".to_string(),
                provider: "openai".to_string(),
                base_url: "https://api.openai.com".to_string(),
                api_key: "test-key".to_string(),
                load_balancer: Default::default(),
                models: vec![],
                timeout_ms: None,
                pool: None,
                max_retries: None,
            },
        ];

        let headers = HashMap::new();
        let cookies = HashMap::new();
        let client_ip = "127.0.0.1";

        // 应该能选到 upstream
        let result = router.select_upstream(&route, &headers, &cookies, client_ip, &upstreams);
        assert!(result.is_ok());
    }
}
