// codex-web-search-mcp — Rust rewrite
// Model-independent web search & deep research MCP server over stdio (JSON-RPC 2.0).
// Backends: OpenAI Codex (default, free with ChatGPT login) and optional Grok/xAI.
// New tools vs the old Node version: web_fetch, get_sources (pagination/budget),
// doctor (self-check), web_map (Tavily Map).

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use regex::Regex;
use serde::Deserialize;
use serde_json::{json, Value};

const CODEX_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/alpha/search";
const DEFAULT_MODEL: &str = "gpt-4o";
const UA: &str = "codex-cli/0.147.0-alpha.6.5";
const REQ_TIMEOUT: u64 = 20;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------
static SESSION_ID: OnceLock<Mutex<String>> = OnceLock::new();
static SOURCE_CACHE: OnceLock<Mutex<HashMap<String, Value>>> = OnceLock::new();
static FILE_CONFIG: OnceLock<FileConfig> = OnceLock::new();

fn session_id() -> &'static Mutex<String> {
    SESSION_ID.get_or_init(|| Mutex::new(format!("ccs_{}", nanoid())))
}
fn source_cache() -> &'static Mutex<HashMap<String, Value>> {
    SOURCE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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
// Codex returns <U+E200>cite<U+E202><id>†<label>†<domain><U+E201> markers. We
// rewrite them into readable [cN: label → domain] so the model can follow links.
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
        let parts: Vec<&str> = inner.split('\u{2020}').collect();
        let cid = parts.first().map(|s| s.trim()).unwrap_or("").to_string();
        if cid.is_empty() {
            return String::new();
        }
        // 新格式：PUA 标记里只有 ref_id，label/domain 在 results 里；优先从 map 取。
        if let Some((title, domain)) = refs.get(&cid) {
            if domain.is_empty() {
                return format!(" [{}: {}]", cid, title);
            }
            return format!(" [{}: {} → {}]", cid, title, domain);
        }
        // 旧格式兜底：标记内自带 †label†domain。
        let label = parts.get(1).map(|s| s.trim()).unwrap_or("");
        let domain = parts.get(2).map(|s| s.trim()).unwrap_or("");
        if domain.is_empty() {
            if label.is_empty() {
                format!(" [{}]", cid)
            } else {
                format!(" [{}: {}]", cid, label)
            }
        } else {
            format!(" [{}: {} → {}]", cid, label, domain)
        }
    });
    let s = pua_re().replace_all(&s, "");
    let s = wordlim_re().replace_all(&s, "");
    let s = linenav_re().replace_all(&s, "$1");
    let s = s.replace("\n\n\n", "\n\n").trim().to_string();
    s
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------
#[derive(Debug, Clone, Default, Deserialize)]
struct FileConfig {
    grok_api_key: Option<String>,
    grok_base_url: Option<String>,
    grok_model: Option<String>,
    tavily_api_key: Option<String>,
    firecrawl_api_key: Option<String>,
    exa_api_key: Option<String>,
    response_max_chars: Option<usize>,
    max_inline_sources: Option<usize>,
}

fn load_file_config() -> FileConfig {
    let path = std::env::var("CODEX_SEARCH_CONFIG")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let home = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .ok()?;
            Some(
                PathBuf::from(home)
                    .join(".config")
                    .join("codex-web-search-mcp")
                    .join("config.toml"),
            )
        });
    if let Some(p) = path {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(cfg) = toml::from_str::<FileConfig>(&s) {
                return cfg;
            }
        }
    }
    FileConfig::default()
}
fn file_config() -> &'static FileConfig {
    FILE_CONFIG.get_or_init(load_file_config)
}
fn cfg(env_key: &str, file_val: &Option<String>) -> Option<String> {
    std::env::var(env_key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| file_val.clone())
}
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Codex auth
// ---------------------------------------------------------------------------
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

