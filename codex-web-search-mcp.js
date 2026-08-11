#!/usr/bin/env node
"use strict";

// ============================================================================
// codex-web-search-mcp
// 一个零依赖的 MCP (Model Context Protocol) server，把 OpenAI Codex 的独立
// 搜索端点封装成 Claude Code 可用的联网搜索工具。
//
// 提供两个工具：
//   - codex_web_search  : 单步快速搜索（问一句、返回答案 + 来源列表）
//   - codex_web_research: 多步深度研究（search → open → find → click 串联）
//
// 用途：当 Claude Code 接入非 Anthropic 模型（Gemini / OpenRouter / 本地模型）
// 时，原生 WebSearch/WebFetch 往往失效或体验很差。本工具直连 Codex 搜索端点，
// 与底层模型完全无关，任何模型都能通过 MCP 工具获得实时联网搜索能力。
//
// 原理来自 https://github.com/mateusdcc/pi-gpt-search （MIT），这里重写为
// Claude Code 可用的 MCP server，并补上 open/find/click 多步研究能力。
//
// 关键机制（来自原项目逆向结论）：
//   - 所有操作（search/open/find/click）都打同一个端点，请求体里 commands
//     字段并列承载这些操作。
//   - 请求体的 `id` 是「会话 id」，后端靠它维持上下文，使后续 open/find/click
//     能解析上一次搜索返回的 ref_id（如 turn0search0）。
//   - 因此本 server 在多次 tool call 之间复用同一 session id，并把 ref_id 暴露
//     给模型，模型便能多轮编排「搜 → 打开 → 页内查找 → 点击」。
// ============================================================================

const fs = require("fs");
const os = require("os");
const path = require("path");

const ENDPOINT = "https://chatgpt.com/backend-api/codex/alpha/search";
const DEFAULT_MODEL = "gpt-4o"; // 仅作为端点要求的标签，不会真正消耗 GPT 推理
const TIMEOUT_MS = 20000;
const UA = "codex-cli/0.147.0-alpha.6.5";
const MAX_RETRIES = 2;

// 持久会话 id：本 server 进程生命周期内复用，保证 open/find/click 能解析
// 之前搜索返回的 ref_id。模块级变量在 MCP server 持续运行期间保持不变。
let sessionId = "ccs_" + Math.random().toString(36).slice(2, 12);

// ---------------------------------------------------------------------------
// 鉴权：优先环境变量，否则读取 `codex login` 生成的 ~/.codex/auth.json
// ---------------------------------------------------------------------------
function loadCodexAuth() {
  if (process.env.CODEX_ACCESS_TOKEN) {
    return {
      accessToken: process.env.CODEX_ACCESS_TOKEN,
      accountId: process.env.CODEX_ACCOUNT_ID || undefined,
    };
  }
  const authPath = path.join(os.homedir(), ".codex", "auth.json");
  if (fs.existsSync(authPath)) {
    try {
      const parsed = JSON.parse(fs.readFileSync(authPath, "utf8"));
      const accessToken = parsed.tokens && parsed.tokens.access_token;
      if (typeof accessToken === "string" && accessToken) {
        const accountId =
          parsed.tokens && typeof parsed.tokens.account_id === "string"
            ? parsed.tokens.account_id
            : undefined;
        return { accessToken, accountId };
      }
    } catch (_) {
      /* ignore */
    }
  }
  return null;
}

// ---------------------------------------------------------------------------
// 响应归一化
// ---------------------------------------------------------------------------
function normalizeRaw(item) {
  if (typeof item !== "object" || item === null) return null;
  const o = item;
  const url = typeof o.url === "string" ? o.url.trim() : undefined;
  const refId = typeof o.ref_id === "string" ? o.ref_id.trim() : undefined;
  const title = typeof o.title === "string" ? o.title.trim() : undefined;
  const snippet = typeof o.snippet === "string" ? o.snippet.trim() : undefined;
  const domain = typeof o.domain === "string" ? o.domain.trim() : undefined;
  const type = typeof o.type === "string" ? o.type.trim() : undefined;
  if (!url && !refId && !title && !snippet) return null;
  return { url, refId, title, snippet, domain, type, raw: item };
}

function normalizeBody(body) {
  const results = [];
  if (body && Array.isArray(body.results)) {
    for (const it of body.results) {
      const n = normalizeRaw(it);
      if (n) results.push(n);
    }
  }
  const output = typeof body.output === "string" ? body.output : undefined;
  return { output, results };
}

