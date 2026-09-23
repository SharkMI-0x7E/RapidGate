//! core/config — 配置数据模型（仅 serde 定义，不负责加载）

pub mod gateway;
pub mod provider;
pub mod route;
pub mod upstream;

pub use gateway::{GatewayConfig, LoggingConfig, SsrfConfig, UpstreamAllowlist};
pub use provider::{ProviderConfig, ProviderKind};
pub use route::{AuthConfig, HeaderMatch, MatchRule, QueryMatch, RateLimitConfig, RouteConfig};
pub use upstream::{LoadBalancer, UpstreamConfig, UpstreamId, UpstreamPoolConfig};
