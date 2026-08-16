use std::time::{Duration, Instant};

/// Default per-test timeout. A test that exceeds it fails as "timed out"
/// instead of stalling the run (and with it, CI).
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Default TOTAL-run cap, in milliseconds, across all tests. The per-test
/// timeout bounds one test; N tests each hanging at their per-test limit
/// would otherwise sum to N * timeout (85 * 30 s = 42 min). The watchdog
/// bounds the SUM instead: generous for the current suite (which finishes in
/// well under a second) but a hard ceiling on the pathological case.
pub const DEFAULT_TOTAL_TIMEOUT_MS: u64 = 120_000;

/// A registered test function.
pub struct Test {
    pub name: &'static str,
    pub category: &'static str,
    /// `+ Send`: the closure is moved into a per-test SUBPROCESS in the
    /// default runner mode, so a hung test is killed at the timeout instead
    /// of stalling (or leaking into) the whole run.
    pub run: Box<dyn Fn() -> Result<(), String> + Send>,
}

/// How each test is executed. `Subprocess` (used by the CLI binary) re-execs
/// the current executable with the hidden `exec` subcommand so a hung test
/// can be KILLED (process isolation) instead of abandoned. `Thread` keeps the
/// test in-process -- used by the runner's own unit tests, which cannot
/// re-exec themselves.
#[derive(Clone, Copy, PartialEq)]
pub enum RunMode {
    Thread,
    Subprocess,
}

/// Result of running a single test.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestRun {
    pub name: String,
    pub category: String,
    pub passed: bool,
    pub message: String,
    pub duration_ms: u64,
}

impl TestRun {
    fn from_test(
        name: &'static str,
        category: &'static str,
        result: Result<(), String>,
        duration_ms: u64,
    ) -> Self {
        let (passed, message) = match result {
            Ok(()) => (true, String::new()),
            Err(e) => (false, e),
        };
        TestRun {
            name: name.to_string(),
            category: category.to_string(),
            passed,
            message,
            duration_ms,
        }
    }
}

/// Collects and runs tests, produces reports.
pub struct TestRunner {
    tests: Vec<Test>,
    runs: Vec<TestRun>,
    /// Per-test cap; `None` disables the timeout. In `Subprocess` mode a
    /// timed-out test's subprocess is KILLED and reaped; in `Thread` mode
    /// its thread is abandoned (leaked) -- either way the run costs at most
    /// one timeout instead of a forever-stalled CI job.
    timeout: Option<Duration>,
    /// Overall run cap across ALL tests; `None` disables it. Bounds the SUM
    /// of every test: even if each test hangs at its per-test limit, the run
    /// ends by this deadline and tests that never start are failed by the
    /// watchdog.
    total_timeout: Option<Duration>,
    mode: RunMode,
}

impl TestRunner {
    /// `timeout_ms == 0` disables the per-test cap (for debugging a genuinely
    /// slow test); `total_timeout_ms == 0` disables the total-run watchdog.
    /// Tests run in-process (`RunMode::Thread`) -- the runner's own unit
    /// tests cannot re-exec themselves; the CLI binary uses `new_subprocess`
    /// instead.
    pub fn new_with_timeouts(timeout_ms: u64, total_timeout_ms: u64) -> Self {
        Self::new_inner(timeout_ms, total_timeout_ms, RunMode::Thread)
    }

    /// Per-test subprocess isolation: each test re-execs the current
    /// executable's hidden `exec` subcommand, so a hung test is KILLED at its
    /// timeout (process isolation) rather than abandoned. Slower than thread
    /// mode (one process spawn per test) but leaks nothing.
    pub fn new_subprocess(timeout_ms: u64, total_timeout_ms: u64) -> Self {
        Self::new_inner(timeout_ms, total_timeout_ms, RunMode::Subprocess)
    }