// 清理 Codex 返回的私有区引用标记（<U+E200>cite<U+E202><id>†<文本>†<域名><U+E201>）。
// 原项目在终端里把它们转成 OSC 8 超链接；Claude Code 文本结果里会变成乱码/豆腐块。
// 关键点：标记里的 <id> 正是 click({ref_id, id}) 所需的元素编号，不能删——
// 转写成可读的 [cN: 文本 → 域名] 暴露给模型，供其跟随链接做深度研究。
// 用 fromCharCode 构造正则与分隔符，避免源码里出现真实私有区字符导致读写损坏。
function cleanOutput(text) {
  const C_START = String.fromCharCode(0xe200); // 引用标记起始
  const C_SEP = String.fromCharCode(0xe202); // 引用标记分隔
  const C_END = String.fromCharCode(0xe201); // 引用标记结束
  const DAG = String.fromCharCode(0x2020); // † 引用内字段分隔符
  const citeRe = new RegExp(
    C_START + "cite" + C_SEP + "([^" + C_END + "]*)" + C_END,
    "g"
  );
  const puaRe = new RegExp(
    "[" + String.fromCharCode(0xe000) + "-" + String.fromCharCode(0xf8ff) + "]",
    "g"
  );
  return text
    .replace(citeRe, (_m, inner) => {
      const parts = String(inner).split(DAG);
      const cid = (parts[0] || "").trim();
      const label = (parts[1] || "").trim();
      const domain = (parts[2] || "").trim();
      if (!cid) return "";
      return ` [c${cid}: ${label}${domain ? " → " + domain : ""}]`;
    })
    .replace(puaRe, "") // 兜底剥离其它私有区字符
    .replace(/\[wordlim:\s*\d+\]/g, "") // 剥离后端词数限制标记
    .replace(/(^|[\s ])L\d+:\s?/gm, "$1") // 剥离行号前缀（含行首空格/不间断空格及行内导航菜单中的内联行号）
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

// 把归一化结果格式化为文本。研究模式下把 ref_id 一并暴露给模型，
// 这样它能在后续 open/find/click 里引用这些 id。
function formatText(normalized, { exposeRefIds = true } = {}) {
  const lines = [];
  if (normalized.output && normalized.output.trim()) {
    lines.push(cleanOutput(normalized.output));
  }
  if (normalized.results.length) {
    lines.push("");
    lines.push("Sources:");
    normalized.results.forEach((r, i) => {
      const title = r.title || r.url || "(untitled)";
      if (exposeRefIds && r.refId) {
        lines.push(`[${i + 1}] (ref: ${r.refId}) ${title}`);
      } else {
        lines.push(`[${i + 1}] ${title}`);
      }
      if (r.url) lines.push(`    ${r.url}`);
      if (r.domain && !r.url) lines.push(`    ${r.domain}`);
      if (r.snippet) lines.push(`    ${r.snippet}`);
    });
  }
  if (!lines.length) return "No results returned.";
  let out = lines.join("\n");
  // 文档内若存在 [cN: ...] 可点击标记，提示模型可用 click({ ref_id, id: N }) 跟随链接
  if (exposeRefIds && /\[c\d+:/i.test(out)) {
    out +=
      "\n\n（文档内容里 [cN: ...] 标记即是可点击的链接元素，编号 N 可直接用于 click({ ref_id, id: N }) 跟随该链接继续深挖）";
  }
  return out;
}

// ---------------------------------------------------------------------------
// 通用执行：向端点提交一个 commands 对象（search_query/open/find/click/...）
// ---------------------------------------------------------------------------
async function codexExecute(commands, signal) {
  const auth = loadCodexAuth();
  if (!auth) {
    const e = new Error(
      "未找到 Codex 登录凭证。请先运行 `codex login`，或设置环境变量 CODEX_ACCESS_TOKEN（与可选的 CODEX_ACCOUNT_ID）。"
    );
    e.code = "NO_AUTH";
    throw e;
  }

  const payload = {
    id: sessionId,
    model: DEFAULT_MODEL,
    commands,
  };

  const headers = {
    Authorization: `Bearer ${auth.accessToken}`,
    "Content-Type": "application/json",
    "User-Agent": UA,
  };
  if (auth.accountId) headers["ChatGPT-Account-ID"] = auth.accountId;

  if (process.env.CODEX_SEARCH_DEBUG) {
    process.stderr.write(
      `[codex-web-search] id=${sessionId} cmd=${JSON.stringify(commands)}\n`
    );
  }

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort("timeout"), TIMEOUT_MS);
  if (signal) signal.addEventListener("abort", () => controller.abort(), { once: true });

  try {
    let resp = null;
    let attempt = 0;
    while (attempt <= MAX_RETRIES) {
      attempt++;
      resp = await fetch(ENDPOINT, {
        method: "POST",
        headers,
        body: JSON.stringify(payload),
        signal: controller.signal,
      });
      if ([502, 503, 504].includes(resp.status) && attempt <= MAX_RETRIES) {
        await new Promise((r) => setTimeout(r, 500 * attempt));
        continue;
      }
      break;
    }
    if (!resp) throw new Error("NO_RESPONSE");

    if (resp.status === 401 || resp.status === 403) {
      const e = new Error("Codex 凭证已过期（HTTP 401/403）。请重新运行 `codex login`。");
      e.code = "AUTH_EXPIRED";
      throw e;
    }
    if (resp.status === 429) {
      const e = new Error("触发 Codex 速率限制（HTTP 429）。请稍后重试。");
      e.code = "RATE_LIMITED";
      throw e;
    }
    if (!resp.ok) {
      const t = await resp.text().catch(() => "");
      const e = new Error(`Codex 请求失败（HTTP ${resp.status}）: ${t.slice(0, 200)}`);
      e.code = `HTTP_${resp.status}`;
      throw e;
    }
    const body = await resp.json();
    return normalizeBody(body);
  } finally {
    clearTimeout(timer);
  }
}

// ---------------------------------------------------------------------------
// 操作构造器：把工具参数转成端点要求的 commands 形状（带宽松校验）
// ---------------------------------------------------------------------------
function buildSearchCommands(args) {
  const sq = { q: String(args.query).trim() };
  if (typeof args.recency === "number" && isFinite(args.recency)) sq.recency = args.recency;
  if (Array.isArray(args.domains)) {
    sq.domains = args.domains
      .filter((d) => typeof d === "string" && d.trim())
      .map((d) => d.trim());
  }
  const commands = { search_query: [sq] };
  if (args.response_length) commands.response_length = args.response_length;
  return commands;
}

function str(v) {
  return typeof v === "string" ? v.trim() : "";
}

function buildResearchCommands(args) {
  const commands = {};

  if (Array.isArray(args.search_query)) {
    const arr = [];
    for (const sq of args.search_query) {
      if (typeof sq !== "object" || sq === null) continue;
      const q = str(sq.q);
      if (!q) continue;
      const item = { q };
      if (typeof sq.recency === "number" && isFinite(sq.recency)) item.recency = sq.recency;
      if (Array.isArray(sq.domains)) {
        const ds = sq.domains.filter((d) => typeof d === "string" && d.trim()).map((d) => d.trim());
        if (ds.length) item.domains = ds;
      }
      arr.push(item);
    }
    if (arr.length) commands.search_query = arr;
  }

  if (Array.isArray(args.open)) {
    const arr = [];
    for (const op of args.open) {
      if (typeof op !== "object" || op === null) continue;
      const ref = str(op.ref_id);
      if (!ref) continue;
      const item = { ref_id: ref };
      if (typeof op.lineno === "number" && isFinite(op.lineno)) item.lineno = op.lineno;
      arr.push(item);
    }
    if (arr.length) commands.open = arr;
  }

  if (Array.isArray(args.find)) {
    const arr = [];
    for (const fn of args.find) {
      if (typeof fn !== "object" || fn === null) continue;
      const ref = str(fn.ref_id);
      const pattern = str(fn.pattern);
      if (!ref || !pattern) continue;
      arr.push({ ref_id: ref, pattern });
    }
    if (arr.length) commands.find = arr;
  }

  if (Array.isArray(args.click)) {
    const arr = [];
    for (const cl of args.click) {
      if (typeof cl !== "object" || cl === null) continue;
      const ref = str(cl.ref_id);
      if (!ref) continue;
      const id = Number(cl.id);
      if (!isFinite(id)) continue;
      arr.push({ ref_id: ref, id });
    }
    if (arr.length) commands.click = arr;
  }

  if (args.response_length) commands.response_length = args.response_length;
  return commands;
}

// ---------------------------------------------------------------------------
// MCP tools 定义
// ---------------------------------------------------------------------------
const TOOL_SEARCH = {
  name: "codex_web_search",
  description:
    "通过 OpenAI Codex 的独立搜索端点执行实时联网搜索，返回模型整理的答案文本以及带标题、URL、摘要的来源列表（来源含 ref_id，可用于 codex_web_research 进一步打开/查找）。" +
    "该工具与底层模型无关——即使 Claude Code 接入的是 Gemini / OpenRouter / 本地模型等非 Anthropic 模型也能正常使用，" +
    "弥补原生 WebSearch 在非 Anthropic 模型下失效或体验差的短板。" +
    "需要有效的 Codex 登录凭证（codex login 或 CODEX_ACCESS_TOKEN 环境变量）。",
  inputSchema: {
    type: "object",
    properties: {
      query: { type: "string", description: "搜索关键词或问题。" },
      recency: { type: "number", description: "仅返回最近 N 天内的结果（可选）。" },
      domains: {
        type: "array",
        items: { type: "string" },
        description: '限定搜索域名列表，例如 ["github.com", "docs.python.org"]（可选）。',
      },
      response_length: {
        type: "string",
        enum: ["short", "medium", "long"],
        description: "返回内容详略程度（可选，默认 medium）。",
      },
    },
    required: ["query"],
  },
};

const TOOL_RESEARCH = {
  name: "codex_web_research",
  description:
    "多步深度研究工具：在 Codex 的联网检索 + 文档浏览引擎上执行「搜索 → 打开文档 → 页内查找 → 点击链接」的迭代研究。" +
    "所有操作可在一次调用中组合，也可分多轮调用（依靠自动维持的会话上下文，用上一轮返回的 ref_id 串联）。" +
    "典型流程：先用 search_query 搜索，拿到 ref_id 后用 open(ref_id) 打开权威文档，用 find(ref_id, pattern) 在长文档里定位关键段落，" +
    "必要时用 click(ref_id, id) 跟随链接。open 返回的文档正文里会内联 [cN: 文本 → 域名] 标记，" +
    "其中的编号 N 就是可点击元素的 id，直接用于 click({ ref_id, id: N })。返回内容同时含 ref_id 以便后续操作引用。" +
    "与底层模型无关，适合非 Anthropic 模型下的联网调研。需要有效的 Codex 登录凭证。",
  inputSchema: {
    type: "object",
    properties: {
      search_query: {
        type: "array",
        items: {
          type: "object",
          properties: {
            q: { type: "string", description: "搜索查询字符串" },
            recency: { type: "number", description: "时间新鲜度过滤（天），可选" },
            domains: {
              type: "array",
              items: { type: "string" },
              description: "限定域名列表，可选",
            },
          },
        },
        description: "要执行的搜索查询列表（可选，但至少需提供一项操作）。",
      },
      open: {
        type: "array",
        items: {
          type: "object",
          properties: {
            ref_id: { type: "string", description: "要打开的搜索结果或文档的 ref_id，例如 turn0search0" },
            lineno: { type: "number", description: "跳转到指定行号（可选）" },
          },
        },
        description: "按 ref_id 打开文档/页面（可选）。",
      },
      find: {
        type: "array",
        items: {
          type: "object",
          properties: {
            ref_id: { type: "string", description: "已打开文档的 ref_id" },
            pattern: { type: "string", description: "要在文档中查找的关键词/模式" },
          },
        },
        description: "在已打开文档中查找关键词（可选）。",
      },
      click: {
        type: "array",
        items: {
          type: "object",
          properties: {
            ref_id: { type: "string", description: "文档的 ref_id" },
            id: { type: "number", description: "要点击的元素/链接 id" },
          },
        },
        description: "点击文档内某元素/链接（可选）。",
      },
      response_length: {
        type: "string",
        enum: ["short", "medium", "long"],
        description: "返回内容详略程度（可选，默认 long）。",
      },
      session_id: {
        type: "string",
        description: "可选：覆盖本次研究的会话 id。不传则复用 server 自动维持的会话，从而接续之前搜索得到的 ref_id 上下文。",
      },
    },
  },
};

const TOOLS = [TOOL_SEARCH, TOOL_RESEARCH];

// ---------------------------------------------------------------------------
// MCP server：stdio 传输，换行分隔的 JSON-RPC 2.0
// ---------------------------------------------------------------------------
// 后端有时返回 HTTP 200 但正文是 "Internal Error / Unable to resolve ..."
// （例如 click 的 ref_id 用错、或 id 不存在）。这类应判为错误，让模型知道并重试。
function isBackendError(norm) {
  const o = norm.output || "";
  return (
    /Internal Error/i.test(o) ||
    /Unable to resolve/i.test(o) ||
    /due to invalid arguments/i.test(o)
  );
}

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}

