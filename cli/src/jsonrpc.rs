//! JSON-RPC 2.0 over stdio, one message a line each way: what the CLI's
//! two servers share (`fragment mcp`, mcp.rs; `fragment acp`, acp/). Each
//! keeps its own methods; a line's reading, an answer's shape and the
//! protocol's error codes are here.

use std::io::{BufRead, Read};

use serde_json::{json, Value};

/// One message, at most: an operation's input is at most 256 KiB
/// (`limits::INPUT_MAX_BYTES`), and the rest of a call is small. A longer
/// line is read to its end and refused, and the session goes on.
pub const MESSAGE_MAX_BYTES: usize = 1024 * 1024;

// JSON-RPC's error codes (MCP answers an unknown tool with -32602).
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

/// An error answered as JSON-RPC's `error`, with the platform's (or the
/// CLI's) code in its `data` when there is one.
#[derive(Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError { code, message: message.into(), data: None }
    }
}

pub enum Line {
    Text,
    TooLong,
    End,
}

/// The next line into `buf`, at most `MESSAGE_MAX_BYTES` of it: a longer
/// one is read to its end, 64 KiB at a time, and dropped.
pub fn read_line(input: &mut impl BufRead, buf: &mut Vec<u8>) -> std::io::Result<Line> {
    buf.clear();
    let limit = u64::try_from(MESSAGE_MAX_BYTES).expect("the limit fits") + 1;
    if input.by_ref().take(limit).read_until(b'\n', buf)? == 0 {
        return Ok(Line::End);
    }
    if buf.last() == Some(&b'\n') || buf.len() <= MESSAGE_MAX_BYTES {
        return Ok(Line::Text);
    }
    let mut rest = Vec::new();
    // bounded by the client's line: each pass reads at most 64 KiB of it
    loop {
        rest.clear();
        if input.by_ref().take(64 * 1024).read_until(b'\n', &mut rest)? == 0 || rest.last() == Some(&b'\n') {
            return Ok(Line::TooLong);
        }
    }
}

pub fn success(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn failure(id: Value, e: RpcError) -> Value {
    let mut error = json!({ "code": e.code, "message": e.message });
    if let Some(data) = e.data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// A message as one line of output: its JSON, then a newline.
pub fn line(message: &Value) -> Vec<u8> {
    let mut text = serde_json::to_vec(message).expect("a JSON value serializes");
    assert!(!text.contains(&b'\n'), "a message is one line");
    text.push(b'\n');
    text
}
