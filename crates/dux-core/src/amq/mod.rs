//! The dux-amq companion: a file-based inject queue that wakes agents, and
//! the Orchestrator watchdog that periodically nudges orchestrator agents.
//!
//! - [`config`]: the `[amq]` config section.
//! - [`queue`]: filesystem layer (scan, claim, validate, reclaim, expire).
//! - [`activity`]: read-only probes of an agent's AMQ mailbox.
//! - [`delivery`]: pure delivery policy (matching, encoding, hold rules).
//! - [`orchestrator`]: pure watchdog prompts and peer selection.
//!
//! The engine state machine that drives all of it is `crate::engine::amq`.

pub mod activity;
pub mod config;
pub mod delivery;
pub mod orchestrator;
pub mod queue;
