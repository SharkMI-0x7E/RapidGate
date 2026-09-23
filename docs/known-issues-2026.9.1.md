# RapidGate — 已知问题与待办清单（2026.9.1 · 深度勘察版）

> 用途：发布前的整改备忘。由用户在 2026-09-23 委托巡检后整理。
> 说明：`cargo check --all-features` 已通过（exit 0），但大量功能"编译通过 ≠ 能用"。
> 状态：仅记录，未开始整改。整改完成一项勾一项。

---

## 0. 发布状态（最高优先级）

- ❌ crates.io 上最新版是 `v0.1.2`：**3 SLoC 的空壳 crate**，README 写着 *"This crate is still empty (I'm learning Rust)"*。有 8 人下载了这个空盒子。真正的网关代码从未发布。
- ✅ 本地 Cargo.toml 已更新为 `version = "2026.9.1"`。
- ⚠️ 发布依赖 §3~§5 的整改，否则发出去的网关是"裸奔 + 启动失败"二选一。

---

## 1. 不该提交且已提交的文件（必须从 git 历史移除）

以下文件**已经进了 git**，它们是 AI 工作产物，提交了没人看：

| 文件 | 性质 | 建议 |
| --- | --- | --- |
| `claude.md` | Claude Code AI 行为准则 | 从 git 历史移除，加入 `.gitignore` |
| `docs/rapidgate-prompt.md` | AI 分阶段作业提示词文件 | 从 git 历史移除，加入 `.gitignore` |
| `docs/rapidgate-stage3.md` | AI 阶段三任务书 | 从 git 历史移除，加入 `.gitignore` |
| `AGENTS.md` | 协作规范（人 + AI 共用） | 保留 |

保留：`docs/rapidgate-spec.md` / `docs/ARCHITECTURE.md` / `docs/OPERATIONS.md` / `docs/CI-CD-SETUP.md`（给人看）。

### 尚未提交但绝不能提交的（本地有，git 未跟踪）

`.claude/` `.cursor/` `.gemini/` `.kiro/` `.codegraph/` `GEMINI.md` `opencode.jsonc` `.mcp.json` `docs/provider-integration-plan.md`

---

## 2. 启动即失败：v2 配置与代码结构不匹配（新增 · 高危）

`config/routes/v2.yaml` 里大量字段在**代码结构体中不存在**，且所有配置结构体都标了 `#[serde(deny_unknown_fields)]`，一旦被加载直接反序列化报错：

