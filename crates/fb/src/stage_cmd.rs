//! `fb stage`: the stage rule as a command, so the shell asks instead of
//! deciding for itself.
//!
//! `fb-pipeline.sh` collapsed three different facts into one branch — the
//! command failed, or it exited 0 and left no artefact, or the artefact could
//! not be looked at at all — and its own comment says those need different
//! fixes. `farmerbob_core::stage_outcome` already keeps the three apart; this
//! module is its command line. `run` parses the arguments after the
//! subcommand word, stats the artefact, and lets `classify` and `may_sign`
//! decide; the answer is printed as the one line `report` renders for the
//! stage, so the log can tell the facts apart without an operator stat-ing
//! the file by hand.
//!
//! The grammar is pinned exactly: the stage name, then the two flags in
//! either order, each flag followed by its value as a separate argument.
//! Anything else is a usage error — a usage line on stderr, nothing on
//! stdout, and status 2.

use farmerbob_core::measurement::Measurement;
use farmerbob_core::stage_outcome::{Stage, classify, may_sign, report};

/// `fb stage <name> --rc <i32> --artefact <path>`
///
/// Prints one line naming the stage and what it did, and returns 0 when the
/// stage may be signed, 1 when it may not. Malformed arguments print a usage
/// line to stderr, print nothing to stdout, and return 2: not a
/// classification, so not on the channel the classification owns.
// No caller yet: a separate task wires this into `fb`'s dispatch match, and
// a binary crate checks dead code from `main`, so the module is unreachable
// until then.
#[allow(dead_code)]
pub fn run(args: &[String]) -> i32 {
    let Some(stage_args) = parse_args(args) else {
        eprintln!("usage: fb stage <name> --rc <i32> --artefact <path>");
        return 2;
    };
    let bytes = measure(&stage_args.artefact);
    let outcome = classify(stage_args.rc, &bytes);
    let stage = Stage {
        name: stage_args.name,
        outcome,
    };
    for line in report(std::slice::from_ref(&stage)) {
        println!("{line}");
    }
    i32::from(!may_sign(&stage.outcome))
}

/// The parsed invocation: the stage's name, its exit status, and the path of
/// the artefact it was supposed to leave.
struct StageArgs {
    name: String,
    rc: i32,
    artefact: String,
}

/// Parses the pinned grammar — `<name>`, then `--rc <i32>` and
/// `--artefact <path>` in either order, each flag and its value taken as two
/// arguments — and returns `None` for anything else, which is a usage error:
/// a missing flag, a repeated flag, an unrecognised flag (the
/// `--flag=value` form included), a flag with no value, a trailing argument,
/// or a name beginning with `--`.
fn parse_args(args: &[String]) -> Option<StageArgs> {
    let (name, flags) = args.split_first()?;
    if name.starts_with("--") {
        return None;
    }
    let mut rc = None;
    let mut artefact = None;
    let (pairs, remainder) = flags.as_chunks::<2>();
    for pair in pairs {
        match pair[0].as_str() {
            "--rc" => {
                if rc.is_some() {
                    return None;
                }
                rc = Some(pair[1].parse().ok()?);
            }
            "--artefact" => {
                if artefact.is_some() {
                    return None;
                }
                artefact = Some(pair[1].clone());
            }
            _ => return None,
        }
    }
    if !remainder.is_empty() {
        return None;
    }
    Some(StageArgs {
        name: name.clone(),
        rc: rc?,
        artefact: artefact?,
    })
}

