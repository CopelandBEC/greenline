//! `greenline-cli mcp`: the command line as an MCP server over standard
//! input/output, for chat clients that can't run a shell. A thin wrapper: each
//! tool runs the same command in-process. Scripting through the command line
//! is still the better way for big jobs; the guide says so.

use crate::cli;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    let pdf = json!({ "type": "string", "description": "Path to the PDF" });
    let page = json!({ "type": "integer", "minimum": 1, "description": "Page number, from 1" });
    let rect = json!({ "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4, "description": "[x0, y0, x1, y1] in page points (origin top-left, y down, 72 per inch)" });
    let proposals = json!({ "type": "boolean", "description": "Include proposals waiting for review" });
    let tool = |name: &str, description: &str, props: Value, required: &[&str]| {
        json!({ "name": name, "description": description, "inputSchema": { "type": "object", "properties": props, "required": required } })
    };
    json!([
        tool("guide", "How to work with drawings through Greenline: coordinates, scales, snapping, proposing. Read this first.", json!({}), &[]),
        tool("op_reference", "The ops `apply` takes: clouds, callouts, text, arrows, lengths, areas, scales.", json!({}), &[]),
        tool("info", "Pages with sizes, scales set and written on each sheet, markup and proposal counts.", json!({ "pdf": pdf }), &["pdf"]),
        tool("text", "A page's text runs with their boxes in page points (tags, notes, dimension strings).", json!({ "pdf": pdf, "page": page, "region": rect, "contains": { "type": "string" } }), &["pdf", "page"]),
        tool("paths", "Vector lines drawn on a page, as [x0, y0, x1, y1]; narrow with region.", json!({ "pdf": pdf, "page": page, "region": rect, "min": { "type": "number", "description": "Shortest line kept, points (default 2)" } }), &["pdf", "page"]),
        tool("snap", "Fit a rough rectangle to the drawn lines round it; with a scale, its width and height.", json!({ "pdf": pdf, "page": page, "rect": rect, "tol": { "type": "number" } }), &["pdf", "page", "rect"]),
        tool("render", "A picture of a page or region, with optional markups, proposals, draft ops and a labelled grid.", json!({
            "pdf": pdf, "page": page, "region": rect, "dpi": { "type": "number", "description": "Default 100; zoom in with region and more dpi" },
            "grid": { "type": "number", "description": "Grid line spacing in points, e.g. 72" },
            "markups": { "type": "boolean" }, "proposals": proposals,
            "ops": { "type": "array", "items": { "type": "object" }, "description": "Draft ops to draw (not proposed)" }
        }), &["pdf", "page"]),
        tool("list", "Markups with geometry and measured values.", json!({ "pdf": pdf, "page": page, "proposals": proposals }), &["pdf"]),
        tool("measurements", "Every measurement's length, area, width and height; optionally a CSV file.", json!({ "pdf": pdf, "proposals": proposals, "csv": { "type": "string", "description": "Write a CSV here" } }), &["pdf"]),
        tool("apply", "Propose a batch of ops for the reviewer to accept or reject in Greenline. dry_run checks them and reports measurements without proposing.", json!({
            "pdf": pdf, "ops": { "type": "array", "items": { "type": "object" } },
            "agent": { "type": "string", "description": "Your name, shown to the reviewer" },
            "summary": { "type": "string", "description": "One line on what the batch is" },
            "dry_run": { "type": "boolean" }
        }), &["pdf", "ops"]),
        tool("proposals", "Batches waiting for review.", json!({ "pdf": pdf }), &["pdf"]),
        tool("withdraw", "Take back a proposed batch (or \"all\").", json!({ "pdf": pdf, "id": { "type": "string" } }), &["pdf", "id"]),
        tool("export", "Write a PDF with the markups (flattened, or editable comments).", json!({ "pdf": pdf, "out": { "type": "string" }, "editable": { "type": "boolean" }, "proposals": proposals }), &["pdf"]),
    ])
}

/// Command-line arguments for a tool call.
fn args_for(name: &str, a: &Value, tmp: &mut Vec<std::path::PathBuf>) -> Result<Vec<String>, String> {
    let s = |k: &str| a.get(k).and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_f64().map(|n| n.to_string())));
    let mut out = vec![name.to_string()];
    out.push(s("pdf").ok_or("needs \"pdf\"")?);
    if name == "withdraw" {
        out.push(s("id").ok_or("needs \"id\"")?);
    }
    if name == "apply" || (name == "render" && a.get("ops").is_some()) {
        let ops = a.get("ops").ok_or("needs \"ops\"")?;
        let f = std::env::temp_dir().join(format!("greenline-mcp-ops-{}-{}.json", std::process::id(), tmp.len()));
        std::fs::write(&f, ops.to_string()).map_err(|e| e.to_string())?;
        if name == "apply" {
            out.push(f.display().to_string());
        } else {
            out.extend(["--ops".to_string(), f.display().to_string()]);
        }
        tmp.push(f);
    }
    for k in ["page", "dpi", "grid", "min", "tol", "contains", "agent", "summary", "out", "csv", "units"] {
        if let Some(v) = s(k) {
            out.extend([format!("--{k}"), v]);
        }
    }
    for k in ["region", "rect"] {
        if let Some(r) = a.get(k).and_then(Value::as_array) {
            out.extend([format!("--{k}"), r.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")]);
        }
    }
    for (k, flag) in [("markups", "markups"), ("proposals", "proposals"), ("editable", "editable"), ("dry_run", "dry-run")] {
        if a.get(k).and_then(Value::as_bool) == Some(true) {
            out.push(format!("--{flag}"));
        }
    }
    Ok(out)
}

fn text(t: impl Into<String>) -> Value {
    json!({ "type": "text", "text": t.into() })
}

fn call(name: &str, a: &Value) -> Value {
    let result = match name {
        "guide" => Ok(vec![text(cli::GUIDE)]),
        "op_reference" => Ok(vec![text(crate::ops::REFERENCE)]),
        "info" | "text" | "paths" | "snap" | "render" | "list" | "measurements" | "apply" | "proposals" | "withdraw" | "export" => {
            let mut tmp = Vec::new();
            let r = args_for(name, a, &mut tmp).and_then(|args| cli::run(args).map_err(|e| e.to_string()));
            for f in tmp {
                let _ = std::fs::remove_file(f);
            }
            r.map(|v| {
                let mut content = vec![text(serde_json::to_string_pretty(&v).unwrap_or_default())];
                if name == "render" {
                    if let Some(png) = v["file"].as_str().and_then(|f| std::fs::read(f).ok()) {
                        content.push(json!({ "type": "image", "data": base64(&png), "mimeType": "image/png" }));
                    }
                }
                content
            })
        }
        other => Err(format!("unknown tool {other}")),
    };
    match result {
        Ok(content) => json!({ "content": content }),
        Err(e) => json!({ "content": [text(e)], "isError": true }),
    }
}

/// Answers one JSON-RPC message; `None` for notifications.
pub fn handle(msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "greenline", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Greenline reads PDF drawing sets and proposes markups and measurements for a person to review in the Greenline app. Call guide first. Measurements read in feet-inches; add \"units\": \"metric\" to a tool's arguments for metres.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            Ok(call(name, params.get("arguments").unwrap_or(&json!({}))))
        }
        _ => Err(json!({ "code": -32601, "message": format!("method not found: {method}") })),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": e }),
    })
}

