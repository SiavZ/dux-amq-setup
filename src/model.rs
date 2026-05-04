use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::pty::PtyHandle;

/// GitHub CLI availability status, checked once at startup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GhStatus {
    /// Not yet checked.
    #[default]
    Unknown,
    /// `gh` binary not found on PATH.
    NotInstalled,
    /// `gh` found but `gh auth status` failed.
    NotAuthenticated,
    /// `gh` installed and authenticated.
    Available,
}

/// State of a GitHub pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrState {
    Open,
    Merged,
    Closed,
}

/// Cached information about a GitHub pull request associated with a session.
#[derive(Clone, Debug)]
pub struct PrInfo {
    pub number: u64,
    pub state: PrState,
    pub title: String,
    pub owner_repo: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderKind(String);

impl ProviderKind {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[allow(clippy::should_implement_trait)] // existing API; FromStr trait migration tracked separately
    pub fn from_str(value: &str) -> Self {
        Self(value.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default_provider: ProviderKind,
    pub current_branch: String,
    pub path_missing: bool,
    /// `false` while metadata (is_git_repo, current_branch, remote default)
    /// is still being resolved on a worker thread. Render code must show a
    /// "(loading…)" placeholder for any field whose value depends on git
    /// until this flips to `true`. See
    /// [`crate::app::workers::dispatch_project_meta`].
    pub meta_loaded: bool,
}

impl Project {
    /// Construct a half-populated `Project` whose git metadata
    /// (`current_branch`, `path_missing`) is filled in later via a
    /// `WorkerEvent::ProjectMetaReady`. Render code must check
    /// [`Project::meta_loaded`] before displaying git-derived fields.
    pub fn placeholder(
        id: String,
        name: String,
        path: String,
        default_provider: ProviderKind,
    ) -> Self {
        Self {
            id,
            name,
            path,
            default_provider,
            current_branch: String::new(),
            path_missing: false,
            meta_loaded: false,
        }
    }
}

/// Explicit per-session lifecycle state for audit02 P1-Z (Phase 18).
///
/// Phase 2 (this revision) embeds the [`PtyHandle`] directly inside the
/// `Live` and `Detached` variants. Ownership of the PTY now flows
/// through the state machine rather than a side-channel `HashMap`. As a
/// consequence:
///
/// - `SessionState` is **not** `Clone`, `PartialEq`, `Serialize`, or
///   `Deserialize` — it owns process resources. Persistence goes through
///   [`PersistedSessionState`] (which mirrors only the persistable
///   variants).
/// - Dropping a `Live` or `Detached` variant runs `PtyHandle`'s `Drop`,
///   which kills the child and joins the reader thread (audit02 P1-Q).
///
/// The variants intentionally mirror Phase 18's plan:
///
/// - `Created` — row exists, no spawn attempt yet.
/// - `Spawning` — spawn job dispatched to a worker.
/// - `Live` — PTY accepting input; the user is interacting.
/// - `Detached` — PTY still alive but no UI pane attached.
/// - `Exited` — child terminated; no PTY.
///
/// Persistence note: `Live` is **never** persisted as `Live` — when a
/// session is reloaded from disk on the next dux start there cannot,
/// by definition, be a running PTY for it yet. The `From<SessionState>
/// for PersistedSessionState` impl folds `Live` into `Detached` and
/// `Detached` into the same on-disk shape (PTY handle is dropped).
#[derive(Debug)]
pub enum SessionState {
    Created {
        created_at: DateTime<Utc>,
    },
    Spawning {
        since: DateTime<Utc>,
    },
    Live {
        pty_handle: PtyHandle,
        spawned_at: DateTime<Utc>,
        last_active_at: DateTime<Utc>,
    },
    Detached {
        pty_handle: PtyHandle,
        detached_at: DateTime<Utc>,
    },
    Exited {
        exit_code: Option<i32>,
        exited_at: DateTime<Utc>,
    },
}

// Some helpers (`is_exited`, `transition`, `can_transition_to`) are
// part of the stable typestate API but only exercised by the
// integration tests in `tests/session_state.rs` (separate crate),
// so the binary build sees them as "unused". Allow dead code for the
// whole impl — the tests are the contract these helpers serve.
#[allow(dead_code)]
impl SessionState {
    /// Short tag used in error messages. The string values match the
    /// targets accepted by [`SessionState::transition`] so that
    /// `state.transition(other.name())` is meaningful when both states
    /// are known.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Created { .. } => "created",
            Self::Spawning { .. } => "spawning",
            Self::Live { .. } => "live",
            Self::Detached { .. } => "detached",
            Self::Exited { .. } => "exited",
        }
    }

