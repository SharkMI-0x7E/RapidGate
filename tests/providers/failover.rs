//! Provider 故障转移集成测试（spec §2 [S3]）
//!
//! 测试多 Provider 场景下的故障检测与切换逻辑。

use bytes::Bytes;
use serde_json::{json, Value};

use rapidgate::core::breaker::{Breaker, BreakerState};
use rapidgate::core::config::ProviderKind;
use rapidgate::core::error::CoreError;
use rapidgate::service::providers::anthropic::AnthropicProvider;
use rapidgate::service::providers::gemini::GeminiProvider;
use rapidgate::service::providers::local::LocalProvider;
use rapidgate::service::providers::openai::OpenAIProvider;
use rapidgate::service::providers::{Provider, ProviderFactory, ProviderRequest, ProviderResponse};

// -------------------- 辅助构造 --------------------

fn make_request(body: Value, stream: bool) -> ProviderRequest {
    ProviderRequest {
        body,
        base_url: "https://api.example.com".to_string(),
        api_key: "sk-test-key-1234567890abcdef".to_string(),
        model: "test-model".to_string(),
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

// -------------------- Provider 工厂测试 --------------------

#[test]
fn provider_factory_creates_openai() {
    let provider = ProviderFactory::create(ProviderKind::OpenAI);
    assert_eq!(provider.kind(), ProviderKind::OpenAI);
}

#[test]
fn provider_factory_creates_anthropic() {
    let provider = ProviderFactory::create(ProviderKind::Anthropic);
    assert_eq!(provider.kind(), ProviderKind::Anthropic);
}

#[test]
fn provider_factory_creates_gemini() {
    let provider = ProviderFactory::create(ProviderKind::Gemini);
    assert_eq!(provider.kind(), ProviderKind::Gemini);
}

#[test]
fn provider_factory_creates_local() {
    let provider = ProviderFactory::create(ProviderKind::Local);
    assert_eq!(provider.kind(), ProviderKind::Local);
}

// -------------------- Anthropic Provider 测试 --------------------

#[test]
fn anthropic_transform_request_adds_max_tokens() {
    let provider = AnthropicProvider;
    let req = make_request(
        json!({
            "model": "claude-3",
            "messages": [
                {"role": "user", "content": "Hello"}
            ]
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    assert_eq!(result["max_tokens"], 4096);
}

#[test]
fn anthropic_transform_request_preserves_system_role() {
    let provider = AnthropicProvider;
    let req = make_request(
        json!({
            "model": "claude-3",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ]
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    let messages = result["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system"); // 保留 system role（spec 要求）
}

#[test]
fn anthropic_api_path() {
    let provider = AnthropicProvider;
    assert_eq!(provider.api_path(), "/v1/messages");
}

#[test]
fn anthropic_transform_response_non_stream() {
    let provider = AnthropicProvider;
    let resp = make_response(
        json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello from Claude!"}
            ],
            "model": "claude-3-opus-20240229",
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 20
            }
        }),
        200,
        false,
    );

    let result = provider.transform_response(&resp).unwrap();
    assert_eq!(result["object"], "chat.completion");
    assert_eq!(result["choices"][0]["message"]["content"], "Hello from Claude!");
    assert_eq!(result["choices"][0]["finish_reason"], "stop");
}

// -------------------- Gemini Provider 测试 --------------------

#[test]
fn gemini_transform_request_converts_messages() {
    let provider = GeminiProvider;
    let req = make_request(
        json!({
            "model": "gemini-pro",
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi!"},
                {"role": "user", "content": "How are you?"}
            ]
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    let contents = result["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(contents[1]["role"], "model"); // assistant -> model
    assert_eq!(contents[2]["role"], "user");
}

#[test]
fn gemini_transform_request_converts_generation_config() {
    let provider = GeminiProvider;
    let req = make_request(
        json!({
            "model": "gemini-pro",
            "messages": [],
            "temperature": 0.8,
            "top_p": 0.95,
            "max_tokens": 500
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    let config = &result["generationConfig"];
    assert_eq!(config["temperature"], 0.8);
    assert_eq!(config["topP"], 0.95);
    assert_eq!(config["maxOutputTokens"], 500);
}

#[test]
fn gemini_api_path() {
    let provider = GeminiProvider;
    assert!(provider.api_path().contains("gemini-pro"));
    assert!(provider.api_path().contains("generateContent"));
}

#[test]
fn gemini_transform_response_non_stream() {
    let provider = GeminiProvider;
    let resp = make_response(
        json!({
            "candidates": [{
                "content": {
                    "parts": [{"text": "Hello from Gemini!"}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 20,
                "totalTokenCount": 30
            }
        }),
        200,
        false,
    );

    let result = provider.transform_response(&resp).unwrap();
    assert_eq!(result["object"], "chat.completion");
    assert_eq!(result["choices"][0]["message"]["content"], "Hello from Gemini!");
}

// -------------------- Local Provider 测试 --------------------

#[test]
fn local_provider_kind() {
    let provider = LocalProvider;
    assert_eq!(provider.kind(), ProviderKind::Local);
}

#[test]
fn local_transform_request_preserves_body() {
    let provider = LocalProvider;
    let req = make_request(
        json!({
            "model": "llama2",
            "messages": [{"role": "user", "content": "Hello"}]
        }),
        false,
    );

    let result = provider.transform_request(&req).unwrap();
    assert_eq!(result["model"], "llama2");
}

#[test]
fn local_api_path() {
    let provider = LocalProvider;
    assert_eq!(provider.api_path(), "/v1/chat/completions");
}

// -------------------- 熔断器与故障转移 --------------------

#[tokio::test]
async fn breaker_opens_after_consecutive_failures() {
    let breaker = Breaker::new("test-upstream", 3, 1000);

    // 连续失败 3 次
    for _ in 0..3 {
        let _ = breaker
            .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("connection failed".into())) })
            .await;
    }

    assert_eq!(breaker.state(), BreakerState::Open);
}

#[tokio::test]
async fn breaker_rejects_requests_when_open() {
    let breaker = Breaker::new("test-upstream", 2, 5000);

    // 触发熔断
    for _ in 0..2 {
        let _ = breaker
            .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("fail".into())) })
            .await;
    }

    // 后续请求应被拒绝
    let result = breaker.call(async { Ok::<_, CoreError>(()) }).await;
    assert!(matches!(result, Err(CoreError::BreakerOpen(_))));
}

#[tokio::test]
async fn breaker_transitions_to_half_open_after_timeout() {
    let breaker = Breaker::new("test-upstream", 1, 50); // 50ms 超时

    // 触发熔断
    let _ = breaker
        .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("fail".into())) })
        .await;
    assert_eq!(breaker.state(), BreakerState::Open);

    // 等待超时
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;

    // 应进入 HalfOpen 状态
    let result = breaker.call(async { Ok::<_, CoreError>(()) }).await;
    assert!(result.is_ok());
    assert_eq!(breaker.state(), BreakerState::Closed);
}

#[tokio::test]
async fn breaker_closes_on_success_after_half_open() {
    let breaker = Breaker::new("test-upstream", 1, 10);

    // 触发熔断
    let _ = breaker
        .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("fail".into())) })
        .await;

    // 等待进入 HalfOpen
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // 成功请求应关闭熔断器
    let result = breaker.call(async { Ok::<_, CoreError>(()) }).await;
    assert!(result.is_ok());
    assert_eq!(breaker.state(), BreakerState::Closed);
}

