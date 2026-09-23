//! 请求/响应变换（spec §4.4）
//!
//! 阶段一提供占位透传；阶段二 [S2+] 在 providers/* 落地协议差异；
//! 阶段三 [S3] 增加可配置的 Transformer：注入 header、改写 body 字段、删除字段。
//! 这些规则可作为路由级变换配置，并在 hander 转发链路中被调用。

use http::{HeaderMap, HeaderValue, Method};
use serde_json::Value;

/// 转换客户端请求到上游请求（保留原签名，protobuf 兼容）
pub struct TransformedRequest {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
}

pub fn transform_request(method: &Method, path: &str, headers: &HeaderMap) -> TransformedRequest {
    TransformedRequest {
        method: method.clone(),
        path: path.to_string(),
        headers: headers.clone(),
    }
}

/// 提取上游响应头里需要透传的子集
pub fn transform_response_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (k, v) in headers.iter() {
        // 透传除 hop-by-hop 之外的所有 header
        if !is_hop_by_hop(k.as_str()) {
            out.insert(k.clone(), v.clone());
        }
    }
    out
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}

/// 单条 body 字段改写/删除规则
#[derive(Debug, Clone)]
pub struct BodyRule {
    /// 字段路径，如 `"max_tokens"` 或 `"request.meta.tags"`
    pub field: String,
    /// Some(值) = 改写为固定值；None = 删除该字段
    pub rewrite_to: Option<serde_json::Value>,
}

/// 请求变换器
///
/// 按配置的规则把上游请求 body 改写为网关侧期望的形态。
/// `Default` 行为 = 透明透传（不注入、不改写、不删除）。
#[derive(Debug, Clone, Default)]
pub struct Transformer {
    /// 注入到请求头 HeaderMap 的 KV 列表
    inject_headers: Vec<(String, String)>,
    /// body 字段改写/删除规则列表
    body_rules: Vec<BodyRule>,
}

impl Transformer {
    /// 创建一个带注入 header 规则的变换器
    pub fn with_injected_headers(
        mut self,
        pairs: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        self.inject_headers = pairs.into_iter().collect();
        self
    }

    /// 创建一个带 body 规则的变换器
    pub fn with_body_rules(mut self, rules: impl IntoIterator<Item = BodyRule>) -> Self {
        self.body_rules = rules.into_iter().collect();
        self
    }

    /// 往请求头注入配置的 header（就地修改传入的 HeaderMap）
    pub fn inject_headers(&self, headers: &mut HeaderMap) {
        for (k, v) in &self.inject_headers {
            if let (Ok(name), Ok(val)) = (
                http::header::HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                headers.insert(name, val);
            }
        }
    }

    /// 构造一个新的 HeaderMap，注入所有配置 header（供无现有 header 的场景）
    pub fn header_map_with_injected(&self) -> HeaderMap {
        let mut m = HeaderMap::new();
        self.inject_headers(&mut m);
        m
    }

    /// 应用 body 规则，返回新的 Value（不改动原 value）
    ///
    /// 对每个规则：
    /// - `rewrite_to = Some(v)` 时把 `field` 路径指向的值整段替换为 `v`
    /// - `rewrite_to = None` 时删除 `field` 指向的字段
    pub fn apply_body(&self, mut body: Value) -> Value {
        for rule in &self.body_rules {
            apply_body_rule(&mut body, rule);
        }
        body
    }

    /// 应用请求变换（body + 注入 header）；透明透传时原样返回 body
    pub fn apply_request(&self, body: Value, headers: &mut HeaderMap) -> Value {
        self.inject_headers(headers);
        self.apply_body(body)
    }

    /// `apply_request` 的别名，供转发链路直接调用（只变换 body）
    pub fn transform_command(&self, body: Value) -> Value {
        self.apply_body(body)
    }
}

/// 按「.」分隔的路径改写/删除 body 字段
fn apply_body_rule(body: &mut Value, rule: &BodyRule) {
    let segments: Vec<&str> = rule.field.split('.').collect();
    if segments.is_empty() {
        return;
    }
    let mut current = body;
    for seg in &segments[..segments.len() - 1] {
        // 路径中间节点必须是对象，否则路径无效，跳过
        if let Some(next) = current.get_mut(*seg) {
            current = next;
        } else {
            return;
        }
    }
    let last = segments[segments.len() - 1];
    if let Some(obj) = current.as_object_mut() {
        match &rule.rewrite_to {
            Some(v) => {
                obj.insert(last.to_string(), v.clone());
            }
            None => {
                obj.remove(last);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;
    use serde_json::json;

    #[test]
    fn strips_hop_by_hop() {
        let mut h = HeaderMap::new();
        h.insert(
            "content-type",
            HeaderValue::from_static("text/event-stream"),
        );
        h.insert("connection", HeaderValue::from_static("close"));
        let out = transform_response_headers(&h);
        assert!(out.contains_key("content-type"));
        assert!(!out.contains_key("connection"));
    }

    #[test]
    fn default_transformer_passthrough() {
        let t = Transformer::default();
        let body = json!({"model": "gpt-4", "max_tokens": 100});
        let out = t.apply_body(body.clone());
        assert_eq!(out, body);
    }

    #[test]
    fn injects_headers() {
        let t = Transformer::default()
            .with_injected_headers(vec![("x-gateway-tag".to_string(), "billing-a".to_string())]);
        let mut h = HeaderMap::new();
        h.insert("content-type", HeaderValue::from_static("application/json"));
        t.inject_headers(&mut h);
        assert_eq!(h.get("x-gateway-tag").unwrap(), "billing-a");
        assert!(h.contains_key("content-type"));
    }

    #[test]
    fn rewrites_and_deletes_body_fields() {
        let t = Transformer::default().with_body_rules(vec![
            BodyRule {
                field: "max_tokens".to_string(),
                rewrite_to: Some(json!(512)),
            },
            BodyRule {
                field: "request.meta.tags".to_string(),
                rewrite_to: None, // 删除嵌套字段
            },
        ]);
        let body = json!({
            "model": "gpt-4",
            "max_tokens": 100,
            "request": { "meta": { "tags": ["a"] }, "id": 1 }
        });
        let out = t.apply_body(body);
        assert_eq!(out["max_tokens"], json!(512));
        assert!(out["request"]["meta"].get("tags").is_none());
        assert_eq!(out["request"]["id"], json!(1));
    }

    #[test]
    fn transform_command_aliases_apply_body() {
        let t = Transformer::default().with_body_rules(vec![BodyRule {
            field: "stream".to_string(),
            rewrite_to: Some(json!(false)),
        }]);
        let body = json!({"stream": true});
        let out = t.transform_command(body);
        assert_eq!(out["stream"], json!(false));
    }
}
