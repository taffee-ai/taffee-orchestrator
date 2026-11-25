//! Intake: a request (sparse or frozen) becomes a frozen manifest that may run, or a reply
//! that says why not. Everything here is deterministic. The only model involvement is the
//! optional `Proposal` a sentence normalizer made from free text, and every part of it is
//! checked and recorded in `assumed`.

use super::context::{Estimator, Policy, Resolver};
use super::kind::{Entry, Lookup, Registry};
use super::manifest::{duration_secs, format_duration, Deliver, Delegation, Effects, IfUnclear, Invalid, Limits, Manifest};
use super::reply::{Normalizer, Question, Reason, Reply, Status};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub struct Ctx<'a> {
    pub registry: &'a Registry,
    pub resolver: &'a Resolver,
    pub policy: &'a Policy,
    pub estimator: &'a dyn Estimator,
    pub normalizer: Normalizer,
}

/// What a sentence normalizer read out of free text. Never trusted as is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Proposal {
    /// None: no kind fits.
    pub kind: Option<String>,
    pub parameters: Map<String, Value>,
    /// Input names exactly as the request said them.
    pub given: BTreeMap<String, String>,
    /// The words that decided the kind.
    pub reason: Option<String>,
}

pub enum Intake {
    Reply(Box<Reply>),
    Run(Box<Manifest>),
}

fn invalid(i: Invalid) -> Reply {
    Reply::refused(Reason::Invalid, i.rule, i.detail)
}

/// Normalize, then admit.
pub fn intake(ctx: &Ctx, req: Manifest, principal: &str, proposal: Option<Proposal>) -> Intake {
    let request_id = req.id.clone();
    let mut reply = match normalize(ctx, req, principal, proposal) {
        Ok(n) => match admit(ctx, n.manifest, principal) {
            Ok(m) => return Intake::Run(Box::new(m)),
            Err(r) => r,
        },
        Err(r) => r,
    };
    reply.request = request_id;
    reply.normalized_by.get_or_insert(ctx.normalizer.clone());
    Intake::Reply(Box::new(reply))
}

pub struct Normalized {
    pub manifest: Manifest,
}

pub fn normalize(ctx: &Ctx, req: Manifest, principal: &str, proposal: Option<Proposal>) -> Result<Normalized, Reply> {
    req.check_structure().map_err(invalid)?;
    if let Some(by) = &req.by {
        if by != principal {
            return Err(Reply::refused(Reason::Invalid, "WM-REQ-03", format!("`by` is {by}, but the request was authenticated as {principal}")));
        }
    }
    if req.is_frozen() {
        return frozen(ctx, req, principal).map(|manifest| Normalized { manifest });
    }
    sparse(ctx, req, principal, proposal).map(|manifest| Normalized { manifest })
}

/// A frozen manifest is checked and never changed (WM-FRZ-07).
fn frozen(ctx: &Ctx, mut m: Manifest, principal: &str) -> Result<Manifest, Reply> {
    m.check_frozen().map_err(invalid)?;
    if m.compute_digest() != *m.digest.as_ref().unwrap() {
        return Err(Reply::refused(Reason::Invalid, "WM-FRZ-07", "the digest does not match the manifest's content"));
    }
    match ctx.registry.lookup(m.kind.as_deref().unwrap()) {
        Lookup::Found(_) => {}
        _ => return Err(Reply::refused(Reason::Invalid, "WM-FRZ-04", "this orchestrator has no kind with that digest")),
    }
    if ctx.registry.content(&m.deliver.as_ref().unwrap().schema).is_none() {
        return Err(Reply::refused(Reason::Invalid, "WM-FRZ-04", "this orchestrator cannot fetch deliver.schema by its digest"));
    }
    m.by = Some(principal.to_string());
    m.assumed = None;
    let pol = ctx.policy.for_principal(principal);
    let l = m.limits.clone().unwrap();
    if l.usd.unwrap() > pol.max_usd || duration_secs(l.time.as_deref().unwrap()).unwrap_or(0) > pol.max_time_secs() {
        let mut counter = m.clone();
        let cl = counter.limits.as_mut().unwrap();
        cl.usd = Some(l.usd.unwrap().min(pol.max_usd));
        cl.time = Some(format_duration(duration_secs(l.time.as_deref().unwrap()).unwrap_or(0).min(pol.max_time_secs())));
        counter.freeze();
        let mut r = Reply::refused(Reason::OutOfPolicy, "WM-FRZ-07", format!("limits exceed the policy for {principal}"));
        r.counter = Some(counter);
        return Err(r);
    }
    Ok(m)
}

