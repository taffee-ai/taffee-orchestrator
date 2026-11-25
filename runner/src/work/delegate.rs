//! Handing work on: to another orchestrator, or to a worker in the inner loop. A child
//! manifest can only narrow its parent (WM-DLG-*).

use super::manifest::{duration_secs, Delegation, Effects, Manifest};
use super::normalize::{normalize, Ctx};
use super::refs::Ref;

/// What the parent has used so far.
#[derive(Debug, Clone, Copy, Default)]
pub struct Spent {
    pub usd: f64,
    pub secs: u64,
}

pub enum Target {
    /// Another orchestrator: `delegation.depth` must go down (WM-DLG-04).
    Orchestrator,
    /// A worker in this orchestrator's inner loop: always depth 0.
    Worker,
}

/// Freeze `child` as a hand-off of `parent`, or say which rules it breaks.
pub fn delegate(ctx: &Ctx, parent: &Manifest, spent: Spent, mut child: Manifest, delegator: &str, target: Target) -> Result<Manifest, Vec<String>> {
    let mut violates: Vec<String> = Vec::new();
    let mut flag = |rule: &str| {
        if !violates.iter().any(|v| v == rule) {
            violates.push(rule.to_string());
        }
    };
    let pl = parent.limits.clone().unwrap_or_default();
    let remaining_usd = pl.usd.unwrap_or(0.0) - spent.usd;
    let remaining_secs = pl.time.as_deref().and_then(duration_secs).unwrap_or(0).saturating_sub(spent.secs);

    // depth (WM-DLG-04)
    let depth = match target {
        Target::Worker => 0,
        Target::Orchestrator => {
            if parent.depth() == 0 {
                flag("WM-DLG-04");
                0
            } else {
                let asked = child.delegation.as_ref().and_then(|d| d.depth).unwrap_or(parent.depth() - 1);
                if asked >= parent.depth() {
                    flag("WM-DLG-04");
                }
                asked.min(parent.depth().saturating_sub(1))
            }
        }
    };
    child.delegation = Some(Delegation { depth: Some(depth) });

    // limits and approval (WM-DLG-02); unset limits take what the parent has left
    let cl = child.limits.get_or_insert_with(Default::default);
    if cl.usd.is_none() {
        cl.usd = Some((remaining_usd.max(0.0) * 100.0).floor() / 100.0);
    }
    if cl.time.is_none() {
        cl.time = Some(super::manifest::format_duration(remaining_secs.max(1)));
    }
    if cl.usd.unwrap() > remaining_usd + 1e-9 {
        flag("WM-DLG-02");
    }
    if cl.time.as_deref().and_then(duration_secs).unwrap_or(u64::MAX) > remaining_secs {
        flag("WM-DLG-02");
    }
    let parent_effects = parent.effects();
    let child_kind_effects = child.kind.as_deref().and_then(|k| match ctx.registry.lookup(k) {
        super::kind::Lookup::Found(e) => e.kind.limits.effects,
        _ => None,
    });
    if cl.effects.or(child_kind_effects).is_some_and(|e| e > parent_effects) {
        flag("WM-DLG-02");
    }
    if child.approved.unwrap_or(Effects::ReadOnly) > parent.approved.unwrap_or(Effects::ReadOnly) {
        flag("WM-DLG-02");
    }

    // inputs (WM-DLG-03): only the parent's pinned inputs
    let parent_inputs: Vec<&String> = parent.given.iter().flatten().map(|(_, v)| v).collect();
    for v in child.given.iter().flatten().map(|(_, v)| v) {
        let pinned = Ref::parse(v).is_ok_and(|r| r.is_pinned());
        if !pinned || !parent_inputs.contains(&v) {
            flag("WM-DLG-03");
        }
    }
    if !violates.is_empty() {
        return Err(violates);
    }

    // freeze it as the delegator's own request
    let mut chain = parent.for_.clone().unwrap_or_default();
    chain.extend(parent.by.clone());
    child.by = None;
    child.for_ = Some(chain);
    child.parent = parent.digest.clone();
    child.approved = child.approved.or(parent.approved);
    child.manifest.get_or_insert(serde_json::Value::String(super::manifest::VERSION.into()));
    match normalize(ctx, child, delegator, None) {
        Ok(n) => {
            let mut m = n.manifest;
            // Delegated work is decided by the parent; nothing in it is the delegator's guess.
            m.assumed = None;
            m.dry_run = None;
            m.freeze();
            Ok(m)
        }
        Err(r) => Err(if r.violates.is_empty() { vec!["WM-DLG-05".into()] } else { r.violates }),
    }
}
