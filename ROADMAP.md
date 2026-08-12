# 演进路线图（ROADMAP）

记录 `codex-web-search-mcp` 的后续优化方向。本仓库定位是 **OpenAI Codex 单后端的纯 Rust MCP server**，
所有演进只在本项目能力内做增强，**不引入参考项目（GrokSearch-rs）的差异化功能**（见文末「明确不做」）。

---

## ✅ 已落地

| 版本 | 方向 | 说明 |
|------|------|------|
| v2.1.0 | `web_fetch` 编码/超时/重定向 | 用 `encoding_rs` 探测 charset（Content-Type → `<meta>` → UTF-8）正确解码 GBK/GB2312 等中文编码；独立 `FETCH_TIMEOUT=30s`；用 ureq Agent 显式开启 `redirects(10)` 跟随 301/302。 |
| v2.1.0 | 一键安装脚本 | `scripts/install.sh`（bash）/ `scripts/install.ps1`（PowerShell），自动识别平台、下载 Release 二进制、可选写入 `.mcp.json`。 |
| v2.1.0 | Release SHA-256 校验清单 | CI 在 `github-release` 阶段生成 `checksums.txt` 随 Release 发布，安装脚本自动校验。 |
| v2.2.0 | 端点容错与重试 | 对 Codex 请求加指数退避重试（最多 3 次：500ms、1s）：`429` / 服务端 `5xx` / 网络抖动可重试；`401/403` 与其他 `4xx` 直接友好降级不重试。新增 `CODEX_ENDPOINT` 环境变量覆盖端点（便于代理 / 联调）。 |
| v2.2.0 | 可分级日志 | 新增 `--verbose`（或 `CODEX_MCP_LOG=debug`）开关，把握手、工具名、HTTP 状态、耗时、重试写到 **stderr**，绝不污染 MCP 的 stdout JSON-RPC 管道。 |
| v2.2.0 | Rust 集成测试 + CI 跑测试 | `src/main.rs` 的 `#[cfg(test)]` 用本地 TcpListener mock Codex 端点，对引用清理 / 编码探测 / 后端错误识别 / `handle` 握手与 `tools/call` 路径做断言（含 `429→200` 重试）。CI 新增 `test` job 跑 `cargo test`，`build` 依赖其通过。 |
| v2.3.0 | 来源去重与按域名聚合 | `normalize_body` 按 `ref_id`/`url` 去重（多步 research 的重复来源不再膨胀）；`format_text` 的 Sources 按 `domain` 分组展示（来源渲染更干净，工具返回结构不变）。新增 `normalize_body_dedupes_results` / `format_text_groups_by_domain` 两个测试。 |
| v2.3.0 | Token 自动刷新（401 时） | `load_codex_auth` 改为 `CodexAuth` 结构体，记录 `auth.json` 路径与 `refresh_token`；`call_codex` 收到 `401` 且为文件凭证且含 `refresh_token` 时，调用 `CODEX_REFRESH_ENDPOINT`（默认 `chatgpt.com/backend-api/auth/refresh`，可覆盖）换发新 token，先备份 `auth.json` 再回写，重试一次。无 refresh_token 或刷新失败则退回友好重登提示。 |
| v2.3.1 | P3：4xx 错误体透传 | `post_codex` 新增 `extract_err_detail`，对 `400/401/403` 等带 `error` 体的响应解析 `error.message`（附 `param`/`code`）并拼进返回消息；工具调用方（模型/用户）现在能直接看到「为什么调用失败」（如 `empty_array: commands.search_query 不能为空`）。新增 `search_400_passthroughs_error_detail` 测试。 |
| v2.3.1 | 修复：非可重试 4xx 误重试 | `call_codex` 原对非可重试错误（如 `400` 校验失败）也会进入指数退避循环重试一次，导致同样的请求被重发、且可能被 mock/上游的默认响应「吞成」成功。新增 `if !retryable { return Err(msg); }` 提前失败，仅 `429/5xx/网络` 才重试。 |
| v2.3.1 | 测试加固 | mock Codex 端点改为按 `Content-Length` 读完整个请求体再响应，消除重试突发的 RST（10054）偶发失败，测试稳定（16 项连跑多次全绿）。 |

---

