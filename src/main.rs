// codex-web-search-mcp — Rust rewrite
// Model-independent web search & deep research MCP server over stdio (JSON-RPC 2.0).
// Backend: OpenAI Codex (free with ChatGPT login).
// Tools: codex_web_search, codex_web_research, web_fetch.

use std::collections::HashMap;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde_json::{json, Value};

const CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/alpha/search";
const DEFAULT_MODEL: &str = "gpt-4o";
const UA: &str = "codex-cli/0.147.0-alpha.6.5";
const REQ_TIMEOUT: u64 = 20;
const FETCH_TIMEOUT: u64 = 30;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------
static SESSION_ID: OnceLock<Mutex<String>> = OnceLock::new();

fn session_id() -> &'static Mutex<String> {
    SESSION_ID.get_or_init(|| Mutex::new(format!("ccs_{}", nanoid())))
}
fn nanoid() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", n)
}

// ---------------------------------------------------------------------------
// PUA (private-use-area) citation marker cleanup
// Codex returns <U+E200>cite<U+E202><id><U+E201> markers (id = ref_id). The
// readable title/domain live in the `results` array; we rewrite markers into
// `[ref_id: Title → domain]` so the model can follow links via open/click.
// ---------------------------------------------------------------------------
fn cite_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("\u{e200}cite\u{e202}([^\u{e201}]*)\u{e201}").unwrap())
}
fn pua_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("[\u{e000}-\u{f8ff}]").unwrap())
}
fn wordlim_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[wordlim:\s*\d+\]").unwrap())
}
fn linenav_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(^|[\s ])(L\d+:\s?)").unwrap())
}
fn clean_output(text: &str, refs: &HashMap<String, (String, String)>) -> String {
    let s = cite_re().replace_all(text, |caps: &regex::Captures| {
        let inner = &caps[1];
        let cid = inner.trim().to_string();
        if cid.is_empty() {
            return String::new();
        }
        if let Some((title, domain)) = refs.get(&cid) {
            if domain.is_empty() {
                return format!(" [{}: {}]", cid, title);
            }
            return format!(" [{}: {} → {}]", cid, title, domain);
        }
        format!(" [{}]", cid)
    });
    let s = pua_re().replace_all(&s, "");
    let s = wordlim_re().replace_all(&s, "");
    let s = linenav_re().replace_all(&s, "$1");
    let s = s.replace("\n\n\n", "\n\n").trim().to_string();
    s
}

