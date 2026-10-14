//! Server profiles on disk (<data>/profiles.json) - the file side of
//! servers.load / save_all, shared with the backend until the servers
//! subsystem moves here as a whole.

use std::io::Write;
use std::path::Path;

use serde_json::Value;

use crate::paths;

pub fn path() -> std::path::PathBuf {
    paths::data().join("profiles.json")
}

pub fn load() -> Vec<Value> {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_array().cloned()).unwrap_or_default()
}

pub fn save_all(items: &[Value]) -> std::io::Result<()> {
    write_json_indent2(&path(), &Value::Array(items.to_vec()))
}

/// JSON the way Python's json.dump(indent=2) writes it: ", " is never used
/// at the end of a line, ": " between key and value, non-ASCII escaped.
/// Written to a temp file and renamed, so a reader never sees half of it.
pub fn write_json_indent2(p: &Path, v: &Value) -> std::io::Result<()> {
    let mut s = String::new();
    py_dump(v, 0, &mut s);
    // ⚠ Python writes these in TEXT mode, so on Windows every newline is
    // CRLF; match it, or the file flips format whichever side wrote last
    #[cfg(windows)]
    let s = s.replace('\n', "\r\n");
    let tmp = p.with_extension("json.part");
    std::fs::File::create(&tmp)?.write_all(s.as_bytes())?;
    std::fs::rename(&tmp, p)
}

fn py_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7f => {
                let mut b = [0u16; 2];
                for u in c.encode_utf16(&mut b) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn py_dump(v: &Value, ind: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                // python repr of a float: integral values keep ".0"
                if f.fract() == 0.0 && f.abs() < 1e16 { out.push_str(&format!("{f:.1}")) } else { out.push_str(&n.to_string()) }
            } else {
                out.push_str(&n.to_string());
            }
        }
        Value::String(s) => py_str(s, out),
        Value::Array(a) => {
            if a.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                out.push('\n');
                out.push_str(&" ".repeat(ind + 2));
                py_dump(x, ind + 2, out);
                if i + 1 < a.len() {
                    out.push(',');
                }
            }
            out.push('\n');
            out.push_str(&" ".repeat(ind));
            out.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                out.push('\n');
                out.push_str(&" ".repeat(ind + 2));
                py_str(k, out);
                out.push_str(": ");
                py_dump(x, ind + 2, out);
                if i + 1 < m.len() {
                    out.push(',');
                }
            }
            out.push('\n');
            out.push_str(&" ".repeat(ind));
            out.push('}');
        }
    }
}

/// srv<unix seconds><4 hex>: the id format the backend uses (random tail,
/// because two made in the same second collided).
pub fn new_id() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut b = [0u8; 2];
    let _ = getrandom::fill(&mut b);
    format!("srv{t}{:02x}{:02x}", b[0], b[1])
}

#[cfg(test)]
mod tests {
    #[test]
    fn python_indent2() {
        let v = serde_json::json!({"a": [1, 2.0, "é"], "b": {}, "c": []});
        let mut s = String::new();
        super::py_dump(&v, 0, &mut s);
        assert_eq!(s, "{\n  \"a\": [\n    1,\n    2.0,\n    \"\\u00e9\"\n  ],\n  \"b\": {},\n  \"c\": []\n}");
    }
}

/// json.dumps(v) with Python's defaults: ", " and ": " separators,
/// non-ASCII escaped. Used for the blobs handed to the server exe.
pub fn py_dumps(v: &Value) -> String {
    let mut s = String::new();
    py_compact(v, &mut s);
    s
}

fn py_compact(v: &Value, out: &mut String) {
    match v {
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 { out.push_str(", "); }
                py_compact(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                if i > 0 { out.push_str(", "); }
                py_str(k, out);
                out.push_str(": ");
                py_compact(x, out);
            }
            out.push('}');
        }
        other => py_dump(other, 0, out),
    }
}