fn format_text(norm: &Value, expose_refs: bool) -> String {
    let cfg = file_config();
    let max_sources = cfg.max_inline_sources.unwrap_or(usize::MAX);
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
            for (i, r) in arr.iter().take(max_sources).enumerate() {
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
    if expose_refs && Regex::new(r"\[(?:c\d+|\w*search\w*):")
        .map(|re| re.is_match(&out))
        .unwrap_or(false)
    {
        out.push_str(
            "\n\n（文档内容里 [turn0searchN: ...] / [cN: ...] 标记即是可点击的链接元素，其 ref_id（如 turn0searchN）可直接用于 open/click 跟随该链接继续深挖）",
        );
    }
    // 预算控制：超过 response_max_chars 则截断，避免一次性灌入过多正文。
    if let Some(budget) = cfg.response_max_chars {
        if out.chars().count() > budget {
            let cut = out.char_indices().nth(budget).map(|(i, _)| i).unwrap_or(out.len());
            out.truncate(cut);
            out.push_str(&format!("\n\n…(输出已按 response_max_chars={} 截断)", budget));
        }
    }
    out
}

fn is_backend_error(norm: &Value) -> bool {
    let o = norm.get("output").and_then(|x| x.as_str()).unwrap_or("");
    o.contains("Internal Error") || o.contains("Unable to resolve") || o.contains("due to invalid arguments")
}

// ---------------------------------------------------------------------------
// Grok / xAI backend (optional, activated by key)
// ---------------------------------------------------------------------------
fn grok_keys() -> Option<String> {
    cfg("GROK_SEARCH_API_KEY", &file_config().grok_api_key)
        .or_else(|| cfg("XAI_API_KEY", &None))
}
fn backend_is_grok() -> bool {
    std::env::var("CODEX_SEARCH_BACKEND").ok().as_deref() == Some("grok")
        && grok_keys().is_some()
}
fn call_grok_web_search(query: &str, response_length: Option<&str>) -> Result<Value, String> {
    let key = grok_keys()
        .ok_or_else(|| "未配置 Grok/xAI API Key（GROK_SEARCH_API_KEY 或 XAI_API_KEY）。".to_string())?;
    let base = cfg("GROK_SEARCH_URL", &file_config().grok_base_url)
        .unwrap_or_else(|| "https://api.x.ai".into());
    let model = cfg("GROK_SEARCH_MODEL", &file_config().grok_model)
        .unwrap_or_else(|| "grok-4.1-fast".into());
    let url = format!("{}/v1/responses", base.trim_end_matches('/'));
    let mut payload = json!({
        "model": model,
        "input": query,
        "tools": [{ "type": "web_search" }],
    });
    if let Some(rl) = response_length {
        payload["response_format"] = json!(if rl == "short" { "concise" } else { "detailed" });
    }
    let resp = ureq::post(&url)
        .set("Authorization", &format!("Bearer {}", key))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(REQ_TIMEOUT))
        .send_json(&payload);
    match resp {
        Ok(r) => {
            let body: Value = r.into_json().map_err(|e| e.to_string())?;
            Ok(grok_normalize(&body))
        }
        Err(e) => Err(format!("Grok 请求失败: {}", e)),
    }
}
fn grok_normalize(body: &Value) -> Value {
    let mut text = String::new();
    let mut results = Vec::new();
    if let Some(out) = body.get("output").and_then(|x| x.as_array()) {
        for item in out {
            if item.get("type").and_then(|t| t.as_str()) == Some("message") {
                if let Some(c) = item.get("content").and_then(|c| c.as_array()) {
                    for cc in c {
                        if let Some(t) = cc.get("text").and_then(|x| x.as_str()) {
                            text.push_str(t);
                            text.push('\n');
                        }
                    }
                }
            }
            if let Some(c) = item.get("content").and_then(|c| c.as_array()) {
                for cc in c {
                    if let Some(anns) = cc.get("annotations").and_then(|a| a.as_array()) {
                        for ann in anns {
                            if let Some(u) = ann.get("url").and_then(|x| x.as_str()) {
                                let title = ann.get("title").and_then(|x| x.as_str()).unwrap_or(u);
                                results.push(json!({ "url": u, "title": title }));
                            }
                        }
                    }
                }
            }
        }
    }
    json!({ "output": text.trim().to_string(), "results": results })
}