    /// Returns the embedded [`PtyHandle`], if any. `Live` and `Detached`
    /// own a handle; the other three variants do not.
    pub fn pty_handle(&self) -> Option<&PtyHandle> {
        match self {
            Self::Live { pty_handle, .. } | Self::Detached { pty_handle, .. } => Some(pty_handle),
            _ => None,
        }
    }

    /// Mutable variant of [`SessionState::pty_handle`].
    pub fn pty_handle_mut(&mut self) -> Option<&mut PtyHandle> {
        match self {
            Self::Live { pty_handle, .. } | Self::Detached { pty_handle, .. } => Some(pty_handle),
            _ => None,
        }
    }

    /// True if this state currently owns a PTY (`Live` or `Detached`).
    /// Mirrors the legacy `runtime.providers.contains_key(id)` check.
    pub fn has_pty(&self) -> bool {
        matches!(self, Self::Live { .. } | Self::Detached { .. })
    }

    /// True only when the state is [`SessionState::Live`].
    pub fn is_live(&self) -> bool {
        matches!(self, Self::Live { .. })
    }

    /// True when the child process has exited.
    pub fn is_exited(&self) -> bool {
        matches!(self, Self::Exited { .. })
    }

    /// Returns `true` if `target` is a legal next state from `self`.
    ///
    /// The legal transitions are deliberately narrow:
    ///
    /// - `Created -> Spawning`
    /// - `Spawning -> Live | Exited` (success or spawn failure)
    /// - `Live -> Detached | Exited`
    /// - `Detached -> Live | Exited` (reattach or child exit while detached)
    /// - `Exited -> Spawning` (re-spawn after exit)
    ///
    /// Anything else — including `Self -> Self` — is rejected.
    pub fn can_transition_to(&self, target: &str) -> bool {
        matches!(
            (self, target),
            (Self::Created { .. }, "spawning")
                | (Self::Spawning { .. }, "live")
                | (Self::Spawning { .. }, "exited")
                | (Self::Live { .. }, "detached")
                | (Self::Live { .. }, "exited")
                | (Self::Detached { .. }, "live")
                | (Self::Detached { .. }, "exited")
                | (Self::Exited { .. }, "spawning")
        )
    }

    /// Apply a transition that does **not** create or destroy a
    /// [`PtyHandle`]: `Created -> Spawning`, `Live -> Exited`,
    /// `Detached -> Exited`, `Spawning -> Exited`, `Exited -> Spawning`.
    /// Use the dedicated typed helpers ([`SessionState::on_spawn_succeeded`],
    /// [`SessionState::detach`], [`SessionState::reattach`]) for the
    /// transitions that move a PTY in or out.
    ///
    /// `now` is the wall-clock timestamp to stamp on the resulting
    /// state.
    pub fn transition(self, target: &str, now: DateTime<Utc>) -> Result<SessionState> {
        if !self.can_transition_to(target) {
            return Err(anyhow!(
                "illegal session-state transition: {} -> {}",
                self.name(),
                target
            ));
        }
        let next = match (self, target) {
            (Self::Created { .. } | Self::Exited { .. }, "spawning") => {
                Self::Spawning { since: now }
            }
            (_, "exited") => Self::Exited {
                exit_code: None,
                exited_at: now,
            },
            (state, target) => {
                return Err(anyhow!(
                    "transition {} -> {} requires a typed helper (on_spawn_succeeded, detach, reattach)",
                    state.name(),
                    target
                ));
            }
        };
        Ok(next)
    }

    /// `Spawning -> Live`: install a freshly-spawned [`PtyHandle`].
    pub fn on_spawn_succeeded(self, pty: PtyHandle, now: DateTime<Utc>) -> Result<SessionState> {
        match self {
            Self::Spawning { .. } => Ok(Self::Live {
                pty_handle: pty,
                spawned_at: now,
                last_active_at: now,
            }),
            other => Err(anyhow!(
                "on_spawn_succeeded requires Spawning, was {}",
                other.name()
            )),
        }
    }

    /// `Live -> Detached`: keep the PTY alive but mark the pane gone.
    pub fn detach(self, now: DateTime<Utc>) -> Result<SessionState> {
        match self {
            Self::Live { pty_handle, .. } => Ok(Self::Detached {
                pty_handle,
                detached_at: now,
            }),
            other => Err(anyhow!("detach requires Live, was {}", other.name())),
        }
    }

