//! The reply document (docs/spec/work-manifest/reply.schema.json) and the status rules.

use super::manifest::{Limits, Manifest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Ready,
    Refused,
    Accepted,
    Completed,
    Partial,
    NotFound,
    Indeterminate,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Unclear,
    NeedsApproval,
    OutOfPolicy,
    OverBudget,
    Unsupported,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    ChecksFailed,
    Timeout,
    BudgetExhausted,
    InputUnavailable,
    EffectDenied,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Fail,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub field: String,
    pub question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub code: FailureCode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub searched: u64,
    pub total: u64,
    pub missed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_calls: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Normalizer {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub reply: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_by: Option<Normalizer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assumed: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub violates: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<Question>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counter: Option<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate: Option<Limits>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<BTreeMap<String, CheckStatus>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<Failure>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proposals: Vec<Manifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<String>,
}

impl Reply {
    pub fn new(status: Status) -> Reply {
        Reply {
            reply: "0.1".into(),
            request: None,
            status,
            digest: None,
            normalized_by: None,
            assumed: None,
            manifest: None,
            reason: None,
            violates: vec![],
            detail: None,
            questions: vec![],
            counter: None,
            estimate: None,
            receipt: None,
            checks: None,
            answer: None,
            coverage: None,
            missing: vec![],
            failure: None,
            proposals: vec![],
            usage: None,
            trace: None,
        }
    }

    pub fn refused(reason: Reason, rule: &str, detail: impl Into<String>) -> Reply {
        let mut r = Reply::new(Status::Refused);
        r.reason = Some(reason);
        r.violates = vec![rule.to_string()];
        r.detail = Some(detail.into());
        r
    }

    pub fn accepted(digest: &str, receipt: &str) -> Reply {
        let mut r = Reply::new(Status::Accepted);
        r.digest = Some(digest.into());
        r.receipt = Some(receipt.into());
        r
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// What came back from the work, before the orchestrator judges it.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// A precondition id that was false (`requires`), in which case no work ran.
    pub missing: Vec<String>,
    /// The work could not finish.
    pub failure: Option<Failure>,
    pub answer: Option<Value>,
    pub checks: BTreeMap<String, CheckStatus>,
    pub coverage: Option<Coverage>,
    pub proposals: Vec<Manifest>,
    pub usage: Usage,
    /// Why the checks failed, when it can be said briefly (for example the answer's shape).
    pub detail: Option<String>,
}

/// WM-RES-02, in order.
pub fn judge(manifest: &Manifest, o: Outcome) -> Reply {
    let partial_ok = manifest.partial_ok.unwrap_or(false);
    let mut r;
    if !o.missing.is_empty() {
        r = Reply::new(Status::NotFound);
        r.missing = o.missing;
    } else if let Some(f) = o.failure {
        r = Reply::new(Status::Failed);
        r.failure = Some(f);
        if !o.checks.is_empty() {
            r.checks = Some(o.checks);
        }
    } else {
        let all_pass = !o.checks.is_empty() && o.checks.values().all(|c| *c == CheckStatus::Pass);
        let deliver_failed = o.checks.get("deliver") != Some(&CheckStatus::Pass);
        let any_fail = o.checks.values().any(|c| *c == CheckStatus::Fail);
        let status = if all_pass {
            Status::Completed
        } else if deliver_failed {
            Status::Failed
        } else if partial_ok {
            Status::Partial
        } else if any_fail {
            Status::Failed
        } else {
            Status::Indeterminate
        };
        r = Reply::new(status);
        if status == Status::Failed {
            r.failure = Some(Failure { code: FailureCode::ChecksFailed, detail: o.detail });
        }
        if matches!(status, Status::Completed | Status::Partial) {
            r.answer = o.answer;
        }
        r.checks = Some(o.checks);
        r.coverage = o.coverage;
        r.proposals = o.proposals;
    }
    r.digest = manifest.digest.clone();
    r.request = manifest.id.clone();
    r.usage = Some(o.usage);
    r
}
