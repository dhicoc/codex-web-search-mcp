#!/usr/bin/env node
"use strict";

// test-auth.js —— 快速验证当前 Codex 登录态能否调用独立搜索端点。
// 用法：node test-auth.js   （在自己登录过 codex login 的机器上跑）

const fs = require("fs");
const os = require("os");
const path = require("path");

const ENDPOINT = "https://chatgpt.com/backend-api/codex/alpha/search";
const MODEL = "gpt-4o";
const UA = "codex-cli/0.147.0-alpha.6.5";
const TIMEOUT_MS = 15000;

function loadCodexAuth() {
  if (process.env.CODEX_ACCESS_TOKEN) {
    return {
      accessToken: process.env.CODEX_ACCESS_TOKEN,
      accountId: process.env.CODEX_ACCOUNT_ID || undefined,
      source: "环境变量 CODEX_ACCESS_TOKEN",
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
        return { accessToken, accountId, source: authPath };
      }
    } catch (_) {
      /* ignore */
    }
  }
  return null;
}

function interpret(status) {
  switch (status) {
    case 200:
      return "✅ 端点接受你的凭证，免费账号可用！";
    case 401:
    case 403:
      return "❌ 凭证被拒（401/403）——免费档很可能被这个端点限制，需 Plus/Pro。";
    case 429:
      return "⚠️ 速率限制（429）——凭证被接受但额度极低，能用但容易限流。";
    case 400:
      return "⚠️ 请求格式问题（400），但凭证本身通过了。";
    default:
      return `⚠️ 非预期状态码 ${status}。`;
  }
}

(async () => {
  const auth = loadCodexAuth();
  if (!auth) {
    console.error("未找到 Codex 登录凭证。");
    console.error("请先运行 `codex login`，或设置环境变量 CODEX_ACCESS_TOKEN。");
    process.exit(1);
  }
  console.log(`凭证来源: ${auth.source}`);
  console.log(`端点: ${ENDPOINT}`);
  console.log("发送单次测试搜索 (query=openai codex) ...\n");

  const payload = {
    id: "test_auth_1",
    model: MODEL,
    commands: { search_query: [{ q: "openai codex" }] },
  };
  const headers = {
    Authorization: `Bearer ${auth.accessToken}`,
    "Content-Type": "application/json",
    "User-Agent": UA,
  };
  if (auth.accountId) headers["ChatGPT-Account-ID"] = auth.accountId;

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort("timeout"), TIMEOUT_MS);
  const start = Date.now();

  try {
    const resp = await fetch(ENDPOINT, {
      method: "POST",
      headers,
      body: JSON.stringify(payload),
      signal: controller.signal,
    });
    const elapsed = Date.now() - start;
    console.log(`HTTP 状态码: ${resp.status}  (耗时 ${elapsed}ms)`);
    console.log(interpret(resp.status));

    if (resp.status === 200) {
      const body = await resp.json();
      const results = Array.isArray(body.results) ? body.results : [];
      console.log(`\n返回结果数: ${results.length}`);
      if (results.length) {
        console.log("前 3 条：");
        results.slice(0, 3).forEach((r, i) => {
          console.log(`  [${i + 1}] ${r.title || "(无标题)"}`);
          if (r.url) console.log(`      ${r.url}`);
        });
      }
      if (body.output) {
        console.log(`\n模型整理文本(前 200 字):\n${String(body.output).slice(0, 200)}`);
      }
    } else {
      const text = await resp.text().catch(() => "");
      if (text) console.log(`响应体(截取): ${text.slice(0, 300)}`);
    }
    process.exit(0);
  } catch (err) {
    const elapsed = Date.now() - start;
    console.error(`请求异常 (耗时 ${elapsed}ms): ${err.message}`);
    if (err.message === "timeout" || (err.name === "AbortError")) {
      console.error("请求超时，可能是网络问题或端点不可达。");
    }
    process.exit(2);
  } finally {
    clearTimeout(timer);
  }
})();
