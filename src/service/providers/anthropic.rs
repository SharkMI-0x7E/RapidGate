//! Anthropic Provider 实现
//!
//! 支持 Anthropic Messages API 格式，包括 Claude 系列模型。
//! 阶段三新增（spec §2 [S3]）。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::core::config::ProviderKind;
use crate::core::error::CoreError;
use crate::service::providers::{Provider, ProviderRequest, ProviderResponse};

/// Anthropic Provider
pub struct AnthropicProvider;

#[async_trait]
impl Provider for AnthropicProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn transform_request(&self, req: &ProviderRequest) -> Result<Value, CoreError> {
        // 将 OpenAI 格式转换为 Anthropic Messages API 格式
        // OpenAI: { "model": "gpt-4", "messages": [...], "stream": true }
        // Anthropic: { "model": "claude-3", "messages": [...], "stream": true, "max_tokens": 4096 }

        let mut anthropic_req = req.body.clone();

        // 添加 max_tokens（Anthropic 必需）
        if anthropic_req.get("max_tokens").is_none() {
            anthropic_req["max_tokens"] = json!(4096);
        }

        // messages 原样透传，保留 role:"system" 语义（spec 要求不得改写为 user）
        Ok(anthropic_req)
    }

    fn transform_response(&self, resp: &ProviderResponse) -> Result<Value, CoreError> {
        // Anthropic 响应格式转换为 OpenAI 格式
        // Anthropic 流式：event: message_start / content_block_delta / message_stop
        // OpenAI 流式：data: { "choices": [{"delta": {...}}] }

        if resp.is_stream {
            // 流式：handler 已剥离 `data:` 前缀，这里收到的是单个纯 JSON chunk，
            // 转换后返回单个 OpenAI Value（不包含 data: 前缀，避免二次包装）。
            let chunk: Value = serde_json::from_slice(&resp.body).map_err(|e| {
                CoreError::Internal(format!("failed to parse Anthropic SSE chunk: {e}"))
            })?;
            Ok(self
                .transform_streaming_chunk(&chunk)
                .unwrap_or(Value::Null))
        } else {
            // 非流式响应
            let anthropic_resp: Value = serde_json::from_slice(&resp.body).map_err(|e| {
                CoreError::Internal(format!("failed to parse Anthropic response: {e}"))
            })?;

            // 转换 Anthropic 响应到 OpenAI 格式
            // Anthropic: { "id": "...", "content": [{"type": "text", "text": "..."}], "usage": {...} }
            // OpenAI: { "id": "...", "choices": [{"message": {"content": "..."}}], "usage": {...} }

            let content = anthropic_resp
                .get("content")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|block| block.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let usage = map_anthropic_usage(anthropic_resp.get("usage"));

            let openai_resp = json!({
                "id": anthropic_resp.get("id").cloned().unwrap_or(json!("")),
                "object": "chat.completion",
                "created": anthropic_resp.get("created").cloned().unwrap_or(json!(0)),
                "model": anthropic_resp.get("model").cloned().unwrap_or(json!("")),
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": content
                    },
                    "finish_reason": "stop"
                }],
                "usage": usage
            });

            Ok(openai_resp)
        }
    }

    fn api_path(&self) -> &str {
        "/v1/messages"
    }
}

impl AnthropicProvider {
    /// 转换 Anthropic 流式 chunk 到 OpenAI 格式
    fn transform_streaming_chunk(&self, chunk: &Value) -> Option<Value> {
        let event_type = chunk.get("type")?.as_str()?;

        match event_type {
            "message_start" => {
                // 消息开始
                Some(json!({
                    "id": chunk.get("message").and_then(|m| m.get("id")).cloned().unwrap_or(json!("")),
                    "object": "chat.completion.chunk",
                    "created": chunk.get("message").and_then(|m| m.get("created")).cloned().unwrap_or(json!(0)),
                    "model": chunk.get("message").and_then(|m| m.get("model")).cloned().unwrap_or(json!("")),
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "role": "assistant"
                        },
                        "finish_reason": null
                    }]
                }))
            }
            "content_block_delta" => {
                // 内容增量
                let delta = chunk.get("delta")?;
                let text = delta.get("text")?.as_str()?;

                Some(json!({
                    "id": "",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": "",
                    "choices": [{
                        "index": 0,
                        "delta": {
                            "content": text
                        },
                        "finish_reason": null
                    }]
                }))
            }
            "message_stop" => {
                // 消息结束
                Some(json!({
                    "id": "",
                    "object": "chat.completion.chunk",
                    "created": 0,
                    "model": "",
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "stop"
                    }]
                }))
            }
            _ => None,
        }
    }
}

/// 将 Anthropic usage 映射为 OpenAI 兼容的 usage 结构。
///
/// Anthropic 返回 `{input_tokens, output_tokens}`，OpenAI 期望
/// `{prompt_tokens, completion_tokens, total_tokens}`。字段缺失时补 0。
fn map_anthropic_usage(raw: Option<&Value>) -> Value {
    let input = raw
        .and_then(|u| u.get("input_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let output = raw
        .and_then(|u| u.get("output_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input + output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::providers::{ProviderRequest, ProviderResponse};

    #[test]
    fn preserves_system_role() {
        let req = ProviderRequest {
            body: json!({
                "model": "claude-3",
                "messages": [
                    {"role": "system", "content": "sys"},
                    {"role": "user", "content": "hi"}
                ],
            }),
            base_url: "http://x".into(),
            api_key: "k".into(),
            model: "claude-3".into(),
            stream: false,
            operation: "chat".into(),
        };
        let p = AnthropicProvider;
        let out = p.transform_request(&req).unwrap();
        let msgs = out["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "sys");
    }

    #[test]
    fn non_stream_usage_mapped() {
        let resp = ProviderResponse {
            body: serde_json::to_vec(&json!({
                "id": "m1",
                "content": [{"type": "text", "text": "hello"}],
                "usage": {"input_tokens": 10, "output_tokens": 5}
            }))
            .unwrap()
            .into(),
            status: 200,
            is_stream: false,
        };
        let p = AnthropicProvider;
        let out = p.transform_response(&resp).unwrap();
        assert_eq!(out["usage"]["prompt_tokens"], 10);
        assert_eq!(out["usage"]["completion_tokens"], 5);
        assert_eq!(out["usage"]["total_tokens"], 15);
    }

    #[test]
    fn stream_chunk_returns_single_value_no_data_prefix() {
        let resp = ProviderResponse {
            body: serde_json::to_vec(&json!({
                "type": "content_block_delta",
                "delta": {"text": "xyz"}
            }))
            .unwrap()
            .into(),
            status: 200,
            is_stream: true,
        };
        let p = AnthropicProvider;
        let out = p.transform_response(&resp).unwrap();
        assert!(out.is_object());
        assert_eq!(out["choices"][0]["delta"]["content"], "xyz");
        // 不包含 data: 前缀，避免二次包装
        assert!(!out.to_string().starts_with("data:"));
    }
}
