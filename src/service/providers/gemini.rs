//! Gemini Provider 实现
//!
//! 支持 Google Gemini API 格式，包括 Gemini Pro/Ultra 模型。
//! 阶段三新增（spec §2 [S3]）。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::core::config::ProviderKind;
use crate::core::error::CoreError;
use crate::service::providers::{Provider, ProviderRequest, ProviderResponse};

/// Gemini Provider
pub struct GeminiProvider;

#[async_trait]
impl Provider for GeminiProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gemini
    }

    fn transform_request(&self, req: &ProviderRequest) -> Result<Value, CoreError> {
        // 将 OpenAI 格式转换为 Gemini generateContent 格式
        // OpenAI: { "model": "gpt-4", "messages": [...], "stream": true }
        // Gemini: { "contents": [{"parts": [{"text": "..."}]}], "generationConfig": {...} }

        let messages = req
            .body
            .get("messages")
            .and_then(|m| m.as_array())
            .ok_or_else(|| CoreError::Internal("missing messages field".to_string()))?;

        // 转换 messages 到 Gemini contents 格式
        let mut contents = Vec::new();
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");

            let content = msg.get("content").and_then(|c| c.as_str()).unwrap_or("");

            // Gemini 使用 "user" 和 "model"（不是 "assistant"）
            let gemini_role = if role == "assistant" { "model" } else { "user" };

            contents.push(json!({
                "role": gemini_role,
                "parts": [{
                    "text": content
                }]
            }));
        }

        // 构建 Gemini 请求
        let mut gemini_req = json!({
            "contents": contents
        });

        // 转换 generationConfig
        let mut generation_config = json!({});
        if let Some(temp) = req.body.get("temperature") {
            generation_config["temperature"] = temp.clone();
        }
        if let Some(top_p) = req.body.get("top_p") {
            generation_config["topP"] = top_p.clone();
        }
        if let Some(max_tokens) = req.body.get("max_tokens") {
            generation_config["maxOutputTokens"] = max_tokens.clone();
        }
        if let Some(obj) = generation_config.as_object() {
            if !obj.is_empty() {
                gemini_req["generationConfig"] = generation_config;
            }
        }

        Ok(gemini_req)
    }

    fn transform_response(&self, resp: &ProviderResponse) -> Result<Value, CoreError> {
        // Gemini 响应格式转换为 OpenAI 格式
        // Gemini 流式：data: { "candidates": [{"content": {"parts": [{"text": "..."}]}}] }
        // OpenAI 流式：data: { "choices": [{"delta": {"content": "..."}}] }

        if resp.is_stream {
            // 流式：handler 已剥离 `data:` 前缀，这里收到的是单个纯 JSON chunk，
            // 转换后返回单个 OpenAI Value（不包含 data: 前缀，避免二次包装）。
            let chunk: Value = serde_json::from_slice(&resp.body).map_err(|e| {
                CoreError::Internal(format!("failed to parse Gemini SSE chunk: {e}"))
            })?;
            Ok(self
                .transform_streaming_chunk(&chunk)
                .unwrap_or(Value::Null))
        } else {
            // 非流式响应
            let gemini_resp: Value = serde_json::from_slice(&resp.body).map_err(|e| {
                CoreError::Internal(format!("failed to parse Gemini response: {e}"))
            })?;

            // 转换 Gemini 响应到 OpenAI 格式
            // Gemini: { "candidates": [{"content": {"parts": [{"text": "..."}]}}], "usageMetadata": {...} }
            // OpenAI: { "choices": [{"message": {"content": "..."}}], "usage": {...} }

            let content = gemini_resp
                .get("candidates")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("content"))
                .and_then(|c| c.get("parts"))
                .and_then(|p| p.as_array())
                .and_then(|arr| arr.first())
                .and_then(|p| p.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let openai_resp = json!({
                "id": "gemini-resp",
                "object": "chat.completion",
                "created": 0,
                "model": "gemini",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": content
                    },
                    "finish_reason": "stop"
                }],
                "usage": map_gemini_usage(gemini_resp.get("usageMetadata"))
            });

            Ok(openai_resp)
        }
    }

    fn api_path(&self) -> &str {
        // Gemini API 路径需要动态构建：/v1beta/models/{model}:generateContent
        // 但 trait 要求返回 &str，所以这里返回默认值，实际使用时通过 build_url 覆盖
        "/v1beta/models/gemini-pro:generateContent"
    }

    fn build_url(&self, req: &ProviderRequest) -> Result<String, CoreError> {
        let base = req.base_url.trim_end_matches('/');
        let model = &req.model;
        Ok(format!("{}/v1beta/models/{}:generateContent", base, model))
    }
}

impl GeminiProvider {
    /// 转换 Gemini 流式 chunk 到 OpenAI 格式
    fn transform_streaming_chunk(&self, chunk: &Value) -> Option<Value> {
        let text = chunk
            .get("candidates")?
            .as_array()?
            .first()?
            .get("content")?
            .get("parts")?
            .as_array()?
            .first()?
            .get("text")?
            .as_str()?;

        Some(json!({
            "id": "",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "gemini",
            "choices": [{
                "index": 0,
                "delta": {
                    "content": text
                },
                "finish_reason": null
            }]
        }))
    }
}

/// 将 Gemini usageMetadata 映射为 OpenAI 兼容的 usage 结构。
///
/// Gemini 返回 `{promptTokenCount, candidatesTokenCount, totalTokenCount}`，
/// OpenAI 期望 `{prompt_tokens, completion_tokens, total_tokens}`。字段缺失时回退到 0。
fn map_gemini_usage(raw: Option<&Value>) -> Value {
    let prompt = raw
        .and_then(|u| u.get("promptTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let completion = raw
        .and_then(|u| u.get("candidatesTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let total = raw
        .and_then(|u| u.get("totalTokenCount"))
        .and_then(|v| v.as_u64())
        .unwrap_or(prompt + completion);
    json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::providers::ProviderResponse;

    #[test]
    fn non_stream_usage_mapped() {
        let resp = ProviderResponse {
            body: serde_json::to_vec(&json!({
                "candidates": [{"content": {"parts": [{"text": "hi"}]}}],
                "usageMetadata": {
                    "promptTokenCount": 7,
                    "candidatesTokenCount": 3,
                    "totalTokenCount": 10
                }
            }))
            .unwrap()
            .into(),
            status: 200,
            is_stream: false,
        };
        let p = GeminiProvider;
        let out = p.transform_response(&resp).unwrap();
        assert_eq!(out["usage"]["prompt_tokens"], 7);
        assert_eq!(out["usage"]["completion_tokens"], 3);
        assert_eq!(out["usage"]["total_tokens"], 10);
    }

    #[test]
    fn stream_chunk_returns_single_value_no_data_prefix() {
        let resp = ProviderResponse {
            body: serde_json::to_vec(&json!({
                "candidates": [{"content": {"parts": [{"text": "xyz"}]}}]
            }))
            .unwrap()
            .into(),
            status: 200,
            is_stream: true,
        };
        let p = GeminiProvider;
        let out = p.transform_response(&resp).unwrap();
        assert_eq!(out["choices"][0]["delta"]["content"], "xyz");
        assert!(!out.to_string().starts_with("data:"));
        // 不需要的事件应返回 Null（handler 会跳过）
        let skip = ProviderResponse {
            body: serde_json::to_vec(&json!({})).unwrap().into(),
            status: 200,
            is_stream: true,
        };
        assert!(p.transform_response(&skip).unwrap().is_null());
    }
}
