//! App state sub-structs.
//!
//! As part of the audit02 P1-V decomposition of the `App` god-object, related
//! fields are grouped into focused sub-structs. The `App` struct still owns
//! these as fields; methods on `App` reach through `self.<substruct>.<field>`.
//!
//! - [`RuntimeState`] — backend/concurrency state (worker channels, PTY map,
//!   atomics, lockfile, gh/PR tracking, refs watchers).
//! - [`UiState`] — visual/interaction state (pane focus, scroll offsets, modal
//!   stack, mouse layout, welcome state, force-redraw flag).
//! - [`GitState`] — projects + sessions + change-file caches + commit-message
//!   editor + in-flight markers for git-driven background workers.

mod git;
mod runtime;
mod ui;

pub(crate) use git::GitState;
pub(crate) use runtime::RuntimeState;
pub(crate) use ui::UiState;
