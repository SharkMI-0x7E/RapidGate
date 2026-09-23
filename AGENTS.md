# AGENTS.md — rapidgate 协作规范

本文档面向所有参与 rapidgate 项目的开发者与 AI 编码助手，约束开发、协作、CI/CD 行为。

## 1. 提交规范

本项目遵循 `https://www.conventionalcommits.org/`，提交信息格式：

    <type>(<scope>): <subject>

允许的 type：

- `feat` — 新功能
- `fix` — Bug 修复
- `docs` — 仅文档变更
- `chore` — 构建/工具/杂项
- `ci` — CI/CD 配置变更
- `test` — 测试新增或修改
- `perf` — 性能优化
- `refactor` — 代码重构（非功能变更、非 Bug 修复）
- `style` — 代码格式（不影响语义）

### 1.1 Commit 粒度原则：**一个功能 / 一个修复 = 一个 commit**

> 这是对 AI 编码助手的**硬性要求**，对人类开发者也强烈推荐。

**核心思想**：每个 commit 必须是**原子单元**，可以独立 review、独立 revert、独立 cherry-pick、独立 release。

#### 对 AI 编码助手的硬性要求

- ✅ **每个独立功能点** = 1 个 commit（例如新增 OpenAI provider、加入限流中间件、修一个 bug）
- ✅ **每个依赖 / 工具变更** = 1 个 commit（例如 `chore: bump axum to 0.7.5`、`ci: add security workflow`）
- ✅ **每个文档 / 配置变更** = 1 个 commit（例如 `docs: update README`、`chore(release): bump to 2026.06.2`）
- ❌ **禁止**把多个不相关功能打包成 `feat: big update` 这种巨型 commit
- ❌ **禁止**一次提交里同时改业务代码 + 改 CI + 改文档（除非三者强耦合）
- ❌ **禁止**在 commit 消息里写"and"、"also"、"顺便"这种暗示多件事的连接词

#### 实际对比

**反面教材**（❌ 全部打包）：

```text
feat: implement gateway

- add OpenAI provider
- add Anthropic provider
- add rate limit middleware
- add retry middleware
- add 10 unit tests
- update README
- bump axum version
```

**正面教材**（✅ 每个功能一个 commit）：

```text
feat(provider): add OpenAI provider
feat(provider): add Anthropic provider
feat(middleware): add rate limit middleware
feat(middleware): add retry middleware
test(middleware): add rate limit unit tests
docs: document middleware pipeline
chore(deps): bump axum to 0.7.5
```

#### Commit 粒度自查清单

写完一组 commit 后，AI 应当自检：

- [ ] 这次 commit 只做一件事吗？
- [ ] 如果回滚这次 commit，会不会连带影响其他功能？
- [ ] commit 消息里的动词是单数（`add` / `fix` / `refactor`）而不是复数（`add multiple` / `fix various`）？
- [ ] scope 准确吗？（`provider` / `middleware` / `ci` / `docs` / `deps`）
- [ ] 如果这个 commit 单独发版，changelog 里能独立列一条吗？

#### 与 PR 的关系

- **推荐** 1 个 PR = 1 个 commit（PR 描述里说明背景，squash merge 后留下 1 个干净的 commit）
- 大型重构可以 **1 个 PR = 多个 commit**（按功能点拆开，merge 时用 merge commit 保留所有 commit 历史）
- **禁止** 1 个 PR = 1 个巨型 commit（哪怕只改了 1 个文件，只要逻辑上分多步就要分多 commit）

#### Commit 拆分提交模板（AI 用）

完成一组功能后，**先列清单给用户确认，再 `git add`**，禁止直接 commit：

```text
本次完成：[一句话功能描述]

计划拆分为 N 个 commit：

1. chore(deps): add <crate> = "<ver>"          # 依赖变更（如有，独立 commit）
2. feat(<scope>): <单数动词 + 具体对象>          # 核心功能
3. test(<scope>): add unit tests for <X>        # 测试
4. docs(<scope>): document <X>                  # 文档（如有）

请确认：
- [ ] commit 拆分粒度 OK？
- [ ] commit 标题符合 Conventional Commits（type 小写、scope 精确、单数动词）？
- [ ] 无遗漏文件、未夹带无关改动？
```