    fn new_inner(timeout_ms: u64, total_timeout_ms: u64, mode: RunMode) -> Self {
        let timeout = if timeout_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(timeout_ms))
        };
        let total_timeout = if total_timeout_ms == 0 {
            None
        } else {
            Some(Duration::from_millis(total_timeout_ms))
        };
        TestRunner {
            tests: Vec::new(),
            runs: Vec::new(),
            timeout,
            total_timeout,
            mode,
        }
    }

    pub fn register(&mut self, test: Test) {
        self.tests.push(test);
    }

    pub fn register_all(&mut self, tests: Vec<Test>) {
        self.tests.extend(tests);
    }

    pub fn run_all(&mut self) {
        self.runs.clear();
        // Consume the registered tests: each closure runs in its own thread or
        // subprocess depending on RunMode. (run_all is called once per runner;
        // re-register before a second run.)
        let tests = std::mem::take(&mut self.tests);
        // Total-run watchdog: one fixed deadline for the WHOLE run, so N tests
        // each hanging at their per-test limit cost at most the total cap
        // (not N * timeout). Tests that never get a chance to start once the
        // budget is gone are failed WITHOUT spawning a thread; a test already
        // running keeps its per-test budget shrunk to what the deadline still
        // allows, so the run ends by the deadline either way.
        let deadline = self.total_timeout.map(|t| Instant::now() + t);
        for test in tests {
            // Destructure first: `run` moves into its thread, while `name` /
            // `category` (both &'static str) stay available for the report.
            let Test { name, category, run } = test;
            let start = Instant::now();
            let budget = match deadline {
                Some(dl) if Instant::now() >= dl => {
                    self.runs.push(TestRun::from_test(
                        name,
                        category,
                        Err("total-run watchdog: overall run cap exceeded, test never started"
                            .to_string()),
                        0,
                    ));
                    continue;
                }
                Some(dl) => {
                    let remaining = dl.saturating_duration_since(Instant::now());
                    match self.timeout {
                        Some(t) => Some(t.min(remaining)),
                        // Per-test cap disabled (`--timeout-ms 0`) but the
                        // total cap is set: the total deadline still bounds
                        // each test, so a hung test can't escape the
                        // watchdog entirely.
                        None => Some(remaining),
                    }
                }
                None => self.timeout,
            };
            let result = match self.mode {
                RunMode::Thread => run_with_timeout(run, budget),
                RunMode::Subprocess => crate::exec::run_in_subprocess(name, budget),
            };
            let duration = start.elapsed().as_millis() as u64;
            self.runs.push(TestRun::from_test(name, category, result, duration));
        }
    }

    pub fn runs(&self) -> &[TestRun] { &self.runs }
    pub fn total(&self) -> usize { self.runs.len() }
    pub fn passed(&self) -> usize { self.runs.iter().filter(|r| r.passed).count() }
    pub fn failed(&self) -> usize { self.runs.iter().filter(|r| !r.passed).count() }

    pub fn report(&self) -> TestReport {
        TestReport {
            runs: self.runs.clone(),
            total: self.total(),
        }
    }
}

/// Serializable test report for JSON/HTML export.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestReport {
    pub runs: Vec<TestRun>,
    pub total: usize,
}

