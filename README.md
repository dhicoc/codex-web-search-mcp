# codex-web-search-mcp

一个**模型无关**的 MCP (Model Context Protocol) server，把 OpenAI Codex 的独立搜索端点
（`chatgpt.com/backend-api/codex/alpha/search`）封装成 Claude Code / 任意 MCP 客户端可用的联网搜索工具。
**v2.0.0 起用 Rust 全量重写**，并参考 [Episkey-G/GrokSearch-rs](https://github.com/Episkey-G/GrokSearch-rs) 做了能力升级：

- 默认后端 **OpenAI Codex**（免费，只要 `codex login` 登录态），可选后端 **Grok/xAI**；
- 6 个工具：`codex_web_search`、`codex_web_research`、`web_fetch`、`get_sources`（分页+预算）、`doctor`、`web_map`；
- 独立二进制，**不再依赖 Node / npx**（编译一次，直接运行 exe）。

> 灵感与端点实现来自 [mateusdcc/pi-gpt-search](https://github.com/mateusdcc/pi-gpt-search)（MIT）；
> v2 的能力模型（多后端、web_fetch、doctor、预算控制、web_map）参考 [Episkey-G/GrokSearch-rs](https://github.com/Episkey-G/GrokSearch-rs)。

## 解决什么问题

Claude Code 原生的 `WebSearch` / `WebFetch` 工具**绑定 Anthropic API**。一旦把基座模型换成
Gemini、OpenRouter、本地模型等非 Anthropic 模型，这些工具就会失灵。

本工具直连 Codex（或可选 Grok）的独立搜索端点，**与底层模型完全无关**——无论客户端用哪个模型，
都能通过 MCP 工具获得实时联网搜索能力，且**不消耗 GPT/Codex 的推理 token**（只占用账号搜索额度，见下）。

## 工作原理

```
Claude Code / 任意 MCP 客户端（任意模型）
   ├── codex_web_search(query)          # 单步快速搜索
   ├── codex_web_research(...)          # 多步深度研究（search→open→find→click，靠 ref_id 串联）
   ├── web_fetch(url)                   # 抓取任意 URL 纯文本（补足搜到却读不到正文的短板）
   ├── get_sources(...)                 # 分页重取上一次来源（避免重复搜索）
   ├── doctor()                         # 自检：连通性 + 脱敏配置
   └── web_map(url)                     # Tavily Map 发现域名下 URL
          │
          ▼
   ┌─────────────────────┐   默认    ┌──────────────────────────┐
   │  OpenAI Codex 搜索   │ ───────▶ │ /backend-api/codex/alpha  │
   │  端点（免费登录态）  │          │ /search                   │
   └─────────────────────┘          └──────────────────────────┘
   ┌─────────────────────┐   可选    ┌──────────────────────────┐
   │  Grok / xAI          │ ───────▶ │ /v1/responses + web_search│
   └─────────────────────┘          └──────────────────────────┘
```

- Codex 端点不执行 GPT 推理，只返回结构化搜索结果（**零 GPT token**）。
- `model` 字段仅作为接口要求的标签（固定 `gpt-4o`），不代表实际调用 GPT。
- `search_query` / `open` / `find` / `click` 都是同一个端点 `commands` 里的并列操作，后端靠请求体的
  会话 `id` 维持上下文，使后续 `open`/`find`/`click` 能解析上一次搜索返回的 `ref_id`。本 server 在多次
  tool call 之间复用同一会话 id，并把 `ref_id` 暴露在来源列表里，模型即可多轮编排。
- 引用标记现在返回为 `[turn0searchN: 标题 → 域名]` 形式（旧版是 PUA 私有区字符，已重写清理），
  模型可直接拿 `turn0searchN` 这种 `ref_id` 去做 `open` / `click`。

## 依赖与环境

1. **Rust 工具链**（`rustup` + `cargo`，stable 即可）。
   - **Windows**：需要 **Visual Studio 2023 的 MSVC 工具链**（含 `link.exe`）+ Windows SDK。
     Git Bash 自带的 `/usr/bin/link` 会抢占 MSVC 的 `link.exe`，且本沙箱禁止从 Bash/PowerShell 调
     `cmd.exe`，所以**不能用 `cmd /c vcvarsall.bat`**。本仓库的 `scripts/build.sh` 改为直接导出 VS
     环境变量来编译（见下文「编译」）。
   - **macOS / Linux**：系统 clang/gcc 即可，标准 `cargo build` 开箱即用。
2. **有效的 Codex 登录凭证（必做）**——直连 Codex 搜索端点，必须有登录态，否则工具返回清晰报错而非崩溃。
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

## 安装与配置（MCP）

Rust 版是**独立二进制**，直接让客户端 spawn 这个 exe（或 macOS/Linux 下的二进制）即可，**不需要 Node**。

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
- 改完重启客户端即可；首次在客户端里查看是否连上（如 Claude Code 的 `/mcp`）。
- 写入用户级配置（如 `~/.claude.json`）的 `mcpServers` 即对所有项目生效。

> 可选项：在 MCP 配置的 `"env"` 里加环境变量（`CODEX_ACCESS_TOKEN`、`CODEX_SEARCH_BACKEND=grok` 等），
> 见下「配置」。若用方式 1 的 `codex login`，连 exe 路径都不用配 env。

> ⚠️ **不要用** `"command": "cmd", "args": ["/c", ...]` —— 会破坏 MCP stdio 管道导致超时 / -32000。

### 旧版 Node 脚本（已弃用，保留作兜底）

仓库里仍保留 `codex-web-search-mcp.js`（v1.x 单文件 Node 版）。如需用 Node 方式运行，参考旧 README
的「方式 A / B」（`node` + 脚本绝对路径，或 `npx -y github:dhicoc/codex-web-search-mcp`）。**推荐用
Rust 版**，无需 Node 且功能更全。

## 配置（config.toml）

默认读取 `~/.config/codex-web-search-mcp/config.toml`（可用 `CODEX_SEARCH_CONFIG` 覆盖路径）。
示例：

```toml
# 可选后端切换：设为 "grok" 且提供 key 时启用 Grok/xAI 后端
# CODEX_SEARCH_BACKEND = "codex"

# Grok / xAI 后端（CODEX_SEARCH_BACKEND=grok 时生效）
grok_api_key   = "xai-..."        # 或环境变量 GROK_SEARCH_API_KEY / XAI_API_KEY
grok_base_url  = "https://api.x.ai"
grok_model     = "grok-4.1-fast"

# web_map 工具所需
tavily_api_key = "tvly-..."

# 预算 / 分页控制
response_max_chars = 8000         # 输出超过该字符数则截断（省 token）
max_inline_sources = 10           # 结果里最多内联展示的来源条数
```

等效的环境变量（优先级高于 config.toml）：`CODEX_SEARCH_BACKEND`、`GROK_SEARCH_API_KEY` /
`XAI_API_KEY`、`GROK_SEARCH_URL`、`GROK_SEARCH_MODEL`、`TAVILY_API_KEY`。

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

### `get_sources`（分页重取来源）

| 参数 | 类型 | 说明 |
|------|------|------|
| `session_id` | string | 可选，默认当前会话 |
| `offset` | number | 起始序号，从 0 |
| `limit` | number | 返回条数，默认 10 |

避免重复搜索：拿上一次 `codex_web_search` / `codex_web_research` 缓存在会话里的来源，分页浏览。

### `doctor`（自检）

无参数。探测 Codex 端点连通性，并脱敏展示当前配置（后端、各 API key 是否已设置）。排错首选。

### `web_map`（域名发现）

| 参数 | 类型 | 说明 |
|------|------|------|
| `url` | string（必填） | 要映射的域名或起始 URL |

经 Tavily Map 发现某域名下的 URL 列表。**需要 `TAVILY_API_KEY`**。

## 调试

直接用 `doctor` 工具做自检；或在终端手动运行 exe 并用 `initialize` / `tools/list` 验证握手。

## 排错

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
| 后端 | Codex | Codex | Codex + **可选 Grok/xAI** |
| 工具数 | search/research | 2 | **6**（新增 web_fetch/get_sources/doctor/web_map） |
| 预算/分页 | — | — | **response_max_chars / max_inline_sources / get_sources** |

## 后续可扩展

- 多 provider 链（Exa / Firecrawl）接入 `web_fetch` / `web_map`（`source_providers` 配置位已预留思路）。
- `codex_web_research` 的 `open` 返回页内链接 `ref_id` 结构化抽取，进一步降低模型引用成本。
