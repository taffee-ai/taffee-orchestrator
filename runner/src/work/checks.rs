//! Checks, evaluated by the orchestrator and never by the worker (WM-CHK-02, WM-CHK-03).
//!
//! A CEL check sees `answer`, `coverage`, `given`, `parameters` and `usage`, plus the kind
//! functions below, which read the pinned inputs through the input store. `true` passes,
//! `false` fails, and anything else (an error, an unavailable function, a non-boolean) is
//! indeterminate. The implicit `deliver` check validates the answer against the deliver schema
//! (WM-CHK-05).

use super::inputs::{mentions, InputStore};
use super::manifest::Manifest;
use super::reply::CheckStatus;
use cel::{Context, Env, Value as Cel};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub const FUNCTIONS: &[&str] = &["declared", "line_exists", "line_mentions"];

fn as_str(v: &Cel) -> Option<String> {
    match v {
        Cel::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn as_usize(v: &Cel) -> Option<usize> {
    match v {
        Cel::Int(i) if *i >= 0 => Some(*i as usize),
        Cel::UInt(u) => Some(*u as usize),
        Cel::Float(f) if *f >= 0.0 && f.fract() == 0.0 => Some(*f as usize),
        _ => None,
    }
}

pub struct Evaluator {
    env: Arc<Env>,
    store: Arc<dyn InputStore>,
}

impl Evaluator {
    pub fn new(store: Arc<dyn InputStore>) -> Evaluator {
        Evaluator { env: Arc::new(Env::stdlib()), store }
    }

    fn context(&self, vars: &Value, unavailable: &[String]) -> Context<'_, '_> {
        let mut ctx = Context::with_env(self.env.clone());
        if let Some(obj) = vars.as_object() {
            for (k, v) in obj {
                let _ = ctx.add_variable(k.as_str(), v.clone());
            }
        }
        let on = |name: &str| !unavailable.iter().any(|u| u == name);
        if on("declared") {
            let s = self.store.clone();
            let _ = ctx.add_function("declared", move |repo: Cel, symbol: Cel| -> Result<bool, cel::ExecutionError> {
                let (r, sym) = (as_str(&repo), as_str(&symbol));
                match (r, sym) {
                    (Some(r), Some(sym)) => s.declared(&r, &sym).ok_or_else(|| cel::ExecutionError::function_error("declared", "input not available")),
                    _ => Err(cel::ExecutionError::function_error("declared", "expects (string, string)")),
                }
            });
        }
        if on("line_exists") {
            let s = self.store.clone();
            let _ = ctx.add_function("line_exists", move |repo: Cel, file: Cel, line: Cel| -> Result<bool, cel::ExecutionError> {
                match (as_str(&repo), as_str(&file), as_usize(&line)) {
                    (Some(r), Some(f), Some(n)) => {
                        if s.files(&r).is_none() {
                            return Err(cel::ExecutionError::function_error("line_exists", "input not available"));
                        }
                        Ok(n >= 1 && s.line_count(&r, &f).is_some_and(|c| n <= c))
                    }
                    _ => Err(cel::ExecutionError::function_error("line_exists", "expects (string, string, int)")),
                }
            });
        }
        if on("line_mentions") {
            let s = self.store.clone();
            let _ = ctx.add_function("line_mentions", move |repo: Cel, file: Cel, line: Cel, symbol: Cel| -> Result<bool, cel::ExecutionError> {
                match (as_str(&repo), as_str(&file), as_usize(&line), as_str(&symbol)) {
                    (Some(r), Some(f), Some(n), Some(sym)) => {
                        if s.files(&r).is_none() {
                            return Err(cel::ExecutionError::function_error("line_mentions", "input not available"));
                        }
                        Ok(s.line(&r, &f, n).is_some_and(|t| mentions(&t, &sym)))
                    }
                    _ => Err(cel::ExecutionError::function_error("line_mentions", "expects (string, string, int, string)")),
                }
            });
        }
        ctx
    }

    pub fn eval(&self, expr: &str, vars: &Value, unavailable: &[String]) -> CheckStatus {
        let Ok(program) = self.env.compile(expr) else { return CheckStatus::Indeterminate };
        let ctx = self.context(vars, unavailable);
        match program.execute(&ctx) {
            Ok(Cel::Bool(true)) => CheckStatus::Pass,
            Ok(Cel::Bool(false)) => CheckStatus::Fail,
            _ => CheckStatus::Indeterminate,
        }
    }

    /// Evaluate a map of checks. Verifier checks are indeterminate: no verifier runs here yet.
    pub fn eval_all(&self, checks: &BTreeMap<String, Value>, vars: &Value, unavailable: &[String]) -> BTreeMap<String, CheckStatus> {
        checks
            .iter()
            .map(|(id, c)| {
                let status = match c.as_str() {
                    Some(expr) => self.eval(expr, vars, unavailable),
                    None => CheckStatus::Indeterminate,
                };
                (id.clone(), status)
            })
            .collect()
    }
}

/// The variables a check sees.
pub fn variables(m: &Manifest, answer: Option<&Value>, coverage: Option<&Value>, usage: &Value) -> Value {
    json!({
        "answer": answer.cloned().unwrap_or(Value::Null),
        "coverage": coverage.cloned().unwrap_or(Value::Null),
        "given": m.given.clone().unwrap_or_default(),
        "parameters": m.parameters.clone().unwrap_or_default(),
        "usage": usage,
    })
}

/// WM-CHK-05.
pub fn deliver_check(schema: &Value, answer: Option<&Value>) -> CheckStatus {
    let Some(a) = answer else { return CheckStatus::Fail };
    match jsonschema::validator_for(schema) {
        Ok(v) if v.is_valid(a) => CheckStatus::Pass,
        Ok(_) => CheckStatus::Fail,
        Err(_) => CheckStatus::Indeterminate,
    }
}

/// Why an answer does not match the `deliver` schema: the first problem found, for the reply's
/// failure detail, so a caller is not left guessing.
pub fn deliver_problem(schema: &Value, answer: Option<&Value>) -> Option<String> {
    let Some(a) = answer else { return Some("no answer was submitted".into()) };
    let v = jsonschema::validator_for(schema).ok()?;
    let first = v.iter_errors(a).next()?;
    let mut text = format!("the answer does not match the agreed shape: {first}");
    text.truncate(300);
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::super::inputs::{Fixture, FixtureFile, Store};
    use super::*;

    fn store() -> Arc<dyn InputStore> {
        let mut files = BTreeMap::new();
        files.insert(
            "a.ts".into(),
            FixtureFile { lines: 3, text: [("2".to_string(), "new Foo()".to_string())].into_iter().collect() },
        );
        let mut s = Store::default();
        s.add_fixture(Fixture { r#ref: "r@x".into(), declarations: [("Foo".into(), "a.ts:1".into())].into_iter().collect(), files });
        Arc::new(s)
    }

    #[test]
    fn cel_with_kind_functions() {
        let e = Evaluator::new(store());
        let vars = json!({ "answer": { "items": [ { "file": "a.ts", "line": 2 } ] }, "given": { "repo": "r@x" }, "parameters": { "symbol": "Foo" } });
        assert_eq!(e.eval("answer.items.all(i, line_exists(given.repo, i.file, i.line))", &vars, &[]), CheckStatus::Pass);
        assert_eq!(e.eval("answer.items.all(i, line_mentions(given.repo, i.file, i.line, parameters.symbol))", &vars, &[]), CheckStatus::Pass);
        assert_eq!(e.eval("declared(given.repo, parameters.symbol)", &vars, &[]), CheckStatus::Pass);
        assert_eq!(e.eval("declared(given.repo, 'Bar')", &vars, &[]), CheckStatus::Fail);
        let bad = json!({ "answer": { "items": [ { "file": "a.ts", "line": 9 } ] }, "given": { "repo": "r@x" } });
        assert_eq!(e.eval("answer.items.all(i, line_exists(given.repo, i.file, i.line))", &bad, &[]), CheckStatus::Fail);
        assert_eq!(e.eval("line_mentions(given.repo, 'a.ts', 2, 'Foo')", &vars, &["line_mentions".into()]), CheckStatus::Indeterminate);
        assert_eq!(e.eval("1 +", &vars, &[]), CheckStatus::Indeterminate);
        assert_eq!(e.eval("'x'", &vars, &[]), CheckStatus::Indeterminate);
    }

    #[test]
    fn deliver_schema() {
        let s = json!({ "type": "object", "required": ["a"] });
        assert_eq!(deliver_check(&s, Some(&json!({ "a": 1 }))), CheckStatus::Pass);
        assert_eq!(deliver_check(&s, Some(&json!({}))), CheckStatus::Fail);
        assert_eq!(deliver_check(&s, None), CheckStatus::Fail);
    }
}