// ---------------------------------------------------------------------------
// Codex auth
// ---------------------------------------------------------------------------
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}
fn load_codex_auth() -> Option<(String, Option<String>)> {
    if let Some(t) = std::env::var("CODEX_ACCESS_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
    {
        let aid = std::env::var("CODEX_ACCOUNT_ID")
            .ok()
            .filter(|s| !s.is_empty());
        return Some((t, aid));
    }
    let home = home_dir()?;
    let p = home.join(".codex").join("auth.json");
    if let Ok(s) = std::fs::read_to_string(&p) {
        if let Ok(v) = serde_json::from_str::<Value>(&s) {
            if let Some(tok) = v
                .get("tokens")
                .and_then(|t| t.get("access_token"))
                .and_then(|x| x.as_str())
            {
                let aid = v
                    .get("tokens")
                    .and_then(|t| t.get("account_id"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                return Some((tok.to_string(), aid));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Codex backend
// ---------------------------------------------------------------------------
fn call_codex(commands: &Value) -> Result<Value, String> {
    let auth = load_codex_auth().ok_or_else(|| {
        "未找到 Codex 登录凭证。请先运行 `codex login`，或设置环境变量 CODEX_ACCESS_TOKEN。".to_string()
    })?;
    let sid = session_id().lock().unwrap().clone();
    let payload = json!({ "id": sid, "model": DEFAULT_MODEL, "commands": commands });
    let resp = ureq::post(CODEX_ENDPOINT)
        .set("Authorization", &format!("Bearer {}", auth.0))
        .set("ChatGPT-Account-ID", auth.1.as_deref().unwrap_or(""))
        .set("Content-Type", "application/json")
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(REQ_TIMEOUT))
        .send_json(&payload);
    match resp {
        Ok(r) => {
            let body: Value = r.into_json().map_err(|e| format!("解析响应失败: {}", e))?;
            Ok(normalize_body(&body))
        }
        Err(e) => match e {
            ureq::Error::Status(code, _resp) => match code {
                401 | 403 => Err(
                    "Codex 凭证已过期（HTTP 401/403）。请重新运行 `codex login`。".into(),
                ),
                429 => Err("触发 Codex 速率限制（HTTP 429）。请稍后重试。".into()),
                c => Err(format!("Codex 请求失败（HTTP {}）", c)),
            },
            other => Err(format!("Codex 请求失败: {}", other)),
        },
    }
}

fn normalize_body(body: &Value) -> Value {
    let mut refs: HashMap<String, (String, String)> = HashMap::new();
    let mut results = Vec::new();
    if let Some(arr) = body.get("results").and_then(|x| x.as_array()) {
        for it in arr {
            let url = it.get("url").and_then(|x| x.as_str());
            let ref_id = it.get("ref_id").and_then(|x| x.as_str());
            let title = it.get("title").and_then(|x| x.as_str());
            let snippet = it.get("snippet").and_then(|x| x.as_str());
            let domain = it.get("domain").and_then(|x| x.as_str());
            if let Some(rid) = ref_id {
                refs.insert(
                    rid.to_string(),
                    (title.unwrap_or("").to_string(), domain.unwrap_or("").to_string()),
                );
            }
            if url.is_none() && ref_id.is_none() && title.is_none() && snippet.is_none() {
                continue;
            }
            results.push(json!({
                "url": url,
                "ref_id": ref_id,
                "title": title,
                "snippet": snippet,
                "domain": domain,
            }));
        }
    }
    let output = body.get("output").and_then(|x| x.as_str()).unwrap_or("");
    let output = clean_output(output, &refs);
    json!({ "output": output, "results": results })
}

fn build_search_commands(args: &Value) -> Value {
    let q = args.get("query").and_then(|x| x.as_str()).unwrap_or("").trim();
    let mut sq = json!({ "q": q });
    if let Some(n) = args.get("recency").and_then(|x| x.as_i64()) {
        if n > 0 {
            sq["recency"] = json!(n);
        }
    }
    if let Some(doms) = args.get("domains").and_then(|x| x.as_array()) {
        let ds: Vec<String> = doms
            .iter()
            .filter_map(|d| d.as_str().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
        if !ds.is_empty() {
            sq["domains"] = json!(ds);
        }
    }
    let mut cmd = json!({ "search_query": [sq] });
    if let Some(rl) = args.get("response_length").and_then(|x| x.as_str()) {
        cmd["response_length"] = json!(rl);
    }
    cmd
}

fn build_research_commands(args: &Value) -> Value {
    let mut commands = json!({});
    if let Some(arr) = args.get("search_query").and_then(|x| x.as_array()) {
        let mut sqs = Vec::new();
        for sq in arr {
            let q = sq.get("q").and_then(|x| x.as_str()).unwrap_or("").trim();
            if q.is_empty() {
                continue;
            }
            let mut item = json!({ "q": q });
            if let Some(n) = sq.get("recency").and_then(|x| x.as_i64()) {
                if n > 0 {
                    item["recency"] = json!(n);
                }
            }
            if let Some(doms) = sq.get("domains").and_then(|x| x.as_array()) {
                let ds: Vec<String> = doms
                    .iter()
                    .filter_map(|d| d.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect();
                if !ds.is_empty() {
                    item["domains"] = json!(ds);
                }
            }
            sqs.push(item);
        }
        if !sqs.is_empty() {
            commands["search_query"] = json!(sqs);
        }
    }
    if let Some(arr) = args.get("open").and_then(|x| x.as_array()) {
        let mut v = Vec::new();
        for op in arr {
            let ref_id = op.get("ref_id").and_then(|x| x.as_str()).unwrap_or("").trim();
            if ref_id.is_empty() {
                continue;
            }
            let mut item = json!({ "ref_id": ref_id });
            if let Some(n) = op.get("lineno").and_then(|x| x.as_i64()) {
                item["lineno"] = json!(n);
            }
            v.push(item);
        }
        if !v.is_empty() {
            commands["open"] = json!(v);
        }
    }
    if let Some(arr) = args.get("find").and_then(|x| x.as_array()) {
        let mut v = Vec::new();
        for fn_ in arr {
            let ref_id = fn_.get("ref_id").and_then(|x| x.as_str()).unwrap_or("").trim();
            let pattern = fn_.get("pattern").and_then(|x| x.as_str()).unwrap_or("").trim();
            if ref_id.is_empty() || pattern.is_empty() {
                continue;
            }
            v.push(json!({ "ref_id": ref_id, "pattern": pattern }));
        }
        if !v.is_empty() {
            commands["find"] = json!(v);
        }
    }
    if let Some(arr) = args.get("click").and_then(|x| x.as_array()) {
        let mut v = Vec::new();
        for cl in arr {
            let ref_id = cl.get("ref_id").and_then(|x| x.as_str()).unwrap_or("").trim();
            let id = cl.get("id").and_then(|x| x.as_i64());
            if ref_id.is_empty() || id.is_none() {
                continue;
            }
            v.push(json!({ "ref_id": ref_id, "id": id.unwrap() }));
        }
        if !v.is_empty() {
            commands["click"] = json!(v);
        }
    }
    if let Some(rl) = args.get("response_length").and_then(|x| x.as_str()) {
        commands["response_length"] = json!(rl);
    }
    commands
}

// ---------------------------------------------------------------------------
// web_fetch (fetch a URL and return clean plain text)
// ---------------------------------------------------------------------------
// Dedicated agent: longer timeout + explicit redirect following (up to 10 hops).
// ureq's default agent already follows redirects, but we make it explicit so a
// moved page (301/302) resolves to the final content instead of an empty body.
fn fetch_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(FETCH_TIMEOUT))
            .redirects(10)
            .build()
    })
}

// Detect the text encoding so GBK / GB2312 / EUC-JP etc. pages decode correctly
// instead of showing mojibake. Priority: Content-Type charset → <meta> tag → UTF-8.
fn meta_charset_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)(?:charset\s*=\s*["']?\s*([\w-]+)|content\s*=\s*["'][^>]*charset=([\w-]+))"#)
            .unwrap()
    })
}
fn detect_encoding(content_type: &str, body: &[u8]) -> &'static encoding_rs::Encoding {
    // 0x27 = single quote; build without a backslash escape to keep the lexer happy.
    let squote = char::from(0x27u8);
    if let Some(pos) = content_type.to_ascii_lowercase().find("charset=") {
        let mut label = content_type[pos + 8..]
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if (label.starts_with('"') && label.ends_with('"'))
            || (label.starts_with(squote) && label.ends_with(squote))
        {
            label = &label[1..label.len() - 1];
        }
        if !label.is_empty() {
            if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                return enc;
            }
        }
    }
    let head = String::from_utf8_lossy(&body[..body.len().min(4096)]);
    if let Some(cap) = meta_charset_re().captures(&head) {
        let label = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str())
            .unwrap_or("");
        if !label.is_empty() {
            if let Some(enc) = encoding_rs::Encoding::for_label(label.as_bytes()) {
                return enc;
            }
        }
    }
    encoding_rs::UTF_8
}