用户确认后**逐个 commit**（不要 `git add -A`），按顺序执行。**禁止**未经确认直接 `git commit`。**禁止**未经用户明确指示 `git push`。

## 2. PR 要求

**必须**通过 CI 所有检查：

- `fmt` 作业（`cargo fmt -- --check`）
- `lint-test` 作业（`cargo clippy --all-targets --all-features -- -D warnings`、`cargo build --all-targets --all-features`、`cargo test --all-features`）
- `unsafe-check` 作业
- `release-build` 作业

**代码硬性约束**：

- 禁止出现 `unsafe` 代码块（除非已充分评审并在 PR 描述中写明原因）
- 敏感配置（API key、token、base URL 等）必须通过环境变量读取，不得硬编码
- 遵循 `.github/PULL_REQUEST_TEMPLATE.md`，写清 What / Why / How / Testing

## 3. 构建说明

```bash
# 本地开发运行
cargo run

# 运行所有测试
cargo test --all-features

# 格式化检查
cargo fmt -- --check

# 静态分析
cargo clippy --all-targets --all-features -- -D warnings

# 发布构建
cargo build --release

# 依赖漏洞扫描
cargo install cargo-audit --locked
cargo audit

# 许可证/重复依赖/公告检查
cargo install cargo-deny --locked
cargo deny check
```

### 3.1 完成度自检（每组代码完成后必跑，**全部输出给用户看**）

```bash
# 1. 格式
cargo fmt -- --check

# 2. 静态分析
cargo clippy --all-targets --all-features -- -D warnings

# 3. 构建
cargo build --all-targets --all-features

# 4. 测试
cargo test --all-features

# 5. unsafe 检查
grep -rn "unsafe" src/ --include="*.rs" && echo "FAIL: unsafe found" || echo "PASS: no unsafe"

# 6. 敏感字符串检查
grep -rEn "(api[_-]?key|token|secret)\s*[:=]\s*\"[A-Za-z0-9_\-]{16,}\"" src/ --include="*.rs" && echo "FAIL: hardcoded secret" || echo "PASS: no hardcoded secret"
```

**任何一步失败** → 修复后重跑，**禁止**用"在我环境里能跑"敷衍。

## 4. 安全策略

- **依赖漏洞**：通过 `cargo audit`（GitHub Actions 中的 `audit` 作业）持续监控，每天 06:00 UTC 自动扫描
- **许可证合规**：通过 `cargo deny` 持续检查，禁止引入未在 `deny.toml` 白名单中的许可证
- **新增依赖评估**：必须评估其维护活跃度（最近 6 个月有提交）、依赖树影响（直接依赖数量、总编译时间）、许可证兼容性

### 4.1 敏感字段管理（硬约束）

- 任何 `api_key` / `token` / `secret` / `base_url` **禁止**硬编码在源码中
- 配置文件中使用 `${RGD_*}` 占位符由环境变量展开，**缺失则配置加载失败**（不 panic、保留旧配置）
- `.env` 文件**禁止**提交，仓库只保留 `.env.example`
- 配置文件示例（如 `config/routes/v1.yaml`）里所有敏感值**必须**是 `${RGD_XXX}` 形式，**禁止**出现形如 `sk-abcdef1234567890` 的真实值

### 4.2 API Key 与签名校验（硬约束）

- 字符串比较 API Key / token / 签名 / HMAC 等**敏感字符串**必须使用 `subtle::ConstantTimeEq`
- **禁止**用 `==` / `String::eq` 直接比较上述字段
- 错误响应**禁止**泄露是 "key 不存在" 还是 "key 错误"（统一返回 `unauthorized`）

### 4.3 上游 SSRF 防护（硬约束）

LLM 网关天然是 SSRF 跳板（用户配置 base_url → 内部网络），必须执行：

