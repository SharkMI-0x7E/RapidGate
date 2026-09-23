//! 路由表 + Router（spec §4.3）
//!
//! `RouteTable` 不可变（&self 操作），切换时整体替换。
//! `Router` 持 `Arc<ArcSwap<RouteTable>>` 支持阶段二 [S2+] 的热重载。

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::core::config::route::RouteConfig;
use crate::core::routing::matcher::{self, Matcher};

/// 不可变路由表
#[derive(Debug, Clone)]
pub struct RouteTable {
    pub routes: Vec<RouteConfig>,
    compiled: Vec<(Matcher, usize)>,
}

impl RouteTable {
    /// 从 routes 构造；同时编译所有 matcher
    pub fn new(routes: Vec<RouteConfig>) -> Result<Self, String> {
        let compiled = matcher::compile_all(&routes)?;
        // 按优先级排序：Exact(0) < Prefix(1) < Regex(2)
        let mut sorted = compiled;
        sorted.sort_by_key(|(m, _)| match m {
            Matcher::Exact { .. } => 0,
            Matcher::Prefix { .. } => 1,
            Matcher::Regex { .. } => 2,
        });
        Ok(Self {
            routes,
            compiled: sorted,
        })
    }

    pub fn empty() -> Self {
        Self {
            routes: Vec::new(),
            compiled: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    /// 匹配请求
    pub fn match_request(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        query: &[(String, String)],
    ) -> Option<(usize, &RouteConfig)> {
        matcher::first_match(&self.routes, &self.compiled, method, path, headers, query)
    }
}

/// 路由容器：内含 Arc<ArcSwap<RouteTable>>
#[derive(Debug, Clone)]
pub struct Router {
    table: Arc<ArcSwap<RouteTable>>,
}

impl Router {
    pub fn new(initial: RouteTable) -> Self {
        Self {
            table: Arc::new(ArcSwap::from_pointee(initial)),
        }
    }

    /// 拿到当前快照（in-flight 请求生命周期内稳定）
    pub fn snapshot(&self) -> Arc<RouteTable> {
        self.table.load_full()
    }

    /// 整体替换路由表（阶段一**不**实现回滚，留 [S2]）
    pub fn replace(&self, table: RouteTable) {
        self.table.store(Arc::new(table));
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new(RouteTable::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::route::{AuthConfig, MatchRule, RouteConfig, UpstreamRef};

    fn route(name: &str, method: &str, path: &str) -> RouteConfig {
        RouteConfig {
            name: name.to_string(),
            match_rule: MatchRule {
                method: method.to_string(),
                path: path.to_string(),
                host: None,
                headers: vec![],
                query: vec![],
                cookies: vec![],
            },
            upstream: Some(UpstreamRef {
                id: "mock".to_string(),
            }),
            auth: AuthConfig::default(),
            rate_limit: None,
            canary: None,
        }
    }

    #[test]
    fn first_registered_wins_on_conflict() {
        let routes = vec![
            route("first", "POST", "/v1/x"),
            route("second", "POST", "/v1/x"),
        ];
        let table = RouteTable::new(routes).unwrap();
        let (idx, r) = table.match_request("POST", "/v1/x", &[], &[]).unwrap();
        assert_eq!(idx, 0);
        assert_eq!(r.name, "first");
    }

    #[test]
    fn replace_swaps_table() {
        let r = Router::new(RouteTable::empty());
        r.replace(RouteTable::new(vec![route("a", "GET", "/a")]).unwrap());
        assert_eq!(r.snapshot().len(), 1);
        r.replace(RouteTable::new(vec![route("b", "GET", "/b"), route("c", "GET", "/c")]).unwrap());
        assert_eq!(r.snapshot().len(), 2);
    }
}