    /// `Detached -> Live`: a UI pane has reattached to the surviving PTY.
    pub fn reattach(self, now: DateTime<Utc>) -> Result<SessionState> {
        match self {
            Self::Detached { pty_handle, .. } => Ok(Self::Live {
                pty_handle,
                spawned_at: now,
                last_active_at: now,
            }),
            other => Err(anyhow!("reattach requires Detached, was {}", other.name())),
        }
    }

    /// Force a session into [`SessionState::Exited`], dropping any
    /// owned [`PtyHandle`] (which kills the child + joins the reader
    /// thread via `Drop`). Always legal; used by tear-down paths
    /// where we cannot easily plumb the typed transition.
    pub fn into_exited(self, exit_code: Option<i32>, now: DateTime<Utc>) -> SessionState {
        // Dropping `self` here also drops any embedded PtyHandle,
        // killing the child and joining the reader thread.
        let _ = self;
        Self::Exited {
            exit_code,
            exited_at: now,
        }
    }

    /// Map the legacy three-state `status` text column onto an initial
    /// [`SessionState`]. Both `"active"` and `"detached"` legacy rows
    /// reload as `SessionState::Created` because the embedded
    /// [`PtyHandle`] in `Detached` cannot survive a process restart;
    /// auto-resume (Phase 15) re-spawns the session from `Created` on
    /// the next tick. `"exited"` rows preserve their terminal state.
    pub fn from_legacy_status_str(status: &str, now: DateTime<Utc>) -> Self {
        match status {
            "exited" => Self::Exited {
                exit_code: None,
                exited_at: now,
            },
            // "active" | "detached" | unknown — all rehydrate as Created
            _ => Self::Created { created_at: now },
        }
    }

    /// Convenience for the storage layer: serialize to JSON for the
    /// `state_json` column. Folds `Live` and `Detached` into the
    /// persistable shape because a running PTY cannot be represented
    /// across process restarts.
    pub fn to_json(&self) -> Result<String> {
        let persisted: PersistedSessionState = self.into();
        serde_json::to_string(&persisted)
            .map_err(|e| anyhow!("failed to serialize SessionState: {e}"))
    }

    /// Inverse of [`SessionState::to_json`].
    pub fn from_json(json: &str) -> Result<Self> {
        let persisted: PersistedSessionState = serde_json::from_str(json)
            .map_err(|e| anyhow!("failed to parse SessionState JSON: {e}"))?;
        Ok(persisted.into())
    }
}

/// Wire format used by the `agent_sessions.state_json` column. The
/// `Live` variant is intentionally absent — a "live" session by
/// definition has a running PTY in this process, and that handle
/// cannot survive a restart. Persisting `Live` would lie about the
/// invariant, so we collapse it to `Detached` on the way out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedSessionState {
    Created {
        created_at: DateTime<Utc>,
    },
    Spawning {
        since: DateTime<Utc>,
    },
    Detached {
        detached_at: DateTime<Utc>,
    },
    Exited {
        exit_code: Option<i32>,
        exited_at: DateTime<Utc>,
    },
}

impl From<&SessionState> for PersistedSessionState {
    fn from(state: &SessionState) -> Self {
        match state {
            SessionState::Created { created_at } => Self::Created {
                created_at: *created_at,
            },
            SessionState::Spawning { since } => Self::Spawning { since: *since },
            // Live folds into Detached on persist — see enum doc.
            SessionState::Live { last_active_at, .. } => Self::Detached {
                detached_at: *last_active_at,
            },
            SessionState::Detached { detached_at, .. } => Self::Detached {
                detached_at: *detached_at,
            },
            SessionState::Exited {
                exit_code,
                exited_at,
            } => Self::Exited {
                exit_code: *exit_code,
                exited_at: *exited_at,
            },
        }
    }
}

