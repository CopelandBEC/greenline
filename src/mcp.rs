//! `greenline-cli mcp`: the command line as an MCP server over standard
//! input/output, for chat clients that can't run a shell. A thin wrapper: each
//! tool checks its arguments against the table below (which is also the
//! schema it publishes) and runs the same command in-process, with arguments
//! passed already parsed, so no value can turn into a flag. Scripting through
//! the command line is still the better way for big jobs; the guide says so.

use crate::cli;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};

/// Protocol versions this server speaks, newest first.
const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

#[derive(Clone, Copy, PartialEq)]
enum Ty {
    Str,
    Page,
    Num,
    Bool,
    Rect,
    Ops,
    Units,
}

struct Param {
    name: &'static str,
    ty: Ty,
    required: bool,
    doc: &'static str,
}

const fn p(name: &'static str, ty: Ty, required: bool, doc: &'static str) -> Param {
    Param { name, ty, required, doc }
}

const PDF: Param = p("pdf", Ty::Str, true, "Path to the PDF");
const PAGE: Param = p("page", Ty::Page, true, "Page number, from 1");
const UNITS: Param = p("units", Ty::Units, false, "imperial (feet-inches, the default) or metric");
const PROPOSALS: Param = p("proposals", Ty::Bool, false, "Include proposals waiting for review");
const RECT_DOC: &str = "[x0, y0, x1, y1] in page points (origin top-left, y down, 72 per inch)";

struct Tool {
    name: &'static str,
    doc: &'static str,
    params: &'static [Param],
}

const TOOLS: &[Tool] = &[
    Tool { name: "guide", doc: "How to work with drawings through Greenline: coordinates, scales, snapping, proposing. Read this first.", params: &[] },
    Tool { name: "op_reference", doc: "The ops `apply` takes: clouds, callouts, text, arrows, lengths, areas, scales.", params: &[] },
    Tool { name: "info", doc: "Pages with sizes, scales set and written on each sheet, markup and proposal counts.", params: &[PDF, UNITS] },
    Tool {
        name: "text",
        doc: "A page's text runs with their boxes in page points (tags, notes, dimension strings).",
        params: &[PDF, PAGE, p("region", Ty::Rect, false, RECT_DOC), p("contains", Ty::Str, false, "Only runs containing this (any case)"), UNITS],
    },
    Tool {
        name: "paths",
        doc: "Vector lines drawn on a page, as [x0, y0, x1, y1]; narrow with region.",
        params: &[PDF, PAGE, p("region", Ty::Rect, false, RECT_DOC), p("min", Ty::Num, false, "Shortest line kept, points (default 2)"), p("limit", Ty::Num, false, "Most lines returned (default 3000)"), UNITS],
    },
    Tool {
        name: "snap",
        doc: "Fit a rough rectangle to the drawn lines round it; with a scale, its width and height.",
        params: &[PDF, PAGE, p("rect", Ty::Rect, true, RECT_DOC), p("tol", Ty::Num, false, "How far an edge may move, points (default 6)"), UNITS],
    },
    Tool {
        name: "render",
        doc: "A picture of a page or region, with optional markups, proposals, draft ops and a labelled grid.",
        params: &[
            PDF, PAGE,
            p("region", Ty::Rect, false, RECT_DOC),
            p("dpi", Ty::Num, false, "Default 100; zoom in with region and more dpi"),
            p("grid", Ty::Num, false, "Grid line spacing in points, e.g. 72"),
            p("markups", Ty::Bool, false, "Draw the accepted markups"),
            PROPOSALS,
            p("ops", Ty::Ops, false, "Draft ops to draw (not proposed)"),
            p("out", Ty::Str, false, "Where to save the PNG (default: a temporary folder)"),
            UNITS,
        ],
    },
    Tool { name: "list", doc: "Markups with geometry and measured values.", params: &[PDF, p("page", Ty::Page, false, "Only this page"), PROPOSALS, UNITS] },
    Tool {
        name: "measurements",
        doc: "Every measurement's length, area, width and height; optionally a CSV file.",
        params: &[PDF, PROPOSALS, p("csv", Ty::Str, false, "Write a CSV here"), UNITS],
    },
    Tool {
        name: "apply",
        doc: "Propose a batch of ops for the reviewer to accept or reject in Greenline. dry_run checks them and reports measurements without proposing.",
        params: &[
            PDF,
            p("ops", Ty::Ops, true, "The ops (see op_reference)"),
            p("agent", Ty::Str, false, "Your name, shown to the reviewer"),
            p("summary", Ty::Str, false, "One line on what the batch is"),
            p("dry_run", Ty::Bool, false, "Check only; propose nothing"),
            UNITS,
        ],
    },
    Tool { name: "proposals", doc: "Batches waiting for review.", params: &[PDF] },
    Tool { name: "withdraw", doc: "Take back a proposed batch (or \"all\").", params: &[PDF, p("id", Ty::Str, true, "The batch id, or all")] },
    Tool {
        name: "export",
        doc: "Write a PDF with the markups (flattened, or editable comments).",
        params: &[PDF, p("out", Ty::Str, false, "Output PDF (default: beside the original)"), p("editable", Ty::Bool, false, "Markups as movable comments"), PROPOSALS],
    },
];

