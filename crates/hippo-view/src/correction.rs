//! What Replay shares with a correction's step rule.
//!
//! The rule that picks a correction's step now lives in the engine
//! (`hippo_core::incidents::subject`), which stores what a correction is
//! about in the rollup.
//! Replay still reads a person's request and a command's first words the
//! same way, and counts working time with the same gaps.

pub use hippo_core::incidents::subject::{command_head, is_request, IDLE_GAP, RUN_CAP};
