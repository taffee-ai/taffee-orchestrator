//! The work manifest: what crosses between a caller and this orchestrator
//! (docs/spec/work-manifest/). Everything that decides an outcome lives here, as plain
//! deterministic code. Models are only asked for proposals (`agents`), and every proposal is
//! checked.

pub mod agents;
pub mod assemble;
pub mod checks;
pub mod context;
pub mod delegate;
pub mod engine;
pub mod inputs;
pub mod jcs;
pub mod kind;
pub mod manifest;
pub mod normalize;
pub mod refs;
pub mod reply;