// ---------------------------------------------------------------------------
// web_fetch / web_map (provider chain)
// ---------------------------------------------------------------------------
fn web_fetch(url: &str) -> Result<String, String> {
    let resp = ureq::get(url)
        .set("User-Agent", UA)
        .timeout(Duration::from_secs(REQ_TIMEOUT))
        .call()
        .map_err(|e| format!("抓取失败: {}", e))?;
    let body = resp
        .into_string()
        .map_err(|e| format!("读取正文失败: {}", e))?;
    Ok(strip_html(&body))
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
fn web_map(domain: &str) -> Result<Vec<String>, String> {
    let key = cfg("TAVILY_API_KEY", &file_config().tavily_api_key)
        .ok_or_else(|| "未配置 TAVILY_API_KEY（web_map 需要）。".to_string())?;
    let resp = ureq::post("https://api.tavily.com/map")
        .set("Authorization", &format!("Bearer {}", key))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(REQ_TIMEOUT))
        .send_json(&json!({ "url": domain }));
    match resp {
        Ok(r) => {
            let body: Value = r.into_json().map_err(|e| e.to_string())?;
            let urls = body
                .get("urls")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|u| u.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            Ok(urls)
        }
        Err(e) => Err(format!("Tavily Map 失败: {}", e)),
    }
}