impl TestReport {
    pub fn passed(&self) -> usize { self.runs.iter().filter(|r| r.passed).count() }
    pub fn failed(&self) -> usize { self.runs.iter().filter(|r| !r.passed).count() }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn to_html(&self) -> String {
        let mut html = String::new();
        html.push_str("<!DOCTYPE html><html><head><meta charset=\"utf-8\">");
        html.push_str("<title>SkyOS Test Report</title>");
        html.push_str("<style>");
        html.push_str("body{font-family:sans-serif;margin:2rem;background:#1e1e1e;color:#ccc}");
        html.push_str("h1{color:#fff}.pass{color:#4caf50}.fail{color:#f44336}");
        html.push_str(".summary{font-size:1.2rem;margin:1rem 0}");
        html.push_str("table{width:100%;border-collapse:collapse}");
        html.push_str("th,td{padding:8px 12px;text-align:left;border-bottom:1px solid #333}");
        html.push_str("th{background:#2d2d2d;color:#fff}");
        html.push_str(".badge{display:inline-block;padding:2px 8px;border-radius:4px;font-size:0.8rem}");
        html.push_str(".badge-pass{background:#1b5e20;color:#a5d6a7}");
        html.push_str(".badge-fail{background:#b71c1c;color:#ef9a9a}");
        html.push_str(".timestamp{color:#888;font-size:0.9rem}");
        html.push_str("</style></head><body>");
        html.push_str("<h1>SkyOS Test Report</h1>");
        html.push_str(&format!("<div class=\"timestamp\">{}</div>", chrono_now()));
        html.push_str(&format!(
            "<div class=\"summary\">Total: {} | <span class=\"pass\">Passed: {}</span> | <span class=\"fail\">Failed: {}</span></div>",
            self.total, self.passed(), self.failed()));
        html.push_str("<table><tr><th>Test</th><th>Category</th><th>Result</th><th>Duration</th><th>Message</th></tr>");
        for run in &self.runs {
            let badge = if run.passed { "badge-pass" } else { "badge-fail" };
            let label = if run.passed { "PASS" } else { "FAIL" };
            html.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td><span class=\"badge {}\">{}</span></td><td>{}ms</td><td>{}</td></tr>",
                run.name, run.category, badge, label, run.duration_ms, run.message));
        }
        html.push_str("</table></body></html>");
        html
    }
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs();
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{:02}:{:02}:{:02} UTC", h, m, s)
}

pub fn write_report_json(report: &TestReport, path: &str) -> std::io::Result<()> {
    std::fs::write(path, report.to_json())
}

pub fn write_report_html(report: &TestReport, path: &str) -> std::io::Result<()> {
    std::fs::write(path, report.to_html())
}

/// Assertions that return Result for test functions.
#[macro_export]
macro_rules! assert_result {
    ($cond:expr) => {
        if !$cond {
            return Err(format!("assertion failed: {}", stringify!($cond)));
        }
    };
    ($cond:expr, $($arg:tt)+) => {
        if !$cond {
            return Err(format!("{}: {}", format!($($arg)+), stringify!($cond)));
        }
    };
}

#[macro_export]
macro_rules! assert_eq_result {
    ($left:expr, $right:expr) => {{
        let l = $left;
        let r = $right;
        if l != r {
            return Err(format!(
                "assertion failed: `{} == {}`\n  left: {:?}\n right: {:?}",
                stringify!($left), stringify!($right), l, r));
        }
    }};
    ($left:expr, $right:expr, $($arg:tt)+) => {{
        let l = $left;
        let r = $right;
        if l != r {
            return Err(format!(
                "{}: assertion failed: `{} == {}`\n  left: {:?}\n right: {:?}",
                format!($($arg)+), stringify!($left), stringify!($right), l, r));
        }
    }};
}

/// `RunMode::Thread` path: run one test closure in-process on its own thread,
/// catching panics and enforcing the per-test timeout. Panics become
/// failures; a timeout becomes a failure with a "timed out" message and the
/// thread is abandoned (leaked). Production runs use `exec::run_in_subprocess`
/// instead, which KILLS the test process; this path exists for the runner's
/// own unit tests, which cannot re-exec themselves.
fn run_with_timeout(
    run: Box<dyn Fn() -> Result<(), String> + Send>,
    timeout: Option<Duration>,
) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawn = std::thread::Builder::new().spawn(move || {
        // Flatten before sending: catch_unwind yields a nested Result; the
        // channel message must be the test's own `Result<(), String>` so the
        // receiver's `Ok(outcome)` unwraps directly.
        let outcome = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)) {
            Ok(r) => r,
            Err(payload) => Err(panic_payload_message(payload)),
        };
        let _ = tx.send(outcome);
    });
    let spawn = match spawn {
        Ok(h) => h,
        Err(e) => return Err(format!("failed to spawn test thread: {}", e)),
    };
    // The channel carries the test's own Result; the recv error is the
    // timeout/vanished case. Never join the thread: a hung test must not
    // block the runner -- it is abandoned and reclaimed at process exit.
    drop(spawn);
    match timeout {
        Some(t) => match rx.recv_timeout(t) {
            Ok(outcome) => outcome,
            Err(_) => Err(format!("timed out after {}ms", t.as_millis())),
        },
        None => match rx.recv() {
            Ok(outcome) => outcome,
            Err(_) => Err("test thread vanished".to_string()),
        },
    }
}

