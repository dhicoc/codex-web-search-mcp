#!/usr/bin/env node
"use strict";
// 动态串联测试：spawn MCP server，搜索 -> 提取 ref_id -> open -> find
// 用真实 Codex 凭证（来自 ~/.codex/auth.json）验证多步研究链路。
const { spawn } = require("child_process");
const path = require("path");

const SERVER = path.join(__dirname, "codex-web-search-mcp.js");
const child = spawn(process.execPath, [SERVER], { stdio: ["pipe", "pipe", "inherit"] });

let pending = new Map();
let buf = "";
child.stdout.setEncoding("utf8");
child.stdout.on("data", (chunk) => {
  buf += chunk;
  let idx;
  while ((idx = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, idx).trim();
    buf = buf.slice(idx + 1);
    if (!line) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id !== undefined && pending.has(msg.id)) {
      const resolve = pending.get(msg.id);
      pending.delete(msg.id);
      resolve(msg);
    }
  }
});

let nextId = 1;
function rpc(method, params) {
  return new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, resolve);
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
    setTimeout(() => {
      if (pending.has(id)) { pending.delete(id); reject(new Error("timeout for " + method)); }
    }, 25000);
  });
}

function textOf(resp) {
  if (!resp.result || !resp.result.content) return "(no content)";
  return resp.result.content.map((c) => c.text).join("\n");
}

function firstRefId(text) {
  const m = text.match(/\(ref:\s*([^)]+)\)/);
  return m ? m[1] : null;
}

async function main() {
  await rpc("initialize", { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "t", version: "1" } });
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");

  // 1) 搜索
  const searchResp = await rpc("tools/call", {
    name: "codex_web_research",
    arguments: { search_query: [{ q: "Rust programming language 2026 release" }], response_length: "short" },
  });
  const searchText = textOf(searchResp);
  const refId = firstRefId(searchText);
  console.log("=== SEARCH (research) ===");
  console.log("isError:", !!(searchResp.result && searchResp.result.isError));
  console.log("first ref_id:", refId);
  console.log("snippet:", searchText.slice(0, 300));
  if (!refId) { console.log("!! 未提取到 ref_id，无法继续 open/find"); cleanup(); return; }

  // 2) open
  const openResp = await rpc("tools/call", {
    name: "codex_web_research",
    arguments: { open: [{ ref_id: refId }], response_length: "short" },
  });
  const openText = textOf(openResp);
  console.log("\n=== OPEN (ref_id=" + refId + ") ===");
  console.log("isError:", !!(openResp.result && openResp.result.isError));
  console.log("snippet:", openText.slice(0, 400));

  // 3) find
  const findResp = await rpc("tools/call", {
    name: "codex_web_research",
    arguments: { find: [{ ref_id: refId, pattern: "version" }], response_length: "short" },
  });
  const findText = textOf(findResp);
  console.log("\n=== FIND (ref_id=" + refId + ', pattern="version") ===');
  console.log("isError:", !!(findResp.result && findResp.result.isError));
  console.log("snippet:", findText.slice(0, 400));

  cleanup();
}

function cleanup() {
  setTimeout(() => { try { child.kill(); } catch {} process.exit(0); }, 500);
}
main().catch((e) => { console.error("FATAL:", e); cleanup(); });
