//! service/handler — 5 个 axum handler（spec §5.3）

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use rand::Rng;
use serde_json::{json, Value};

use crate::core::config::provider::ProviderKind;
use crate::core::config::route::{CanaryTarget, RouteConfig};
use crate::core::config::upstream::UpstreamConfig;
use crate::core::error::CoreError;
use crate::core::proxy::transformer::Transformer;
use crate::core::routing::RouteTable;
use crate::service::error::ServiceError;
use crate::service::middleware::auth;
use crate::service::middleware::ratelimit;
use crate::service::providers::{ProviderFactory, ProviderRequest, ProviderResponse};
use crate::service::state::AppState;

/// POST /v1/chat/completions — OpenAI 兼容聊天补全（含 SSE 流式）
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, ServiceError> {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let (route, upstream) = resolve_route(&state, &method, &path)?;
    // 鉴权 + 限流（先同步提取凭据/限流 key，再 await，避免 &req 跨 await 导致 future 非 Send）
    if let Err(resp) = auth::check_route_auth(&route, &req) {
        return Ok(resp);
    }
    let limit_key = ratelimit::extract_limit_key(&req);
    if let Err(resp) = ratelimit::check_route_rate_limit(&state, &route, &limit_key).await {
        return Ok(resp);
    }
    let start = std::time::Instant::now();
    let out = forward_streaming(state.clone(), &route, &upstream, req, "chat").await;
    record_request_metrics(&state, route.name.as_str(), &method, &out, start);
    out
}

