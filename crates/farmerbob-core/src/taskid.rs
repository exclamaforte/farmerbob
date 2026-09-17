//! Content-addressed task identity.
//!
//! A task's id is derived from its *normalised* specification text, so re-running the same
//! spec lands on the same task and accumulates history. That is what makes a leaderboard
//! mean anything: "arm A beat arm B on task X" is only a claim if X is the same object next
//! week. Anonymous one-off ids would give a pleasant ergonomic and a leaderboard with a
//! sample size of one everywhere.
//!
//! Normalisation exists so that a trivially reformatted spec still collides onto one task.
//! It deliberately does **not** lowercase and does **not** collapse internal runs of spaces:
//! both change meaning inside a specification, and two specs that differ in what they ask for
//! must not share a posterior.

/// Normalise specification text prior to hashing.
///
/// Strips a leading BOM, converts CRLF and lone CR to LF, removes trailing whitespace from
/// every line, drops leading and trailing blank lines, and collapses runs of three or more
/// blank lines to one.
pub fn normalise(spec: &str) -> String {
    let spec = spec.strip_prefix('\u{feff}').unwrap_or(spec);
    let unified = spec.replace("\r\n", "\n").replace('\r', "\n");

    let trimmed: Vec<&str> = unified.lines().map(|l| l.trim_end()).collect();

    let first = trimmed.iter().position(|l| !l.is_empty());
    let last = trimmed.iter().rposition(|l| !l.is_empty());
    let (first, last) = match (first, last) {
        (Some(f), Some(l)) => (f, l),
        // nothing but whitespace: normalise every such spec to the same empty text
        _ => return String::new(),
    };

    // A run of 3+ blank lines collapses to exactly one. Runs of 1 or 2 are left alone:
    // a single blank separates paragraphs and a double is sometimes deliberate structure.
    // The run has to be buffered rather than counted inline, or a run of four leaves two.
    let mut out: Vec<&str> = Vec::with_capacity(last - first + 1);
    let mut run = 0usize;
    for line in &trimmed[first..=last] {
        if line.is_empty() {
            run += 1;
            continue;
        }
        match run {
            0 => {}
            1..=2 => out.extend(std::iter::repeat_n("", run)),
            _ => out.push(""),
        }
        run = 0;
        out.push(line);
    }
    out.join("\n")
}

/// FNV-1a, 128-bit. Implemented here rather than taken as a dependency so the id is stable
/// across machines and across dependency upgrades — a task id that changes when a crate is
/// bumped would silently fork every task's history.
fn fnv1a_128(bytes: &[u8]) -> u128 {
    const OFFSET: u128 = 0x6c62272e07bb014262b821756295c58d;
    const PRIME: u128 = 0x0000000001000000000000000000013b;
    let mut hash = OFFSET;
    for b in bytes {
        hash ^= *b as u128;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// The task id for a specification: `t-` followed by 10 lowercase hex characters.
pub fn task_id(spec: &str) -> String {
    let digest = fnv1a_128(normalise(spec).as_bytes());
    // take the high bits: the low bits of FNV-1a are the least mixed
    format!("t-{:010x}", (digest >> 88) as u64 & 0xff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = "# Implement the lease manager\n\nIt must be FIFO.\n";

    #[test]
    fn identical_text_gives_an_identical_id() {
        assert_eq!(task_id(SPEC), task_id(SPEC));
    }

    #[test]
    fn trailing_whitespace_does_not_change_the_id() {
        let noisy = "# Implement the lease manager   \n\nIt must be FIFO.\t\n";
        assert_eq!(task_id(SPEC), task_id(noisy));
    }

    #[test]
    fn line_endings_do_not_change_the_id() {
        let crlf = SPEC.replace('\n', "\r\n");
        let cr = SPEC.replace('\n', "\r");
        assert_eq!(task_id(SPEC), task_id(&crlf));
        assert_eq!(task_id(SPEC), task_id(&cr));
    }

    #[test]
    fn surrounding_blank_lines_do_not_change_the_id() {
        let padded = format!("\n\n\n{SPEC}\n\n  \n");
        assert_eq!(task_id(SPEC), task_id(&padded));
    }

    #[test]
    fn a_leading_bom_does_not_change_the_id() {
        assert_eq!(task_id(SPEC), task_id(&format!("\u{feff}{SPEC}")));
    }

    #[test]
    fn long_blank_runs_collapse_but_a_single_blank_is_significant_structure() {
        let a = "one\n\n\n\n\ntwo\n";
        let b = "one\n\ntwo\n";
        assert_eq!(task_id(a), task_id(b));
    }

    #[test]
    fn one_changed_character_changes_the_id() {
        let changed = SPEC.replace("FIFO", "LIFO");
        assert_ne!(task_id(SPEC), task_id(&changed));
    }

    #[test]
    fn case_is_significant_because_it_is_significant_in_a_spec() {
        assert_ne!(task_id("Return None"), task_id("return none"));
    }

    #[test]
    fn internal_spacing_is_significant() {
        // `a  b` and `a b` can mean different things in code fences and tables
        assert_ne!(task_id("fn f(a  b)"), task_id("fn f(a b)"));
    }

    #[test]
    fn distinct_specs_do_not_collide() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..2000 {
            assert!(
                seen.insert(task_id(&format!("task number {i}"))),
                "collision at {i}"
            );
        }
    }

    #[test]
    fn the_id_is_well_formed() {
        let id = task_id(SPEC);
        assert!(id.starts_with("t-"));
        assert_eq!(id.len(), 12);
        assert!(
            id[2..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn whitespace_only_specs_normalise_to_the_same_thing() {
        assert_eq!(task_id("   \n\n  \t\n"), task_id(""));
    }
}