function handleToolCall(name, args, id) {
  if (name === TOOL_SEARCH.name) {
    if (typeof args.query !== "string" || !args.query.trim()) {
      send({
        jsonrpc: "2.0",
        id,
        result: { content: [{ type: "text", text: "参数 query 不能为空。" }], isError: true },
      });
      return;
    }
    codexExecute(buildSearchCommands(args))
      .then((norm) => {
        send({
          jsonrpc: "2.0",
          id,
          result: {
            content: [{ type: "text", text: formatText(norm) }],
            isError: isBackendError(norm),
          },
        });
      })
      .catch((err) => {
        send({
          jsonrpc: "2.0",
          id,
          result: { content: [{ type: "text", text: `搜索出错: ${err.message}` }], isError: true },
        });
      });
    return;
  }

  if (name === TOOL_RESEARCH.name) {
    if (typeof args.session_id === "string" && args.session_id.trim()) {
      sessionId = args.session_id.trim();
    }
    const commands = buildResearchCommands(args);
    if (!commands.search_query && !commands.open && !commands.find && !commands.click) {
      send({
        jsonrpc: "2.0",
        id,
        result: {
          content: [
            { type: "text", text: "至少需要提供一项操作：search_query / open / find / click。" },
          ],
          isError: true,
        },
      });
      return;
    }
    codexExecute(commands)
      .then((norm) => {
        send({
          jsonrpc: "2.0",
          id,
          result: {
            content: [{ type: "text", text: formatText(norm) }],
            isError: isBackendError(norm),
          },
        });
      })
      .catch((err) => {
        send({
          jsonrpc: "2.0",
          id,
          result: { content: [{ type: "text", text: `研究出错: ${err.message}` }], isError: true },
        });
      });
    return;
  }

  send({
    jsonrpc: "2.0",
    id,
    result: { content: [{ type: "text", text: `未知工具: ${name}` }], isError: true },
  });
}

