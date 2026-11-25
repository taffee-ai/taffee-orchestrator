//! RFC 8785 JSON Canonicalization Scheme (JCS) and `sha256:` digests (WM-FRZ-02).
//!
//! Objects are written with keys sorted by UTF-16 code units, no whitespace, strings escaped
//! the way ECMAScript's JSON.stringify does, and numbers in ECMAScript Number-to-String form.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

pub fn digest(v: &Value) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(canonical(v).as_bytes())))
}

pub fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|h| h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                out.push_str(&es_number(n.as_f64().unwrap_or(0.0)));
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out);
            }
            out.push(']');
        }
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(k, out);
                out.push(':');
                write(&m[*k], out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// ECMAScript Number::toString for a finite double.
fn es_number(f: f64) -> String {
    if f == 0.0 || !f.is_finite() {
        return "0".into();
    }
    let neg = f < 0.0;
    // Rust's `{:e}` gives the shortest round-trip digits: "1.2345e-7".
    let e = format!("{:e}", f.abs());
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let exp: i32 = exp.parse().unwrap_or(0);
    let k = digits.len() as i32;
    let n = exp + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n - 1 < 0 { "-" } else { "+" };
        let rest = if k > 1 { format!(".{}", &digits[1..]) } else { String::new() };
        format!("{}{rest}e{sign}{}", &digits[..1], (n - 1).abs())
    };
    if neg {
        format!("-{body}")
    } else {
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sorts_keys_and_drops_whitespace() {
        let v = json!({ "b": [1, { "z": true, "a": null }], "a": "x" });
        assert_eq!(canonical(&v), r#"{"a":"x","b":[1,{"a":null,"z":true}]}"#);
    }

    #[test]
    fn numbers_follow_ecmascript() {
        for (f, s) in [(0.5, "0.5"), (2.0, "2"), (1e21, "1e+21"), (1e-7, "1e-7"), (123.456, "123.456"), (0.000001, "0.000001"), (-1.5, "-1.5")] {
            assert_eq!(canonical(&json!(f)), s, "{f}");
        }
    }

    #[test]
    fn strings_escape_like_json_stringify() {
        assert_eq!(canonical(&json!("a\"b\\c\n\u{1}é")), "\"a\\\"b\\\\c\\n\\u0001é\"");
    }

    #[test]
    fn rfc8785_key_order_uses_utf16_units() {
        // RFC 8785 §3.2.3: sorting is by UTF-16 code units, so U+20AC (euro) comes before
        // U+1F600, whose first unit is the surrogate 0xD83D.
        let v = json!({ "\u{1f600}": 3, "\u{20ac}": 1, "\r": 2, "1": 4 });
        assert_eq!(canonical(&v), "{\"\\r\":2,\"1\":4,\"\u{20ac}\":1,\"\u{1f600}\":3}");
    }

    #[test]
    fn digest_shape() {
        let d = digest(&json!({}));
        assert!(is_digest(&d), "{d}");
    }
}
