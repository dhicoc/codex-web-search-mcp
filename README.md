# codex-web-search-mcp

一个**模型无关**的 MCP (Model Context Protocol) server，把 OpenAI Codex 的独立搜索端点
（`chatgpt.com/backend-api/codex/alpha/search`）封装成 Claude Code / 任意 MCP 客户端可用的联网搜索工具。
**v2.0.0 起用 Rust 全量重写**为独立二进制（不再依赖 Node / npx）：

- 后端 **OpenAI Codex**（免费，只要 `codex login` 登录态）；
- 3 个工具：`codex_web_search`、`codex_web_research`、`web_fetch`；
- 独立二进制，**无需 Rust 运行时即可运行**（下载预编译 exe 即用）。

> 灵感与端点实现来自 [mateusdcc/pi-gpt-search](https://github.com/mateusdcc/pi-gpt-search)（MIT）。

## 解决什么问题

Claude Code 原生的 `WebSearch` / `WebFetch` 工具**绑定 Anthropic API**。一旦把基座模型换成
Gemini、OpenRouter、本地模型等非 Anthropic 模型，这些工具就会失灵。

本工具直连 Codex 的独立搜索端点，**与底层模型完全无关**——无论客户端用哪个模型，
都能通过 MCP 工具获得实时联网搜索能力，且**不消耗 GPT/Codex 的推理 token**（只占用账号搜索额度，见下）。

## 工作原理

```
Claude Code / 任意 MCP 客户端（任意模型）
   ├── codex_web_search(query)          # 单步快速搜索
   ├── codex_web_research(...)          # 多步深度研究（search→open→find→click，靠 ref_id 串联）
   └── web_fetch(url)                   # 抓取任意 URL 纯文本（补足搜到却读不到正文的短板）
          │
          ▼
   ┌─────────────────────┐
   │  OpenAI Codex 搜索   │ ───────▶  /backend-api/codex/alpha/search
   │  端点（免费登录态）  │
   └─────────────────────┘
```

- Codex 端点不执行 GPT 推理，只返回结构化搜索结果（**零 GPT token**）。
- `model` 字段仅作为接口要求的标签（固定 `gpt-4o`），不代表实际调用 GPT。
- `search_query` / `open` / `find` / `click` 都是同一个端点 `commands` 里的并列操作，后端靠请求体的
  会话 `id` 维持上下文，使后续 `open`/`find`/`click` 能解析上一次搜索返回的 `ref_id`。本 server 在多次
  tool call 之间复用同一会话 id，并把 `ref_id` 暴露在来源列表里，模型即可多轮编排。
- 引用标记现在返回为 `[turn0searchN: 标题 → 域名]` 形式（旧版是 PUA 私有区字符，已重写清理），
  模型可直接拿 `turn0searchN` 这种 `ref_id` 去做 `open` / `click`。

## 依赖与环境

1. **有效的 Codex 登录凭证（必做）**——直连 Codex 搜索端点，必须有登录态，否则工具返回清晰报错而非崩溃。
   凭证二选一：
   - **方式 1（推荐，零手动配置）**：`codex login`，OAuth 自动把 token 写入 `~/.codex/auth.json`；
   - **方式 2（免 auth.json）**：环境变量 `CODEX_ACCESS_TOKEN`（可选 `CODEX_ACCOUNT_ID`）。

> 没有 ChatGPT/Codex 账号、未登录、或会话过期（`401/403`）时，工具会返回明确的中文报错，而不是崩溃。

> **成本与额度提醒**：本工具**不按 GPT 生成 token 计费**——它调用的是 Codex 的 `search` 端点
> （`/backend-api/codex/alpha/search`），而非 `chat/completions` 文本生成。但每次搜索都会**占用你
> ChatGPT/Codex 账号的搜索额度与速率配额**（服务端按账号限流，超限返回 `429`）。要点：
> - 需要**有效的 ChatGPT/Codex 登录态**；免费账号通常可用但频率/总量受限，高频或重度使用建议 Pro/Plus。
> - 它**不是「零 OpenAI 资源」**：与纯本地 Playwright 类浏览器工具（完全不碰 OpenAI）不同，本工具依赖
>   OpenAI 搜索后端，每次调用都会消耗对应账号额度。
> - 触发 `401/403`（凭证过期/权限不足）或 `429`（速率超限）时，重登录或稍后重试即可。

### 获取 Codex 凭证（必做）

> **不必手动编写 `auth.json`**：它是 `codex login` 的 OAuth 产物，手搓无效。让 `codex login` 自动生成，或改用环境变量。

**方式 1：`codex login`（推荐）**

```bash
npm install -g @openai/codex        # 国内: --registry=https://registry.npmmirror.com
codex login                         # 浏览器走 ChatGPT/OpenAI OAuth
```
登录成功后自动写入 `~/.codex/auth.json`（含 `tokens.access_token` / `tokens.account_id`）。
**server 会自动读取，无需额外配置。**

**方式 2：环境变量 `CODEX_ACCESS_TOKEN`（免 auth.json）**

```bash
# Windows PowerShell
$env:CODEX_ACCESS_TOKEN = "你的token"
$env:CODEX_ACCOUNT_ID  = "你的account_id"   # 可选

# macOS / Linux
export CODEX_ACCESS_TOKEN="你的token"
export CODEX_ACCOUNT_ID="你的account_id"     # 可选
```

> 凭证过期（`401/403`）：方式 1 重新 `codex login`；方式 2 换新 token。

## 安装（开箱即用，推荐）

本项目是**独立原生二进制**，无需安装 Rust、无需 Node 即可使用。两种拿到二进制的方式：

- **方式 A（推荐）：去 [Releases](https://github.com/dhicoc/codex-web-search-mcp/releases) 下载预编译文件**
  —— 下载即用，零依赖。各平台文件名见下「配置 MCP · 方式 A」。
- **方式 B：从源码编译**（见下「编译（Build）」），产物直接运行。

## 编译（Build）

### Windows（MSVC）

```bash
# 本仓库脚本已注入 VS 2023 环境（INCLUDE/LIB 用反斜杠，避开 LNK1181）
bash scripts/build.sh --release
# 产物： target/release/codex-web-search-mcp.exe
```

> ⚠️ **编译坑（已踩过）**：MSVC 的 `link.exe` / `cl.exe` 只认「反斜杠 + `C:\` 盘符」的
> `INCLUDE`/`LIB` 路径。用正斜杠 `/` 或 `/c/` 风格会报
> `LNK1181: 无法打开输入文件“kernel32.lib”`。本脚本已处理；若手动编译，务必导出带反斜杠的
> `INCLUDE`/`LIB` 并指向 MSVC 的 `Hostx64/x64/link.exe`。

### macOS / Linux

```bash
cargo build --release
# 产物： target/release/codex-web-search-mcp
```

## 配置 MCP

### 方式 A：下载预编译二进制（开箱即用，推荐）

去 [Releases](https://github.com/dhicoc/codex-web-search-mcp/releases) 下载与你平台匹配的文件，
放到任意目录即可使用——**不需要 Rust、不需要 Node**：

| 平台 | 文件名 |
|------|--------|
| Windows x64 | `codex-web-search-mcp-win32-x64.exe` |
| Windows ARM64 | `codex-web-search-mcp-win32-arm64.exe` |
| macOS（Intel / Apple Silicon 通用） | `codex-web-search-mcp-darwin-universal` |
| Linux x64 | `codex-web-search-mcp-linux-x64` |
| Linux ARM64 | `codex-web-search-mcp-linux-arm64` |

MCP 配置（把 `command` 换成你下载的文件路径）：

```json
{
  "mcpServers": {
    "codex-web-search": {
      "command": "C:/path/to/codex-web-search-mcp-win32-x64.exe"
    }
  }
}
```

- macOS / Linux：把 `command` 换成你下载文件的实际路径（如 `/path/to/codex-web-search-mcp-darwin-universal`）。
- 改完重启客户端即可；首次在客户端里查看是否连上（如 Claude Code 的 `/mcp`）。
- 写入用户级配置（如 `~/.claude.json`）的 `mcpServers` 即对所有项目生效。

### 方式 B：从源码编译（无预编译 / 想自己构建）

Rust 版是**独立二进制**，编译一次后直接让客户端 spawn 这个 exe（或 macOS/Linux 下的二进制）即可，**不需要 Node**。

```json
{
  "mcpServers": {
    "codex-web-search": {
      "command": "C:/path/to/codex-web-search-mcp/target/release/codex-web-search-mcp.exe"
    }
  }
}
```

- macOS / Linux：把 `command` 换成 `/path/to/codex-web-search-mcp/target/release/codex-web-search-mcp`。
- 编译步骤见下「编译（Build）」。
- 改完重启客户端即可；首次在客户端里查看是否连上（如 Claude Code 的 `/mcp`）。
- 写入用户级配置（如 `~/.claude.json`）的 `mcpServers` 即对所有项目生效。

> 可选项：在 MCP 配置的 `"env"` 里加 `CODEX_ACCESS_TOKEN` 覆盖凭证（方式 2）。若用 `codex login`，连 exe 路径都不用配 env。

> ⚠️ **不要用** `"command": "cmd", "args": ["/c", ...]` —— 会破坏 MCP stdio 管道导致超时 / -32000。

## 工具一览

### `codex_web_search`（单步搜索）

| 参数 | 类型 | 说明 |
|------|------|------|
| `query` | string（必填） | 搜索关键词或问题 |
| `recency` | number | 仅返回最近 N 天内的结果 |
| `domains` | string[] | 限定搜索域名，如 `["github.com"]` |
| `response_length` | `short`/`medium`/`long` | 返回详略程度 |

### `codex_web_research`（多步深度研究）

适合「打开官网文档、长文里找关键段落、跟随链接深挖」的场景。所有操作可在一次调用里组合，
也可分多轮调用（靠自动维持的会话上下文，用上一轮返回的 `ref_id` 串联）。来源列表里会带
`[turn0search0: 标题 → 域名]` 这样的标记，模型在后续 `open`/`find`/`click` 里直接引用 `turn0search0` 即可。

| 参数 | 类型 | 说明 |
|------|------|------|
| `search_query` | `{q, recency?, domains?}[]` | 要执行的搜索查询列表 |
| `open` | `{ref_id, lineno?}[]` | 按 `ref_id` 打开文档/页面 |
| `find` | `{ref_id, pattern}[]` | 在已打开文档中查找关键词 |
| `click` | `{ref_id, id}[]` | 点击文档内某元素/链接 |
| `response_length` | `short`/`medium`/`long` | 返回详略程度（默认 `long`） |
| `session_id` | string | 可选：覆盖/接续会话 id |

> 至少提供 `search_query` / `open` / `find` / `click` 中的一项；四项都空会报错。

### `web_fetch`（抓正文）

| 参数 | 类型 | 说明 |
|------|------|------|
| `url` | string（必填） | 要抓取的网址 |

返回剥离脚本/样式/标签后的纯文本。补足「搜到链接却读不到正文、JS 渲染页读不到」的短板。
（注意：纯 JS 动态渲染、需登录的页面仍可能读不到内容，这是服务端 fetch 的能力边界。）

## 调试与排错

直接用 `initialize` / `tools/list` 在终端手动运行 exe 验证握手；或在客户端里用 `/mcp` 查看是否连上。

| 现象 | 原因 / 解决 |
|------|-------------|
| `未找到 Codex 登录凭证` | 没登录。运行 `codex login` 或设置 `CODEX_ACCESS_TOKEN` |
| `Codex 凭证已过期（HTTP 401/403）` | 会话过期，重新 `codex login` |
| `触发 Codex 速率限制（HTTP 429）` | 稍后重试，或减少调用频率 |
| Windows 编译报 `LNK1181: 无法打开输入文件“kernel32.lib”` | `INCLUDE`/`LIB` 用了正斜杠。用 `scripts/build.sh`（已处理反斜杠）或手动导出带 `C:\` 反斜杠的 VS 环境变量 |
| MCP 显示未连接 / `timed out` / `-32000` | 检查 exe 路径是否正确、JSON 是否合法；确认没用 `cmd /c` 包裹命令 |

## 与原项目的差异

| 维度 | pi-gpt-search（原，TS） | 旧版本项目（Node） | **v2.0.0（Rust 重写）** |
|------|------------------------|-------------------|------------------------|
| 语言 | TypeScript | 单文件 Node 脚本 | **Rust** |
| 运行依赖 | Node + TS | Node | **无（独立二进制）** |
| 后端 | Codex | Codex | Codex |
| 工具数 | search/research | 2 | **3**（新增 web_fetch） |
| 引用清理 | PUA 私有区字符 | PUA 私有区字符 | **重写为可读 `[turn0searchN: 标题 → 域名]`** |

## 发布（维护者）

二进制由 GitHub Actions 自动构建（`.github/workflows/release.yml`）：打 tag 即跨平台编译，
并在 GitHub Release 附上 5 个平台的原生二进制（`codex-web-search-mcp-<platform>`）。
用户走「方式 A」下载即用，**无需任何 npm 账号**。

```bash
git tag v2.0.1 && git push origin v2.0.1
```

> 二进制文件名在 CI 里按平台重命名（`win32-x64` / `darwin-universal` 等），与上方「方式 A」表格一致。