1. 解析 `base_url` 的 host，DNS 解析拿到所有 A/AAAA 记录
2. 检查每个 IP 是否在 `RGD_IP_BLOCKLIST`（CIDR 列表）内或属于私有 / 回环 / 链路本地段
3. 任一 IP 命中 → 拒绝请求，返回 `bad_request`，**不**发起上游连接
4. 默认 blocklist：`127.0.0.0/8`、`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`169.254.0.0/16`、`::1/128`、`fc00::/7`、`fe80::/10`
5. 真实请求时使用**校验后 IP** + `Host` Header（防 DNS rebinding）

具体实现细节见 [rapidgate-spec.md §8](file:///e:/pythonxiangmuwenjianjia/RapidGate/docs/rapidgate-spec.md)。

## 5. CI/CD 工作流说明

| 工作流 | 触发条件 | 作用 |
| --- | --- | --- |
| `ci.yml` | push/PR 到 main（排除 .md、docs/、.trae/） | 格式、clippy、test、unsafe 检查、发布构建冒烟 |
| `release.yml` | push to main（`Cargo.toml` 变更）或手动推送 `v*` tag | 自动读 `version` 打 tag + 多平台构建 + 创建 GitHub Release |
| `security.yml` | 每天 06:00 UTC + PR 改动 Cargo 文件 | `cargo audit` 漏洞扫描 + `cargo deny` 许可证检查 |

## 6. 版本号管理策略

**版本号完全由开发者手动控制**。本项目不依赖任何工具自动算号、提议号或合并发版 PR。

### 版本号格式：`YYYY.MM.N`

- `YYYY` — 4 位年份，例如 `2026`
- `MM` — 2 位月份，固定两位（`01`–`12`）
- `N` — 当月内的发布序号，从 `1` 开始自增

**示例**：

```text
2026.06.1   # 2026 年 6 月的第 1 个发布
2026.06.2   # 2026 年 6 月的第 2 个发布（修复或小迭代）
2026.07.1   # 2026 年 7 月的第 1 个发布（新月份，序号重置）
2026.06.1-rc.1  # 预发布版，会被自动标为 pre-release
```

> ⚠️ 当月内是否需要递增序号，由开发者自行判断。一般而言只要 `Cargo.toml` 的
> version 字段发生改变且你希望发布，就递增序号。

### 发版流程

1. **改版本号**：编辑 `Cargo.toml`，把 `version` 改成下一个 `YYYY.MM.N`
2. **本地校验**（强烈建议）：
   ```bash
   cargo fmt -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all-features
   ```
3. **提交并推送**到 `main`：
   ```bash
   git add Cargo.toml
   git commit -m "chore(release): bump to 2026.06.2"
   git push origin main
   ```
4. **CI 自动处理**：[release.yml](file:///e:/pythonxiangmuwenjianjia/RapidGate/.github/workflows/release.yml) 检测到 `Cargo.toml` 变更后：
   - 读取 `version` 字段，校验格式
   - 检查 `v{version}` tag 是否已存在（避免重复打 tag）
   - 自动创建并推送 `v{version}` tag
   - 触发多平台构建 + 创建 GitHub Release
5. **校验失败**怎么办：CI 会报错，提示格式不对或 tag 已存在，开发者改完重推即可

### 应急通道：手动打 tag

如果自动打 tag 链路出问题，可以绕过：

```bash
git tag v2026.06.2
git push origin v2026.06.2
```

[release.yml](file:///e:/pythonxiangmuwenjianjia/RapidGate/.github/workflows/release.yml) 的 `push tags` 触发条件会直接接管构建流程。

### 硬性约束

- **禁止**让任何工具自动修改 `Cargo.toml` 的 `version` 字段
- **禁止**用 `0.x.y` 这种 semver 格式（CI 会拒绝合并）
- **预发布版本**用 `-` 后缀，例如 `2026.06.1-rc.1`（必须同时保证父版本 `2026.06.1` 已存在）

## 7. 技术栈约束

- **Web 框架**：axum 0.7（路由、状态提取、响应）
- **异步运行时**：tokio（full features）
- **HTTP 客户端**：reqwest（json + stream）
- **日志**：tracing + tracing-subscriber
- **序列化**：serde / serde_json
- **Rust 版本**：stable（CI 还覆盖 beta 频道）

### 7.1 依赖锁定唯一源

`Cargo.toml` 的 `[dependencies]` 表是**依赖锁定的唯一权威**。允许并已批准的依赖列表（含"计划中 / 待接入"项）以 [docs/rapidgate-spec.md §3](file:///e:/pythonxiangmuwenjianjia/RapidGate/docs/rapidgate-spec.md) 的**允许列表**为准；AGENTS 不再维护独立的禁止列表。任何 AI / 开发者：

- 添加 spec §3 允许列表之外的新依赖前**必须**先更新 spec §3 表格，并经用户确认
- spec §3.3 允许列表中的 crate（prometheus / opentelemetry / redis / wasmtime / async-graphql 等）**允许保留**
- 升级版本号**必须**列在 commit 标题中体现，例如 `chore(deps): bump axum to 0.8.1`

## 8. AI 协作工作流

本节是 AI 编码助手（Claude / GPT / Trae Agent / Cursor 等）参与本项目的**标准工作流**。

### 8.1 开工前必读清单

动手前按顺序读这些文件：

1. `AGENTS.md`（本文档）— 提交规范、CI 约束、版本号、安全策略
2. `docs/rapidgate-spec.md` — 完整项目规格、目录树、依赖表、配置契约
3. `Cargo.toml` — 当前依赖，确认与 spec §3 一致
4. `deny.toml` — 许可证白名单
5. `.github/workflows/ci.yml` — 本地必须复现的 CI 检查

**禁止**在未读完以上文件的情况下写代码。

### 8.2 标准工作循环

每完成一个功能点，按以下循环执行：

```
1. 读 spec 对应章节 → 列出"要改哪些文件 / 新增哪些文件"
2. 输出文件清单给用户确认（不要直接动手）
3. 按文件顺序写代码，每个文件写完跑 `cargo check`
4. 一组文件写完跑 §3.1 的 6 步自检
5. 全过 → 按 §1.1 拆 commit → 输出 commit 列表让用户确认
6. 用户确认后再 `git add <files> && git commit`（不主动 push）
```

**禁止**跳步骤。**禁止**写完不跑校验就让用户试。

### 8.3 硬性红线

**代码层面**：

- ❌ 任何 `unsafe { ... }` 块
- ❌ 服务路径里的 `unwrap()` / `expect()` / `panic!()`（用 `?` + `ServiceError` 传播）
- ❌ 源码硬编码 `api_key` / `token` / `base_url` / 任何敏感字符串
- ❌ 流式响应路径调用 `.bytes().await?`（必须透传 `Body`）
- ❌ 引入未在 spec §3 依赖表中的 crate
- ❌ 创建 spec §2 目录树之外的目录（`crates/` / `benches/` / `examples/` 等）
- ❌ 上游 URL 不经白名单校验就发起请求
- ❌ 用 `lazy_static!` / `OnceLock` 装全局可变状态（必须 `Arc<AppState>`）
- ❌ 占位符 `TODO` / `FIXME` / `unimplemented!()` / `// 以后再写` 出现在提交代码里