## 📋 待办 / 待实测验证（Codex 接口能力校准）

> 以下为「Codex `/codex/alpha/search` 接口能力校准」清单。每一项都通过向真实后端发送
> 构造请求来实测是否真可用，验证后回填结论（✅ 可用 / ⚠️ 部分可用 / ❌ 不可用）。
> 实测方法：加载 `~/.codex/auth.json` 复刻客户端请求头（Bearer + ChatGPT-Account-ID + UA），
> 用 curl 发送多组 payload，落盘原始响应后分析（token 已脱敏，响应不入库）。
> 原始响应与脚本见 `artifacts/probe/`（临时目录，不提交）。

| 编号 | 候选能力 | 当前实现状态 | 实测结论（2026-08-12 真实请求） | 状态 |
|------|----------|--------------|----------|------|
| P1 | `recency` 时间过滤 | 已传整数天 | ✅ 可用。后端**仅接受整数（天）**；传字符串（`"week"`）或非法值直接 `400 invalid_type`。当前实现传整数，正确。是否硬过滤待定（观测到 recency=7 仍出现旧结果，疑似软排序）。 | ✅ 已实现正确 |
| P2 | per-query `response_length` | 仅在 `commands` 顶层设 | ✅ 顶层 `response_length` 可用；**项内嵌 `response_length` 被 `400 unknown_parameter` 拒绝**。当前只在顶层设，设计正确，无需改。 | ✅ 已实现正确 |
| P3 | 后端错误形态兜底 | `post_codex` 对 4xx 只回 "HTTP 400"，丢弃响应体 | ⚠️ **发现真缺口**：校验类错误一律 `HTTP 400` + 结构化 `error{message,type,param,code}`（如 `missing_required_parameter`/`invalid_type`/`unknown_parameter`/`empty_array`）。当前把 `error.message` 丢了，模型/用户拿不到原因。 | ✅ 已修复（v2.3.1） |
| P4 | live / cached 模式 | 未传 | ❌ `mode` / `web_search` 在 payload 中均被 `400 unknown_parameter` 拒绝。该端点无 live/cached 开关，纯 CLI 层概念，无需传。 | ❌ 不可用 |
| P5 | 分页 / cursor | 明确不做 | ❌ 真实 200 响应顶层仅 `encrypted_output`/`output`/`results`，**无任何 `cursor`/`has_more`/`next`/`page`**。分页确属接口不支持，非漏做。 | ❌ 确认不可行 |

### 实证结论与后续

- **P1 / P2：当前实现已正确**，实测反向验证（整数 recency、顶层 response_length 才合法；字符串/项内嵌会被 400 拒绝）。无需改动。
- **P3 已落地（v2.3.1）**：`post_codex` 现在对 `400/401/403` 等带 `error` 体的响应解析并附带 `error.message`（及 `param`/`code`），工具调用方（模型/用户）能直接看到失败原因（如 `empty_array: commands.search_query 不能为空`）。纯可观测性增强，不改变工具语义。
- **P4 / P5：确认不可用**，与「明确不做」一致，无需投入。
- 附带观测（非缺口）：200 响应含 `encrypted_output` 字段（加密，MCP 侧无法使用）；`results[]` 含 `type` 字段（如 `youtube`），当前未暴露，属可选增强。
- 实测原始响应见 `artifacts/probe/results.json`（临时，不入库）。

---

## 🚫 明确不做（边界）

以下功能是之前从参考项目带进来的差异化能力，**已按项目定位回退**，不再加回：

- 其它搜索后端（Grok / Tavily / Bing 等）——保持 Codex 单后端。
- `doctor` 自检工具。
- `get_sources` 分页 + 预算控制。
- `web_map` 域名发现。
- `config.toml` 配置文件（凭证只走 `codex login` 或环境变量）。
- 恢复 npm 多平台子包分发（Release 二进制路线已满足「开箱即用」，且无需 npm 账号）。

---

## 版本与发布

- 版本号遵循语义化：`vX.Y.Z`。
- 打 tag 即触发 CI：跨平台编译 + 生成 `checksums.txt` + 发布到 GitHub Release。
- 用户经 `scripts/install.sh` / `install.ps1` 下载即用，可选 `--write-config` 生成 `.mcp.json`。
