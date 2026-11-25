//! Intent kinds (docs/spec/work-manifest/kind.schema.json) and the registry that holds them.
//!
//! A published kind version is immutable; its digest is sha256 over its JCS form
//! (WM-KND-01). The registry also keeps every deliver schema by digest, so a frozen manifest
//! can be run from its own references without looking anything up by name (WM-FRZ-04).

use super::jcs;
use super::manifest::Limits;
use super::refs::Ref;
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct GivenSlot {
    pub is: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KindDeliver {
    pub schema: Value,
    #[serde(default)]
    pub order: Option<Vec<String>>,
    #[serde(default)]
    pub equivalence: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Decide {
    pub text: String,
    pub options: Vec<String>,
    pub default: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Coverage {
    pub given: String,
    pub paths: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Kind {
    pub id: String,
    pub summary: String,
    pub want_template: String,
    #[serde(default)]
    pub given: BTreeMap<String, GivenSlot>,
    pub parameters: Value,
    pub deliver: KindDeliver,
    #[serde(default)]
    pub functions: BTreeMap<String, Value>,
    #[serde(default)]
    pub coverage: Option<Coverage>,
    #[serde(default)]
    pub requires: BTreeMap<String, Value>,
    pub done_when: BTreeMap<String, String>,
    pub checks: BTreeMap<String, Value>,
    #[serde(default)]
    pub decide: BTreeMap<String, Decide>,
    pub limits: Limits,
    pub partial_ok: bool,
    #[serde(default)]
    pub delegation: Option<super::manifest::Delegation>,
    #[serde(default)]
    pub examples: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: Kind,
    /// sha256 over the JCS form of the kind document.
    pub digest: String,
    /// `<id>/deliver#<digest of the deliver schema>`
    pub deliver_ref: String,
    pub source: Value,
}

impl Entry {
    /// `code.impact_analysis@1#sha256:…`
    pub fn pinned(&self) -> String {
        format!("{}#{}", self.kind.id, self.digest)
    }

    pub fn family(&self) -> (&str, u32) {
        split_id(&self.kind.id).unwrap_or((self.kind.id.as_str(), 0))
    }

    /// Defaults from the parameter schema (`properties.*.default`).
    pub fn parameter_defaults(&self) -> Map<String, Value> {
        let mut out = Map::new();
        if let Some(props) = self.kind.parameters.get("properties").and_then(Value::as_object) {
            for (k, p) in props {
                if let Some(d) = p.get("default") {
                    out.insert(k.clone(), d.clone());
                }
            }
        }
        out
    }

    pub fn required_parameters(&self) -> Vec<String> {
        self.kind.parameters.get("required").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
    }

    /// `want_template` with `{name}` replaced by each parameter (strings as is, others as JSON).
    pub fn render_want(&self, params: &Map<String, Value>) -> String {
        let mut s = self.kind.want_template.clone();
        for (k, v) in params {
            let text = match v {
                Value::String(t) => t.clone(),
                other => other.to_string(),
            };
            s = s.replace(&format!("{{{k}}}"), &text);
        }
        s
    }
}

fn split_id(id: &str) -> Option<(&str, u32)> {
    let (fam, major) = id.rsplit_once('@')?;
    Some((fam, major.parse().ok()?))
}

#[derive(Debug, Clone, Default)]
pub struct Registry {
    entries: BTreeMap<String, Entry>,
    /// deliver schemas by digest
    content: BTreeMap<String, Value>,
}

pub enum Lookup<'a> {
    Found(&'a Entry),
    Unknown,
    /// A pinned reference whose digest does not match the published kind.
    DigestMismatch,
}

impl Registry {
    pub fn load_dir(dir: &Path) -> Result<Registry> {
        let mut r = Registry::default();
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading kinds directory {}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("yaml" | "yml" | "json")))
            .collect();
        paths.sort();
        for p in paths {
            let text = std::fs::read_to_string(&p)?;
            let v: Value = if p.extension().is_some_and(|e| e == "json") {
                serde_json::from_str(&text)?
            } else {
                yaml_to_json(&text)?
            };
            r.insert(v).with_context(|| format!("kind {}", p.display()))?;
        }
        Ok(r)
    }

    pub fn insert(&mut self, source: Value) -> Result<&Entry> {
        let kind: Kind = serde_json::from_value(source.clone()).context("not a valid kind")?;
        if split_id(&kind.id).is_none() {
            bail!("kind id '{}' must be namespace.name@major", kind.id);
        }
        for (id, d) in &kind.decide {
            if !d.options.contains(&d.default) {
                bail!("decide.{id}: default '{}' is not one of its options", d.default);
            }
        }
        if kind.done_when.keys().ne(kind.checks.keys()) {
            bail!("done_when and checks must have the same ids");
        }
        let le = &kind.limits;
        if le.effects.is_none() || le.usd.is_none() || le.time.is_none() {
            bail!("limits must set effects, usd and time");
        }
        let deliver_digest = jcs::digest(&kind.deliver.schema);
        let entry = Entry {
            digest: jcs::digest(&source),
            deliver_ref: format!("{}/deliver#{deliver_digest}", kind.id),
            kind,
            source,
        };
        self.content.insert(deliver_digest, entry.kind.deliver.schema.clone());
        let id = entry.kind.id.clone();
        self.entries.insert(id.clone(), entry);
        Ok(&self.entries[&id])
    }

    /// `code.impact_analysis` (latest major), `code.impact_analysis@1`, or the pinned form.
    pub fn lookup(&self, reference: &str) -> Lookup<'_> {
        let Ok(r) = Ref::parse(reference) else { return Lookup::Unknown };
        let entry = match &r.version {
            Some(v) => self.entries.get(&format!("{}@{v}", r.name)),
            None => self.entries.values().filter(|e| e.family().0 == r.name).max_by_key(|e| e.family().1),
        };
        match (entry, &r.digest) {
            (None, _) => Lookup::Unknown,
            (Some(e), Some(d)) if *d != e.digest => Lookup::DigestMismatch,
            (Some(e), _) => Lookup::Found(e),
        }
    }

    pub fn by_digest(&self, digest: &str) -> Option<&Entry> {
        self.entries.values().find(|e| e.digest == digest)
    }

    /// Content fetched by digest: deliver schemas.
    pub fn content(&self, reference: &str) -> Option<&Value> {
        let r = Ref::parse(reference).ok()?;
        self.content.get(r.digest.as_deref()?)
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }
}

/// YAML (with merge keys) to JSON.
pub fn yaml_to_json(text: &str) -> Result<Value> {
    let mut y: serde_yaml::Value = serde_yaml::from_str(text)?;
    y.apply_merge()?;
    Ok(serde_json::to_value(y)?)
}