/// The artefact's size as the `Measurement` `classify` takes: the length
/// wherever `fs::metadata` read one — a directory included, its length being
/// whatever the filesystem reports — zero where the path does not exist,
/// which is the same `NotFound` a dangling symlink reports, and an instrument
/// failure carrying the operating system's reason for every other way the
/// stat can fail.
fn measure(path: &str) -> Measurement<u64> {
    match std::fs::metadata(path) {
        Ok(meta) => Measurement::observed(meta.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Measurement::observed(0),
        Err(err) => Measurement::instrument_failed(&err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    // The pinned surface is `run(&[String]) -> i32`: it returns the status and
    // prints the line to the process stdout, which no stable-Rust test can
    // read in process without a new dependency. The line assertions therefore
    // observe `run` inside a child copy of this very test binary — the child
    // test at the bottom calls `run` and exits with its status — so every
    // assertion goes through the pinned surface alone and the suite compiles
    // unchanged against any other implementation of this specification.
    use super::run;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temporary directory the test creates and removes, never a path in
    /// this repository, whose logs are shared with a running harness.
    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "fb_stage_cmd_{}_{}_{}",
                std::process::id(),
                n,
                tag
            ));
            std::fs::create_dir_all(&path).expect("create temporary directory");
            TempDir(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp_dir(tag: &str) -> TempDir {
        TempDir::new(tag)
    }

    /// Writes a fixture file and returns its path as `run` receives one.
    fn artefact_path(dir: &TempDir, file_name: &str, contents: &[u8]) -> String {
        let path = dir.path().join(file_name);
        std::fs::write(&path, contents).expect("write artefact fixture");
        path.display().to_string()
    }

    /// A symlink whose target does not exist: the one case where the honest
    /// answer (`Observed(0)`, the same as a file never created) differs from
    /// the tempting one (an instrument failure).
    fn dangling_symlink(dir: &TempDir, name: &str) -> String {
        let link = dir.path().join(name);
        std::os::unix::fs::symlink(dir.path().join("never-created-target"), &link)
            .expect("create dangling symlink");
        link.display().to_string()
    }

    /// A path the filesystem refuses to stat for a reason other than
    /// `NotFound`: a plain file used as a directory component. Returns the
    /// path and the operating system's own reason, derived from the same
    /// `std` call the command makes, so no error text is hard-coded here.
    fn not_a_directory(dir: &TempDir) -> (String, String) {
        let file = dir.path().join("plain-file");
        std::fs::write(&file, b"x").expect("write plain file");
        let under = file.join("child");
        let reason = std::fs::metadata(&under)
            .expect_err("a file used as a directory must fail to stat")
            .to_string();
        (under.display().to_string(), reason)
    }

    fn s(value: &str) -> String {
        value.to_string()
    }

    fn stage_args(name: &str, rc: i32, artefact: &str) -> Vec<String> {
        vec![
            s(name),
            s("--rc"),
            rc.to_string(),
            s("--artefact"),
            s(artefact),
        ]
    }

    /// The lines of a child's stdout that name the stage. The harness's own
    /// output names test paths only, never the stage, so this count is
    /// exactly what `run` printed about the stage.
    fn lines_naming<'a>(stdout: &'a str, stage: &str) -> Vec<&'a str> {
        stdout.lines().filter(|line| line.contains(stage)).collect()
    }

    /// The single stdout line naming the stage, failing the test otherwise.
    fn single_naming_line<'a>(stdout: &'a str, stage: &str) -> &'a str {
        let named = lines_naming(stdout, stage);
        assert_eq!(
            named.len(),
            1,
            "expected exactly one line naming {stage:?}, got {stdout:?}"
        );
        named[0]
    }

    const CHILD_ARGS_ENV: &str = "FB_STAGE_CMD_CHILD_ARGS";
    const CHILD_RUNNER_FILTER: &str = "stage_cmd_child_runner";
    const SILENT_PROBE_FILTER: &str = "stage_cmd_silent_probe";
    const PROBE_ENV: &str = "FB_STAGE_CMD_PROBE";

    /// The child side of the stdout capture: runs `run` once with arguments
    /// supplied by the capturing parent and exits with `run`'s status, so the
    /// parent observes the printed line on the child's stdout and the status
    /// on the child's exit code without either polluting the other. In a
    /// plain `cargo test` run no parent has planted the environment and this
    /// is a no-op.
    #[test]
    fn stage_cmd_child_runner() {
        let Ok(payload) = std::env::var(CHILD_ARGS_ENV) else {
            return;
        };
        let mut fields = payload.split('\u{1f}');
        let count: usize = fields
            .next()
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
        let args: Vec<String> = fields.take(count).map(str::to_string).collect();
        // The harness's own `test <name> ...` prefix carries no newline until
        // the test finishes, and this child exits before it does; the blank
        // line completes that prefix so everything `run` prints sits on its
        // own line of the captured output.
        println!();
        std::process::exit(run(&args));
    }

    /// A no-op test whose spawned child measures the harness's own chatter
    /// around a child that prints nothing, so the capturing tests can pin
    /// exactly what `run` itself added to stdout.
    #[test]
    fn stage_cmd_silent_probe() {
        if std::env::var(PROBE_ENV).is_ok() {
            println!();
            std::process::exit(0);
        }
    }

    /// Spawns this test binary running only the named test: returns the
    /// child's exit status, stdout and stderr.
    fn spawn_child(test_filter: &str, args: Option<&[String]>) -> (i32, String, String) {
        let mut command = Command::new(std::env::current_exe().expect("this test binary's path"));
        command
            .args([test_filter, "--nocapture", "--test-threads", "1"])
            .env(PROBE_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(args) = args {
            command.env(
                CHILD_ARGS_ENV,
                format!("{}\u{1f}{}", args.len(), args.join("\u{1f}")),
            );
        }
        let output = command.output().expect("re-invoke this test binary");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// Runs `run` in a child of this test binary: (status, stdout, stderr).
    fn run_captured(args: &[String]) -> (i32, String, String) {
        spawn_child(CHILD_RUNNER_FILTER, Some(args))
    }

    /// The stdout of a child in which `run` printed nothing.
    fn silent_stdout() -> String {
        spawn_child(SILENT_PROBE_FILTER, None).1
    }

    #[test]
    fn exit_zero_is_one_byte_and_up() {
        let dir = temp_dir("one-byte");
        let one = artefact_path(&dir, "one.bin", b"x");
        assert_eq!(run(&stage_args("compile", 0, &one)), 0);
        let big = artefact_path(&dir, "big.bin", &vec![b'x'; 4096]);
        assert_eq!(run(&stage_args("compile", 0, &big)), 0);
    }

    #[test]
    fn zero_bytes_is_produced_nothing_not_ran() {
        let dir = temp_dir("zero-bytes");
        let empty = artefact_path(&dir, "empty.bin", b"");
        assert_eq!(run(&stage_args("compile", 0, &empty)), 1);
    }

    #[test]
    fn a_missing_artefact_is_produced_nothing_too() {
        let dir = temp_dir("missing");
        let missing = dir.path().join("never-created.bin").display().to_string();
        assert_eq!(run(&stage_args("compile", 0, &missing)), 1);
    }

    #[test]
    fn a_dangling_symlink_is_produced_nothing_not_an_instrument_failure() {
        let dir = temp_dir("dangling");
        let dangling = dangling_symlink(&dir, "dangling.bin");
        assert_eq!(run(&stage_args("compile", 0, &dangling)), 1);
    }

    #[test]
    fn an_unstatable_artefact_exits_one() {
        let dir = temp_dir("unstatable");
        let (under, _) = not_a_directory(&dir);
        assert_eq!(run(&stage_args("compile", 0, &under)), 1);
    }

    #[test]
    fn any_non_zero_rc_fails_whatever_the_artefact_says() {
        let dir = temp_dir("nonzero-rc");
        let big = artefact_path(&dir, "big.bin", &vec![b'x'; 4096]);
        for rc in [1, 2, 127, -1, -9, i32::MIN] {
            assert_eq!(run(&stage_args("compile", rc, &big)), 1, "rc {rc}");
        }
    }

    #[test]
    fn exit_zero_iff_rc_zero_and_artefact_non_empty() {
        // Clause 6: no other combination of rc and artefact produces exit 0.
        let dir = temp_dir("clause-six");
        let empty = artefact_path(&dir, "empty.bin", b"");
        let one = artefact_path(&dir, "one.bin", b"x");
        let missing = dir.path().join("never-created.bin").display().to_string();
        let dangling = dangling_symlink(&dir, "dangling.bin");
        let (under, _) = not_a_directory(&dir);
        let artefacts = vec![empty, one.clone(), missing, dangling, under];
        for rc in [0, 1, -1] {
            for artefact in &artefacts {
                let expected = i32::from(!(rc == 0 && artefact == &one));
                assert_eq!(
                    run(&stage_args("compile", rc, artefact)),
                    expected,
                    "rc {rc}, artefact {artefact}"
                );
            }
        }
    }

    #[test]
    fn a_directory_artefact_is_classified_not_rejected() {
        // Its metadata reads, and its length is the filesystem's answer, so
        // only the channel is pinned here: the invocation classifies and the
        // status is one of the two classification statuses, never the usage
        // status and never a panic. The length itself is not asserted.
        let dir = temp_dir("directory");
        let status = run(&stage_args("compile", 0, &dir.path().display().to_string()));
        assert!(
            matches!(status, 0 | 1),
            "a directory must classify, got {status}"
        );
    }

    #[test]
    fn an_empty_stage_name_classifies_like_any_other() {
        let dir = temp_dir("empty-name");
        let one = artefact_path(&dir, "one.bin", b"x");
        assert_eq!(run(&stage_args("", 0, &one)), 0);
        assert_eq!(run(&stage_args("", 5, &one)), 1);
    }

    #[test]
    fn a_name_may_begin_with_one_dash_but_not_two() {
        let dir = temp_dir("dash-name");
        let one = artefact_path(&dir, "one.bin", b"x");
        assert_eq!(run(&stage_args("-odd-but-legal", 0, &one)), 0);
        assert_eq!(run(&stage_args("--looks-like-a-flag", 0, &one)), 2);
    }

    #[test]
    fn an_empty_artefact_path_does_not_exist_so_it_produced_nothing() {
        assert_eq!(run(&stage_args("compile", 0, "")), 1);
    }

    #[test]
    fn the_flags_may_appear_in_either_order() {
        let dir = temp_dir("flag-order");
        let one = artefact_path(&dir, "one.bin", b"x");
        let artefact_first = vec![s("compile"), s("--artefact"), one, s("--rc"), s("0")];
        assert_eq!(run(&artefact_first), 0);
    }

    #[test]
    fn every_malformed_input_exits_two() {
        let dir = temp_dir("malformed");
        let artefact = artefact_path(&dir, "a.bin", b"x");
        let bad_inputs = vec![
            vec![],                                                // empty
            vec![s("compile")],                                    // no flags at all
            vec![s("compile"), s("--rc")],                         // flag with no value
            vec![s("compile"), s("--artefact")],                   // flag with no value
            vec![s("compile"), s("--rc"), s("0")],                 // missing --artefact
            vec![s("compile"), s("--artefact"), artefact.clone()], // missing --rc
            vec![
                s("compile"),
                s("--rc"),
                s("nope"),
                s("--artefact"),
                artefact.clone(),
            ],
            vec![
                s("compile"),
                s("--rc"),
                s("1.5"),
                s("--artefact"),
                artefact.clone(),
            ],
            vec![
                s("compile"),
                s("--rc"),
                s("0"),
                s("--rc"),
                s("1"),
                s("--artefact"),
                artefact.clone(),
            ],
            vec![
                s("compile"),
                s("--artefact"),
                artefact.clone(),
                s("--artefact"),
                artefact.clone(),
            ],
            vec![s("compile"), s("--frobnicate")], // unknown flag
            vec![s("compile"), s("--rc=0"), s("--artefact"), artefact.clone()], // = form
            vec![
                s("--lead"),
                s("--rc"),
                s("0"),
                s("--artefact"),
                artefact.clone(),
            ],
            vec![
                s("--"),
                s("--rc"),
                s("0"),
                s("--artefact"),
                artefact.clone(),
            ],
            vec![
                s("compile"),
                s("--rc"),
                s("0"),
                s("--artefact"),
                artefact.clone(),
                s("trailing"),
            ],
        ];
        for bad in &bad_inputs {
            assert_eq!(run(bad), 2, "expected exit 2 for {bad:?}");
        }
    }

    #[test]
    fn a_classifying_invocation_prints_exactly_one_line_naming_the_stage() {
        let dir = temp_dir("exactly-one-line");
        let artefact = artefact_path(&dir, "artefact.bin", b"compiled bytes");
        let baseline = silent_stdout().lines().count();
        let (status, stdout, _) = run_captured(&stage_args("compile", 0, &artefact));
        assert_eq!(status, 0);
        assert_eq!(
            stdout.lines().count(),
            baseline + 1,
            "exactly one line beyond the harness's own output: {stdout:?}"
        );
        assert_eq!(
            lines_naming(&stdout, "compile").len(),
            1,
            "the one line must name the stage: {stdout:?}"
        );
    }

    #[test]
    fn the_three_collapsed_facts_report_apart() {
        // Clauses 3, 4 and 5 in one table, because they are the three facts
        // the shell collapses into one branch: a suite that tested any one of
        // them alone would pass an implementation that collapses the other
        // two, and that collapse is the defect this command exists to remove.
        // The dangling symlink is here too: pinned to the same report as a
        // file that was never created, not to the instrument failure.
        let dir = temp_dir("three-facts");
        let empty = artefact_path(&dir, "empty.bin", b"");
        let missing = dir.path().join("never-created.bin").display().to_string();
        let dangling = dangling_symlink(&dir, "dangling.bin");
        let (unstatable, reason) = not_a_directory(&dir);

        let (empty_status, empty_stdout, _) = run_captured(&stage_args("ship", 0, &empty));
        assert_eq!(empty_status, 1);
        let empty_line = single_naming_line(&empty_stdout, "ship");
        assert!(
            empty_line.to_lowercase().contains("nothing"),
            "an empty artefact must be reported as producing nothing: {empty_line:?}"
        );

        let (missing_status, missing_stdout, _) = run_captured(&stage_args("ship", 0, &missing));
        assert_eq!(missing_status, 1);
        let missing_line = single_naming_line(&missing_stdout, "ship");
        assert!(
            missing_line.to_lowercase().contains("nothing"),
            "a missing artefact must be reported as producing nothing: {missing_line:?}"
        );

        let (dangling_status, dangling_stdout, _) = run_captured(&stage_args("ship", 0, &dangling));
        assert_eq!(dangling_status, 1);
        let dangling_line = single_naming_line(&dangling_stdout, "ship");
        assert!(
            dangling_line.to_lowercase().contains("nothing"),
            "a dangling symlink must be reported as producing nothing: {dangling_line:?}"
        );

        let (unstatable_status, unstatable_stdout, _) =
            run_captured(&stage_args("ship", 0, &unstatable));
        assert_eq!(unstatable_status, 1);
        let unstatable_line = single_naming_line(&unstatable_stdout, "ship");
        assert!(
            unstatable_line.to_lowercase().contains("inspect"),
            "an unstatable artefact must be reported as not inspectable: {unstatable_line:?}"
        );
        assert!(
            unstatable_line.contains(&reason),
            "the operating system's reason {reason:?} must reach the line: {unstatable_line:?}"
        );
    }

    #[test]
    fn a_negative_rc_is_reported_verbatim_in_the_line() {
        let dir = temp_dir("negative-rc");
        let artefact = artefact_path(&dir, "artefact.bin", &vec![b'x'; 4096]);
        let (status, stdout, _) = run_captured(&stage_args("signal", -9, &artefact));
        assert_eq!(status, 1);
        let line = single_naming_line(&stdout, "signal");
        assert!(line.contains("-9"), "the rc must appear verbatim: {line:?}");
    }

    #[test]
    fn malformed_arguments_put_usage_on_stderr_and_nothing_on_stdout() {
        let baseline = silent_stdout().lines().count();

        let (status, stdout, stderr) = run_captured(&[]);
        assert_eq!(status, 2);
        assert!(!stderr.is_empty(), "a usage line was expected on stderr");
        assert_eq!(
            stdout.lines().count(),
            baseline,
            "stdout must carry nothing beyond the harness's own output: {stdout:?}"
        );

        let dir = temp_dir("stderr-usage");
        let artefact = artefact_path(&dir, "a.bin", b"x");
        for bad in [
            vec![s("compile"), s("--rc"), s("0"), s("--artefact")], // flag with no value
            vec![
                s("compile"),
                s("--frobnicate"),
                s("--rc"),
                s("0"),
                s("--artefact"),
                artefact.clone(),
            ],
        ] {
            let (status, stdout, stderr) = run_captured(&bad);
            assert_eq!(status, 2, "expected exit 2 for {bad:?}");
            assert!(!stderr.is_empty(), "a usage line was expected on stderr");
            assert_eq!(
                stdout.lines().count(),
                baseline,
                "stdout must carry nothing beyond the harness's own output: {stdout:?}"
            );
        }
    }
}