fn gap(field: String, question: String, options: Vec<String>, rule: &str) -> Reply {
    let mut r = Reply::refused(Reason::Unclear, rule, format!("{field}: {question}"));
    r.questions = vec![Question { field, question, options }];
    r
}

fn sparse(ctx: &Ctx, req: Manifest, principal: &str, proposal: Option<Proposal>) -> Result<Manifest, Reply> {
    let mut assumed: BTreeMap<String, String> = BTreeMap::new();
    // Interpretations that `if_unclear: refuse` does not let through (WM-NRM-08).
    let mut guesses: Vec<Question> = Vec::new();
    let said = req.want.clone();
    let proposal = if req.kind.is_none() { proposal } else { None };

    // ---- kind
    let (entry, _from_text): (&Entry, bool) = match (&req.kind, &proposal) {
        (Some(k), _) => match ctx.registry.lookup(k) {
            Lookup::Found(e) => (e, false),
            Lookup::DigestMismatch => return Err(Reply::refused(Reason::Invalid, "WM-KND-01", format!("kind {k}: digest does not match the published kind"))),
            Lookup::Unknown => return Err(Reply::refused(Reason::Unsupported, "WM-NRM-06", format!("no kind {k} here"))),
        },
        (None, Some(Proposal { kind: Some(k), reason, .. })) => match ctx.registry.lookup(k) {
            Lookup::Found(e) => {
                let why = reason.as_ref().map(|r| format!("\"{r}\" read as ")).unwrap_or_else(|| "read as ".into());
                assumed.insert("kind".into(), format!("{why}{}", e.kind.id));
                guesses.push(Question { field: "kind".into(), question: format!("Is this {}: {}?", e.kind.id, e.kind.summary), options: vec![e.kind.id.clone()] });
                (e, true)
            }
            _ => return Err(Reply::refused(Reason::Unsupported, "WM-NRM-06", "no kind fits this request")),
        },
        (None, _) => return Err(Reply::refused(Reason::Unsupported, "WM-NRM-06", "no kind fits this request")),
    };
    let kind = &entry.kind;

    // ---- parameters
    let mut params = req.parameters.clone().unwrap_or_default();
    if let Some(p) = &proposal {
        for (k, v) in &p.parameters {
            if params.contains_key(k) {
                continue;
            }
            let verbatim = matches!((v, &said), (Value::String(s), Some(text)) if text.contains(s.as_str()));
            if !verbatim {
                assumed.insert(format!("parameters.{k}"), format!("{v} taken from the request"));
                guesses.push(Question { field: format!("parameters.{k}"), question: format!("Is {k} {v}?"), options: vec![v.to_string()] });
            }
            params.insert(k.clone(), v.clone());
        }
    }
    for (k, d) in entry.parameter_defaults() {
        params.entry(k).or_insert(d);
    }
    for k in entry.required_parameters() {
        if !params.contains_key(&k) {
            return Err(gap(format!("parameters.{k}"), format!("What is {k}?"), vec![], "WM-NRM-07"));
        }
    }
    let schema_ok = jsonschema::validator_for(&kind.parameters).map(|v| v.iter_errors(&Value::Object(params.clone())).map(|e| e.to_string()).collect::<Vec<_>>());
    match schema_ok {
        Ok(errors) if errors.is_empty() => {}
        Ok(errors) => return Err(Reply::refused(Reason::Invalid, "WM-NRM-01", format!("parameters: {}", errors.join("; ")))),
        Err(e) => return Err(Reply::refused(Reason::Invalid, "WM-KND-02", format!("kind parameter schema: {e}"))),
    }

    // ---- given
    let mut given_raw = req.given.clone().unwrap_or_default();
    let mut given_from_text = std::collections::BTreeSet::new();
    let mut dropped_inputs: Vec<String> = Vec::new();
    if let Some(p) = &proposal {
        for (k, v) in &p.given {
            // An input the normalizer read from the sentence that the kind does not take is its
            // mistake, not the caller's: drop it and say so, instead of refusing the request.
            if !kind.given.contains_key(k) {
                dropped_inputs.push(k.clone());
                continue;
            }
            if !given_raw.contains_key(k) {
                given_raw.insert(k.clone(), v.clone());
                given_from_text.insert(k.clone());
            }
        }
    }
    for k in &dropped_inputs {
        assumed.insert(format!("given.{k}"), format!("read from the request, but kind {} takes no such input; ignored", kind.id));
    }
    if let Some(k) = given_raw.keys().find(|k| !kind.given.contains_key(*k)) {
        return Err(Reply::refused(Reason::Invalid, "WM-NRM-01", format!("given.{k}: kind {} has no such input", kind.id)));
    }
    for (k, slot) in &kind.given {
        if slot.required && !given_raw.contains_key(k) {
            return Err(gap(format!("given.{k}"), format!("Which {}?", slot.is), vec![], "WM-NRM-07"));
        }
    }
    let mut given = BTreeMap::new();
    for (k, raw) in &given_raw {
        let res = ctx.resolver.resolve(raw).map_err(|g| gap(format!("given.{k}"), g.question, g.options, "WM-NRM-07"))?;
        if let Some(note) = &res.note {
            assumed.insert(format!("given.{k}"), note.clone());
        }
        if res.note.is_some() && (!res.only_pinned_version || given_from_text.contains(k)) {
            guesses.push(Question {
                field: format!("given.{k}"),
                question: format!("Confirm {}: {}", k, res.note.clone().unwrap_or_default()),
                options: vec![res.confirm.clone()],
            });
        }
        given.insert(k.clone(), res.pinned);
    }

    // ---- decide
    let mut decide: BTreeMap<String, String> = kind.decide.iter().map(|(k, d)| (k.clone(), d.default.clone())).collect();
    for (k, v) in req.decide.iter().flatten() {
        match kind.decide.get(k) {
            Some(d) if d.options.contains(v) => {
                decide.insert(k.clone(), v.clone());
            }
            Some(d) => return Err(Reply::refused(Reason::Invalid, "WM-NRM-12", format!("decide.{k}: '{v}' is not one of {}", d.options.join(", ")))),
            None => return Err(Reply::refused(Reason::Invalid, "WM-NRM-12", format!("decide.{k}: kind {} has no such decision", kind.id))),
        }
    }

    // ---- done_when, checks, requires: the kind's plus the caller's (WM-NAR-01)
    let mut done_when = kind.done_when.clone();
    let mut checks = kind.checks.clone();
    let caller_dw = req.done_when.clone().unwrap_or_default();
    let caller_ck = req.checks.clone().unwrap_or_default();
    for (k, text) in &caller_dw {
        if kind.done_when.contains_key(k) {
            return Err(Reply::refused(Reason::Invalid, "WM-NAR-01", format!("done_when.{k} is the kind's; add a check under a new id")));
        }
        let Some(c) = caller_ck.get(k) else {
            return Err(gap(format!("checks.{k}"), format!("How is \"{text}\" checked? Send checks.{k} as a CEL expression."), vec![], "WM-FRZ-06"));
        };
        done_when.insert(k.clone(), text.clone());
        checks.insert(k.clone(), c.clone());
    }
    if let Some(k) = caller_ck.keys().find(|k| !caller_dw.contains_key(*k)) {
        return Err(Reply::refused(Reason::Invalid, "WM-FRZ-06", format!("checks.{k} has no done_when.{k}")));
    }
    let mut requires = kind.requires.clone();
    for (k, c) in req.requires.iter().flatten() {
        if requires.insert(k.clone(), c.clone()).is_some() {
            return Err(Reply::refused(Reason::Invalid, "WM-NAR-01", format!("requires.{k} is the kind's")));
        }
    }

    // ---- limits (WM-NAR-02, WM-NAR-03)
    let pol = ctx.policy.for_principal(principal);
    let kl = &kind.limits;
    let rl = req.limits.clone().unwrap_or_default();
    let effects = kl.effects.unwrap();
    if let Some(e) = rl.effects {
        if e != effects {
            return Err(Reply::refused(Reason::Invalid, "WM-NRM-04", format!("kind {} works at effects {}, not {}", kind.id, effects.as_str(), e.as_str())));
        }
    }
    let mut over_policy = Vec::new();
    let usd = match rl.usd {
        Some(u) => {
            if u > pol.max_usd {
                over_policy.push("usd");
            }
            u
        }
        None => {
            let d = kl.usd.unwrap();
            if d > pol.max_usd {
                assumed.insert("limits.usd".into(), format!("kind default {d} lowered to {} by the policy for {principal}", pol.max_usd));
                pol.max_usd
            } else {
                d
            }
        }
    };
    let time_secs = match &rl.time {
        Some(t) => {
            let s = duration_secs(t).unwrap_or(0);
            if s > pol.max_time_secs() {
                over_policy.push("time");
            }
            s
        }
        None => {
            let d = duration_secs(kl.time.as_deref().unwrap()).unwrap_or(0);
            if d > pol.max_time_secs() {
                assumed.insert("limits.time".into(), format!("kind default {} lowered to {} by the policy for {principal}", format_duration(d), pol.max_time));
                pol.max_time_secs()
            } else {
                d
            }
        }
    };
    let limits = Limits { effects: Some(effects), usd: Some(usd), time: Some(format_duration(time_secs)), tokens: rl.tokens.or(kl.tokens) };

    // ---- the manifest
    let mut m = Manifest {
        manifest: Some(Value::String(super::manifest::VERSION.into())),
        by: Some(principal.to_string()),
        for_: req.for_.clone(),
        said,
        want: Some(entry.render_want(&params)),
        why: req.why.clone(),
        given: Some(given),
        done_when: Some(done_when),
        limits: Some(limits),
        assumed: if assumed.is_empty() { None } else { Some(assumed.clone()) },
        kind: Some(entry.pinned()),
        parameters: Some(params),
        decide: Some(decide),
        deliver: Some(Deliver { schema: entry.deliver_ref.clone(), order: kind.deliver.order.clone(), equivalence: kind.deliver.equivalence.clone() }),
        requires: Some(requires),
        checks: Some(checks),
        partial_ok: Some(req.partial_ok.unwrap_or(kind.partial_ok)),
        delegation: Some(req.delegation.clone().or_else(|| kind.delegation.clone()).unwrap_or(Delegation { depth: Some(0) })),
        parent: req.parent.clone(),
        if_unclear: req.if_unclear,
        approved: req.approved,
        dry_run: req.dry_run,
        reply: req.reply,
        id: req.id.clone(),
        digest: None,
        extra: req.extra.clone(),
    };
    if m.delegation.as_ref().unwrap().depth.is_none() {
        m.delegation = Some(Delegation { depth: Some(0) });
    }
    m.freeze();

    if !over_policy.is_empty() {
        let mut counter = m.clone();
        let cl = counter.limits.as_mut().unwrap();
        cl.usd = Some(usd.min(pol.max_usd));
        cl.time = Some(format_duration(time_secs.min(pol.max_time_secs())));
        counter.freeze();
        let mut r = Reply::refused(Reason::OutOfPolicy, "WM-NAR-03", format!("limits.{} above the policy for {principal}", over_policy.join(", limits.")));
        r.counter = Some(counter);
        r.assumed = m.assumed.clone();
        return Err(r);
    }

    let if_unclear = req.if_unclear.unwrap_or(if effects == Effects::ReadOnly { IfUnclear::Assume } else { IfUnclear::Refuse });
    if if_unclear == IfUnclear::Refuse && !guesses.is_empty() {
        let mut r = Reply::refused(Reason::Unclear, "WM-NRM-08", "the request needed interpretation, and if_unclear is refuse");
        r.questions = guesses;
        r.assumed = m.assumed.clone();
        r.digest = m.digest.clone();
        r.manifest = Some(m);
        return Err(r);
    }
    Ok(m)
}