fn web_fetch(url: &str) -> Result<String, String> {
    let resp = fetch_agent()
        .get(url)
        .set("User-Agent", UA)
        .call()
        .map_err(|e| format!("抓取失败: {}", e))?;
    let content_type = resp.header("Content-Type").unwrap_or("").to_string();
    let mut body_bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut body_bytes)
        .map_err(|e| format!("读取正文失败: {}", e))?;
    // Robust decode: if the bytes are already valid UTF-8, use UTF-8 directly. This
    // covers pages that declare charset=gbk/gb2312 in the header/meta but actually
    // send a UTF-8 body (mislabeled). Otherwise trust the declared/charset detection
    // so genuine GBK/GB2312 pages decode correctly instead of showing mojibake.
    let html = if std::str::from_utf8(&body_bytes).is_ok() {
        String::from_utf8_lossy(&body_bytes).into_owned()
    } else {
        let enc = detect_encoding(&content_type, &body_bytes);
        let (text, _actual_enc, _had_errors) = enc.decode(&body_bytes);
        text.into_owned()
    };
    Ok(strip_html(&html))
}
fn strip_html(html: &str) -> String {
    let re_script = Regex::new(r"(?is)<script.*?</script>").unwrap();
    let re_style = Regex::new(r"(?is)<style.*?</style>").unwrap();
    let re_tags = Regex::new(r"<[^>]+>").unwrap();
    let s = re_script.replace_all(html, " ");
    let s = re_style.replace_all(&s, " ");
    let s = re_tags.replace_all(&s, " ");
    let s = s
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ");
    let s = Regex::new(r"\n[ \t]*\n[ \t]*\n+")
        .unwrap()
        .replace_all(&s, "\n\n")
        .trim()
        .to_string();
    s
}

