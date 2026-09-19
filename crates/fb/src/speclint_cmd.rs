//! `fb speclint <spec>...` — refuse a spec that will split the field.
//!
//! Ported from fb-speclint.sh. The line-shape rules already lived in
//! `farmerbob_core::speclint_shape`; the script carried the declaration checks and the
//! reporting, and those are here.

use farmerbob_core::speclint_shape;
use std::fs;
use std::path::Path;

/// What a spec's `fb:` markers say about whether it can be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declarations {
    /// One or more deliverables, which is dispatchable. A task may declare several files
    /// since 2026-09-19, so more than one is no longer a fault.
    Declared(usize),
    /// None at all: the dispatcher cannot check preconditions and the pipeline cannot find
    /// the target.
    None,
}

/// Count the literal `fb:creates` / `fb:modifies` markers.
///
/// The script rejected any spec with more than one, because a task declared exactly one
/// file and a second marker meant a prose example the dispatcher would misread. That rule
/// is now wrong -- a grouped task lists every file it touches -- so the count is reported
/// and only ZERO is a fault.
pub fn declarations(text: &str) -> Declarations {
    let n = text
        .lines()
        .filter(|l| {
            let t = l.trim();
            t.starts_with("<!-- fb:creates ") || t.starts_with("<!-- fb:modifies ")
        })
        .count();
    if n == 0 {
        Declarations::None
    } else {
        Declarations::Declared(n)
    }
}

/// Lint one spec. Returns the lines to print and whether it passed.
pub fn lint_one(name: &str, text: &str) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    for l in speclint_shape::lint(text) {
        out.push(format!("{name}:{}: {}", l.line, describe(&l.rule)));
        out.push(format!(
            "{name}:{}:   > {}",
            l.line,
            l.sentence.trim().chars().take(90).collect::<String>()
        ));
    }
    if declarations(text) == Declarations::None {
        out.push(format!(
            "{name}: no fb:creates/fb:modifies declaration -- the dispatcher cannot check \
             preconditions"
        ));
    }
    let ok = out.is_empty();
    (out, ok)
}

fn describe(rule: &speclint_shape::Rule) -> String {
    match rule {
        speclint_shape::Rule::DelegatedDecision => {
            "delegation-shaped instruction -- pin the answer, or say DELEGATED and add \
             'your tests may not assert on it'"
                .to_string()
        }
        speclint_shape::Rule::DelegatedFormat => {
            "asks the ARM to pin a FORMAT -- the spec must fix it, or every arm fixes a \
             different one"
                .to_string()
        }
    }
}

/// Lint every named spec. Non-zero when any of them would split a field.
pub fn run(specs: &[String]) -> i32 {
    let mut rc = 0;
    for spec in specs {
        let path = Path::new(spec);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| spec.clone());
        match fs::read_to_string(path) {
            Ok(text) => {
                let (lines, ok) = lint_one(&name, &text);
                for l in lines {
                    println!("{l}");
                }
                if !ok {
                    rc = 1;
                }
            }
            Err(_) => {
                eprintln!("{spec}: no such spec");
                rc = 1;
            }
        }
    }
    rc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spec declaring nothing cannot be dispatched, and that is the one declaration fault
    /// left. The script also rejected MORE than one marker; that rule is now wrong, because
    /// a grouped task lists every file it touches.
    #[test]
    fn only_zero_declarations_is_a_fault() {
        assert_eq!(declarations("# no markers\n"), Declarations::None);
        assert_eq!(
            declarations("<!-- fb:creates a.rs -->\n"),
            Declarations::Declared(1)
        );
        assert_eq!(
            declarations("<!-- fb:modifies a.rs -->\n<!-- fb:modifies b.rs -->\n"),
            Declarations::Declared(2)
        );
    }

    /// A marker must start the line. One quoted inside prose is an example, not a
    /// declaration, and counting it is how a dispatcher reads an example as a target.
    #[test]
    fn a_marker_quoted_in_prose_is_not_a_declaration() {
        assert_eq!(
            declarations("the spec says `<!-- fb:creates x.rs -->` somewhere\n"),
            Declarations::None
        );
    }

    /// A clean spec produces no output and passes; a spec missing its declaration fails and
    /// says why. Pinned as a pair -- silence and a complaint must not look alike.
    #[test]
    fn a_clean_spec_is_silent_and_a_faulty_one_is_not() {
        let (lines, ok) = lint_one(
            "good.md",
            "<!-- fb:creates a.rs -->\n# Task\nDo the thing.\n",
        );
        assert!(ok, "{lines:?}");
        assert!(lines.is_empty());
        let (lines, ok) = lint_one("bad.md", "# Task\nDo the thing.\n");
        assert!(!ok);
        assert!(
            lines.iter().any(|l| l.contains("no fb:creates")),
            "{lines:?}"
        );
    }

    /// A delegation-shaped instruction is reported with the offending line quoted, so the
    /// author can see what tripped it rather than being told a rule name.
    #[test]
    fn a_delegated_decision_is_quoted_back() {
        let spec = "<!-- fb:creates a.rs -->\nState which one wins and pin it in a test.\n";
        let (lines, ok) = lint_one("d.md", spec);
        assert!(!ok);
        assert!(
            lines.iter().any(|l| l.contains("delegation-shaped")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("State which one wins")),
            "{lines:?}"
        );
    }
}
