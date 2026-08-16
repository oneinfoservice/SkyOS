//! Session lifecycle — process supervision (reap), lifecycle tracking, and
//! the session-end protocol with `init`.
//!
//! One owner for every launched process: `LifecycleManager` records the
//! table, `reap` classifies exits and notifies every subsystem that tracked
//! the child, and `request_end`/`exit_code` define how a deliberate logout
//! unwinds back to `init`.

use alloc::vec::Vec;
use libsarga::process;

use crate::core::window_manager::WindowManager;
use crate::ipc::transport::IpcTransport;
use crate::sec::perms::PermissionManager;
use crate::service::service_manager::ServiceManager;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AppState {
    Starting,
    Running,
    Terminated,
    Crashed,
}

/// How a process exited, derived from the raw wait4 status.
/// The kernel encodes this per Unix convention: 0 = clean exit,
/// 1..=127 = exit code, 128+sig = killed by fatal signal, negative = killed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExitClass {
    Clean,
    Error(u32),
    Signal(u32),
    Killed,
}

/// Classifies a raw wait4 status into how the process exited.
pub(crate) fn exit_class(status: i32) -> ExitClass {
    if status == 0 {
        ExitClass::Clean
    } else if status < 0 {
        ExitClass::Killed
    } else if status > 128 {
        ExitClass::Signal((status - 128) as u32)
    } else {
        ExitClass::Error(status as u32)
    }
}

pub(crate) struct AppLifecycle {
    pub pid: u64,
    pub state: AppState,
    pub crash_count: u32,
}

pub(crate) struct LifecycleManager {
    pub procs: Vec<AppLifecycle>,
}

impl LifecycleManager {
    pub fn new() -> Self {
        LifecycleManager { procs: Vec::new() }
    }

    pub fn register(&mut self, pid: u64) {
        self.procs.push(AppLifecycle {
            pid,
            state: AppState::Starting,
            crash_count: 0,
        });
    }

    pub fn mark_running(&mut self, pid: u64) {
        for p in &mut self.procs {
            if p.pid == pid && p.state == AppState::Starting {
                p.state = AppState::Running;
                return;
            }
        }
    }

    pub fn mark_terminated(&mut self, pid: u64) {
        for p in &mut self.procs {
            if p.pid == pid {
                p.state = AppState::Terminated;
                return;
            }
        }
    }

    pub fn mark_crashed(&mut self, pid: u64) {
        for p in &mut self.procs {
            if p.pid == pid {
                p.state = AppState::Crashed;
                p.crash_count += 1;
                return;
            }
        }
    }

    pub fn remove(&mut self, pid: u64) {
        self.procs.retain(|p| p.pid != pid);
    }
}

/// Session end codes — the exit-code contract with `init`: 0 = clean logout
/// (init resets its crash counter and respawns the login service); non-zero
/// = crash (init counts it toward `MAX_RESPAWNS`). A future reboot/poweroff
/// request would add its own code here.
const EXIT_LOGOUT: i32 = 0;

pub struct SessionManager {
    boot_tick: u64,
    pub(crate) lifecycle: LifecycleManager,
    ending: bool,
}

impl SessionManager {
    pub fn new(boot_tick: u64) -> Self {
        SessionManager {
            boot_tick,
            lifecycle: LifecycleManager::new(),
            ending: false,
        }
    }

    pub fn uptime(&self, current_tick: u64) -> u64 {
        current_tick.saturating_sub(self.boot_tick)
    }

    /// Begin the session-end sequence (logout). The main loop observes
    /// `is_ending()` and unwinds to a clean `process::exit(exit_code())`.
    ///
    /// Idempotency is structural, not guarded: the body is a single
    /// monotonic store to a private bool (`false -> true`, then
    /// `true -> true` — a no-op). There is no counter, toggle, error path,
    /// or side effect, so re-entering this — e.g. a second Esc press in the
    /// a11y arm while the unwind is already in flight — can neither reset
    /// `ending` nor alter the exit code: `exit_code()` below is a
    /// compile-time constant that ignores `ending` entirely. The unwind is
    /// driven only by the main loop's `while !is_ending()`, and any
    /// `request_end` after the first is observably a no-op.
    pub fn request_end(&mut self) {
        self.ending = true;
    }

    pub fn is_ending(&self) -> bool {
        self.ending
    }

    /// The exit code the session hands to `init` when it ends. Deliberate
    /// logouts are clean (0) so init treats them as graceful; crashes must
    /// exit non-zero (they never route through here).
    ///
    /// A constant by construction: it reads no state and takes no input, so
    /// the number of times the a11y Esc arm (or the main loop) re-enters the
    /// end path cannot change the value handed to `init` — the unwind value
    /// is fixed at compile time.
    pub fn exit_code(&self) -> i32 {
        EXIT_LOGOUT
    }

    /// Reap exited children and update every subsystem that tracked them:
    /// the crash notification goes to `services`, the permission/IPC
    /// registries are unregistered, and the window manager drops the window
    /// (which also frees terminal pty masters). Returns true when at least
    /// one child was reaped so the caller can repaint.
    pub(crate) fn reap(
        &mut self,
        wm: &mut WindowManager,
        services: &mut ServiceManager,
        permissions: &mut PermissionManager,
        ipc_transport: &mut IpcTransport,
        current_tick: u64,
    ) -> bool {
        let mut reaped = false;
        loop {
            match process::waitpid(-1, 1) {
                Ok((pid, status)) if pid > 0 => {
                    reaped = true;
                    match exit_class(status) {
                        ExitClass::Clean => self.lifecycle.mark_terminated(pid),
                        cls => {
                            self.lifecycle.mark_crashed(pid);
                            let reason = match cls {
                                ExitClass::Killed => alloc::string::String::from("killed"),
                                ExitClass::Signal(sig) => alloc::format!("signal {}", sig),
                                ExitClass::Error(code) => alloc::format!("exit {}", code),
                                ExitClass::Clean => unreachable!(),
                            };
                            services.notify("Application Crashed", &reason, 2, 8000, current_tick);
                        }
                    }
                    self.lifecycle.remove(pid);
                    permissions.unregister(pid);
                    ipc_transport.unregister(pid);
                    wm.close_by_pid(pid);
                }
                _ => break,
            }
        }
        reaped
    }
}