/// POST /v1/embeddings — OpenAI 兼容 embedding
pub async fn embeddings(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, ServiceError> {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let (route, upstream) = resolve_route(&state, &method, &path)?;
    if let Err(resp) = auth::check_route_auth(&route, &req) {
        return Ok(resp);
    }
    let limit_key = ratelimit::extract_limit_key(&req);
    if let Err(resp) = ratelimit::check_route_rate_limit(&state, &route, &limit_key).await {
        return Ok(resp);
    }
    let start = std::time::Instant::now();
    let out = forward_streaming(state.clone(), &route, &upstream, req, "embeddings").await;
    record_request_metrics(&state, route.name.as_str(), &method, &out, start);
    out
}

/// 记录请求级指标；失败路径统一以 500 采样（阈值/分布不受影响）
fn record_request_metrics(
    state: &Arc<AppState>,
    route: &str,
    method: &str,
    out: &Result<Response, ServiceError>,
    start: std::time::Instant,
) {
    let status = match out {
        Ok(resp) => resp.status().as_u16(),
        Err(_) => 500,
    };
    state
        .metrics
        .record_request(route, method, status, start.elapsed().as_secs_f64());
}

/// GET /v1/models — 列出可用模型
pub async fn list_models(State(state): State<Arc<AppState>>) -> Response {
    let mut models: Vec<String> = Vec::new();
    for up in &state.upstream_configs {
        for m in &up.models {
            if !models.contains(m) {
                models.push(m.clone());
            }
        }
    }
    if models.is_empty() {
        models.push("rapidgate-stage1-placeholder".to_string());
    }
    let body = json!({
        "object": "list",
        "data": models.iter().map(|id| json!({"id": id, "object": "model"})).collect::<Vec<_>>(),
    });
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// GET /healthz — 存活探针（不查上游）
pub async fn healthz() -> Response {
    let body = json!({"status": "ok"});
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// GET /readyz — 就绪探针（检查配置有效性 + 上游可达性）
pub async fn readyz(State(state): State<Arc<AppState>>) -> Result<Response, ServiceError> {
    if state.upstream_configs.is_empty() {
        let body = json!({
            "error": {
                "code": "not_ready",
                "message": "no upstreams configured",
            }
        });
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            [(CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response());
    }
    let route_count = state.route_table.snapshot().len();
    if route_count == 0 {
        let body = json!({
            "error": {
                "code": "not_ready",
                "message": "no routes loaded",
            }
        });
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            [(CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response());
    }
    let body = json!({"status": "ready", "routes": route_count, "upstreams": state.upstream_configs.len()});
    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response())
}

// -------------------- 内部辅助 --------------------

/// 在路由表 + upstream 配置里一次性解析出 (RouteConfig, UpstreamConfig)
fn resolve_route(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
) -> Result<(RouteConfig, UpstreamConfig), ServiceError> {
    let table: Arc<RouteTable> = state.route_table.snapshot();
    let (_idx, route) = table
        .match_request(method, path, &[], &[])
        .ok_or_else(|| ServiceError::Core(CoreError::RouteNotFound(path.to_string())))?;
    let upstream = resolve_upstream(state, route)?;
    Ok((route.clone(), upstream))
}

/// 从路由解析目标上游：优先 canary 灰度按权重选，其次直连 upstream
fn resolve_upstream(
    state: &Arc<AppState>,
    route: &RouteConfig,
) -> Result<UpstreamConfig, ServiceError> {
    let target_id = if let Some(canary) = &route.canary {
        let mut rng = rand::thread_rng();
        let total: u32 = canary.targets.iter().map(|t| t.weight).sum();
        if total == 0 {
            return Err(ServiceError::Core(CoreError::Config(format!(
                "route '{}' canary total weight is zero",
                route.name
            ))));
        }
        let roll: u32 = rng.gen_range(0..total);
        select_canary_target(&canary.targets, roll)
            .map(|t| t.upstream_id.clone())
            .map_err(ServiceError::Core)?
    } else if let Some(upstream) = &route.upstream {
        upstream.id.clone()
    } else {
        return Err(ServiceError::Core(CoreError::Config(format!(
            "route '{}' has neither upstream nor canary",
            route.name
        ))));
    };

    state
        .upstream_configs
        .iter()
        .find(|u| u.id == target_id)
        .cloned()
        .ok_or_else(|| {
            ServiceError::Core(CoreError::Config(format!(
                "route '{}' references unknown upstream '{}'",
                route.name, target_id
            )))
        })
}

/// 按权重累计区间命中一个灰度 target（roll ∈ [0, total_weight)）
fn select_canary_target(targets: &[CanaryTarget], roll: u32) -> Result<&CanaryTarget, CoreError> {
    let total: u32 = targets.iter().map(|t| t.weight).sum();
    if total == 0 {
        return Err(CoreError::Config("canary total weight is zero".into()));
    }
    let mut cumulative = 0u32;
    for t in targets {
        if t.weight == 0 {
            continue; // 权重 0 的 target 不接收流量
        }
        cumulative += t.weight;
        if roll < cumulative {
            return Ok(t);
        }
    }
    Err(CoreError::Config("canary selection failed".into()))
}

/// 真实转发到上游 LLM Provider
async fn forward_streaming(
    state: Arc<AppState>,
    _route: &RouteConfig,
    upstream: &UpstreamConfig,
    req: Request,
    operation: &str,
) -> Result<Response, ServiceError> {
    // 1. 解析 Provider 类型
    let provider_kind = match upstream.provider.as_str() {
        "openai" => ProviderKind::OpenAI,
        "anthropic" => ProviderKind::Anthropic,
        "gemini" => ProviderKind::Gemini,
        "local" => ProviderKind::Local,
        other => return Err(CoreError::Config(format!("unknown provider: {}", other)).into()),
    };

    // 2. 创建 Provider 实例
    let provider = ProviderFactory::create(provider_kind);

    // 3. 提取请求 body
    let body_bytes = axum::body::to_bytes(req.into_body(), state.max_body_bytes)
        .await
        .map_err(|e| CoreError::Internal(format!("failed to read request body: {}", e)))?;

    let body_json: Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| CoreError::Internal(format!("invalid JSON body: {}", e)))?;

    // 4. 应用请求变换器（默认透明透传；可配置注入 header / 改写 body 字段）
    //    在判断流式之前变换，保证 stream 标志基于变换后的 body。
    let body_json = Transformer::default().transform_command(body_json);

    // 5. 判断是否流式
    let is_stream = body_json
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 6. 提取模型名称
    let model = body_json
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // 7. 构建 ProviderRequest
    let provider_req = ProviderRequest {
        body: body_json.clone(),
        base_url: upstream.base_url.clone(),
        api_key: upstream.api_key.clone(),
        model,
        stream: is_stream,
        operation: operation.to_string(),
    };

    // 8. 转换请求格式
    let transformed_body = provider
        .transform_request(&provider_req)
        .map_err(ServiceError::Core)?;

    // 8. 构建上游 URL
    let upstream_url = provider
        .build_url(&provider_req)
        .map_err(ServiceError::Core)?;

    // 9. 发送请求到上游（复用连接池 client，client_for 内执行 SSRF 校验）
    let client = state.upstream_pool.client_for(upstream).await?;
    let upstream_req = client
        .post(&upstream_url)
        .header("Authorization", format!("Bearer {}", upstream.api_key))
        .header("Content-Type", "application/json");

    let upstream_resp = upstream_req
        .json(&transformed_body)
        .send()
        .await
        .map_err(|e| CoreError::Internal(format!("upstream request failed: {}", e)))?;

    // 10. 处理响应
    let status = upstream_resp.status();
    if !status.is_success() {
        let error_body = upstream_resp
            .text()
            .await
            .unwrap_or_else(|_| "unknown error".to_string());
        return Err(
            CoreError::Internal(format!("upstream returned {}: {}", status, error_body)).into(),
        );
    }

    // 11. 流式 vs 非流式
    if is_stream {
        // 流式：透传 SSE 流，按 SSE 协议解析并转换每个事件
        let stream = upstream_resp.bytes_stream();

        // 使用 scan 缓冲 SSE 事件，处理 chunk 边界问题
        let sse_events = stream
            .scan(
                (Vec::new(), Vec::new()), // (buffer, pending_events)
                |(buffer, pending), chunk_result| {
                    match chunk_result {
                        Ok(chunk) => {
                            buffer.extend_from_slice(&chunk);

                            // 按 \n\n 分割 SSE 事件
                            while let Some(pos) = find_sse_event_end(buffer) {
                                let event_bytes = buffer.drain(..pos + 2).collect::<Vec<_>>();
                                let event_str = String::from_utf8_lossy(&event_bytes);

                                // 解析 data: 字段
                                if let Some(data) = parse_sse_data(&event_str) {
                                    pending.push(Ok(data));
                                }
                            }

                            if pending.is_empty() {
                                futures_util::future::ready(None)
                            } else {
                                futures_util::future::ready(Some(std::mem::take(pending)))
                            }
                        }
                        Err(e) => {
                            pending.push(Err(std::io::Error::other(e)));
                            futures_util::future::ready(Some(std::mem::take(pending)))
                        }
                    }
                },
            )
            .flat_map(futures_util::stream::iter);

        // 转换每个 SSE 事件
        let transformed_stream = sse_events.map(move |event_result| {
            match event_result {
                Ok(data_str) => {
                    // [DONE] 标记直接透传
                    if data_str == "[DONE]" {
                        return Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"));
                    }

                    // 解析 JSON
                    let json: Value = match serde_json::from_str(&data_str) {
                        Ok(j) => j,
                        Err(e) => {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("invalid SSE JSON: {}", e),
                            ));
                        }
                    };

                    // 调用 Provider 转换
                    let provider_resp = ProviderResponse {
                        body: serde_json::to_vec(&json).unwrap().into(),
                        status: status.as_u16(),
                        is_stream: true,
                    };

                    match provider.transform_response(&provider_resp) {
                        Ok(transformed) => {
                            // 某些不需要的事件（如 content_block_start）返回 Null，跳过以免输出 `data: null`。
                            if transformed.is_null() {
                                return Ok::<Bytes, std::io::Error>(Bytes::new());
                            }
                            let sse_line = format!("data: {}\n\n", transformed);
                            Ok(Bytes::from(sse_line))
                        }
                        Err(e) => Err(std::io::Error::other(format!("transform failed: {e}"))),
                    }
                }
                Err(e) => Err(e),
            }
        });

        let body = Body::from_stream(transformed_stream);
        Ok((StatusCode::OK, [(CONTENT_TYPE, "text/event-stream")], body).into_response())
    } else {
        // 非流式：等待完整响应
        let resp_bytes = upstream_resp
            .bytes()
            .await
            .map_err(|e| CoreError::Internal(format!("failed to read upstream response: {}", e)))?;

        let provider_resp = ProviderResponse {
            body: resp_bytes,
            status: status.as_u16(),
            is_stream: false,
        };

        let transformed = provider
            .transform_response(&provider_resp)
            .map_err(ServiceError::Core)?;

        Ok((
            StatusCode::OK,
            [(CONTENT_TYPE, "application/json")],
            transformed.to_string(),
        )
            .into_response())
    }
}

// 引入 Body 以保持 import 整洁（未来流式接管时用）
#[allow(dead_code)]
fn _force_use_body() -> Body {
    Body::empty()
}

// ==================== SSE 解析辅助函数 ====================

/// 查找 SSE 事件的结束位置（\n\n 或 \r\n\r\n）
fn find_sse_event_end(buffer: &[u8]) -> Option<usize> {
    // 查找 \n\n
    for i in 0..buffer.len().saturating_sub(1) {
        if buffer[i] == b'\n' && buffer[i + 1] == b'\n' {
            return Some(i);
        }
        // 也支持 \r\n\r\n
        if i + 3 < buffer.len()
            && buffer[i] == b'\r'
            && buffer[i + 1] == b'\n'
            && buffer[i + 2] == b'\r'
            && buffer[i + 3] == b'\n'
        {
            return Some(i);
        }
    }
    None
}

/// 解析 SSE 事件中的 data 字段
fn parse_sse_data(event: &str) -> Option<String> {
    let mut data_lines = Vec::new();

    for line in event.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim());
        }
        // 忽略其他字段（id:, retry:, event: 等）
    }

    if data_lines.is_empty() {
        None
    } else {
        // 多行 data 用 \n 连接
        Some(data_lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn t(id: &str, weight: u32) -> CanaryTarget {
        CanaryTarget {
            upstream_id: id.to_string(),
            weight,
        }
    }

    #[test]
    fn canary_targets_all_reachable_by_weight() {
        let targets = vec![t("a", 60), t("b", 25), t("c", 10), t("d", 5)];
        let total: u32 = targets.iter().map(|t| t.weight).sum();
        let mut hits: HashMap<String, u32> = HashMap::new();
        for roll in 0..total {
            let sel = select_canary_target(&targets, roll).unwrap();
            *hits.entry(sel.upstream_id.clone()).or_insert(0) += 1;
        }
        // 60/25/10/5 → 各 target 均可达，且区间边界与权重一致
        assert_eq!(hits["a"], 60);
        assert_eq!(hits["b"], 25);
        assert_eq!(hits["c"], 10);
        assert_eq!(hits["d"], 5);
    }

    #[test]
    fn canary_zero_weight_target_not_selected() {
        let targets = vec![t("a", 60), t("b", 25), t("c", 10), t("d", 0)];
        let total: u32 = targets.iter().map(|t| t.weight).sum();
        assert_eq!(total, 95);
        for roll in 0..total {
            let sel = select_canary_target(&targets, roll).unwrap();
            assert_ne!(
                sel.upstream_id, "d",
                "zero-weight target must not be selected"
            );
        }
    }

    #[test]
    fn canary_all_zero_weight_errors() {
        let targets = vec![t("a", 0), t("b", 0)];
        assert!(select_canary_target(&targets, 0).is_err());
    }
}