// ---------------------------------------------------------------------------
// Format output
// ---------------------------------------------------------------------------
fn format_text(norm: &Value, expose_refs: bool) -> String {
    let mut lines = Vec::new();
    if let Some(o) = norm.get("output").and_then(|x| x.as_str()) {
        if !o.is_empty() {
            lines.push(o.to_string());
        }
    }
    if let Some(arr) = norm.get("results").and_then(|x| x.as_array()) {
        if !arr.is_empty() {
            lines.push(String::new());
            lines.push("Sources:".into());
            for (i, r) in arr.iter().enumerate() {
                let title = r
                    .get("title")
                    .and_then(|x| x.as_str())
                    .or_else(|| r.get("url").and_then(|x| x.as_str()))
                    .unwrap_or("(untitled)");
                if expose_refs {
                    if let Some(rid) = r.get("ref_id").and_then(|x| x.as_str()) {
                        lines.push(format!("[{}] (ref: {}) {}", i + 1, rid, title));
                    } else {
                        lines.push(format!("[{}] {}", i + 1, title));
                    }
                } else {
                    lines.push(format!("[{}] {}", i + 1, title));
                }
                if let Some(u) = r.get("url").and_then(|x| x.as_str()) {
                    lines.push(format!("    {}", u));
                } else if let Some(d) = r.get("domain").and_then(|x| x.as_str()) {
                    lines.push(format!("    {}", d));
                }
                if let Some(s) = r.get("snippet").and_then(|x| x.as_str()) {
                    lines.push(format!("    {}", s));
                }
            }
        }
    }
    if lines.is_empty() {
        return "No results returned.".into();
    }
    let mut out = lines.join("\n");
    if expose_refs
        && Regex::new(r"\[(?:c\d+|\w*search\w*):")
            .map(|re| re.is_match(&out))
            .unwrap_or(false)
    {
        out.push_str(
            "\n\n（文档内容里 [turn0searchN: ...] / [cN: ...] 标记即是可点击的链接元素，其 ref_id（如 turn0searchN）可直接用于 open/click 跟随该链接继续深挖）",
        );
    }
    out
}

fn is_backend_error(norm: &Value) -> bool {
    let o = norm.get("output").and_then(|x| x.as_str()).unwrap_or("");
    o.contains("Internal Error") || o.contains("Unable to resolve") || o.contains("due to invalid arguments")
}

