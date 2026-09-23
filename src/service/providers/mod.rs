//! Provider 适配层
//!
//! 负责将不同 LLM Provider 的请求/响应格式转换为统一的内部格式。
//! 阶段三新增（spec §2 [S3]）。

pub mod anthropic;
pub mod gemini;
pub mod local;
pub mod openai;

use async_trait::async_trait;
use bytes::Bytes;
use serde_json::Value;

use crate::core::config::provider::ProviderKind;
use crate::core::error::CoreError;

/// Provider 请求上下文
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    /// 原始请求体（JSON）
    pub body: Value,
    /// 上游 base_url
    pub base_url: String,
    /// API Key
    pub api_key: String,
    /// 模型名称
    pub model: String,
    /// 是否流式
    pub stream: bool,
    /// 操作类型："chat" | "embeddings"（决定构建 /v1/chat/completions 还是 /v1/embeddings）
    pub operation: String,
}

/// Provider 响应上下文
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    /// 响应体（JSON 或 SSE chunk）
    pub body: Bytes,
    /// HTTP 状态码
    pub status: u16,
    /// 是否流式响应
    pub is_stream: bool,
}

/// Provider trait
///
/// 所有 LLM Provider 必须实现此 trait，负责协议转换。
#[async_trait]
pub trait Provider: Send + Sync {
    /// Provider 类型
    fn kind(&self) -> ProviderKind;

    /// 将统一请求转换为 Provider 特定格式
    fn transform_request(&self, req: &ProviderRequest) -> Result<Value, CoreError>;

    /// 将 Provider 响应转换回统一格式
    fn transform_response(&self, resp: &ProviderResponse) -> Result<Value, CoreError>;

    /// 获取 Provider 的 API 路径（如 /v1/chat/completions）
    fn api_path(&self) -> &str;

    /// 获取 Provider 的 embedding API 路径（如 /v1/embeddings）
    ///
    /// 默认回退到 `api_path`，仅支持 embeddings 的 provider（如 OpenAI）需覆写。
    fn embedding_path(&self) -> &str {
        self.api_path()
    }

    /// 构建上游请求 URL
    fn build_url(&self, req: &ProviderRequest) -> Result<String, CoreError> {
        let base = req.base_url.trim_end_matches('/');
        let path = if req.operation == "embeddings" {
            self.embedding_path()
        } else {
            self.api_path()
        };
        Ok(format!("{base}{path}"))
    }
}

/// Provider 工厂
pub struct ProviderFactory;

impl ProviderFactory {
    /// 根据 ProviderKind 创建对应的 Provider 实例
    pub fn create(kind: ProviderKind) -> Box<dyn Provider> {
        match kind {
            ProviderKind::OpenAI => Box::new(openai::OpenAIProvider),
            ProviderKind::Anthropic => Box::new(anthropic::AnthropicProvider),
            ProviderKind::Gemini => Box::new(gemini::GeminiProvider),
            ProviderKind::Local => Box::new(local::LocalProvider),
        }
    }
}
