//! JSON-RPC over stdio (Content-Length framing).

use anyhow::Result;
use serde_json::Value;
use std::io::{self, BufRead, Write};
use std::sync::Mutex;

/// Cap LSP message bodies so a malicious/buggy client cannot OOM the server.
pub(super) const MAX_LSP_CONTENT_LENGTH: usize = 16 * 1024 * 1024;

static STDOUT_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn read_message(r: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse::<usize>()?);
        }
    }
    let len = match content_length {
        Some(l) => l,
        None => return Ok(None),
    };
    if len > MAX_LSP_CONTENT_LENGTH {
        anyhow::bail!("LSP Content-Length {len} exceeds limit {MAX_LSP_CONTENT_LENGTH}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(Some(serde_json::from_slice(&buf)?))
}

pub(super) fn write_message(w: &mut impl Write, v: &Value) -> Result<()> {
    let body = serde_json::to_vec(v)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()?;
    Ok(())
}

/// Thread-safe stdout write for responses and `publishDiagnostics` notifications.
pub(super) fn write_stdout(v: &Value) -> Result<()> {
    let _g = STDOUT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = io::stdout().lock();
    write_message(&mut out, v)
}