**提交层面**：

- ❌ `feat: big update` / `update: xxx` 巨型或模糊 commit
- ❌ 一个 commit 改了 N 个不相关功能
- ❌ commit 标题用 `and` / `also` / `顺便` 连接多件事
- ❌ commit scope 写成 `all` / `misc` / `various`
- ❌ 直接 `git push` 到 main

**文档层面**：

- ❌ 主动创建 README.md / CHANGELOG.md / 其他 .md 文档（用户没明确要）
- ❌ 在 `AGENTS.md` / `spec.md` 之外另立规范
- ❌ commit message / 源码注释中堆 emoji

### 8.4 遇到不确定的事

**不要猜**。按优先级处理：

1. **AGENTS.md / spec.md 写过的** → 按文档做
2. **文档没写但有明确行业惯例的** → 按惯例做 + 在回复里说明"我参考了 X 惯例"
3. **文档没写且无惯例的** → 停下来用 `AskUserQuestion` 问用户，列出 2~3 个选项 + 各自权衡
4. **用户已说过的偏好** → 记住，不再问

**绝对禁止**：

- 静默选择其中一个方案继续做
- 编造一个 spec / AGENTS.md 里不存在的"设计原则"
- 引入 spec §3 之外的依赖"因为更好用"

### 8.5 两阶段流水线（项目演进机制）

