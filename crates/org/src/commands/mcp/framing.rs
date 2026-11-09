// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Dual JSON-RPC 2.0 `stdio` framing (`\n`-delimited and HTTP/LSP `Content-Length`) for `org mcp`.

use std::io::{self, BufRead, Write};

use serde_json::Value;

/// Upper bound (`64 MiB`) on `Content-Length` payloads to prevent unbounded allocations.
const MAX_CONTENT_LENGTH_BYTES: usize = 64 * 1024 * 1024;

/// Maximum number of HTTP-style header lines accepted in a single framed message.
const MAX_HEADER_LINES: usize = 32;

/// JSON-RPC 2.0 parse error code (`-32700`).
const PARSE_ERROR_CODE: i64 = -32700;

/// Wire framing mode detected on an incoming `stdio` JSON-RPC message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FramingMode {
    /// Standard MCP newline-delimited UTF-8 JSON (`{"jsonrpc":"2.0",...}\n`).
    Newline,
    /// Header-framed UTF-8 JSON (`Content-Length: <N>\r\n\r\n<body_bytes>`).
    ContentLength,
}

/// Decoded outcome of reading a single frame from the `stdin` stream.
#[derive(Debug)]
pub(crate) enum IncomingFrame {
    /// Complete UTF-8 JSON frame paired with its wire [`FramingMode`].
    Message {
        /// Framing mode used by the sender.
        mode: FramingMode,
        /// Raw UTF-8 JSON payload text.
        raw_json: String,
    },
    /// Malformed header or invalid UTF-8 payload requiring a JSON-RPC error reply.
    ProtocolError {
        /// Framing mode to use for the error response.
        mode: FramingMode,
        /// JSON-RPC 2.0 error code.
        code: i64,
        /// Human-readable diagnostic message.
        message: String,
    },
}

/// Returns `Some((name, value))` when `line` looks like an HTTP/LSP `Header-Name: value` line.
fn split_http_header(line: &str) -> Option<(&str, &str)> {
    if line.starts_with('{') || line.starts_with('[') {
        return None;
    }
    let (raw_name, raw_val) = line.split_once(':')?;
    let name = raw_name.trim();
    let first = *name.as_bytes().first()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some((name, raw_val.trim()))
}

/// Parses and bounds-checks the numeric `Content-Length` header value.
fn parse_content_length_value(raw_len: &str) -> Result<usize, String> {
    match raw_len.trim().parse::<usize>() {
        Ok(n) if n <= MAX_CONTENT_LENGTH_BYTES => Ok(n),
        Ok(n) => Err(format!(
            "Content-Length {n} exceeds maximum allowed frame size"
        )),
        Err(err) => Err(format!("Invalid Content-Length header: {err}")),
    }
}

/// Updates `content_len` if `trimmed` is a `Content-Length` or malformed HTTP header line.
fn record_header_line(trimmed: &str, content_len: &mut Option<Result<usize, String>>) {
    match split_http_header(trimmed) {
        Some((name, val)) if name.eq_ignore_ascii_case("content-length") => {
            *content_len = Some(parse_content_length_value(val));
        }
        Some(_) => {}
        None => {
            *content_len = Some(Err(format!("Malformed HTTP header line: {trimmed}")));
        }
    }
}

/// Reads unordered HTTP/LSP header lines up to the empty separator line, returning `Content-Length`.
fn collect_header_content_length<R: BufRead>(
    reader: &mut R,
    first_name: &str,
    first_val: &str,
) -> io::Result<Option<Result<usize, String>>> {
    let mut content_len = first_name
        .eq_ignore_ascii_case("content-length")
        .then(|| parse_content_length_value(first_val));
    let mut header_line = String::new();
    let mut count = 1usize;

    loop {
        header_line.clear();
        if reader.read_line(&mut header_line)? == 0 {
            return Ok(None);
        }
        let trimmed = header_line.trim();
        if trimmed.is_empty() {
            let err = || Err("Missing Content-Length header".to_string());
            return Ok(Some(content_len.unwrap_or_else(err)));
        }
        count += 1;
        if count > MAX_HEADER_LINES {
            return Ok(Some(Err("Too many HTTP header lines".to_string())));
        }
        record_header_line(trimmed, &mut content_len);
    }
}

/// Reads a header-framed JSON-RPC payload after detecting the initial HTTP/LSP header line.
fn read_header_framed_message<R: BufRead>(
    reader: &mut R,
    first_name: &str,
    first_val: &str,
) -> io::Result<Option<IncomingFrame>> {
    let Some(len_res) = collect_header_content_length(reader, first_name, first_val)? else {
        return Ok(None);
    };
    let parsed_len = match len_res {
        Ok(n) => n,
        Err(message) => {
            return Ok(Some(IncomingFrame::ProtocolError {
                mode: FramingMode::ContentLength,
                code: PARSE_ERROR_CODE,
                message,
            }));
        }
    };

    let mut body = vec![0u8; parsed_len];
    if let Err(err) = reader.read_exact(&mut body) {
        return if err.kind() == io::ErrorKind::UnexpectedEof {
            Ok(None)
        } else {
            Err(err)
        };
    }

    Ok(Some(match String::from_utf8(body) {
        Ok(raw_json) => IncomingFrame::Message {
            mode: FramingMode::ContentLength,
            raw_json,
        },
        Err(err) => IncomingFrame::ProtocolError {
            mode: FramingMode::ContentLength,
            code: PARSE_ERROR_CODE,
            message: format!("Invalid UTF-8 in JSON-RPC payload: {err}"),
        },
    }))
}

/// Reads the next non-blank JSON-RPC frame from `reader`, returning `Ok(None)` at EOF.
pub(crate) fn read_frame<R: BufRead>(reader: &mut R) -> io::Result<Option<IncomingFrame>> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((name, val)) = split_http_header(trimmed) {
            return read_header_framed_message(reader, name, val);
        }
        return Ok(Some(IncomingFrame::Message {
            mode: FramingMode::Newline,
            raw_json: trimmed.to_string(),
        }));
    }
}

/// Serializes and flushes a JSON-RPC response `payload` to `writer` using `mode`.
pub(crate) fn write_frame<W: Write>(
    writer: &mut W,
    mode: FramingMode,
    payload: &Value,
) -> io::Result<()> {
    let serialized = serde_json::to_string(payload).unwrap_or_else(|_| {
        "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"Internal error\"}}"
            .to_string()
    });
    match mode {
        FramingMode::Newline => writeln!(writer, "{serialized}")?,
        FramingMode::ContentLength => {
            write!(
                writer,
                "Content-Length: {}\r\n\r\n{serialized}",
                serialized.len()
            )?;
        }
    }
    writer.flush()
}
