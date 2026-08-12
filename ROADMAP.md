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

---

## 📋 待办

> 长期项（Token 自动刷新、来源去重聚合）已在 v2.3.0 落地，目前无明确待办。后续若收到新需求再补充。

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
