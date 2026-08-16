//! Shared authentication throttle constants.
//!
//! Single home for the attempt-cap / backoff numbers previously duplicated
//! in `login` and `login-manager` and pinned in lockstep by
//! `tests/test_login_flow.py` — a cap change now happens in exactly one
//! place, mirroring the MAX_RESPAWNS single-home precedent.

/// Failed login attempts tolerated before the auth path pauses. Both the
/// console getty and the GUI login-manager re-prompt afterwards (never
/// exit), so init's MAX_RESPAWNS accounting is untouched — the cap only
/// throttles the PBKDF2 verify (10k iterations per attempt) so a
/// brute-forcer or a stuck terminal cannot hammer it at full speed.
pub const MAX_FAILED_ATTEMPTS: u32 = 10;

/// Backoff pause in nanoseconds after MAX_FAILED_ATTEMPTS (30 s).
pub const BACKOFF_NS: u64 = 30_000_000_000;