/// Approval, policy and budget; dry run (WM-HND-*).
pub fn admit(ctx: &Ctx, m: Manifest, principal: &str) -> Result<Manifest, Reply> {
    let pol = ctx.policy.for_principal(principal);
    let effects = m.effects();
    let with = |mut r: Reply, m: &Manifest| {
        r.assumed = m.assumed.clone();
        r.digest = m.digest.clone();
        r
    };
    if effects > pol.max_effects {
        let r = Reply::refused(Reason::OutOfPolicy, "WM-HND-08", format!("{principal} may not run {} work", effects.as_str()));
        return Err(with(r, &m));
    }
    let approved = m.approved.unwrap_or(Effects::ReadOnly);
    if effects > approved {
        let mut r = with(Reply::refused(Reason::NeedsApproval, "WM-HND-03", format!("this work is {}; send it back with approved: {}", effects.as_str(), effects.as_str())), &m);
        r.manifest = Some(m);
        return Err(r);
    }
    if let Some(est) = ctx.estimator.estimate(&m.work_fields()) {
        let l = m.limits.as_ref().unwrap();
        let over_usd = est.usd.zip(l.usd).is_some_and(|(e, u)| e > u);
        let over_time = est.time.as_deref().and_then(duration_secs).zip(l.time.as_deref().and_then(duration_secs)).is_some_and(|(e, t)| e > t);
        if over_usd || over_time {
            let mut r = with(Reply::refused(Reason::OverBudget, "WM-HND-07", "the estimate exceeds the limits"), &m);
            r.estimate = Some(est);
            return Err(r);
        }
    }
    if m.dry_run == Some(true) {
        let mut r = with(Reply::new(Status::Ready), &m);
        r.normalized_by = Some(ctx.normalizer.clone());
        r.manifest = Some(m);
        return Err(r);
    }
    Ok(m)
}
