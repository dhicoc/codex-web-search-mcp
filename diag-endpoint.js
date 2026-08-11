#!/usr/bin/env node
"use strict";
// 诊断：直接打 Codex 端点，看 search / open 返回的 results 结构里是否有
// click 所需的元素数字 id，以及 output 里是否有可点击元素标记。
const fs = require("fs");
const os = require("os");
const path = require("path");

const ENDPOINT = "https://chatgpt.com/backend-api/codex/alpha/search";
const UA = "codex-cli/0.147.0-alpha.6.5";

function loadAuth() {
  const authPath = path.join(os.homedir(), ".codex", "auth.json");
  const p = JSON.parse(fs.readFileSync(authPath, "utf8"));
  return { token: p.tokens.access_token, accountId: p.tokens.account_id };
}

async function call(auth, commands, id) {
  const headers = {
    Authorization: `Bearer ${auth.token}`,
    "Content-Type": "application/json",
    "User-Agent": UA,
  };
  if (auth.accountId) headers["ChatGPT-Account-ID"] = auth.accountId;
  const resp = await fetch(ENDPOINT, {
    method: "POST",
    headers,
    body: JSON.stringify({ id, model: "gpt-4o", commands }),
  });
  return { status: resp.status, body: await resp.json() };
}

(async () => {
  const auth = loadAuth();
  const session = "diag_" + Math.random().toString(36).slice(2, 8);

  // 1) search
  const s = await call(auth, { search_query: [{ q: "Rust 1.96 release notes" }] }, session);
  console.log("=== SEARCH status", s.status, "===");
  const sResults = s.body.results || [];
  console.log("search results count:", sResults.length);
  console.log("search results[0] keys:", Object.keys(sResults[0] || {}));
  console.log("search results[0] sample:", JSON.stringify(sResults[0] || {}, null, 1).slice(0, 500));
  const refId = sResults[0] && sResults[0].ref_id;
  console.log("first ref_id:", refId);

  // 2) open
  if (!refId) { console.log("NO ref_id, stop"); return; }
  const o = await call(auth, { open: [{ ref_id: refId }] }, session);
  console.log("\n=== OPEN status", o.status, "===");
  const oResults = o.body.results || [];
  console.log("open results count:", oResults.length);
  if (oResults.length) {
    console.log("open results[0] keys:", Object.keys(oResults[0]));
    console.log("open results[0] full:", JSON.stringify(oResults[0], null, 1).slice(0, 800));
  }
  const out = typeof o.body.output === "string" ? o.body.output : "";
  console.log("\nopen output length:", out.length);
  console.log("open output first 1500 chars:\n" + out.slice(0, 1500));
  // 查找可能的可点击元素 id 标记
  const idMarks = out.match(/\[id[:\s]*\d+\]/gi) || out.match(/\bid[:\s]*\d+/gi);
  console.log("\n可能的 id 标记:", idMarks ? idMarks.slice(0, 20) : "无");
})().catch((e) => { console.error("ERR", e); process.exit(1); });
