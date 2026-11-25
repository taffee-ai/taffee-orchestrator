//! What the orchestrator knows about the world when it normalizes and admits a request:
//! how names and versions resolve, each principal's policy, and prices.

use super::manifest::{duration_secs, Effects};
use super::refs::{is_commit, Ref};
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolver {
    /// Short name → full names it may mean.
    #[serde(default)]
    pub names: BTreeMap<String, Vec<String>>,
    /// `name@version` → commit.
    #[serde(default)]
    pub versions: BTreeMap<String, String>,
    /// Used when a reference names no version.
    #[serde(default = "default_version")]
    pub default_version: String,
}

fn default_version() -> String {
    "main".into()
}

impl Default for Resolver {
    fn default() -> Self {
        Resolver { names: BTreeMap::new(), versions: BTreeMap::new(), default_version: default_version() }
    }
}

/// A reference turned into a pinned one, and what that took.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    pub pinned: String,
    /// What was interpreted, for `assumed`. None when the reference was already pinned.
    pub note: Option<String>,
    /// Only a version the caller named was pinned (WM-NRM-08 lets that through `refuse`).
    pub only_pinned_version: bool,
    /// What the caller would confirm: the full name at the version it named.
    pub confirm: String,
}

/// Why a reference could not be resolved to exactly one pinned reference.
#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    pub question: String,
    pub options: Vec<String>,
}

impl Resolver {
    pub fn resolve(&self, raw: &str) -> Result<Resolved, Gap> {
        let r = Ref::parse(raw).map_err(|e| Gap { question: e, options: vec![] })?;
        if r.is_pinned() {
            return Ok(Resolved { pinned: r.to_string(), note: None, only_pinned_version: true, confirm: r.to_string() });
        }
        let mut notes = Vec::new();
        let full = match self.names.get(&r.name) {
            Some(c) if c.len() == 1 => {
                notes.push(format!("\"{}\" is {}", r.name, c[0]));
                c[0].clone()
            }
            Some(c) if c.len() > 1 => {
                let options = c.iter().map(|n| match &r.version {
                    Some(v) => format!("{n}@{v}"),
                    None => n.clone(),
                });
                return Err(Gap { question: format!("\"{}\" matches more than one repository. Which one?", r.name), options: options.collect() });
            }
            _ if r.name.contains('/') => r.name.clone(),
            _ => return Err(Gap { question: format!("Which repository is \"{}\"?", r.name), options: vec![] }),
        };
        let named_version = r.version.is_some();
        let version = r.version.clone().unwrap_or_else(|| self.default_version.clone());
        let commit = if is_commit(&version) {
            version.clone()
        } else {
            match self.versions.get(&format!("{full}@{version}")) {
                Some(c) => c.clone(),
                None => {
                    return Err(Gap { question: format!("{full} has no version \"{version}\" that can be pinned. Which commit?"), options: vec![] })
                }
            }
        };
        if commit != version {
            let short = &commit[..7.min(commit.len())];
            notes.push(if named_version { format!("{version} is {short}") } else { format!("no version named; {version} is {short}") });
        }
        let only_pinned_version = notes.len() == 1 && named_version && self.names.get(&r.name).is_none();
        Ok(Resolved {
            pinned: format!("{full}@{commit}"),
            note: Some(notes.join("; ")),
            only_pinned_version,
            confirm: format!("{full}@{version}"),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyEntry {
    pub max_effects: Effects,
    pub max_usd: f64,
    pub max_time: String,
}

impl PolicyEntry {
    pub fn max_time_secs(&self) -> u64 {
        duration_secs(&self.max_time).unwrap_or(0)
    }
}

impl Default for PolicyEntry {
    fn default() -> Self {
        PolicyEntry { max_effects: Effects::ReadOnly, max_usd: 1.0, max_time: "30m".into() }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub principals: BTreeMap<String, PolicyEntry>,
    /// For principals with no entry of their own.
    #[serde(default)]
    pub default: PolicyEntry,
}

impl Policy {
    pub fn for_principal(&self, p: &str) -> &PolicyEntry {
        self.principals.get(p).unwrap_or(&self.default)
    }
}

/// A cost estimate made before work starts (WM-HND-07). The default makes none.
pub trait Estimator: Send + Sync {
    fn estimate(&self, _work: &serde_json::Value) -> Option<super::manifest::Limits> {
        None
    }
}

pub struct NoEstimate;
impl Estimator for NoEstimate {}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> Resolver {
        let mut r = Resolver::default();
        r.names.insert("payments-service".into(), vec!["github.com/acme/payments-service".into()]);
        r.names.insert("billing".into(), vec!["github.com/acme/billing".into(), "github.com/acme/billing-legacy".into()]);
        r.versions.insert("github.com/acme/payments-service@main".into(), "4f2c9e1b7a3d5c8e9f0a1b2c3d4e5f6a7b8c9d0e".into());
        r
    }

    #[test]
    fn resolves_short_name_and_default_version() {
        let x = resolver().resolve("payments-service").unwrap();
        assert_eq!(x.pinned, "github.com/acme/payments-service@4f2c9e1b7a3d5c8e9f0a1b2c3d4e5f6a7b8c9d0e");
        assert!(!x.only_pinned_version);
        assert_eq!(x.confirm, "github.com/acme/payments-service@main");
    }

    #[test]
    fn pinning_a_named_version_is_not_a_guess() {
        let x = resolver().resolve("github.com/acme/payments-service@main").unwrap();
        assert!(x.only_pinned_version);
        assert_eq!(x.note.as_deref(), Some("main is 4f2c9e1"));
    }

    #[test]
    fn ambiguous_and_unknown_names_are_gaps() {
        let g = resolver().resolve("billing").unwrap_err();
        assert_eq!(g.options, vec!["github.com/acme/billing", "github.com/acme/billing-legacy"]);
        assert!(resolver().resolve("nowhere").is_err());
    }
}