#[tokio::test]
async fn breaker_reopens_on_failure_in_half_open() {
    let breaker = Breaker::new("test-upstream", 1, 10);

    // 触发熔断
    let _ = breaker
        .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("fail".into())) })
        .await;

    // 等待进入 HalfOpen
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // 失败请求应重新打开熔断器
    let result = breaker
        .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("fail again".into())) })
        .await;
    assert!(result.is_err());
    assert_eq!(breaker.state(), BreakerState::Open);
}

// -------------------- 多 Provider 故障转移场景 --------------------

#[tokio::test]
async fn failover_to_secondary_provider() {
    // 模拟主 Provider 熔断，切换到备用 Provider
    let primary_breaker = Breaker::new("primary", 1, 10000);
    let secondary_breaker = Breaker::new("secondary", 5, 1000);

    // 主 Provider 熔断
    let _ = primary_breaker
        .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("primary down".into())) })
        .await;
    assert_eq!(primary_breaker.state(), BreakerState::Open);

    // 备用 Provider 正常
    let result = secondary_breaker.call(async { Ok::<_, CoreError>(()) }).await;
    assert!(result.is_ok());
    assert_eq!(secondary_breaker.state(), BreakerState::Closed);
}

#[tokio::test]
async fn all_providers_down_returns_error() {
    let breakers = vec![
        Breaker::new("p1", 1, 10000),
        Breaker::new("p2", 1, 10000),
        Breaker::new("p3", 1, 10000),
    ];

    // 所有 Provider 熔断
    for breaker in &breakers {
        let _ = breaker
            .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("down".into())) })
            .await;
    }

    // 尝试请求应全部失败
    for breaker in &breakers {
        let result = breaker.call(async { Ok::<_, CoreError>(()) }).await;
        assert!(matches!(result, Err(CoreError::BreakerOpen(_))));
    }
}

#[tokio::test]
async fn failover_recovers_when_primary_restored() {
    let primary_breaker = Breaker::new("primary", 1, 50);
    let secondary_breaker = Breaker::new("secondary", 5, 10000);

    // 主 Provider 熔断
    let _ = primary_breaker
        .call(async { Err::<(), _>(CoreError::UpstreamUnreachable("down".into())) })
        .await;

    // 切换到备用
    let result = secondary_breaker.call(async { Ok::<_, CoreError>(()) }).await;
    assert!(result.is_ok());

    // 等待主 Provider 恢复
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;

    // 主 Provider 应进入 HalfOpen，成功请求后关闭
    let result = primary_breaker.call(async { Ok::<_, CoreError>(()) }).await;
    assert!(result.is_ok());
    assert_eq!(primary_breaker.state(), BreakerState::Closed);
}