// ---------------------------------------------------------------------------
// MCP tool definitions
// ---------------------------------------------------------------------------
fn tools_list() -> Value {
    json!([
        {
            "name": "codex_web_search",
            "description": "通过 OpenAI Codex 独立搜索端点执行实时联网搜索（需要 Codex 凭证）。返回答案文本与带标题/URL/摘要的来源列表。与底层模型无关，适合非 Anthropic 模型下的联网搜索。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "搜索关键词或问题" },
                    "recency": { "type": "number", "description": "仅返回最近 N 天内的结果（可选）" },
                    "domains": { "type": "array", "items": { "type": "string" }, "description": "限定搜索域名列表（可选）" },
                    "response_length": { "type": "string", "enum": ["short","medium","long"], "description": "返回详略（可选，默认 medium）" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "codex_web_research",
            "description": "多步深度研究：在 Codex 联网检索+文档浏览引擎上执行「搜索→打开→页内查找→点击」迭代研究。search_query/open/find/click 可组合，也可多轮用 ref_id 串联。返回内容含 [cN: ...] 可点击标记。需要 Codex 凭证。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "search_query": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "q": { "type": "string" },
                            "recency": { "type": "number" },
                            "domains": { "type": "array", "items": { "type": "string" } }
                        }
                    }, "description": "搜索查询列表（可选，至少一项操作）" },
                    "open": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string", "description": "要打开的 ref_id，如 turn0search0" },
                            "lineno": { "type": "number" }
                        }
                    } },
                    "find": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string" },
                            "pattern": { "type": "string" }
                        }
                    } },
                    "click": { "type": "array", "items": {
                        "type": "object",
                        "properties": {
                            "ref_id": { "type": "string" },
                            "id": { "type": "number" }
                        }
                    } },
                    "response_length": { "type": "string", "enum": ["short","medium","long"] },
                    "session_id": { "type": "string", "description": "覆盖会话 id 以接续之前上下文（可选）" }
                }
            }
        },
        {
            "name": "web_fetch",
            "description": "抓取任意 URL 并返回干净的纯文本（剥离脚本/样式/标签，自动探测 charset 解码 GBK/GB2312 等中文编码，跟随 301/302 重定向）。补齐「搜到链接却读不到正文、JS 渲染页读不到」的短板。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要抓取的网址" }
                },
                "required": ["url"]
            }
        }
    ])
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------
fn handle_tool_call(name: &str, args: &Value) -> (String, bool) {
    match name {
        "codex_web_search" => {
            let query = args.get("query").and_then(|x| x.as_str()).unwrap_or("").trim();
            if query.is_empty() {
                return ("参数 query 不能为空。".into(), true);
            }
            match call_codex(&build_search_commands(args)) {
                Ok(norm) => (format_text(&norm, true), is_backend_error(&norm)),
                Err(e) => (e, true),
            }
        }
        "codex_web_research" => {
            if let Some(sid) = args.get("session_id").and_then(|x| x.as_str()) {
                if !sid.trim().is_empty() {
                    *session_id().lock().unwrap() = sid.trim().to_string();
                }
            }
            let commands = build_research_commands(args);
            if commands.get("search_query").is_none()
                && commands.get("open").is_none()
                && commands.get("find").is_none()
                && commands.get("click").is_none()
            {
                return ("至少需要提供一项操作：search_query / open / find / click。".into(), true);
            }
            match call_codex(&commands) {
                Ok(norm) => (format_text(&norm, true), is_backend_error(&norm)),
                Err(e) => (e, true),
            }
        }
        "web_fetch" => {
            let url = args.get("url").and_then(|x| x.as_str()).unwrap_or("").trim();
            if url.is_empty() {
                return ("参数 url 不能为空。".into(), true);
            }
            match web_fetch(url) {
                Ok(t) => (t, false),
                Err(e) => (e, true),
            }
        }
        _ => (format!("未知工具: {}", name), true),
    }
}

fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|x| x.as_str())?;
    match method {
        "initialize" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": msg.get("params").and_then(|p| p.get("protocolVersion")).and_then(|x| x.as_str()).unwrap_or("2024-11-05"),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "codex-web-search", "version": "2.1.0" }
            }
        })),
        "notifications/initialized" | "initialized" => None,
        "tools/list" => Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": tools_list() }
        })),
        "tools/call" => {
            let name = msg
                .get("params")
                .and_then(|p| p.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let args = msg
                .get("params")
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            let (text, is_error) = handle_tool_call(name, &args);
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": text }],
                    "isError": is_error
                }
            }))
        }
        "ping" => Some(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        _ => {
            if id.is_some() {
                Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("Method not found: {}", method) }
                }))
            } else {
                None
            }
        }
    }
}

fn main() {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if let Some(resp) = handle(&msg) {
            let _ = writeln!(out, "{}", serde_json::to_string(&resp).unwrap_or_default());
            let _ = out.flush();
        }
    }
}