项目按**两阶段流水线**推进，对应两份提示词文件：

| 阶段 | 输入 | 提示词文件 | 输出 |
| --- | --- | --- | --- |
| **阶段一：基础落地** | spec.md 描述的所有模块 | `docs/rapidgate-prompt.md` 阶段一 | 可编译、fmt/clippy/test 全过的代码骨架 |
| **阶段二：强化** | 阶段一产出的代码 + spec.md | `docs/rapidgate-prompt.md` 阶段二 | 性能优化、安全加固、测试覆盖、可观测性完善 |

**两阶段硬约束**：

- 阶段二**禁止**修改阶段一已 commit 的历史（cherry-pick 友好）
- 阶段二**禁止**添加 spec §3 之外的新依赖（要先回到 spec §3 走变更流程）
- 阶段二的所有 commit 必须在 commit body 标注 `Refine: stage-1 <commit-sha>`，方便溯源
- 单次会话**只跑一个阶段**，跨阶段必须开新会话并重新读 spec.md（避免上下文污染）

### 8.6 提交节奏：每完成一个 commit 单元必须立即 commit

> 本节是**对 §1.1 的强约束补丁**，对人类开发者与 AI 编码助手**同时生效**。

**核心原则**：本地工作树**永远不能**积压未提交的代码。

#### 硬性要求

- ✅ **每个独立功能点 / 每个文件新增 / 每个 fix / 每个测试用例 = 1 个 commit，写完即提交**
- ✅ **依赖变更** = 单独 commit（不能与业务代码混）
- ✅ **配置变更** = 单独 commit（不能与代码混）
- ✅ **文档变更** = 单独 commit（`docs(agents): ...` / `docs(spec): ...`）
- ❌ **禁止** 一次 `git add -A` 把十几个未提交文件打包成 1 个巨型 commit
- ❌ **禁止** 在 session 末尾 / "阶段性完成"时一次性补 commit
- ❌ **禁止** "稍后再提交" / "等全部完成再提交" / "先用 git stash 存着"

#### 与 §1.1 的关系

§1.1 已经规定 "**一个 commit = 一个功能 / 一个修复**"，§8.6 把这条规则升级为**节奏要求**：

- §1.1 关心的是**怎么切**（commit 粒度）
- §8.6 关心的是**何时切**（commit 时机）

**两个原则相乘 = 写完一个文件 / 一个测试 / 一个 fix → 立刻 `git add <files> && git commit` → 然后才能写下一个**。

#### 适用范围

| 角色 | 适用 |
| --- | --- |
| 人类开发者 | ✅ 强烈建议 |
| AI 编码助手 | ✅ **硬性要求**（Claude / GPT / Trae Agent / Cursor 等） |

#### 违规识别

以下行为一律视为违反 §8.6：

1. session 末尾 `git status` 显示 10+ 个 Modified / Untracked 文件
2. 一次 commit 改了 N 个不相关目录（`git log --stat` 看 scope）
3. 助手回复里说"我们最后统一提交" / "先记着回头补"

#### 应急通道

如果真的遇到 30+ 文件的大重构，**必须**：

1. 在第一个文件写完**之前**，列出 commit 计划（按目录 / 模块切分）给用户确认
2. 每写完 1 个 commit 单元就 commit
3. 进度可视化（用 todo 列表 / 表格追踪）

**禁止** 因"重构太大"作为一次性 commit 的理由。

## 9. 代码风格与命名约定

### 9.1 注释与文档

