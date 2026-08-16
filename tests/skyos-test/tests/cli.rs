//! End-to-end CLI contract tests against the real built binary.
//!
//! Pins the CI-safety behaviors: (1) a misconfigured invocation -- unknown
//! flag, unknown subcommand, stray positional -- must fail HARD (non-zero
//! exit), never be silently ignored; (2) the `--timeout-ms` and
//! `--total-timeout-ms` flags are accepted and a filtered run still works;
//! (3) the hidden `exec` subcommand (per-test subprocess isolation) runs a
//! named test and reports via the result envelope.
//!
//! The binary is invoked as a subprocess so these tests pin the actual
//! process exit code, not just clap's parse result.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_skyos-test"))
}

#[test]
fn unknown_flag_fails_hard() {
    let out = bin().args(["run", "--bogus-flag"]).output().expect("run binary");
    assert!(
        !out.status.success(),
        "unknown flag must fail: status {:?}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--bogus-flag"), "usage must name the offending arg: {stderr}");
}

#[test]
fn unknown_subcommand_fails_hard() {
    let out = bin().arg("bogus-subcommand").output().expect("run binary");
    assert!(!out.status.success(), "unknown subcommand must fail: {:?}", out.status);
}

#[test]
fn stray_positional_fails_hard() {
    let out = bin().args(["run", "extra"]).output().expect("run binary");
    assert!(!out.status.success(), "stray positional must fail: {:?}", out.status);
}

#[test]
fn valid_filtered_run_succeeds() {
    let out = bin()
        .args(["run", "--category", "kernel::alloc", "--timeout-ms", "1000"])
        .output()
        .expect("run binary");
    assert!(out.status.success(), "valid run must succeed: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Total: 6 | Passed: 6 | Failed: 0"), "summary: {stdout}");
}

#[test]
fn timeout_flag_is_rejected_when_unparseable() {
    let out = bin().args(["run", "--timeout-ms", "abc"]).output().expect("run binary");
    assert!(!out.status.success(), "non-numeric timeout must fail: {:?}", out.status);
}

#[test]
fn total_timeout_flag_is_accepted() {
    // 0 disables the overall run cap: the full suite must still pass cleanly.
    let out = bin().args(["run", "--total-timeout-ms", "0"]).output().expect("run binary");
    assert!(out.status.success(), "disabled cap must not break a green run: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Total: 85 | Passed: 85 | Failed: 0"), "summary: {stdout}");
}

#[test]
fn list_categories_prints_unique_categories_with_counts() {
    // Exact pin of the current suite's category census: breaks loudly if a
    // category is added, removed, or renamed (update it deliberately). The
    // counts must sum to the pinned total (85).
    let out = bin().arg("--list-categories").output().expect("run binary");
    assert!(out.status.success(), "--list-categories must succeed: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.trim(),
        "kernel::alloc: 6\n\
         kernel::fs: 20\n\
         kernel::futex: 12\n\
         kernel::mouse: 11\n\
         kernel::paging: 14\n\
         kernel::vfs: 7\n\
         kernel::wait: 15"
    );
}

#[test]
fn list_categories_conflicts_with_subcommands() {
    // --list-categories is an alternative to the subcommands, never a prefix.
    let out = bin().args(["--list-categories", "run"]).output().expect("run binary");
    assert!(
        !out.status.success(),
        "--list-categories must not combine with a subcommand: {:?}",
        out.status
    );
}

#[test]
fn exec_subcommand_runs_a_named_test() {
    // The per-test subprocess isolation re-execs the binary with `exec`: it
    // must run the named test and print the result envelope.
    let out = bin()
        .args(["exec", "--name", "ext2_allocate_block_first_free_bit"])
        .output()
        .expect("run binary");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "exec of a real test must succeed: {:?}\n{stdout}",
        out.status
    );
    assert!(stdout.contains("SKYOS_TEST_RESULT"), "envelope: {stdout}");
    assert!(stdout.contains("\"passed\":true"), "envelope: {stdout}");
}

#[test]
fn exec_subcommand_unknown_name_fails() {
    let out = bin().args(["exec", "--name", "no_such_test"]).output().expect("run binary");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "unknown test name must fail: {:?}", out.status);
    assert!(stdout.contains("no test named"), "stdout: {stdout}");
}

#[test]
fn total_timeout_flag_is_rejected_when_unparseable() {
    let out = bin()
        .args(["run", "--total-timeout-ms", "abc"])
        .output()
        .expect("run binary");
    assert!(!out.status.success(), "non-numeric total timeout must fail: {:?}", out.status);
}