| v2.yaml 字段 | 代码结构体 | 结果 |
| --- | --- | --- |
| upstream `max_retries: 3` | `UpstreamConfig` 无此字段（[upstream.rs](../src/core/config/upstream.rs)） | ❌ 解析失败 |
| route `canary:` 段 | `RouteConfig` 无此字段（[route.rs](../src/core/config/route.rs#L10-L19)） | ❌ 解析失败 |
| match `cookies:` 段 | `MatchRule` 无此字段（[route.rs](../src/core/config/route.rs#L24-L33)） | ❌ 解析失败 |

而 `collect_routes`（[config_loader.rs](../src/service/config_loader.rs#L102-L116)）会把 `config/routes/*.yaml` **全部合并读入**——`v1.yaml` + `v2.yaml` 同时存在 → **启动时配置加载失败，服务起不来**。

> ⚠️ 结论：**当前代码在 `config/routes/` 有 v2.yaml 的情况下无法启动**。必须二选一：删除 v2.yaml，或给结构体补字段（canary/cookies/max_retries）。`experiments/` 子目录不会被 `read_dir` 读到（不递归），但该文件 schema 与 `LoadedConfig` 完全不搭，纯摆设。

---

## 3. 鉴权 = 零（高危：网关裸奔）

- `src/service/middleware/auth.rs::auth_middleware` 注释自认"此处简化：始终通过"，函数体直接 `next.run(req).await` **放行一切**（[auth.rs](../src/service/middleware/auth.rs#L13-L18)）。
- `extract_credential` 写了，但 `auth_middleware` 从不调用它。
- 该中间件**未挂载**到 [server.rs](../src/service/server.rs#L63-L75)。
- `handler.rs` 的 `resolve_route` 拿到 route 后**从不检查** `route.auth.kind`。

→ 无论路由配置 `auth: bearer/apikey`，请求全部裸放。

## 4. 限流 = 零（高危）

- `src/service/middleware/ratelimit.rs::ratelimit_middleware` 同样"阶段二框架"直接放行。
- 未挂载到 server.rs。
- `AppState` 有 `limiters: LimiterCache` 缓存和 `default_rate_limit`，但**无任何代码初始化 limiter 或调用 `RateLimiter::check`**。
- `core/ratelimit/` 下 `TokenBucket` / `SlidingWindow` / `RedisRateLimiter` / `LocalStore` 全是孤立实现：**没有任何生产调用点**（LocalStore 注释自认"阶段一占位"且无人用）。

## 5. SSRF 防护写了但没生效（红线违规 · 高危）

- `src/service/upstream_pool.rs` 完整实现了 `check_ssrf`（allowlist + DNS 解析 + 私有网段 IP 拦截）**但从未被调用**——全项目无一行 `UpstreamPool::` 生产调用。
- [handler.rs](../src/service/handler.rs#L210-L221) 转发时直接 `reqwest::Client::new()` 裸发，绕过了 connection pool、超时配置、SSRF 校验。
- `AppState.upstreams`（客户端 cache）从未被使用。
- `config/default.yaml` 的 allowlist 里白名单 `127.0.0.1` 与 **AGENTS §4.3 默认 blocklist 要求拦截回环地址自相矛盾**（即便校验生效也会放飞行不通的地址）。本地测试将无法使用 `127.0.0.1` 上游——需在 allowlist 之外重新设计（如 local provider 例外）。

## 6. Provider 转发逻辑错误（高危：能跑但答错题）

- **embeddings 会被转发到 chat 端点**：`OpenAIProvider::api_path()` 硬编码 `/v1/chat/completions`，[handler.rs](../src/service/handler.rs#L34-L43) 的 `embeddings` 走同一套 `forward_streaming` → POST `/v1/embeddings` 实际发往上游 `/v1/chat/completions`。
- **SSE 流式双重包装**：`transform_streaming_chunk` 返回的已是完整 SSE chunk（`data: {...}`），[handler.rs L306](../src/service/handler.rs#L304-L313) 又包一层 `data: {}\n\n` → 上游发来的 `data:` 前缀进不了 provider 解析（provider 内部 `strip_prefix("data: ")`，而 handler 只传了 JSON 纯文本）→ **Anthropic / Gemini 流式响应恒为空**。
- Anthropic `transform_request` 把 `system` role 直接改写为 `user`（[anthropic.rs L40-L44](../src/service/providers/anthropic.rs#L40-L44)），system prompt 语义丢失。
- Gemini / Anthropic 非流式与 OpenAI 兼容转换仅做了字段映射，`usage` / 错误体等结构不符原协议。

## 7. 中间件 / admin / 可观测性全线未接线

- admin API（[routes.rs](../src/service/admin/routes.rs)）实现了 `/admin/routes|upstreams|limits|config` 4 个端点 + 鉴权，**main.rs / server.rs 未 merge** → 端口 9090 不存在。
- panic recovery / 审计中间件存在于 `middleware/` 但都未挂载。
- [server.rs L37-L60](../src/service/server.rs#L37-L60) body 限制**硬编码 10MB**，与 `config/default.yaml` 的 `max_body_bytes: 52428800`（50MB）不一致，且不从 AppState 取。
- Prometheus metrics / OTel 有实现无 `/metrics` 端点、无 init 调用。
- `state.rs` 创建了 `audit_tx` 通道但 `_audit_rx` 直接丢弃（`let (audit_tx, _audit_rx) = ...`）→ 审计事件发了没人收。

## 8. 占位 / 空壳实现清单（写了没有做）

| 位置 | 问题 |
| --- | --- |
| `src/core/plugins/wasm.rs` | `WasmPluginLoader` 空壳，`load_from_wasm` 直接 `Err("not yet implemented")`；白背 `wasmtime 19.0` 重依赖 |
| `src/core/plugins/native.rs` | `NativePluginLoader::load_from_library` 同样是"not yet implemented"骨架，且注释含 `# Safety` 却无任何 unsafe 实际逻辑 |
| `src/service/config_center/etcd.rs` | etcd-client 依赖被注释（需 protoc），所有操作直接报错，纯 stub |
| `src/core/proxy/transformer.rs` | `transform_request` 原样透传，**无人调用**，死代码 |
| `src/core/ratelimit/local_store.rs` | `LocalStore` 无人调用，注释自认"阶段一占位" |
| `src/service/handler.rs` L353 | `_force_use_body()` `#[allow(dead_code)]` 挂死代码 |
| `tests/hot_reload.rs` | 空测试（仅注释"验证可编译"），函数体为空 |
| `Cargo.toml` | `etcd-client`、`goose` 注释（"暂时移除"）半途而废；`async-graphql` 在依赖里但 admin 无 GraphQL 用法 |

## 9. 依赖与规范冲突

- Cargo.toml 已含 `prometheus` / `opentelemetry*` / `redis` / `wasmtime` / `oauth2` / `async-graphql` 等（spec §3.3 括弧"阶段三新增"），**与 AGENTS.md §7.1 "禁止添加"的描述冲突**（AGENTS 禁止列表 vs spec 允许列表，两份文档打架，需统一）。
- 新增依赖大多无实际调用方（dead dependency）：`redis`（store 无调用）、`oauth2`（有实现无路由无挂载）、`wasmtime`（空壳）、`async-graphql`（未用）、`clap`（bin/cli.rs 是否用到待确认）。

## 10. 测试虚假繁荣

- e2e 只测了 `/healthz` 和"无路由返回 404/401"，**无一条真实转发测试**。
- canary / plugins / failover 等 tests 全是单元级构造测试，不经过 server.rs 主链路。
- 无鉴权、限流、SSRF、流式、admin 的端到端测试。

---

## 11. 需要用户确认的设计决策

1. `v2.yaml` 与代码结构不匹配：删 v2.yaml 还是补结构体字段（canary/cookies/max_retries）？
2. AGENTS §7.1 与 spec §3.3 对依赖的约定冲突，以哪份为准？
3. crates.io 旧 `0.1.x` 空壳无法删除，只能发布新版本覆盖 → 是否接受？
4. WASM / ETCD / Redis / GraphQL 短期是否全部摘除依赖，避免白背？
5. `127.0.0.1` 本地 upstream 与 SSRF blocklist 冲突如何处理（本地开发例外配置）？
6. 不该提交的文档移除是否动用远程仓库（`git push --force` 需明确授权）？

## 12. 整改顺序建议（勾选即完成）

- [x] 配置结构体补齐（canary/cookies/max_retries 已支持，v2.yaml 可加载）
- [x] 中间件接线（auth 真正校验 / ratelimit 真正限流 / recovery / audit 消费者）
- [x] **SSRF + 连接池接入真实转发路径（红线）**
- [x] 修正 provider 转发：embeddings 独立 api_path、SSE 流式单层包装、system role 保留
- [x] admin API 挂载到独立端口 127.0.0.1:9090
- [x] `/metrics` 端点 + OTel 初始化（默认 noop）
- [x] 空壳补完（transformer 接入 / local_store 默认后端 / wasm / native / etcd feature-gated）
- [x] 依赖与文档统一（AGENTS §7.1 改为以 spec §3.3 为准）
- [x] body limit 从 AppState 读取
- [ ] git 历史清理（§1）——按用户决策：不改历史、不删文件，仅加 `.gitignore`
- [x] `.gitignore` 补全
- [x] cargo 六步自检 + 补 e2e 测试（鉴权 / 限流 / SSRF / 流式 / admin）
- [x] `cargo package --list` 验证发布内容（AI 文档已用 exclude 排除）
- [ ] 发布 `2026.9.1` 到 crates.io（待用户批准 + 凭据）