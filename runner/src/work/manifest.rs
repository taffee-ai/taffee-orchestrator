//! The work manifest (docs/spec/work-manifest/manifest.schema.json): a request for work,
//! sparse when a caller sends it, frozen (it has a `digest`) when it may run.

use super::jcs;
use super::refs::Ref;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const VERSION: &str = "0.1";

/// The fields that define the work. The digest covers these and nothing else (WM-FRZ-02),
/// and they are all the outer plan ever sees (WM-FRZ-05).
pub const WORK_FIELDS: &[&str] = &[
    "manifest", "want", "why", "given", "kind", "parameters", "decide", "deliver", "requires", "done_when", "checks",
    "limits", "partial_ok", "delegation", "parent",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Effects {
    #[serde(rename = "read-only")]
    ReadOnly,
    #[serde(rename = "reversible")]
    Reversible,
    #[serde(rename = "irreversible")]
    Irreversible,
}

impl Effects {
    pub fn as_str(self) -> &'static str {
        match self {
            Effects::ReadOnly => "read-only",
            Effects::Reversible => "reversible",
            Effects::Irreversible => "irreversible",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IfUnclear {
    Assume,
    Refuse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReplyMode {
    #[default]
    Wait,
    Receipt,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effects: Option<Effects>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Deliver {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub equivalence: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Delegation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// "0.1". YAML callers may send it as a number; it is normalized to the string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, rename = "for", skip_serializing_if = "Option::is_none")]
    pub for_: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub said: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub want: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub given: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_when: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumed: Option<BTreeMap<String, String>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decide: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<Deliver>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial_ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegation: Option<Delegation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub if_unclear: Option<IfUnclear>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved: Option<Effects>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ReplyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,

    /// Everything else. Only `x-*` names are allowed here (WM-REQ-04).
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// A structural problem with a request: which rule it breaks, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Invalid {
    pub rule: &'static str,
    pub detail: String,
}

impl Invalid {
    pub fn new(rule: &'static str, detail: impl Into<String>) -> Self {
        Invalid { rule, detail: detail.into() }
    }
}

pub fn is_id(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 64 && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

pub fn is_principal(s: &str) -> bool {
    match s.split_once(':') {
        Some((kind, rest)) => {
            is_id(&kind.replace('-', "_")) && !rest.is_empty() && rest.len() <= 200 && !rest.chars().any(char::is_whitespace)
        }
        None => false,
    }
}

/// "10m" → 600 seconds.
pub fn duration_secs(s: &str) -> Option<u64> {
    let (num, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: u64 = num.parse().ok().filter(|n| *n > 0)?;
    match unit {
        "s" => Some(n),
        "m" => Some(n * 60),
        "h" => Some(n * 3600),
        _ => None,
    }
}

/// 600 → "10m"; 94 → "94s".
pub fn format_duration(secs: u64) -> String {
    let secs = secs.max(1);
    if secs % 3600 == 0 {
        format!("{}h", secs / 3600)
    } else if secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

impl Manifest {
    pub fn from_value(v: Value) -> Result<Manifest, Invalid> {
        if !v.is_object() {
            return Err(Invalid::new("WM-REQ-02", "a manifest is a JSON object"));
        }
        serde_json::from_value(v).map_err(|e| Invalid::new("WM-REQ-02", format!("not a valid manifest: {e}")))
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    pub fn is_frozen(&self) -> bool {
        self.digest.is_some()
    }

    pub fn effects(&self) -> Effects {
        self.limits.as_ref().and_then(|l| l.effects).unwrap_or(Effects::ReadOnly)
    }

    pub fn depth(&self) -> u32 {
        self.delegation.as_ref().and_then(|d| d.depth).unwrap_or(0)
    }

    /// The JSON object the digest is computed over, and the outer plan's only input.
    pub fn work_fields(&self) -> Value {
        let mut v = self.to_value();
        let obj = v.as_object_mut().expect("manifest serializes to an object");
        obj.retain(|k, _| WORK_FIELDS.contains(&k.as_str()));
        obj.insert("manifest".into(), Value::String(VERSION.into()));
        v
    }

    pub fn compute_digest(&self) -> String {
        jcs::digest(&self.work_fields())
    }

    /// Set `digest` from the current work fields.
    pub fn freeze(&mut self) {
        self.manifest = Some(Value::String(VERSION.into()));
        self.digest = Some(self.compute_digest());
    }

    /// Checks every request must pass, sparse or frozen (WM-REQ-02, WM-REQ-04).
    pub fn check_structure(&self) -> Result<(), Invalid> {
        match &self.manifest {
            Some(Value::String(s)) if s == VERSION => {}
            Some(Value::Number(n)) if n.as_f64() == Some(0.1) => {}
            Some(other) => return Err(Invalid::new("WM-REQ-02", format!("manifest version {other} is not supported (supported: 0.1)"))),
            None => return Err(Invalid::new("WM-REQ-02", "`manifest` (the version, 0.1) is required")),
        }
        if let Some(k) = self.extra.keys().find(|k| !k.starts_with("x-")) {
            return Err(Invalid::new("WM-REQ-04", format!("unknown field `{k}`")));
        }
        if self.want.as_deref().is_none_or(|w| w.trim().is_empty()) && self.kind.is_none() {
            return Err(Invalid::new("WM-REQ-02", "send `want` or `kind`"));
        }
        if self.want.as_ref().is_some_and(|w| w.len() > 8000) {
            return Err(Invalid::new("WM-REQ-02", "`want` is longer than 8000 characters"));
        }
        for p in self.by.iter().chain(self.for_.iter().flatten()) {
            if !is_principal(p) {
                return Err(Invalid::new("WM-REQ-02", format!("'{p}' is not a principal (kind:name)")));
            }
        }
        let id_maps: [(&str, Option<Vec<&String>>); 5] = [
            ("given", self.given.as_ref().map(|m| m.keys().collect())),
            ("done_when", self.done_when.as_ref().map(|m| m.keys().collect())),
            ("checks", self.checks.as_ref().map(|m| m.keys().collect())),
            ("requires", self.requires.as_ref().map(|m| m.keys().collect())),
            ("decide", self.decide.as_ref().map(|m| m.keys().collect())),
        ];
        for (field, keys) in id_maps {
            for k in keys.into_iter().flatten() {
                if !is_id(k) {
                    return Err(Invalid::new("WM-REQ-02", format!("`{field}.{k}` is not a valid id")));
                }
                if (field == "checks" || field == "requires") && k == "deliver" {
                    return Err(Invalid::new("WM-REQ-02", format!("`{field}.deliver` is reserved for the implicit check")));
                }
            }
        }
        for (field, map) in [("checks", &self.checks), ("requires", &self.requires)] {
            for (k, c) in map.iter().flatten() {
                let ok = match c {
                    Value::String(s) => !s.trim().is_empty(),
                    Value::Object(o) => o.len() == 1 && o.get("verifier").and_then(Value::as_str).and_then(|r| Ref::parse(r).ok()).is_some_and(|r| r.is_pinned()),
                    _ => false,
                };
                if !ok {
                    return Err(Invalid::new("WM-CHK-01", format!("`{field}.{k}` must be a CEL expression or {{verifier: <pinned ref>}}")));
                }
            }
        }
        for (k, r) in self.given.iter().flatten() {
            Ref::parse(r).map_err(|e| Invalid::new("WM-REQ-02", format!("given.{k}: {e}")))?;
        }
        if let Some(k) = &self.kind {
            Ref::parse(k).map_err(|e| Invalid::new("WM-REQ-02", format!("kind: {e}")))?;
        }
        if let Some(l) = &self.limits {
            if let Some(t) = &l.time {
                duration_secs(t).ok_or_else(|| Invalid::new("WM-REQ-02", format!("limits.time '{t}' is not a duration like 90s, 10m or 1h")))?;
            }
            if l.usd.is_some_and(|u| !(u >= 0.0)) {
                return Err(Invalid::new("WM-REQ-02", "limits.usd must be zero or more"));
            }
        }
        if let Some(p) = &self.parent {
            if !jcs::is_digest(p) {
                return Err(Invalid::new("WM-REQ-02", "parent must be a sha256: digest"));
            }
        }
        Ok(())
    }

    /// Checks only a frozen manifest must pass (WM-FRZ-01, WM-FRZ-03, WM-FRZ-06).
    pub fn check_frozen(&self) -> Result<(), Invalid> {
        let missing = [
            ("by", self.by.is_none()),
            ("want", self.want.is_none()),
            ("given", self.given.is_none()),
            ("kind", self.kind.is_none()),
            ("parameters", self.parameters.is_none()),
            ("decide", self.decide.is_none()),
            ("deliver", self.deliver.is_none()),
            ("requires", self.requires.is_none()),
            ("done_when", self.done_when.is_none()),
            ("checks", self.checks.is_none()),
            ("limits", self.limits.is_none()),
            ("partial_ok", self.partial_ok.is_none()),
            ("delegation", self.delegation.is_none()),
        ];
        if let Some((f, _)) = missing.iter().find(|(_, m)| *m) {
            return Err(Invalid::new("WM-FRZ-01", format!("a frozen manifest needs `{f}`")));
        }
        let l = self.limits.as_ref().unwrap();
        if l.effects.is_none() || l.usd.is_none() || l.time.is_none() {
            return Err(Invalid::new("WM-FRZ-01", "a frozen manifest needs limits.effects, limits.usd and limits.time"));
        }
        let pinned = |s: &str| Ref::parse(s).is_ok_and(|r| r.is_pinned());
        for (k, r) in self.given.iter().flatten() {
            if !pinned(r) {
                return Err(Invalid::new("WM-FRZ-03", format!("given.{k} '{r}' is not pinned")));
            }
        }
        if !pinned(self.kind.as_deref().unwrap()) {
            return Err(Invalid::new("WM-FRZ-03", "kind is not pinned"));
        }
        if !pinned(&self.deliver.as_ref().unwrap().schema) {
            return Err(Invalid::new("WM-FRZ-03", "deliver.schema is not pinned"));
        }
        let dw: Vec<&String> = self.done_when.as_ref().unwrap().keys().collect();
        let ck: Vec<&String> = self.checks.as_ref().unwrap().keys().collect();
        if dw != ck {
            return Err(Invalid::new("WM-FRZ-06", "done_when and checks must have the same ids"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_ignores_who_wording_and_handling() {
        let mut a = Manifest::from_value(json!({
            "manifest": "0.1", "want": "w", "kind": "k@1", "parameters": {"a": 1}, "by": "user:a",
            "said": "hi", "dry_run": true, "x-note": 3, "assumed": {"kind": "x"}
        }))
        .unwrap();
        let b = Manifest::from_value(json!({ "manifest": 0.1, "want": "w", "kind": "k@1", "parameters": {"a": 1}, "by": "service:b" })).unwrap();
        assert_eq!(a.compute_digest(), b.compute_digest());
        a.want = Some("other".into());
        assert_ne!(a.compute_digest(), b.compute_digest());
    }

    #[test]
    fn unknown_fields_are_refused_and_x_fields_kept() {
        let m = Manifest::from_value(json!({ "manifest": "0.1", "want": "w", "priority": "high" })).unwrap();
        assert_eq!(m.check_structure().unwrap_err().rule, "WM-REQ-04");
        let m = Manifest::from_value(json!({ "manifest": "0.1", "want": "w", "x-ticket": "PAY-1" })).unwrap();
        assert!(m.check_structure().is_ok());
        assert_eq!(m.extra["x-ticket"], "PAY-1");
    }

    #[test]
    fn needs_want_or_kind() {
        let m = Manifest::from_value(json!({ "manifest": "0.1" })).unwrap();
        assert_eq!(m.check_structure().unwrap_err().rule, "WM-REQ-02");
    }

    #[test]
    fn durations() {
        assert_eq!(duration_secs("10m"), Some(600));
        assert_eq!(duration_secs("0s"), None);
        assert_eq!(duration_secs("5d"), None);
        assert_eq!(format_duration(600), "10m");
        assert_eq!(format_duration(94), "94s");
    }

    #[test]
    fn principals() {
        assert!(is_principal("user:alice"));
        assert!(is_principal("orchestrator:taffee-a"));
        assert!(!is_principal("alice"));
        assert!(!is_principal("User:alice"));
    }
}