- 注释**解释"为什么"**，不解释"做什么"（代码本身表达"做什么"）
- 注释**简洁专业**，3 行内能讲清的不写 30 行
- **禁止**用几十个 `=`、`-`、`*` 当注释装饰线
- 公开 API（trait 公开方法、pub struct 字段）**必须**有 `///` doc 注释，说明用途 / 参数 / 错误条件
- 复杂算法 / 非显然的并发逻辑**必须**有 `//` 解释（一句就够）

### 9.2 emoji 与 ASCII art

- 源码注释、commit message、文档标题**禁止**堆 emoji
- 终端打印**禁止**用 `==========` / `----------` / `**********` 当分隔符
- 允许：极少量的功能性 emoji（如 changelog 分类），其余场合一律文字

### 9.3 日志规范

- 服务路径**必须**用 `tracing` 系列宏（`info!` / `warn!` / `error!` / `debug!` / `trace!`）
- **禁止**用 `println!` / `eprintln!` 出现在提交代码中
- 日志字段用结构化键值对：`tracing::info!(route = %route.name, status = %status, "request handled")`
- 错误日志**必须**包含 `error = %e` 字段（用 `%` 而非 `{}` 以避免提前消费 Display）

### 9.4 命名约定

| 类别 | 风格 | 示例 |
| --- | --- | --- |
| 文件名 | snake_case | `route_table.rs` |
| 类型 | PascalCase | `RouteTable` |
| 函数与变量 | snake_case | `match_request` |
| 错误枚举变体 | PascalCase | `RouteNotFound` |
| 路由 ID 字符串 | kebab-case | `openai-chat` |
| 环境变量 | UPPER_SNAKE | `RGD_OPENAI_API_KEY` |
| 模块路径 | 全小写 | `core::routing` |
| trait 方法 | snake_case | `fn check(&self) -> bool` |

### 9.5 错误处理约定

- 所有 axum handler 的返回类型**统一**为 `Result<axum::response::Response, ServiceError>`
- `ServiceError` 唯一实现 `axum::response::IntoResponse`，统一输出 JSON：

  ```json
  { "error": { "code": "unauthorized", "message": "..." } }
  ```

- `core` 模块定义 `CoreError`（无 axum 依赖），`service` 模块定义 `ServiceError` 并 `From<CoreError>`
- 服务路径**禁止**出现 `unwrap()` / `expect()` / `panic!()`
- 测试代码允许 `unwrap()` / `expect()`

### 9.6 状态与共享

- 跨请求共享状态**必须**用 `Arc<AppState>`，由 `axum::extract::State` 注入
- **禁止**用 `lazy_static!` / `OnceLock` / `static mut` 装全局可变状态
- 配置热重载**必须**用 `arc_swap::ArcSwap<T>`（无锁读、整体替换）

## 10. 文档与变更控制

### 10.1 文档创建原则

- **禁止**主动创建 README.md / CHANGELOG.md / CONTRIBUTING.md / 其他 .md 文档
- **禁止**主动创建 `docs/` 下的新文件，除非用户明确要求
- 修改 `AGENTS.md` / `docs/rapidgate-spec.md` / `docs/rapidgate-prompt.md` 是允许的，但要走 commit：`docs(spec): ...` / `docs(agents): ...`
- 涉及架构变更必须**先改 spec.md**，再写代码，**最后**改 `Cargo.toml`（单一事实源）

### 10.2 变更影响范围

任何变更前，自问：

- 改了 `Cargo.toml` 的依赖 → spec §3 是否同步更新？
- 改了目录结构 → spec §2 是否同步更新？
- 改了错误响应格式 → spec §5.1 / AGENTS §9.5 是否同步更新？
- 改了版本号 → 是否按 §6 走发版流程？
- 改了 CI / Release / 配置文件 → 是否影响 CI 行为？

**变更必须可追溯**：每个 commit 标题 + 关联文档修改 commit，**必须**能由 `git log` + `git blame` 重建完整决策链。

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tools** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them. `codegraph_node` returns one symbol's source + callers, or reads a whole file with line numbers. If the tools are listed but deferred, load them by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` and `codegraph node <symbol-or-file>` print the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
