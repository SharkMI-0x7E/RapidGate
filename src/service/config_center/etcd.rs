//! ETCD configuration center backend
//!
//! 默认构建（不带 `config-center-etcd` feature）时为 stub 实现，所有操作返回明确错误。
//! 启用该 feature（`--features config-center-etcd`）会引入 `etcd-client` 依赖（需要 protoc）；
//! 真实实现应在此处以 `#[cfg(feature = "config-center-etcd")]` 包裹 etcd-client 调用。

use std::pin::Pin;

use async_trait::async_trait;
use futures::stream::Stream;

use super::ConfigCenter;
use crate::core::error::CoreError;

/// ETCD connection configuration
#[derive(Debug, Clone)]
pub struct EtcdConfig {
    /// ETCD endpoint list
    pub endpoints: Vec<String>,
    /// Optional username for authentication
    pub username: Option<String>,
    /// Optional password for authentication
    pub password: Option<String>,
    /// Configuration prefix (default: "/rapidgate/config")
    pub prefix: String,
}

impl Default for EtcdConfig {
    fn default() -> Self {
        Self {
            endpoints: vec!["http://localhost:2379".to_string()],
            username: None,
            password: None,
            prefix: "/rapidgate/config".to_string(),
        }
    }
}

/// ETCD configuration center
pub struct EtcdConfigCenter {
    config: EtcdConfig,
}

impl EtcdConfigCenter {
    /// Create an ETCD configuration center instance
    pub fn new(config: EtcdConfig) -> Self {
        tracing::warn!("ETCD config center created but etcd-client is not compiled in; operations will return errors");
        Self { config }
    }

    /// Get reference to ETCD configuration
    pub fn config(&self) -> &EtcdConfig {
        &self.config
    }
}

#[async_trait]
impl ConfigCenter for EtcdConfigCenter {
    async fn fetch(&self, key: &str) -> Result<String, CoreError> {
        let full_key = format!("{}/{}", self.config.prefix, key);
        tracing::warn!(key = %full_key, "ETCD fetch attempted but etcd-client is not compiled in");
        Err(CoreError::Config(
            "ETCD config center requires the 'config-center-etcd' feature and protoc at build time"
                .to_string(),
        ))
    }

    async fn watch(
        &self,
        key: &str,
    ) -> Result<Pin<Box<dyn Stream<Item = String> + Send>>, CoreError> {
        tracing::warn!(key = %key, "ETCD watch attempted but etcd-client is not compiled in");
        Err(CoreError::Config(
            "ETCD config center requires the 'config-center-etcd' feature and protoc at build time"
                .to_string(),
        ))
    }
}
