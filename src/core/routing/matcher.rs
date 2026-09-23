//! 路由匹配器（spec §4.3）
//!
//! 匹配顺序：精确（Exact）→ 前缀（Prefix）→ 正则（Regex）。
//! 冲突时取先注册者。

use regex::Regex;

use crate::core::config::route::{HeaderMatch, MatchRule, QueryMatch, RouteConfig};
use crate::core::util::path::normalize;

/// 匹配器：把 MatchRule 编译为可执行的 matcher
#[derive(Debug, Clone)]
pub enum Matcher {
    Exact { method: String, path: String },
    Prefix { method: String, prefix: String },
    Regex { method: String, pattern: Regex },
}

impl Matcher {
    /// 从 MatchRule 编译为 Matcher；选择最严的形态
    pub fn compile(rule: &MatchRule) -> Result<Self, String> {
        let raw_path = &rule.path;
        let method = rule.method.to_uppercase();

        // regex 模式：~ 开头
        if let Some(stripped) = raw_path.strip_prefix('~') {
            let pat = stripped.trim();
            let re = Regex::new(pat).map_err(|e| format!("invalid regex '{pat}': {e}"))?;
            return Ok(Matcher::Regex {
                method,
                pattern: re,
            });
        }

        // 路径必须以 / 开头
        if !raw_path.starts_with('/') {
            return Err(format!("path must start with '/': {raw_path}"));
        }

        // 以 / 结尾（且不为 /）-> 前缀匹配
        if raw_path.len() > 1 && raw_path.ends_with('/') {
            let prefix = raw_path.trim_end_matches('/').to_string();
            return Ok(Matcher::Prefix { method, prefix });
        }

        // 否则精确匹配（处理 . / .. / 多余 /）
        let path = normalize(raw_path);
        Ok(Matcher::Exact { method, path })
    }

    /// 判断请求是否匹配
    pub fn matches(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        query: &[(String, String)],
    ) -> bool {
        if !self.method_matches(method) {
            return false;
        }
        if !self.path_matches(path) {
            return false;
        }
        // header / query 在编译期不确定，matcher 不知道 rule 列表；具体 header/query 校验由上层
        // 拿到 RouteConfig 后再做
        let _ = (headers, query);
        true
    }

    fn method_matches(&self, method: &str) -> bool {
        let m = self.method();
        m == method.to_uppercase() || m == "ANY"
    }

    fn path_matches(&self, path: &str) -> bool {
        match self {
            Matcher::Exact { path: p, .. } => path == p,
            Matcher::Prefix { prefix, .. } => {
                path == *prefix || path.starts_with(&format!("{prefix}/"))
            }
            Matcher::Regex { pattern, .. } => pattern.is_match(path),
        }
    }

    #[inline]
    pub fn method(&self) -> &str {
        match self {
            Matcher::Exact { method, .. }
            | Matcher::Prefix { method, .. }
            | Matcher::Regex { method, .. } => method,
        }
    }
}

/// 完整匹配（含 header / query 子条件）
pub fn full_match(
    matcher: &Matcher,
    rule: &MatchRule,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    query: &[(String, String)],
) -> bool {
    if !matcher.matches(method, path, headers, query) {
        return false;
    }
    if let Some(host) = &rule.host {
        // 阶段一 host 留待 axum::extract::Host 接入；此处仅占位
        let _ = host;
    }
    header_matches(&rule.headers, headers) && query_matches(&rule.query, query)
}

fn header_matches(want: &[HeaderMatch], have: &[(String, String)]) -> bool {
    want.iter().all(|w| {
        have.iter().any(|(n, v)| {
            if !n.eq_ignore_ascii_case(&w.name) {
                return false;
            }
            if w.regex {
                Regex::new(&w.value)
                    .map(|re| re.is_match(v))
                    .unwrap_or(false)
            } else {
                v == &w.value
            }
        })
    })
}

fn query_matches(want: &[QueryMatch], have: &[(String, String)]) -> bool {
    want.iter()
        .all(|w| have.iter().any(|(n, v)| n == &w.name && v == &w.value))
}

/// 按注册顺序找出第一条匹配；返回 (index, &RouteConfig)
#[inline]
pub fn first_match<'a>(
    routes: &'a [RouteConfig],
    compiled: &[(Matcher, usize)],
    method: &str,
    path: &str,
    headers: &[(String, String)],
    query: &[(String, String)],
) -> Option<(usize, &'a RouteConfig)> {
    for (m, idx) in compiled {
        let rule = &routes[*idx];
        if full_match(m, &rule.match_rule, method, path, headers, query) {
            return Some((*idx, rule));
        }
    }
    None
}

/// 编译所有路由，返回 (Matcher, 原始 index) 列表
pub fn compile_all(routes: &[RouteConfig]) -> Result<Vec<(Matcher, usize)>, String> {
    let mut out = Vec::with_capacity(routes.len());
    for (i, r) in routes.iter().enumerate() {
        let m = Matcher::compile(&r.match_rule).map_err(|e| format!("route '{}': {e}", r.name))?;
        out.push((m, i));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rule(method: &str, path: &str) -> MatchRule {
        MatchRule {
            method: method.to_string(),
            path: path.to_string(),
            host: None,
            headers: vec![],
            query: vec![],
            cookies: vec![],
        }
    }

    #[test]
    fn exact_path_requires_exact_match() {
        let m = Matcher::compile(&make_rule("POST", "/v1/chat/completions")).unwrap();
        assert!(m.matches("POST", "/v1/chat/completions", &[], &[]));
        assert!(!m.matches("POST", "/v1/chat/completions/extra", &[], &[]));
    }

    #[test]
    fn prefix_path_matches_subpaths() {
        let m = Matcher::compile(&make_rule("GET", "/v1/models/")).unwrap();
        assert!(m.matches("GET", "/v1/models", &[], &[]));
        assert!(m.matches("GET", "/v1/models/abc", &[], &[]));
    }

    #[test]
    fn regex_path_matches() {
        let m = Matcher::compile(&make_rule("GET", "~^/v1/models(/.*)?$")).unwrap();
        assert!(m.matches("GET", "/v1/models", &[], &[]));
        assert!(m.matches("GET", "/v1/models/gpt-4", &[], &[]));
    }
}