fn schema(t: &Tool) -> Value {
    let mut props = Map::new();
    for p in t.params {
        let mut s = match p.ty {
            Ty::Str => json!({ "type": "string" }),
            Ty::Page => json!({ "type": "integer", "minimum": 1 }),
            Ty::Num => json!({ "type": "number" }),
            Ty::Bool => json!({ "type": "boolean" }),
            Ty::Rect => json!({ "type": "array", "items": { "type": "number" }, "minItems": 4, "maxItems": 4 }),
            Ty::Ops => json!({ "type": "array", "items": { "type": "object" } }),
            Ty::Units => json!({ "type": "string", "enum": ["imperial", "metric"] }),
        };
        s["description"] = json!(p.doc);
        props.insert(p.name.to_string(), s);
    }
    let required: Vec<&str> = t.params.iter().filter(|p| p.required).map(|p| p.name).collect();
    json!({ "name": t.name, "description": t.doc, "inputSchema": { "type": "object", "properties": props, "required": required, "additionalProperties": false } })
}

/// A tool call, checked: the PDF, a withdraw id, command-line flags and ops.
struct Call {
    pdf: String,
    id: Option<String>,
    flags: HashMap<String, String>,
    ops: Option<Value>,
}

/// Checks `args` against the tool's parameters: every one known, of its type,
/// the required ones present. An error message for the client otherwise.
fn check(t: &Tool, args: &Value) -> Result<Call, String> {
    let empty = Map::new();
    let obj = match args {
        Value::Object(o) => o,
        Value::Null => &empty,
        _ => return Err("arguments must be an object".into()),
    };
    if let Some(k) = obj.keys().find(|k| !t.params.iter().any(|p| p.name == k.as_str())) {
        let known: Vec<&str> = t.params.iter().map(|p| p.name).collect();
        return Err(format!("unknown argument \"{k}\" for {} (takes {})", t.name, known.join(", ")));
    }
    let mut call = Call { pdf: String::new(), id: None, flags: HashMap::new(), ops: None };
    for p in t.params {
        let Some(v) = obj.get(p.name).filter(|v| !v.is_null()) else {
            if p.required {
                return Err(format!("{} needs \"{}\"", t.name, p.name));
            }
            continue;
        };
        let bad = || format!("\"{}\" must be {}", p.name, match p.ty {
            Ty::Str => "a string",
            Ty::Page => "a whole number from 1",
            Ty::Num => "a number",
            Ty::Bool => "true or false",
            Ty::Rect => "[x0, y0, x1, y1] numbers",
            Ty::Ops => "an array of op objects",
            Ty::Units => "\"imperial\" or \"metric\"",
        });
        let text = match p.ty {
            Ty::Str => Some(v.as_str().ok_or_else(bad)?.to_string()),
            Ty::Page => Some(v.as_u64().filter(|n| *n >= 1).ok_or_else(bad)?.to_string()),
            Ty::Num => Some(v.as_f64().filter(|n| n.is_finite()).ok_or_else(bad)?.to_string()),
            Ty::Bool => v.as_bool().ok_or_else(bad)?.then(String::new),
            Ty::Rect => {
                let a = v.as_array().filter(|a| a.len() == 4).ok_or_else(bad)?;
                let n: Vec<String> = a.iter().map(|x| x.as_f64().filter(|n| n.is_finite()).map(|n| n.to_string())).collect::<Option<_>>().ok_or_else(bad)?;
                Some(n.join(","))
            }
            Ty::Ops => {
                v.as_array().filter(|a| a.iter().all(Value::is_object)).ok_or_else(bad)?;
                call.ops = Some(v.clone());
                None
            }
            Ty::Units => Some(v.as_str().filter(|u| matches!(*u, "imperial" | "metric")).ok_or_else(bad)?.to_string()),
        };
        match (p.name, text) {
            ("pdf", Some(t)) => call.pdf = t,
            ("id", Some(t)) => call.id = Some(t),
            (name, Some(t)) => {
                call.flags.insert(name.replace('_', "-"), t);
            }
            _ => {}
        }
    }
    Ok(call)
}