/// Serves newline-delimited JSON-RPC on standard input/output until it closes.
pub fn serve() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&msg),
            Err(e) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("parse error: {e}") } })),
        };
        if let Some(r) = reply {
            if writeln!(out, "{r}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
    0
}

fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn speaks_mcp() {
        let init = handle(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let list = handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"apply") && names.contains(&"render") && names.contains(&"guide"));
        let guide = handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "guide", "arguments": {}}})).unwrap();
        assert!(guide["result"]["content"][0]["text"].as_str().unwrap().contains("page points"));
        let bad = handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "info", "arguments": {}}})).unwrap();
        assert_eq!(bad["result"]["isError"], true);
        let unknown = handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "resources/list"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
    }

    #[test]
    fn tool_arguments_become_command_lines() {
        let mut tmp = Vec::new();
        let a = json!({"pdf": "/x.pdf", "page": 3, "region": [1, 2, 3.5, 4], "proposals": true, "dpi": 150});
        let args = args_for("render", &a, &mut tmp).unwrap();
        assert_eq!(args, vec!["render", "/x.pdf", "--page", "3", "--dpi", "150", "--region", "1,2,3.5,4", "--proposals"]);
        let a = json!({"pdf": "/x.pdf", "ops": [{"op": "cloud"}], "dry_run": true, "agent": "claude"});
        let args = args_for("apply", &a, &mut tmp).unwrap();
        assert_eq!(args[..2], ["apply", "/x.pdf"]);
        assert!(args.contains(&"--dry-run".to_string()) && args.contains(&"claude".to_string()));
        assert_eq!(std::fs::read_to_string(&tmp[0]).unwrap(), r#"[{"op":"cloud"}]"#);
        for f in tmp {
            std::fs::remove_file(f).ok();
        }
    }
}