pub(crate) fn panic_payload_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "panicked (non-string payload)".to_string()
}

pub mod exec;
pub mod mock;
pub mod suites;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::{wait_and_drain, wait_for_child};

    fn test(name: &'static str, f: impl Fn() -> Result<(), String> + Send + 'static) -> Test {
        Test { name, category: "self", run: Box::new(f) }
    }

    #[test]
    fn passing_test_reports_ok() {
        let mut r = TestRunner::new_with_timeouts(1000, 0);
        r.register(test("pass", || Ok(())));
        r.run_all();
        assert_eq!(r.total(), 1);
        assert_eq!(r.passed(), 1);
        assert_eq!(r.failed(), 0);
        assert_eq!(r.runs()[0].name, "pass");
        assert_eq!(r.runs()[0].message, "");
    }

    #[test]
    fn failing_test_reports_message() {
        let mut r = TestRunner::new_with_timeouts(1000, 0);
        r.register(test("fail", || Err("boom".to_string())));
        r.run_all();
        assert_eq!(r.failed(), 1);
        assert!(r.runs()[0].message.contains("boom"), "message: {}", r.runs()[0].message);
    }

    #[test]
    fn hung_test_times_out_and_is_failed() {
        let mut r = TestRunner::new_with_timeouts(50, 0);
        r.register(test("hang", || {
            std::thread::sleep(Duration::from_secs(30));
            Ok(())
        }));
        r.run_all();
        assert_eq!(r.failed(), 1);
        assert!(r.runs()[0].message.contains("timed out"), "message: {}", r.runs()[0].message);
        assert!(r.runs()[0].duration_ms >= 50, "duration: {}", r.runs()[0].duration_ms);
    }

    #[test]
    fn panicking_test_is_a_failure_not_an_abort() {
        let mut r = TestRunner::new_with_timeouts(1000, 0);
        r.register(test("panic", || panic!("kaboom")));
        r.run_all();
        assert_eq!(r.failed(), 1);
        assert!(r.runs()[0].message.contains("kaboom"), "message: {}", r.runs()[0].message);
    }

    #[test]
    fn zero_timeout_disables_the_cap() {
        let mut r = TestRunner::new_with_timeouts(0, 0);
        r.register(test("pass", || Ok(())));
        r.run_all();
        assert_eq!(r.passed(), 1);
    }

    #[test]
    fn all_tests_run_even_with_failures() {
        let mut r = TestRunner::new_with_timeouts(1000, 0);
        r.register(test("a", || Err("x".to_string())));
        r.register(test("b", || Ok(())));
        r.register(test("c", || Ok(())));
        r.run_all();
        assert_eq!(r.total(), 3);
        assert_eq!(r.passed(), 2);
        assert_eq!(r.failed(), 1);
    }

    #[test]
    fn total_watchdog_bounds_the_run() {
        // Generous per-test budget (60 s) but a tiny overall cap: only ~2 of
        // the 5 slow tests can fit, so the rest are failed by the watchdog
        // without ever starting. Without the watchdog this run would take
        // 5 x 100 ms; with a real 30 s per-test timeout, 5 x 30 s.
        let mut r = TestRunner::new_with_timeouts(60_000, 200);
        for name in ["slow0", "slow1", "slow2", "slow3", "slow4"] {
            r.register(test(name, || {
                std::thread::sleep(Duration::from_millis(100));
                Ok(())
            }));
        }
        let start = Instant::now();
        r.run_all();
        assert!(start.elapsed() < Duration::from_secs(5), "run must end by the total cap");
        assert_eq!(r.total(), 5, "all tests are reported (some as watchdog failures)");
        assert!(r.passed() < 5, "watchdog must cut the run short");
        for run in r.runs() {
            if !run.passed {
                assert!(
                    run.message.contains("total-run watchdog") || run.message.contains("timed out"),
                    "unexpected failure message: {}",
                    run.message
                );
            }
        }
        assert!(
            r.runs().iter().any(|x| !x.passed && x.message.contains("total-run watchdog")),
            "at least one test must be skipped by the watchdog"
        );
    }

    #[test]
    fn total_watchdog_binds_even_when_per_test_timeout_disabled() {
        // `--timeout-ms 0 --total-timeout-ms N`: the per-test cap being off
        // is not a watchdog escape hatch -- the total deadline must still
        // bound a hung test.
        let mut r = TestRunner::new_with_timeouts(0, 300);
        r.register(test("hang", || {
            std::thread::sleep(Duration::from_secs(60));
            Ok(())
        }));
        let start = Instant::now();
        r.run_all();
        assert!(start.elapsed() < Duration::from_secs(10), "total cap must bind the run");
        assert_eq!(r.failed(), 1);
        assert!(r.runs()[0].message.contains("timed out"), "message: {}", r.runs()[0].message);
    }

    #[test]
    fn total_watchdog_zero_disables_the_cap() {
        let mut r = TestRunner::new_with_timeouts(1000, 0);
        for name in ["fast0", "fast1", "fast2"] {
            r.register(test(name, || Ok(())));
        }
        r.run_all();
        assert_eq!(r.passed(), 3);
        assert_eq!(r.failed(), 0);
    }

    /// Never exits. Only ever run via direct invocation -- the subprocess
    /// kill test below re-execs the harness with `--include-ignored` and
    /// this exact name -- so a normal `cargo test` never touches it.
    #[test]
    #[ignore]
    fn zzz_internal_hang() {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// Prints far more than the ~4KB anonymous pipe buffer, then returns.
    /// Only ever run via direct invocation -- the regression test below
    /// re-execs the harness with `--include-ignored` and this exact name --
    /// so a normal `cargo test` never touches it.
    #[test]
    #[ignore]
    fn zzz_internal_chatty() {
        for _ in 0..2000 {
            println!("padding-padding-padding-padding-padding-padding");
        }
    }

    #[test]
    fn subprocess_output_larger_than_pipe_buffer_is_captured_not_killed() {
        // Regression for the pipe-deadlock mis-kill: a test printing more
        // than the pipe buffer must complete normally (drained on a helper
        // thread), not be killed as "timed out" because its write blocked.
        let exe = std::env::current_exe().expect("current exe");
        // `--nocapture`: the harness captures test stdout by default, so the
        // child must be told to let the prints reach the real pipe.
        let mut child = std::process::Command::new(&exe)
            .args(["tests::zzz_internal_chatty", "--exact", "--include-ignored", "--nocapture"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn harness");
        let start = Instant::now();
        let out = wait_and_drain(&mut child, Some(Duration::from_secs(30)))
            .expect("chatty test must complete, not be killed");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "drained, not killed at the timeout"
        );
        assert!(out.len() > 50_000, "full output captured ({} bytes)", out.len());
    }

    #[test]
    fn subprocess_is_killed_on_timeout() {
        // Re-exec the unit-test harness running ONLY the ignored hang test:
        // wait_for_child must KILL it at the deadline instead of waiting for
        // the loop (which never finishes). This exercises the exact poll +
        // kill + reap machinery the production runner uses.
        let exe = std::env::current_exe().expect("current exe");
        let mut child = std::process::Command::new(&exe)
            .args(["tests::zzz_internal_hang", "--exact", "--include-ignored"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn harness");
        let start = Instant::now();
        let res = wait_for_child(&mut child, Some(Duration::from_millis(800)));
        let err = res.unwrap_err();
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "must be killed at the deadline, not waited on"
        );
        assert!(err.contains("killed"), "message: {}", err);
    }
}
