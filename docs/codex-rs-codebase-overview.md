# codex-rs 代码库概览

> **最后更新**: 2026-04-27

本文档描述了 `codex-rs/` 下的 Rust 工作区，该工作区实现了 Codex CLI 的运行时、协议、工具和沙箱功能。这是一个 Cargo 工作区，包含约 90 个库/二进制 crate，按逻辑分为多个层级。

## 架构分层

```
┌─────────────────────────────────────────────────────┐
│  入口点 (cli, tui, exec, mcp-server)                 │
├─────────────────────────────────────────────────────┤
│  App Server / 协议层 (app-server*)                    │
├─────────────────────────────────────────────────────┤
│  核心引擎 (core, protocol, state, rollout, ...)       │
├─────────────────────────────────────────────────────┤
│  AI 提供商 & API (codex-api, codex-client, ...)       │
├─────────────────────────────────────────────────────┤
│  沙箱 & 安全 (sandboxing, arg0, ...)                  │
├─────────────────────────────────────────────────────┤
│  工具库 (utils/*, ansi-escape, ...)                   │
└─────────────────────────────────────────────────────┘
```

---

## 1. 应用入口点

以下 crate 生成面向用户的二进制文件。

| Crate | 二进制 | 用途 |
|---|---|---|
| `cli/` | `codex-moon` | 主多工具 CLI — 分发到所有子命令 (`exec`, `app-server`, `sandbox`, `mcp` 等) |
| `tui/` | `codex-tui` | 基于 [Ratatui](https://ratatui.rs/) 构建的全屏终端 UI，是主要的交互体验 |
| `exec/` | `codex-exec` | 无头/非交互式 CLI 模式 (`codex exec PROMPT`)，适用于自动化和脚本场景 |
| `exec-server/` | (lib) | 服务端进程执行引擎，管理 PTY 会话、命令 I/O 和沙箱执行 |
| `mcp-server/` | `codex-mcp-server` | 实验性 MCP 服务器二进制，将 Codex 作为工具暴露给其他 MCP 兼容客户端 |

---

## 2. App Server & 协议层

JSON-RPC 风格的 API 层，为 VS Code、JetBrains 等 IDE 集成提供支持。

| Crate | 用途 |
|---|---|
| `app-server/` | 主 app server 二进制/库，实现所有 RPC 方法（thread、turn、command、fs、auth、skills、plugins、MCP、approvals）。支持 stdio (JSONL) 和 WebSocket 传输 |
| `app-server-protocol/` | 共享协议类型：v1 和 v2 RPC 的 params、responses 和 notifications。使用 `#[ts(...)]` 宏生成 TypeScript |
| `app-server-client/` | 连接运行的 `codex-app-server` 的客户端库，被 `tui/` 和 `exec/` 使用 |
| `app-server-test-client/` | 仅用于集成测试的测试客户端 |
| `protocol/` | `codex-core` 及其消费者之间使用的核心协议类型，包含 items（消息、工具调用、文件编辑）、agents 和流事件。设计为轻量依赖 |

---

## 3. 核心引擎

核心业务逻辑和编排层。

| Crate | 用途 |
|---|---|
| `core/` | **codex-core** — Codex 的心脏。包含 agent 编排、turn/item 管理、LLM 响应流式处理、工具执行（shell、文件编辑、apply patch）、rollout/thread 持久化、沙箱集成以及跨平台支持（macOS Seatbelt、Linux Landlock/bubblewrap、Windows） |
| `state/` | 基于 SQLite 的状态管理，用于 rollout 持久化（threads/turns/items） |
| `rollout/` | Rollout 协调：管理 rollout 会话的文件系统布局、文件搜索索引和 git 集成 |
| `connectors/` | App 连接器 — 与外部服务（GitHub 等）的集成，可通过 `$app-slug` 从 thread 中调用 |
| `tools/` | 工具定义和执行。实现 FileEdit、shell、read、search 等 agent 可调用的工具 |
| `hooks/` | 钩子系统 — 在生命周期事件（如 session start、turn complete）时运行用户定义的脚本 |
| `instructions/` | Agent 指令模板 — 模型使用的系统提示和开发者指令 |
| `features/` | 特性开关系统 — 运行时切换实验性功能，从配置中加载 |
| `cloud-tasks/` | 基于云的任务提交和轮询（基于 OpenAI API 的异步任务） |
| `cloud-tasks-client/` | 与云端任务后端通信的 HTTP 客户端 |
| `cloud-tasks-mock-client/` | 云端任务客户端的 Mock 实现，用于测试 |
| `cloud-requirements/` | 云服务可用性检查和云功能认证验证 |

---

## 4. AI 提供商 & API 客户端

连接 LLM 后端（OpenAI、Ollama、LM Studio 等）。

| Crate | 用途 |
|---|---|
| `codex-api/` | OpenAI Responses API 和 Realtime API 的高级客户端，处理流式、SSE 解析和请求构建 |
| `codex-client/` | OpenAI 兼容端点的低级 HTTP/SSE 客户端，提供 TLS (rustls)、事件流解析和重试逻辑 |
| `chat-completions/` | Chat Completions API 适配器 — 将 Responses API 包装为 Chat Completions 接口，用于向后兼容 |
| `login/` | 认证管理：API key 登录、ChatGPT OAuth（浏览器 + 设备码流程）、令牌刷新和凭据持久化 |
| `model-provider-info/` | 模型提供商元数据 — 描述可用提供商、API 端点、认证要求和支持的功能 |
| `models-manager/` | 运行时模型选择与发现，管理模型列表、提供商解析和协作模式模板 |
| `ollama/` | Ollama 提供商集成 — 连接本地 Ollama 实例运行开放权重模型 |
| `lmstudio/` | LM Studio 提供商集成 — 连接本地 LM Studio 服务器 |
| `chatgpt/` | ChatGPT 专用 CLI/连接器，提供 `codex chatgpt` 子命令 |
| `responses-api-proxy/` | 代理二进制，通过认证中继转发 Responses API 调用 |
| `response-debug-context/` | 从 Responses API 请求/响应中提取调试上下文元数据 |
| `codex-backend-openapi-models/` | Codex 后端 API 的 OpenAPI 模型生成代码 |

---

## 5. MCP（模型上下文协议）

| Crate | 用途 |
|---|---|
| `codex-mcp/` | MCP 客户端管理器 — 管理外部 MCP 服务器的连接、工具发现、OAuth 流程和工具执行路由。MCP 工具注册和生命周期的核心枢纽 |
| `mcp-server/` | MCP 服务器二进制 — 将 Codex 自身的功能（thread/turn 生命周期、配置、认证）暴露为 MCP 工具 |
| `rmcp-client/` | Rust MCP 客户端库 — 通过 stdio 或 HTTP 连接 MCP 服务器的底层 JSON-RPC 传输和协议处理 |

---

## 6. Skills & 插件

| Crate | 用途 |
|---|---|
| `skills/` | Skill 文件发现和加载，从文件系统读取 `SKILL.md` 文件并提供给 agent |
| `core-skills/` | 内置 skill 实现：skill-creator、图片生成等。提供 skill 指令与 agent 行为之间的运行时桥梁 |
| `plugin/` | 插件清单处理 — 读取、验证和解析 `plugin.json` 文件，包括其捆绑的 MCP 服务器、apps 和 skills |
| `collaboration-mode-templates/` | 协作模式预设模板 — 定义内置的 `paired`、`manager`、`solo` 模式配置及其开发者指令 |

---

## 7. 沙箱 & 安全

平台特定的进程沙箱强制执行。

| Crate | 用途 |
|---|---|
| `sandboxing/` | 跨平台沙箱策略定义和执行调度器。定义 `SandboxPolicy`、可写根目录、网络访问和文件系统访问约束，路由到平台特定实现 |
| `arg0/` | Arg0 重新执行调度器 — 根据 argv[0] 通过 `codex-linux-sandbox`、`codex-execve-wrapper` 或 `apply_patch` 在沙箱下重新执行进程 |
| `linux-sandbox/` | Linux 沙箱实现，使用 Landlock 和 bubblewrap。处理用户命名空间创建、文件系统策略执行和重新执行 |
| `windows-sandbox-rs/` | Windows 沙箱实现，使用 job objects、restricted tokens 和 AppContainer silos。包含提权设置助手和命令运行器二进制 |
| `shell-escalation/` | macOS 的 Shell 命令提权 — 提供 `codex-execve-wrapper` 二进制通过 authorisation-exec(2) 实现权限提升 |
| `process-hardening/` | 跨平台进程级强化：基于 libc 的沙箱原语（setrlimit、prctl、pledge 等） |
| `execpolicy/` | 执行策略引擎 — 评估基于前缀的 Starlark 规则以决定是否允许/拒绝命令 |
| `execpolicy-legacy/` | 旧版执行策略引擎 — 在基于 Starlark 的系统之前使用的更简单的正则+allocative 策略评估器 |
| `network-proxy/` | 网络代理配置 — 从配置和环境解析 HTTP 代理设置 |
| `secrets/` | 密钥存储/加密 — 通过 age 加密和系统钥匙串管理密钥，用于凭据存储 |
| `keyring-store/` | 操作系统钥匙串（macOS Keychain、Linux Secret Service、Windows Credential Manager）的轻量封装 |

---

## 8. 配置

| Crate | 用途 |
|---|---|
| `config/` | 配置加载、解析和 schema。从标准路径读取 `config.toml`，解析 MCP 服务器配置、模型提供商设置、特性开关、执行策略、沙箱策略和审批预设。生成 `config.schema.json` 用于 IDE 支持 |
| `features/` | 特性开关定义和运行时评估。定义已知的特性开关及其启用条件 |
| `feedback/` | 通过 Sentry 集成的崩溃/错误报告，以及用户反馈收集 |

---

## 9. 工具库 (`utils/*`)

小型、专注的工具 crate，依赖最小化。

| Crate | 用途 |
|---|---|
| `utils/absolute-path/` | 解析和规范化文件系统路径。支持波浪号展开、dunce（Windows 路径规范化）和 serde |
| `utils/approval-presets/` | 审批策略预设定义（always、never、granular、on-failure、on-request、untrusted） |
| `utils/cache/` | 内存 LRU 缓存，支持可选的 SHA-1 键控 |
| `utils/cargo-bin/` | 在 Cargo 和 Bazel runfiles 下查找第一方二进制用于集成测试 |
| `utils/cli/` | 基于 Clap 的子命令共享 CLI 参数解析辅助函数 |
| `utils/elapsed/` | 人类可读的持续时间格式化（如 "2m 34s"） |
| `utils/fuzzy-match/` | 模糊字符串匹配工具 |
| `utils/home-dir/` | 定位和缓存用户的家目录和 Codex 家目录 (`~/.codex`) |
| `utils/image/` | 图片加载、编码和缓存。支持 JPEG、PNG、GIF、WebP |
| `utils/json-to-toml/` | 将 JSON 转换为 TOML（用于旧版 TypeScript CLI 的配置迁移） |
| `utils/oss/` | OSS 提供商路由器 — 在 Ollama、LM Studio 和其他本地提供商之间选择 |
| `utils/output-truncation/` | 截断 agent 输出以便显示，同时保持可读性 |
| `utils/path-utils/` | 扩展路径操作：在 CWD 内解析相对路径、git 仓库检测等 |
| `utils/plugins/` | 插件文件系统扫描和配置读取 |
| `utils/pty/` | 伪终端 (PTY) 管理，用于 agent 内的交互式 shell 会话 |
| `utils/readiness/` | 带超时的异步就绪检查 — 轮询检查函数直到成功或超时 |
| `utils/rustls-provider/` | 选择系统合适的 rustls TLS 提供商（ring 或 aws-lc-rs） |
| `utils/sandbox-summary/` | 渲染当前沙箱策略的人类可读摘要 |
| `utils/sleep-inhibitor/` | 防止 agent 工作时系统休眠（macOS：IOKit 电源断言） |
| `utils/stream-parser/` | SSE（服务器发送事件）流解析器，用于处理流式 API 响应 |
| `utils/string/` | 字符串操作工具（简化正则表达式操作、格式化） |
| `utils/template/` | 用于系统提示和指令的最小模板渲染引擎 |

---

## 10. 其他 Crate

| Crate | 用途 |
|---|---|
| `analytics/` | 分析事件收集 — 提交遥测事件用于产品分析 |
| `ansi-escape/` | TUI 的 ANSI 转义码解析 — 将 ANSI 着色的终端输出转换为 Ratatui styled spans |
| `apply-patch/` | 文件编辑应用引擎 — 实现 `apply_patch` 工具，对源文件应用结构化编辑 |
| `async-utils/` | 异步工具 — 用 `spawn_blocking` 包装阻塞操作，提供异步超时和重试 |
| `backend-client/` | Codex 云端后端的 HTTP 客户端（API key 管理、用户信息等） |
| `code-mode/` | 代码分析模式 — 使用 [Deno](https://deno.com/) 运行基于 JavaScript 的代码分析工具（tree-sitter 解析、类型检查等） |
| `codex-experimental-api-macros/` | `#[experimental(...)]` 注解的过程宏，用于协议类型上的门控 API |
| `debug-client/` | 调试/测试客户端，用于向 app-server 发送原始 JSON-RPC 消息 |
| `file-search/` | 工作区内的模糊文件搜索 — 使用基于 trie 的索引进行快速文件名匹配 |
| `git-utils/` | Git 集成：差异生成、提交历史、分支信息、`git blame` 和 git 元数据提取 |
| `otel/` | OpenTelemetry 追踪集成 — 配置追踪和 span 导出用于可观测性 |
| `realtime-webrtc/` | 通过 WebRTC 的实时音频/文本会话支持 — 启用语音模式交互 |
| `stdio-to-uds/` | 将 stdio 转发到 Unix Domain Socket 的工具二进制，用于传输桥接 |
| `terminal-detection/` | 检测终端能力和模拟器类型（支持 macOS Terminal、iTerm2、VS Code、Windows Terminal 等） |
| `test-binary-support/` | 测试基础设施 — 为集成测试创建带有正确 `codex-arg0` 符号链接的隔离临时目录 |
| `v8-poc/` | V8 JavaScript 运行时集成概念验证，用于沙箱化 JavaScript 执行 |
| `vendor/` | 供应商第三方源代码和补丁 |

---

## 11. 构建 & 工具支持文件

| 路径 | 用途 |
|---|---|
| `BUILD.bazel` | 根 Bazel 构建定义 |
| `Cargo.toml` | 工作区清单，列出所有成员 crate、共享依赖和工作区默认配置（edition 2024、resolver 2） |
| `Cargo.lock` | 可复现 Cargo 构建的锁定文件 |
| `clippy.toml` | 工作区的 Clippy lint 配置 |
| `rustfmt.toml` | 统一代码格式化的 Rustfmt 配置 |
| `rust-toolchain.toml` | 固定的 Rust 工具链版本 |
| `deny.toml` | `cargo-deny` 配置，用于许可证/crate 审计 |
| `default.nix` | 基于 Nix 的开发环境兼容性配置 |
| `node-version.txt` | code-mode 的 Deno 运行时引导所需的 Node.js 版本 |
| `scripts/` | 构建和开发脚本 |
| `docs/` | Crate 级文档（bazel.md、codex_mcp_interface.md、protocol_v1.md） |

---

## 12. 关键组件关系

```
用户
  ├── codex-tui  (Ratatui 交互式 TUI)
  ├── codex-exec (无头自动化)
  ├── codex-mcp-server  (MCP 协议暴露)
  └── codex-app-server  (VS Code / IDE 集成)
        │
        └── codex-app-server-client ── codex-core
                                          │
                                     ┌────┴────────┐
                                     │              │
                              codex-protocol    codex-sandboxing
                              codex-state       │
                              codex-rollout     ├── arg0 → linux-sandbox / windows-sandbox
                              codex-tools       │
                              codex-hooks             └── execpolicy
                                     │
                              codex-api ── codex-client (OpenAI Responses API)
                                     │
                              codex-mcp ── rmcp-client (MCP 服务器)
```

## 13. 开发规范

- Crate 名称以 `codex-` 为前缀
- 整个工作区使用 Edition 2024
- 优先使用私有模块和显式导出的公共 API
- **避免向 `codex-core` 添加新代码** — 优先使用现有专用 crate 或创建新的 crate
- 所有影响 TUI 的更改都需要 `insta` 快照测试更新
- Rust 代码修改后运行 `just fmt`；对特定 crate 运行 `just fix -p <crate>` 进行 Clippy 修复
- 依赖变更后运行 `just bazel-lock-update` 和 `just bazel-lock-check`
- Config schema 变更后运行 `just write-config-schema`
- App-server 协议变更后运行 `just write-app-server-schema`