impl From<PersistedSessionState> for SessionState {
    fn from(persisted: PersistedSessionState) -> Self {
        match persisted {
            PersistedSessionState::Created { created_at } => Self::Created { created_at },
            PersistedSessionState::Spawning { since } => Self::Spawning { since },
            // A persisted `Detached` row had a PTY at write time but
            // cannot have one after a restart — collapse to `Created`
            // so the typestate invariant "Detached has a PtyHandle"
            // stays watertight. Auto-resume (Phase 15) will pick the
            // session up from `Created` on the next tick. We thread
            // the original `detached_at` through as `created_at` so the
            // session retains a reasonable timestamp.
            PersistedSessionState::Detached { detached_at } => Self::Created {
                created_at: detached_at,
            },
            PersistedSessionState::Exited {
                exit_code,
                exited_at,
            } => Self::Exited {
                exit_code,
                exited_at,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompanionTerminalStatus {
    NotLaunched,
    Running,
    Exited,
}

impl CompanionTerminalStatus {
    pub fn is_running(self) -> bool {
        matches!(self, Self::Running)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionSurface {
    Agent,
    Terminal,
}

/// In-memory representation of a session. After audit02 P1-Z phase 2
/// this struct owns its [`SessionState`] (which in turn may own a
/// [`PtyHandle`]).
///
/// `AgentSession` implements [`Clone`] manually via
/// [`AgentSession::metadata_snapshot`]: cloning produces a PTY-less
/// copy where any `Live` / `Detached` state collapses to `Created`.
/// This preserves the existing call-site ergonomics (many code paths
/// clone a session for read-only metadata access — storage upserts,
/// log lines, fixture builders) while making the typestate invariant
/// "PtyHandle has at most one owner" structurally enforced. Callers
/// that need the PTY must borrow the canonical session out of
/// `App::sessions` rather than holding a clone.
#[derive(Debug)]
pub struct AgentSession {
    pub id: String,
    pub project_id: String,
    pub project_path: Option<String>,
    pub provider: ProviderKind,
    pub source_branch: String,
    pub branch_name: String,
    pub worktree_path: String,
    pub title: Option<String>,
    pub started_providers: Vec<String>,
    /// Authoritative session lifecycle state. Owns the PTY when in
    /// `Live` or `Detached`. Direct mutation is allowed inside the
    /// `dux` crate but should go through `App::transition_*` helpers
    /// where possible so we get one chokepoint for state changes.
    pub state: SessionState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AgentSession {
    pub fn has_started_provider(&self, provider: &ProviderKind) -> bool {
        self.started_providers
            .iter()
            .any(|started| started == provider.as_str())
    }

    pub fn mark_provider_started(&mut self, provider: &ProviderKind) -> bool {
        if self.has_started_provider(provider) {
            return false;
        }
        self.started_providers.push(provider.as_str().to_string());
        true
    }

    /// Produce a PTY-less clone of this session's metadata. Used by
    /// the [`Clone`] impl, and directly in places where the intent
    /// "give me a metadata-only copy" should be obvious to the reader.
    /// The resulting session's `state` mirrors the persisted shape:
    /// `Created`, `Spawning`, or `Exited` carry over verbatim; `Live`
    /// and `Detached` collapse to `Created` (their PTY cannot be
    /// duplicated).
    pub fn metadata_snapshot(&self) -> AgentSession {
        let state = match &self.state {
            SessionState::Created { created_at } => SessionState::Created {
                created_at: *created_at,
            },
            SessionState::Spawning { since } => SessionState::Spawning { since: *since },
            SessionState::Live { last_active_at, .. } => SessionState::Created {
                created_at: *last_active_at,
            },
            SessionState::Detached { detached_at, .. } => SessionState::Created {
                created_at: *detached_at,
            },
            SessionState::Exited {
                exit_code,
                exited_at,
            } => SessionState::Exited {
                exit_code: *exit_code,
                exited_at: *exited_at,
            },
        };
        AgentSession {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            project_path: self.project_path.clone(),
            provider: self.provider.clone(),
            source_branch: self.source_branch.clone(),
            branch_name: self.branch_name.clone(),
            worktree_path: self.worktree_path.clone(),
            title: self.title.clone(),
            started_providers: self.started_providers.clone(),
            state,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

impl Clone for AgentSession {
    /// Cloning an `AgentSession` produces a PTY-less metadata copy via
    /// [`AgentSession::metadata_snapshot`]. The original retains
    /// ownership of any embedded [`PtyHandle`]; the clone's `state`
    /// folds `Live` / `Detached` to `Created`. This is the only
    /// definition of "clone" that is sound for a session that may own
    /// process resources.
    fn clone(&self) -> Self {
        self.metadata_snapshot()
    }
}

#[derive(Clone, Debug)]
pub struct ChangedFile {
    pub status: String,
    pub path: String,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
}
