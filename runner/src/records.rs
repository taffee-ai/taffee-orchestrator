//! Turn records: what happened in a turn, without its content.
//!
//! A record names the agent, the spec digest, the requester, timing, the outcome, token usage
//! and every outbound call (destination, status, credential name). It never holds the prompt,
//! the answer, tool inputs or headers. Records are kept in memory per agent and, when
//! configured, appended as JSON lines to a file.

use crate::egress::CallRecord;
use serde::Serialize;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct TurnRecord {
    pub agent_id: String,
    pub turn: u64,
    pub spec_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requester: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intent_id: Option<String>,
    pub started_at_ms: u64,
    pub duration_ms: f64,
    /// ok | turn_failed | spec_conflict | spec_invalid | timeout | trap | bad_output
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    pub calls: Vec<CallRecord>,
}

pub struct RecordSink {
    file: Option<Mutex<std::fs::File>>,
}

impl RecordSink {
    pub fn open(path: Option<&PathBuf>) -> anyhow::Result<Self> {
        let file = match path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                Some(Mutex::new(std::fs::OpenOptions::new().create(true).append(true).open(p)?))
            }
            None => None,
        };
        Ok(Self { file })
    }

    pub fn write(&self, r: &TurnRecord) {
        let Some(f) = &self.file else { return };
        if let Ok(mut line) = serde_json::to_vec(r) {
            line.push(b'\n');
            if let Err(e) = f.lock().unwrap().write_all(&line) {
                tracing::warn!("writing turn record failed: {e}");
            }
        }
    }
}

pub fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}
