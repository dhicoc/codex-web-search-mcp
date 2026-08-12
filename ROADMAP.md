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

---

## 📋 待办（按优先级）

### 长期（生态，按需）

#### 4. Token 自动刷新
- **动机**：`auth.json` 含 `refresh_token`，过期需手动 `codex login`。
- **做法**：检测 `401` 时用 `refresh_token` 换发新 `access_token` 并回写 `auth.json`（带备份）。
- **风险**：需确认 Codex refresh 端点与 `auth.json` 结构，改动集中在 `load_codex_auth`。

#### 5. 来源去重与按域名聚合
- **动机**：多步 `research` 返回的来源列表可能重复，模型引用不够干净。
- **做法**：在 `normalize_body` 里按 `ref_id`/`url` 去重，并按 `domain` 聚合输出，工具返回结构不变（仍在 `results` 数组内处理）。
- **范围**：仅影响 `format_text` 的来源渲染。

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
