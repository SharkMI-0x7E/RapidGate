//! OpenAI 兼容 Provider 集成测试（spec §2 [S3]）
//!
//! 测试 OpenAI Provider 的请求/响应转换。

use bytes::Bytes;
use serde_json::{json, Value};

use rapidgate::core::config::ProviderKind;
use rapidgate::core::error::CoreError;
use rapidgate::service::providers::openai::OpenAIProvider;
use rapidgate::service::providers::{Provider, ProviderRequest, ProviderResponse};

// -------------------- 辅助构造 --------------------

fn make_request(body: Value, stream: bool) -> ProviderRequest {
    ProviderRequest {
        body,
        base_url: "https://api.openai.com".to_string(),
        api_key: "sk-test-key-1234567890abcdef".to_string(),
        model: "gpt-4".to_string(),
        stream,
        operation: "chat".to_string(),
    }
}

fn make_response(body: Value, status: u16, is_stream: bool) -> ProviderResponse {
    ProviderResponse {
        body: Bytes::from(body.to_string().into_bytes()),
        status,
        is_stream,
    }
}

// -------------------- Provider 元数据 --------------------

#[test]
fn openai_provider_kind() {
    let provider = OpenAIProvider;
    assert_eq!(provider.kind(), ProviderKind::OpenAI);
}

#[test]
fn openai_provider_api_path() {
    let provider = OpenAIProvider;
    assert_eq!(provider.api_path(), "/v1/chat/completions");
}

// -------------------- 请求转换 --------------------

#[test]
fn openai_transform_request_preserves_body() {
    let provider = OpenAIProvider;
    let req = make_request(
        json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "stream": true
        }),
        true,
    );

    let result = provider.transform_request(&req).unwrap();
    assert_eq!(result["model"], "gpt-4");
    assert!(result["messages"].is_array());
    assert_eq!(result["stream"], true);
}

#[test]
fn openai_transform_request_with_temperature() {
    let provider = OpenAIProvider;
    let req = make_request(
        json!({
            "model": "gpt-4",
            "messages": [],
            "temperature": 0.7,
            "top_p": 0.9
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    assert_eq!(result["temperature"], 0.7);
    assert_eq!(result["top_p"], 0.9);
}

#[test]
fn openai_transform_request_with_max_tokens() {
    let provider = OpenAIProvider;
    let req = make_request(
        json!({
            "model": "gpt-4",
            "messages": [],
            "max_tokens": 1000
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    assert_eq!(result["max_tokens"], 1000);
}

// -------------------- 响应转换 --------------------

#[test]
fn openai_transform_response_non_stream() {
    let provider = OpenAIProvider;
    let resp = make_response(
        json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello! How can I help you?"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        }),
        200,
        false,
    );

    let result = provider.transform_response(&resp).unwrap();
    assert_eq!(result["id"], "chatcmpl-123");
    assert_eq!(result["object"], "chat.completion");
    assert_eq!(result["choices"][0]["message"]["content"], "Hello! How can I help you?");
    assert_eq!(result["usage"]["total_tokens"], 30);
}

#[test]
fn openai_transform_response_stream() {
    let provider = OpenAIProvider;
    // 流式响应是 SSE 格式，但 OpenAI Provider 直接透传
    let stream_body = json!({
        "id": "chatcmpl-123",
        "object": "chat.completion.chunk",
        "created": 1677652288,
        "model": "gpt-4",
        "choices": [{
            "index": 0,
            "delta": {
                "content": "Hello"
            },
            "finish_reason": null
        }]
    });

    let resp = ProviderResponse {
        body: Bytes::from(stream_body.to_string().into_bytes()),
        status: 200,
        is_stream: true,
    };

    let result = provider.transform_response(&resp).unwrap();
    assert_eq!(result["choices"][0]["delta"]["content"], "Hello");
}

#[test]
fn openai_transform_response_invalid_json_returns_error() {
    let provider = OpenAIProvider;
    let resp = ProviderResponse {
        body: Bytes::from("invalid json"),
        status: 200,
        is_stream: false,
    };

    let result = provider.transform_response(&resp);
    assert!(result.is_err());
    assert!(matches!(result, Err(CoreError::Internal(_))));
}

// -------------------- URL 构建 --------------------

#[test]
fn openai_build_url() {
    let provider = OpenAIProvider;
    let req = make_request(json!({}), false);

    let url = provider.build_url(&req).unwrap();
    assert_eq!(url, "https://api.openai.com/v1/chat/completions");
}

#[test]
fn openai_build_url_with_trailing_slash() {
    let provider = OpenAIProvider;
    let mut req = make_request(json!({}), false);
    req.base_url = "https://api.openai.com/".to_string();

    let url = provider.build_url(&req).unwrap();
    assert_eq!(url, "https://api.openai.com/v1/chat/completions");
}

#[test]
fn openai_build_url_with_custom_base() {
    let provider = OpenAIProvider;
    let mut req = make_request(json!({}), false);
    req.base_url = "https://custom.api.com/v1".to_string();

    let url = provider.build_url(&req).unwrap();
    assert_eq!(url, "https://custom.api.com/v1/v1/chat/completions");
}

#[test]
fn openai_build_url_uses_embedding_path() {
    let provider = OpenAIProvider;
    let mut req = make_request(json!({"input": "hello"}), false);
    req.operation = "embeddings".to_string();

    let url = provider.build_url(&req).unwrap();
    assert_eq!(url, "https://api.openai.com/v1/embeddings");
}

#[test]
fn openai_embedding_path() {
    let provider = OpenAIProvider;
    assert_eq!(provider.embedding_path(), "/v1/embeddings");
}

// -------------------- 完整请求流程 --------------------

#[test]
fn openai_full_request_flow() {
    let provider = OpenAIProvider;

    // 1. 构建请求
    let req = make_request(
        json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello"}
            ],
            "temperature": 0.7,
            "stream": false
        }),
        false,
    );

    // 2. 转换请求
    let transformed_req = provider.transform_request(&req).unwrap();
    assert_eq!(transformed_req["model"], "gpt-4");

    // 3. 构建 URL
    let url = provider.build_url(&req).unwrap();
    assert!(url.ends_with("/v1/chat/completions"));

    // 4. 模拟响应
    let resp = make_response(
        json!({
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hi there!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 15,
                "completion_tokens": 5,
                "total_tokens": 20
            }
        }),
        200,
        false,
    );

    // 5. 转换响应
    let transformed_resp = provider.transform_response(&resp).unwrap();
    assert_eq!(transformed_resp["choices"][0]["message"]["content"], "Hi there!");
}