fn text(t: impl Into<String>) -> Value {
    json!({ "type": "text", "text": t.into() })
}

/// Runs a tool whose arguments passed `check`: its content, or a failure as an
/// `isError` result (an execution error the model can read and act on).
fn run(t: &Tool, call: Call) -> Value {
    let result = match t.name {
        "guide" => Ok(vec![text(cli::GUIDE)]),
        "op_reference" => Ok(vec![text(crate::ops::REFERENCE)]),
        name => cli::run_with(name, &call.pdf, call.id, call.flags, call.ops).map_err(|e| e.to_string()).map(|v| {
            let mut content = vec![text(serde_json::to_string_pretty(&v).unwrap_or_default())];
            if name == "render" {
                if let Some(png) = v["file"].as_str().and_then(|f| std::fs::read(f).ok()) {
                    content.push(json!({ "type": "image", "data": base64(&png), "mimeType": "image/png" }));
                }
            }
            content
        }),
    };
    match result {
        Ok(content) => json!({ "content": content }),
        Err(e) => json!({ "content": [text(e)], "isError": true }),
    }
}

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

/// Answers one JSON-RPC message; `None` for a notification.
pub fn handle(msg: &Value) -> Option<Value> {
    let Some(obj) = msg.as_object() else { return Some(error(Value::Null, -32600, "invalid request: not an object")) };
    let id = obj.get("id");
    // Requests carry a string or integer id; one without an id is a notification.
    let id_ok = match id {
        None => true,
        Some(Value::String(_)) => true,
        Some(Value::Number(n)) => n.is_i64() || n.is_u64(),
        _ => false,
    };
    let method = obj.get("method").and_then(Value::as_str);
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !id_ok || method.is_none() {
        let reply_id = if id_ok { id.cloned().unwrap_or(Value::Null) } else { Value::Null };
        return Some(error(reply_id, -32600, "invalid request: needs \"jsonrpc\": \"2.0\", a method, and a string or integer id"));
    }
    let id = id?.clone();
    let method = method.unwrap_or("");
    let params = obj.get("params").cloned().unwrap_or(json!({}));
    let result = match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = asked.filter(|v| PROTOCOLS.contains(v)).unwrap_or(PROTOCOLS[0]);
            json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "greenline", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Greenline reads PDF drawing sets and proposes markups and measurements for a person to review in the Greenline app. Call guide first.",
            })
        }
        "ping" => json!({}),
        "tools/list" => json!({ "tools": TOOLS.iter().map(schema).collect::<Vec<_>>() }),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else { return Some(error(id, -32602, "tools/call needs a tool \"name\"")) };
            let Some(tool) = TOOLS.iter().find(|t| t.name == name) else { return Some(error(id, -32602, format!("unknown tool \"{name}\""))) };
            match check(tool, params.get("arguments").unwrap_or(&Value::Null)) {
                Ok(call) => run(tool, call),
                Err(e) => return Some(error(id, -32602, e)),
            }
        }
        other => return Some(error(id, -32601, format!("method not found: {other}"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
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
            Err(e) => Some(error(Value::Null, -32700, format!("parse error: {e}"))),
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

    fn req(id: Value, method: &str, params: Value) -> Value {
        handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})).unwrap()
    }

    fn tool(name: &str) -> &'static Tool {
        TOOLS.iter().find(|t| t.name == name).unwrap()
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn negotiates_and_lists() {
        assert_eq!(req(json!(1), "initialize", json!({"protocolVersion": "2025-03-26"}))["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(req(json!(1), "initialize", json!({"protocolVersion": "2099-01-01"}))["result"]["protocolVersion"], PROTOCOLS[0], "an unknown version gets ours");
        assert!(handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let list = req(json!("a"), "tools/list", json!({}));
        assert_eq!(list["id"], "a");
        let tools = list["result"]["tools"].as_array().unwrap();
        let paths = tools.iter().find(|t| t["name"] == "paths").unwrap();
        assert!(paths["inputSchema"]["properties"]["limit"].is_object() && paths["inputSchema"]["properties"]["units"].is_object());
        assert_eq!(paths["inputSchema"]["required"], json!(["pdf", "page"]));
        let guide = req(json!(3), "tools/call", json!({"name": "guide", "arguments": {}}));
        assert!(guide["result"]["content"][0]["text"].as_str().unwrap().contains("page points"));
    }

    #[test]
    fn bad_requests_get_protocol_errors() {
        assert_eq!(handle(&json!({"jsonrpc": "1.0", "id": 1, "method": "ping"})).unwrap()["error"]["code"], -32600);
        assert_eq!(handle(&json!({"jsonrpc": "2.0", "id": true, "method": "ping"})).unwrap()["error"]["code"], -32600);
        assert_eq!(handle(&json!({"jsonrpc": "2.0", "id": 1.5, "method": "ping"})).unwrap()["error"]["code"], -32600);
        assert_eq!(handle(&json!([1, 2])).unwrap()["error"]["code"], -32600);
        assert_eq!(req(json!(1), "resources/list", json!({}))["error"]["code"], -32601);
        assert_eq!(req(json!(1), "tools/call", json!({"name": "nope"}))["error"]["code"], -32602);
        assert_eq!(req(json!(1), "tools/call", json!({}))["error"]["code"], -32602);
        assert_eq!(req(json!(1), "tools/call", json!({"name": "info", "arguments": {}}))["error"]["code"], -32602, "missing pdf");
        let run_failure = req(json!(1), "tools/call", json!({"name": "info", "arguments": {"pdf": "/no/such.pdf"}}));
        assert_eq!(run_failure["result"]["isError"], true, "a failure while running is a tool result");
    }

    #[test]
    fn arguments_are_checked_and_never_become_flags() {
        let c = check(tool("render"), &json!({"pdf": "--help", "page": 3, "region": [1, 2, 3.5, 4], "proposals": true, "markups": false, "dpi": 150})).unwrap();
        assert_eq!(c.pdf, "--help", "a path, whatever it looks like");
        assert_eq!(c.flags.get("region").map(String::as_str), Some("1,2,3.5,4"));
        assert_eq!(c.flags.get("page").map(String::as_str), Some("3"));
        assert!(c.flags.contains_key("proposals") && !c.flags.contains_key("markups"));
        let c = check(tool("apply"), &json!({"pdf": "/x.pdf", "ops": [{"op": "cloud"}], "dry_run": true})).unwrap();
        assert!(c.flags.contains_key("dry-run"));
        assert_eq!(c.ops, Some(json!([{"op": "cloud"}])));
        let err = |name: &str, args: Value| check(tool(name), &args).err().unwrap();
        assert!(err("apply", json!({"pdf": "/x.pdf", "ops": [], "dry_run": "true"})).contains("true or false"), "a string isn't a yes");
        assert!(err("text", json!({"pdf": "/x.pdf", "page": 1, "region": "0,0,1,1"})).contains("region"));
        assert!(err("text", json!({"pdf": "/x.pdf", "page": 0})).contains("page"));
        assert!(err("text", json!({"pdf": "/x.pdf", "page": 1, "units": "furlongs"})).contains("metric"));
        assert!(err("info", json!({"pdf": "/x.pdf", "dryrun": true})).contains("unknown argument \"dryrun\""));
        // Through the server, a pdf of "--help" is just a missing file, and nothing is printed.
        let r = req(json!(9), "tools/call", json!({"name": "info", "arguments": {"pdf": "--help"}}));
        assert_eq!(r["result"]["isError"], true);
    }
}
