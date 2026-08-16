//! Per-test subprocess execution.
//!
//! `RunMode::Subprocess` re-execs the current executable's hidden `exec`
//! subcommand for each test, so a hung test can be KILLED (process
//! isolation) instead of abandoned. The child runs the closure in-process
//! (`run_test_in_process`) and prints a `SKYOS_TEST_RESULT <json>` envelope;
//! the parent enforces the per-test timeout by killing the child, and
//! `wait_and_drain` keeps the child's stdout moving so a test that prints
//! more than the pipe buffer is never mistaken for a hang.

use crate::{panic_payload_message, Test};
use std::time::{Duration, Instant};

/// Runs one test in a per-test SUBPROCESS: re-exec the current executable
/// with the hidden `exec --name <test>` subcommand. The child prints a
/// `SKYOS_TEST_RESULT <json>` envelope as its last stdout line; on timeout
/// the child is KILLED (process isolation) rather than abandoned.
pub(crate) fn run_in_subprocess(name: &'static str, timeout: Option<Duration>) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot locate own executable for test subprocess: {}", e))?;
    let mut child = std::process::Command::new(&exe)
        .args(["exec", "--name", name])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit()) // panic traces reach the console
        .spawn()
        .map_err(|e| format!("failed to spawn test subprocess: {}", e))?;
    let out = wait_and_drain(&mut child, timeout)?;
    parse_envelope(&String::from_utf8_lossy(&out))
}

/// Waits for `child` (killing it at `timeout`) while draining its stdout on
/// a helper thread. The drain starts IMMEDIATELY: if the pipe (a few KB)
/// fills while the child is still writing, the child BLOCKS on write and
/// would be mis-killed as "timed out" even though it isn't hung -- a legit
/// test printing a large buffer must not look like a hang. The reader thread
/// ends on its own once the child exits (EOF on the pipe) or is killed (the
/// pipe closes), so it never leaks. Returns the full captured stdout.
pub(crate) fn wait_and_drain(
    child: &mut std::process::Child,
    timeout: Option<Duration>,
) -> Result<Vec<u8>, String> {
    let mut stdout = child.stdout.take().expect("piped stdout present");
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    wait_for_child(child, timeout)?;
    let _ = child.wait(); // reap (wait_for_child already observed the exit)
    Ok(reader.join().unwrap_or_default())
}

/// Polls `try_wait` until the child exits; on timeout KILLS the child and
/// reaps it so it can't linger. Returns `Err` with a "killed" message when
/// the deadline hits.
pub(crate) fn wait_for_child(child: &mut std::process::Child, timeout: Option<Duration>) -> Result<(), String> {
    let deadline = timeout.map(|t| Instant::now() + t);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(e) => return Err(format!("failed to wait on test subprocess: {}", e)),
        }
        if let Some(dl) = deadline {
            if Instant::now() >= dl {
                // Kill, then reap: the child must not linger as a zombie.
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "timed out after {}ms, subprocess killed",
                    timeout.map(|t| t.as_millis()).unwrap_or(0)
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Extracts the `SKYOS_TEST_RESULT {"passed":bool,"message":str}` envelope
/// from the subprocess stdout (scanning from the end, so stray prints from
/// the test itself earlier in the stream don't interfere).
fn parse_envelope(stdout: &str) -> Result<(), String> {
    for line in stdout.lines().rev() {
        if let Some(rest) = line.strip_prefix("SKYOS_TEST_RESULT ") {
            let v: serde_json::Value = serde_json::from_str(rest)
                .map_err(|e| format!("malformed test result envelope: {}", e))?;
            let passed = v.get("passed").and_then(|p| p.as_bool()).unwrap_or(false);
            let message = v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            return if passed { Ok(()) } else { Err(message) };
        }
    }
    Err("test subprocess produced no result envelope".to_string())
}

/// Runs a single test closure in the CURRENT process, converting panics to
/// `Err`. The CLI's hidden `exec` subcommand uses this; the parent enforces
/// the timeout by killing this process, so no timeout logic lives here.
pub fn run_test_in_process(test: Test) -> Result<(), String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (test.run)())) {
        Ok(r) => r,
        Err(payload) => Err(panic_payload_message(payload)),
    }
}
