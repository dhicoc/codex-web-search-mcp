#!/usr/bin/env node
"use strict";
// 单独实测 click：search -> open -> click(ref_id, id)，自动尝试两种 ref_id。
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
      const r = pending.get(msg.id);
      pending.delete(msg.id);
      r(msg);
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
      if (pending.has(id)) { pending.delete(id); reject(new Error("timeout " + method)); }
    }, 25000);
  });
}
function textOf(r) {
  if (!r.result || !r.result.content) return "(none)";
  return r.result.content.map((c) => c.text).join("\n");
}
function firstRef(text) { const m = text.match(/\(ref:\s*([^)]+)\)/); return m ? m[1] : null; }

async function main() {
  await rpc("initialize", { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "t", version: "1" } });
  child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");

  const s = await rpc("tools/call", {
    name: "codex_web_research",
    arguments: { search_query: [{ q: "Rust 1.96 release notes" }], response_length: "short" },
  });
  const searchRef = firstRef(textOf(s));
  console.log("SEARCH ref:", searchRef, "| isError:", !!(s.result && s.result.isError));

  const o = await rpc("tools/call", {
    name: "codex_web_research",
    arguments: { open: [{ ref_id: searchRef }], response_length: "short" },
  });
  const openText = textOf(o);
  const viewRef = firstRef(openText); // 通常为 turn1view0
  console.log("OPEN view ref:", viewRef, "| isError:", !!(o.result && o.result.isError));
  console.log("OPEN 是否含 [cN 标记]:", /\[c\d+:/i.test(openText));

  // click 必须用打开后的「视图 ref_id」（turn1view0），而非搜索结果 ref_id。
  // 选用文档内一个真实存在的深链 id：优先抓 [cN: ... → doc.rust-lang.org] 这种（N=8 附近），
  // 退而求其次取第一个 [cN:]。
  let clickId = 8;
  const cm = openText.match(/\[c(\d+):[^\]]*→\s*doc\./i) || openText.match(/\[c(\d+):/i);
  if (cm) clickId = Number(cm[1]);
  console.log("准备 click id =", clickId, "(使用视图 ref:", viewRef + ")");

  const tryClick = async (ref) => {
    const r = await rpc("tools/call", {
      name: "codex_web_research",
      arguments: { click: [{ ref_id: ref, id: clickId }], response_length: "short" },
    });
    const txt = textOf(r);
    const backendErr = /Internal Error|Unable to resolve|invalid arguments/i.test(txt);
    const err = !!(r.result && r.result.isError) || backendErr;
    return { ref, resp: r, text: txt, err };
  };

  let res = await tryClick(viewRef);
  console.log("\n=== CLICK with viewRef(" + viewRef + ") ===");
  console.log("isError:", res.err);
  console.log("snippet:", res.text.slice(0, 400));
  if (res.err && searchRef && searchRef !== viewRef) {
    const res2 = await tryClick(searchRef);
    console.log("\n=== CLICK with searchRef(" + searchRef + ") (fallback) ===");
    console.log("isError:", res2.err);
    console.log("snippet:", res2.text.slice(0, 400));
    res = res2;
  }
  console.log("\n最终 click 成功:", !res.err);
  setTimeout(() => { try { child.kill(); } catch {} process.exit(0); }, 400);
}
main().catch((e) => { console.error("FATAL", e); setTimeout(() => process.exit(1), 400); });
