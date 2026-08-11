# codex-web-search-mcp

一个**零依赖**的 MCP (Model Context Protocol) server，把 OpenAI Codex 的独立搜索端点
（`chatgpt.com/backend-api/codex/alpha/search`）封装成 Claude Code 可用的两个工具：

- **`codex_web_search`** —— 单步快速搜索（问一句、返回答案 + 来源列表）；
- **`codex_web_research`** —— 多步深度研究（搜 → 打开文档 → 页内查找 → 点击链接，靠 `ref_id` 串联）。

> 灵感与端点实现来自 [mateusdcc/pi-gpt-search](https://github.com/mateusdcc/pi-gpt-search)（MIT）。
> 原项目是给 **Pi Coding Agent**（`pi` CLI）用的插件，无法在 Claude Code 里直接跑；
> 这里重写为 Claude Code 可用的 MCP server。

## 解决什么问题

Claude Code 原生的 `WebSearch` / `WebFetch` 工具是**绑定 Anthropic API** 的。
一旦你把基座模型换成 Gemini、OpenRouter、本地模型等非 Anthropic 模型，这些工具就会失灵或体验很差。

本工具直连 Codex 的独立搜索端点，**与底层模型完全无关**——无论 Claude Code 当前用哪个模型，
都能通过 MCP 工具获得实时联网搜索能力，且不会消耗 GPT/Codex 的推理 token。

## 工作原理

```
Claude Code（任意模型）
   ├── codex_web_search(query)              # 单步快速搜索
   │      └── POST /codex/alpha/search  { commands: { search_query:[{q}] } }
   │
   └── codex_web_research(...)              # 多步深度研究
          └── POST /codex/alpha/search  { commands: { search_query/open/find/click } }
                └── 同一会话(id)内靠 ref_id 串联多次操作
                      search → 拿到 ref_id(turn0search0)
                      open(ref_id) → 返回文档正文
                      find(ref_id, pattern) → 在文档内定位
                      click(ref_id, id) → 跟随链接
```

- 端点不执行 GPT 推理，只返回结构化搜索结果（零 GPT token）。
- `model` 字段仅作为接口要求的标签（固定 `gpt-4o`），不代表实际调用 GPT。
- **`search_query` / `open` / `find` / `click` 都是同一个端点 `commands` 里的并列操作**，
  后端靠请求体的 `id`（会话 id）维持上下文，使后续 `open`/`find`/`click` 能解析上一次搜索返回的 `ref_id`。
  本 server 在多次 tool call 之间**复用同一会话 id**，并把 `ref_id` 暴露在来源列表里，模型即可多轮编排。
- 502/503/504 会自动重试最多 2 次。

## 依赖与环境

1. **Node.js** v18+（v22 已自带全局 `fetch`，无需安装任何 npm 包）。
2. **Codex 登录凭证**，二选一：
   - 运行过 `codex login`（会在 `~/.codex/auth.json` 写入 `tokens.access_token`）；
   - 或设置环境变量 `CODEX_ACCESS_TOKEN`（可选 `CODEX_ACCOUNT_ID`）。

> 没有 ChatGPT/Codex 账号或会话过期时，工具会返回清晰的中文报错，而不是崩溃。

## 安装与配置（Claude Code）

> ⚠️ **Windows 关键坑（已踩过）**：配置里**千万不要用 `cmd /c` 包裹命令**（例如 `"command": "cmd", "args": ["/c", "npx", ...]`）。
> `cmd /c` 会破坏 MCP 的 stdio 管道，导致 Claude Code 握手超时，报错
> `Failed to reconnect ... connection timed out after 30000ms`。
> **Claude Code 会自己直接拉起 `command`**，所以 `command` 直接写裸命令名即可——
> `"command": "npx"` / `"command": "codex-web-search-mcp"` / `"command": "node"`，**不要套 `cmd /c`**。
> 本仓库下面的三种方式都没有 `cmd /c`，照抄即可。

### 方式 A：从 GitHub 安装（★ 推荐，无需 npm 账号 / 密码）

`npx` 和 `npm` 都支持**直接从 GitHub 仓库安装**——所以你既不用注册/登录 npm，也不用把包装到 npmjs.com。
本仓库已验证可这样安装（`npm pack github:dhicoc/codex-web-search-mcp` 通过，含 `bin` 正常解析）。

在项目根（或任意位置）创建 `.mcp.json`：

```json
{
  "mcpServers": {
    "codex-web-search": {
      "command": "npx",
      "args": ["-y", "github:dhicoc/codex-web-search-mcp"]
    }
  }
}
```

- 首次会自动 clone 并运行，之后走 npx 本地缓存；**无需 node 路径、无需手动 clone 到本地**。
- 运行时会执行仓库里的 `bin`（`codex-web-search-mcp.js`，含 `#!/usr/bin/env node` shebang），所以不用你指定 node 路径。
- 升级：重新执行 `npx -y github:dhicoc/codex-web-search-mcp`（或下面方式 B 的全局安装）即拉取最新提交。
- 把同样内容写进用户级 `~/.claude.json` 的 `mcpServers`，即可对所有项目生效。

### 方式 B：全局安装后用命令名（可选）

从 GitHub 全局安装：

```bash
npm install -g github:dhicoc/codex-web-search-mcp
```

随后在 `.mcp.json` 里直接用命令名（同样无需 node 路径）：

```json
{
  "mcpServers": {
    "codex-web-search": {
      "command": "codex-web-search-mcp"
    }
  }
}
```

### 方式 C：从源码运行（开发 / 调试用）

把本仓库 clone / 下载下来，在 `.mcp.json` 里用 `node` 指向脚本绝对路径：

```json
{
  "mcpServers": {
    "codex-web-search": {
      "command": "node",
      "args": ["/absolute/path/to/codex-web-search-mcp.js"]
    }
  }
}
```

> 首次在 Claude Code 里运行 `/mcp` 查看是否连上，首次会要求批准；改完重启 Claude Code 即可。

### 发布到 npm（可选，获得更短的命令名）

如果你想要不带 `github:` 前缀的 `npx -y codex-web-search-mcp`（更易记），需要把包装到 npmjs.com。
这需要你有一个 npm 账号——**没有账号或忘了密码都不影响上面三种用法**，只是短命令名要用：

- 没账号：去 https://www.npmjs.com/signup 免费注册一个；
- 忘了密码：去 https://www.npmjs.com/forgot-password 用注册邮箱重置；
- 登录官方源后发布（本仓库 `package.json` 的 `publishConfig` 已锁定官方源）：

```bash
npm login --registry https://registry.npmjs.org/
npm publish
```

包名 `codex-web-search-mcp` 已确认未被占用。

## 使用

配置连上后，直接在对话里让模型去搜就行，例如：

- “帮我搜一下最新版 Rust 的发布说明”
- “查一下 Vite 6 和 Vite 7 的破坏性变更”

模型会自动调用 `codex_web_search` 工具。你也可以显式要求它使用这个工具而不是其他搜索方式。

### `codex_web_search`（单步搜索）

| 参数 | 类型 | 说明 |
|------|------|------|
| `query` | string（必填） | 搜索关键词或问题 |
| `recency` | number | 仅返回最近 N 天内的结果 |
| `domains` | string[] | 限定搜索域名，如 `["github.com"]` |
| `response_length` | `short`/`medium`/`long` | 返回详略程度 |

### `codex_web_research`（多步深度研究）

适合「需要打开官网文档、在长文里找关键段落、跟随链接深挖」的场景。所有操作可在**一次调用**里组合，
也可**分多轮**调用（依靠自动维持的会话上下文，用上一轮返回的 `ref_id` 串联）。来源列表里会带 `(ref: turn0search0)` 这样的 id，
模型在后续 `open`/`find`/`click` 里直接引用即可。

| 参数 | 类型 | 说明 |
|------|------|------|
| `search_query` | `{q, recency?, domains?}[]` | 要执行的搜索查询列表 |
| `open` | `{ref_id, lineno?}[]` | 按 `ref_id` 打开文档/页面 |
| `find` | `{ref_id, pattern}[]` | 在已打开文档中查找关键词 |
| `click` | `{ref_id, id}[]` | 点击文档内某元素/链接 |
| `response_length` | `short`/`medium`/`long` | 返回详略程度（默认 `long`） |
| `session_id` | string | 可选：覆盖/接续会话 id |

> 至少提供 `search_query` / `open` / `find` / `click` 中的一项；四项都空会报错。

典型用法（让模型自己编排即可，无需手动拼参数）：

- “搜一下 Rust 最新版发布说明，打开官方博客，找到 1.96 里关于 async 的改动”
- “查 Vite 7 的迁移指南，打开文档后定位 breaking changes 那一节”

### 调试

设环境变量 `CODEX_SEARCH_DEBUG=1`，server 启动与每次请求会向 stderr 打印日志（含会话 id 与请求 commands）。

## 排错

| 现象 | 原因 / 解决 |
|------|-------------|
| `未找到 Codex 登录凭证` | 没登录。运行 `codex login` 或设置 `CODEX_ACCESS_TOKEN` |
| `Codex 凭证已过期（HTTP 401/403）` | 会话过期，重新 `codex login` |
| `触发 Codex 速率限制（HTTP 429）` | 稍后重试，或减少调用频率 |
| `/mcp` 里显示未连接 | 检查 `node` 是否在 PATH、路径是否正确、JSON 是否合法 |
| `/mcp` 报 `connection timed out after 30000ms` | **Windows 上配置用了 `cmd /c` 包裹命令**，破坏了 MCP stdio 管道。改成裸命令（`npx` / `codex-web-search-mcp` / `node`），去掉 `cmd /c` |

调试时可设环境变量 `CODEX_SEARCH_DEBUG=1`，server 启动时会向 stderr 打印日志。

## 与原项目的差异

| 维度 | pi-gpt-search（原） | 本项目 |
|------|---------------------|--------|
| 运行平台 | Pi Coding Agent（`pi` CLI） | Claude Code（MCP） |
| 接入方式 | `~/.pi/agent/extensions/` 插件 | `node` 启动的 stdio MCP server |
| 暴露形态 | `/gpt-search` 命令 + `codex-search`/`codex-research` 工具 | `codex_web_search` + `codex_web_research` 两个 MCP 工具 |
| 依赖 | TypeScript 项目 | 零依赖单文件 Node 脚本 |
| 研究 harness | 支持 `open`/`find`/`click` 多步研究 | 已实现（`codex_web_research`，会话 id 自动复用） |

## 后续可扩展

- 增加结果缓存，降低重复查询的速率限制风险。
- 把 `open` 返回的页面内链接 `ref_id` 显式抽取成结构化列表，进一步降低模型引用成本。