function handle(msg) {
  if (!msg || typeof msg !== "object") return;
  const id = msg.id;

  if (msg.method === "initialize") {
    send({
      jsonrpc: "2.0",
      id,
      result: {
        protocolVersion: (msg.params && msg.params.protocolVersion) || "2024-11-05",
        capabilities: { tools: {} },
        serverInfo: { name: "codex-web-search", version: "1.1.0" },
      },
    });
    return;
  }

  // 通知类，无需回复
  if (msg.method === "notifications/initialized" || msg.method === "initialized") {
    return;
  }

  if (msg.method === "tools/list") {
    send({ jsonrpc: "2.0", id, result: { tools: TOOLS } });
    return;
  }

  if (msg.method === "tools/call") {
    const name = msg.params && msg.params.name;
    const args = (msg.params && msg.params.arguments) || {};
    handleToolCall(name, args, id);
    return;
  }

  if (msg.method === "ping") {
    send({ jsonrpc: "2.0", id, result: {} });
    return;
  }

  // 未知方法
  if (id !== undefined && id !== null) {
    send({
      jsonrpc: "2.0",
      id,
      error: { code: -32601, message: `Method not found: ${msg.method}` },
    });
  }
}

let buf = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buf += chunk;
  let idx;
  while ((idx = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, idx).trim();
    buf = buf.slice(idx + 1);
    if (line) {
      try {
        handle(JSON.parse(line));
      } catch (_) {
        /* 非法行，忽略 */
      }
    }
  }
});
// stdin 关闭时不再强制退出：让进行中的异步请求（网络搜索）自然完成后再退出，
// 避免管道测试或客户端提前关闭 stdin 时杀掉正在执行的搜索。事件循环清空后会自然退出。
process.stdin.on("end", () => {
  /* 不调用 process.exit，交给事件循环在请求完成后自然退出 */
});

if (process.env.CODEX_SEARCH_DEBUG) {
  process.stderr.write(`[codex-web-search] MCP server started (session=${sessionId})\n`);
}