// ---------------------------------------------------------------------------
// get_sources (pagination over cached session results)
// ---------------------------------------------------------------------------
fn get_sources(args: &Value) -> String {
    let sid = args
        .get("session_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| session_id().lock().unwrap().clone());
    let offset = args
        .get("offset")
        .and_then(|x| x.as_i64())
        .unwrap_or(0)
        .max(0) as usize;
    let limit = args
        .get("limit")
        .and_then(|x| x.as_i64())
        .unwrap_or(10)
        .max(1) as usize;
    let cache = source_cache().lock().unwrap();
    match cache.get(&sid) {
        Some(n) => {
            if let Some(arr) = n.get("results").and_then(|x| x.as_array()) {
                let total = arr.len();
                let end = (offset + limit).min(total);
                let mut lines = vec![format!(
                    "Session {}：共 {} 个来源，显示第 {}-{} 条：",
                    sid,
                    total,
                    offset + 1,
                    end
                )];
                for (i, r) in arr.iter().enumerate().skip(offset).take(limit) {
                    let title = r
                        .get("title")
                        .and_then(|x| x.as_str())
                        .or_else(|| r.get("url").and_then(|x| x.as_str()))
                        .unwrap_or("(untitled)");
                    lines.push(format!("[{}] {}", i + 1, title));
                    if let Some(u) = r.get("url").and_then(|x| x.as_str()) {
                        lines.push(format!("    {}", u));
                    }
                }
                lines.join("\n")
            } else {
                "该 session 无缓存来源。".into()
            }
        }
        None => "未找到该 session 的缓存来源，请先执行 codex_web_search。".into(),
    }
}

// ---------------------------------------------------------------------------
// doctor (connectivity + redacted config)
// ---------------------------------------------------------------------------
fn mask_key(v: &Option<String>) -> String {
    match v {
        Some(s) if s.len() >= 8 => format!("{}…{}", &s[..4], &s[s.len() - 4..]),
        Some(s) => format!("{}…(短)", &s[..s.len().min(4)]),
        None => "未设置".into(),
    }
}
fn doctor() -> String {
    let mut lines = Vec::new();
    lines.push("=== codex-web-search-mcp doctor ===".into());
    lines.push(format!(
        "Codex 凭证: {}",
        if load_codex_auth().is_some() {
            "已配置 ✓"
        } else {
            "缺失 ✗"
        }
    ));
    lines.push(format!("当前后端: {}", if backend_is_grok() { "grok" } else { "codex" }));
    lines.push(format!(
        "GROK/XAI key: {}",
        mask_key(&grok_keys())
    ));
    lines.push(format!(
        "TAVILY key: {}",
        mask_key(&cfg("TAVILY_API_KEY", &file_config().tavily_api_key))
    ));
    lines.push(format!(
        "FIRECRAWL key: {}",
        mask_key(&cfg("FIRECRAWL_API_KEY", &file_config().firecrawl_api_key))
    ));
    lines.push(format!(
        "EXA key: {}",
        mask_key(&cfg("EXA_API_KEY", &file_config().exa_api_key))
    ));
    match load_codex_auth() {
        Some(_) => {
            let probe = call_codex(&json!({ "search_query": [{ "q": "connectivity probe" }] }));
            match probe {
                Ok(_) => lines.push("Codex 端点连通性: 正常 ✓".into()),
                Err(e) => lines.push(format!("Codex 端点连通性: 失败 ✗ ({})", e)),
            }
        }
        None => lines.push("Codex 端点连通性: 跳过（无凭证）".into()),
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// MCP tool definitions
// ---------------------------------------------------------------------------
fn tools_list() -> Value {
    json!([
        {
            "name": "codex_web_search",
            "description": "通过 OpenAI Codex 独立搜索端点执行实时联网搜索（默认后端），或可选 Grok/xAI（设 GROK_SEARCH_BACKEND=grok + key）。返回答案文本与带标题/URL/摘要的来源列表。与底层模型无关，适合非 Anthropic 模型下的联网搜索。需要 Codex 凭证或 Grok key。",
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
            "description": "抓取任意 URL 并返回干净的纯文本（剥离脚本/样式/标签）。补齐「搜到链接却读不到正文、JS 渲染页读不到」的短板。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要抓取的网址" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "get_sources",
            "description": "按 session_id 分页重取上一次搜索的来源列表（避免重复搜索）。offset/limit 控制分页。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "会话 id（可选，默认当前会话）" },
                    "offset": { "type": "number", "description": "起始序号（从 0）" },
                    "limit": { "type": "number", "description": "返回条数（默认 10）" }
                }
            }
        },
        {
            "name": "doctor",
            "description": "自检：连通性探测 + 脱敏展示当前配置（后端、各 API key 是否设置）。用于排错。",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "web_map",
            "description": "经 Tavily Map 发现某域名下的 URL 列表。需要 TAVILY_API_KEY。",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "要映射的域名或起始 URL" }
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
            let rl = args.get("response_length").and_then(|x| x.as_str());
            let norm = if backend_is_grok() {
                match call_grok_web_search(query, rl) {
                    Ok(n) => n,
                    Err(e) => return (e, true),
                }
            } else {
                match call_codex(&build_search_commands(args)) {
                    Ok(n) => n,
                    Err(e) => return (e, true),
                }
            };
            source_cache()
                .lock()
                .unwrap()
                .insert(session_id().lock().unwrap().clone(), norm.clone());
            (format_text(&norm, true), is_backend_error(&norm))
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
                Ok(norm) => {
                    source_cache()
                        .lock()
                        .unwrap()
                        .insert(session_id().lock().unwrap().clone(), norm.clone());
                    (format_text(&norm, true), is_backend_error(&norm))
                }
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
        "get_sources" => (get_sources(args), false),
        "doctor" => (doctor(), false),
        "web_map" => {
            let url = args.get("url").and_then(|x| x.as_str()).unwrap_or("").trim();
            if url.is_empty() {
                return ("参数 url 不能为空。".into(), true);
            }
            match web_map(url) {
                Ok(urls) => {
                    if urls.is_empty() {
                        ("未返回任何 URL。".into(), false)
                    } else {
                        (format!("发现 {} 个 URL：\n{}", urls.len(), urls.join("\n")), false)
                    }
                }
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
                "serverInfo": { "name": "codex-web-search", "version": "2.0.0" }
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
