use clap::{Parser, Subcommand};
use skyos_test_core::{suites, write_report_html, write_report_json, TestRunner};
use std::path::PathBuf;

#[derive(Parser)]
// `args_conflicts_with_subcommands`: `--list-categories` is an alternative to
// the subcommands, so it must not combine with one. `arg_required_else_help`:
// a bare `skyos-test` (no flag, no subcommand) prints help instead of silently
// doing nothing now that the subcommand is optional.
#[command(
    name = "skyos-test",
    about = "SkyOS Test Framework",
    args_conflicts_with_subcommands = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Print unique test categories with their test counts, one `category:
    /// count` line each (sorted), and exit.
    #[arg(long)]
    list_categories: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List all available test suites
    List,
    /// Run tests
    Run {
        /// Filter by category (e.g. kernel::mouse, kernel::alloc)
        #[arg(short, long)]
        category: Option<String>,

        /// Output format: json, html, or console (default)
        #[arg(short, long, default_value = "console")]
        format: String,

        /// Output file path (for json/html)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Per-test timeout in milliseconds (0 = no timeout). A test that
        /// exceeds it fails as 'timed out' instead of stalling the run.
        #[arg(long, default_value_t = skyos_test_core::DEFAULT_TIMEOUT_MS)]
        timeout_ms: u64,

        /// Overall run cap in milliseconds across ALL tests (0 = no cap).
        /// Even if every test hangs at its per-test limit, the run ends by
        /// this deadline; tests that never start are failed by the watchdog.
        #[arg(long, default_value_t = skyos_test_core::DEFAULT_TOTAL_TIMEOUT_MS)]
        total_timeout_ms: u64,
    },
    /// Internal: run one registered test by name and print a JSON envelope.
    /// The runner's subprocess isolation re-execs this binary with this
    /// subcommand so a hung test can be killed; not for humans.
    #[command(hide = true)]
    Exec {
        /// Exact test name as shown by `list`.
        #[arg(long)]
        name: String,
    },
    /// Generate HTML report from existing JSON results
    Report {
        /// JSON results file
        #[arg(default_value = "skyos-test-results.json")]
        input: PathBuf,
        /// Output HTML file
        #[arg(short, long, default_value = "skyos-test-report.html")]
        output: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    if cli.list_categories {
        list_categories();
        return;
    }
    // arg_required_else_help guarantees a subcommand here (unless
    // --list-categories was given, handled above).
    let command = cli.command.expect("clap requires a subcommand or --list-categories");
    match command {
        Commands::List => {
            let tests = suites::all();
            println!("SkyOS Test Suites:");
            for test in &tests {
                println!("  [{:20}] {}", test.category, test.name);
            }
            println!("\nTotal: {} tests", tests.len());
        }
        Commands::Run {
            category,
            format,
            output,
            timeout_ms,
            total_timeout_ms,
        } => {
            // Subprocess isolation: each test re-execs this binary's hidden
            // `exec` subcommand, so a hung test is killed at its timeout.
            let mut runner = TestRunner::new_subprocess(timeout_ms, total_timeout_ms);
            // Select ONCE: register_all extends, so registering the unfiltered
            // list and then the filtered subset would run everything twice
            // (e.g. --category kernel::alloc ran 23 tests instead of 6).
            let selected: Vec<skyos_test_core::Test> = match category {
                Some(ref cat) => suites::all()
                    .into_iter()
                    .filter(|t| t.category.contains(cat.as_str()))
                    .collect(),
                None => suites::all(),
            };
            runner.register_all(selected);

            runner.run_all();
            let report = runner.report();

            // Print/write the report FIRST (it is the diagnostics a failed run
            // needs) and only then enforce exit-code discipline. The previous
            // ordering exited before the console arm ran, so a piped failed
            // run printed NOTHING -- which test failed was invisible.
            match format.as_str() {
                "json" => {
                    let json = report.to_json();
                    if let Some(path) = output {
                        let _ = write_report_json(&report, path.to_str().unwrap_or("results.json"));
                        println!("Report written to {:?}", path);
                    } else {
                        println!("{}", json);
                    }
                }
                "html" => {
                    let path = output.unwrap_or_else(|| PathBuf::from("skyos-test-report.html"));
                    let _ = write_report_html(&report, path.to_str().unwrap());
                    println!("Report written to {:?}", path);
                }
                _ => {
                    // Console output
                    println!("\n=== SkyOS Test Results ===\n");
                    for run in runner.runs() {
                        let status = if run.passed { "PASS" } else { "FAIL" };
                        println!(
                            "  [{}] {} [{:20}] {}",
                            status, run.name, run.category, run.duration_ms
                        );
                        if !run.message.is_empty() {
                            println!("       {}", run.message);
                        }
                    }
                    println!(
                        "\nTotal: {} | Passed: {} | Failed: {}",
                        runner.total(),
                        runner.passed(),
                        runner.failed()
                    );
                }
            }

            // Exit-code discipline: a run with failures, or a run that
            // executed nothing (e.g. a typo'd --category, or a suite silently
            // dropped from registration), must fail the process -- otherwise
            // CI and scripts stay green on a broken suite. The report above is
            // already emitted; std::process::exit skips destructors, so flush
            // the block-buffered streams before bailing.
            if runner.failed() > 0 || runner.total() == 0 {
                use std::io::Write;
                let _ = std::io::stdout().flush();
                let _ = std::io::stderr().flush();
                std::process::exit(1);
            }
        }
        Commands::Exec { name } => {
            // Runs in THIS process (the parent enforces the timeout by
            // killing us), prints the result envelope, exits non-zero on a
            // failure so the parent's parse_envelope + exit discipline agree.
            let result = match skyos_test_core::suites::all()
                .into_iter()
                .find(|t| t.name == name)
            {
                Some(t) => skyos_test_core::exec::run_test_in_process(t),
                None => Err(format!("no test named {:?}", name)),
            };
            let (passed, message) = match &result {
                Ok(()) => (true, ""),
                Err(e) => (false, e.as_str()),
            };
            println!(
                "SKYOS_TEST_RESULT {}",
                serde_json::json!({ "passed": passed, "message": message })
            );
            use std::io::Write;
            let _ = std::io::stdout().flush();
            if result.is_err() {
                std::process::exit(1);
            }
        }
        Commands::Report { input, output } => {
            let json_str = std::fs::read_to_string(&input).expect("Failed to read input file");
            let report: skyos_test_core::TestReport =
                serde_json::from_str(&json_str).expect("Failed to parse JSON");
            write_report_html(&report, output.to_str().unwrap()).expect("Failed to write report");
            println!("Report written to {:?}", output);
        }
    }
}

/// `--list-categories`: unique categories from the registered suite, with the
/// number of tests in each, one `category: count` line per category (sorted
/// for deterministic output -- the CLI contract tests pin it exactly).
fn list_categories() {
    let mut counts: std::collections::BTreeMap<&'static str, usize> =
        std::collections::BTreeMap::new();
    for test in suites::all() {
        *counts.entry(test.category).or_insert(0) += 1;
    }
    for (category, count) in counts {
        println!("{}: {}", category, count);
    }
}
