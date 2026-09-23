//! OpenAI Provider 实现
//!
//! 支持 OpenAI API 格式，包括 GPT 系列模型。
//! 阶段三新增（spec §2 [S3]）。

use async_trait::async_trait;
use serde_json::Value;

use crate::core::config::ProviderKind;
use crate::core::error::CoreError;
use crate::service::providers::{Provider, ProviderRequest, ProviderResponse};

/// OpenAI Provider
pub struct OpenAIProvider;

#[async_trait]
impl Provider for OpenAIProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenAI
    }

    fn transform_request(&self, req: &ProviderRequest) -> Result<Value, CoreError> {
        // OpenAI 格式：直接使用原始请求
        // {
        //   "model": "gpt-4",
        //   "messages": [...],
        //   "stream": true
        // }
        Ok(req.body.clone())
    }

    fn transform_response(&self, resp: &ProviderResponse) -> Result<Value, CoreError> {
        // OpenAI 响应格式：直接返回
        // 非流式：{ "id": "...", "choices": [...], "usage": {...} }
        // 流式：SSE chunks，每个 chunk 格式相同
        if resp.is_stream {
            // 流式响应已经是 SSE 格式，直接返回
            Ok(serde_json::from_slice(&resp.body).unwrap_or(Value::Null))
        } else {
            // 非流式响应，解析 JSON
            serde_json::from_slice(&resp.body)
                .map_err(|e| CoreError::Internal(format!("failed to parse OpenAI response: {e}")))
        }
    }

    fn api_path(&self) -> &str {
        "/v1/chat/completions"
    }

    fn embedding_path(&self) -> &str {
        "/v1/embeddings"
    }
}
