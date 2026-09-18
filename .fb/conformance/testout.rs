


// ESCALATED: 1 confirmed finding(s) by critic claude-sonnet, found on codex-luna.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_testout_codex_luna {
    use super::*;

    #[test]
    // claim_1: `has_compile_diagnostic` treats "error[E" and "]:" as independent, non-adjacent
    // substring checks on a line, so a failing test's own captured assertion text that happens to
    // contain both (with no real rustc diagnostic anywhere in the output) is misclassified as a
    // compile failure, discarding genuine failure evidence.
    fn claim_1() {
        let output = r#"running 1 test
test tests::odd_message ... FAILED

failures:

---- tests::odd_message stdout ----
thread 'tests::odd_message' panicked at src/lib.rs:10:5:
assertion failed: msg == "error[E1] unexpected input, got ]: token"
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

failures:
    tests::odd_message

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;

        assert_eq!(
            classify(output),
            Outcome::Failed {
                failed: vec!["tests::odd_message".to_string()],
            }
        );
    }
}

