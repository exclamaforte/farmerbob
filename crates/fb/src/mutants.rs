//! `fb mutants <file> <task>` — generate a defect set mechanically.
//!
//! Gives `farmerbob_core::mutate` its caller, and unblocks the measurement this harness has
//! wanted most. DefectSensitivity is the adjudicator's third criterion, ranked above
//! Survival, and the only DIRECT evidence of suite quality — everything else is a proxy. It
//! has been measurable for ONE task out of forty-eight, because `fb-defects.sh` needs a
//! hand-written defect set at `.fb/defects/<task>/` and nobody writes them. Every recent
//! adjudication has printed "NEVER MEASURED for any candidate: DefectSensitivity".
//!
//! The patch format `fb-defects.sh` consumes is `@@@ old ---> new`, applied with
//! `replace(old, new, 1)` — the FIRST occurrence. A mutant's own text is rarely unique: a
//! bare `<` appears dozens of times in a file, so writing `@@@ < ---> <=` would mutate the
//! wrong site and measure nothing. Each mutant therefore carries its whole CONTAINING LINE as
//! context, and a mutant whose line is not unique in the file is skipped rather than emitted
//! with an anchor that might land anywhere.
//!   (bead farmerbob-jd2.11)

use std::fs;

use farmerbob_core::mutate::{apply, mutants, Mutant};

/// How many defects to emit. Each one costs a full suite run per candidate, so the whole
/// field pays `defects x candidates` cargo cycles; thirty is minutes, not hours.
const MAX_DEFECTS: usize = 30;

/// The line containing `offset`, and its byte range.
fn line_at(src: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > src.len() {
        return None;
    }
    let start = src[..offset].rfind('\n').map_or(0, |i| i + 1);
    let end = src[offset..].find('\n').map_or(src.len(), |i| offset + i);
    Some((start, end))
}

/// A mutant expressed as a patch, or `None` when it cannot be anchored unambiguously.
fn patch_for(src: &str, m: &Mutant) -> Option<(String, String)> {
    let (ls, le) = line_at(src, m.byte_offset)?;
    let line = &src[ls..le];
    if line.trim().is_empty() {
        return None;
    }
    // The anchor must identify exactly one site. `replace(.., 1)` takes the first match, so a
    // line appearing twice would silently mutate the earlier one.
    if src.matches(line).count() != 1 {
        return None;
    }
    let mutated_whole = apply(src, m)?;
    let (ms, me) = line_at(&mutated_whole, m.byte_offset)?;
    let mutated_line = &mutated_whole[ms..me];
    (mutated_line != line).then(|| (line.to_string(), mutated_line.to_string()))
}

pub fn run_cmd(file: &str, task: &str, max: usize) -> i32 {
    let src = match fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read {file}: {e}");
            return 2;
        }
    };
    let all = mutants(&src);
    if all.is_empty() {
        // Not "zero defects generated, sensitivity 0". Nothing was measurable here.
        eprintln!("no mutable sites in {file} -- nothing to measure, and no defect set written");
        return 3;
    }

    let dir = crate::paths::repo().join(".fb/defects").join(task);
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("error: cannot create {}: {e}", dir.display());
        return 2;
    }

    let cap = if max == 0 { MAX_DEFECTS } else { max };
    let mut written = 0usize;
    let mut skipped = 0usize;
    // Spread the sample across the file rather than taking the first N, which would all land
    // in the imports and the first function.
    let stride = (all.len() / cap).max(1);
    for m in all.iter().step_by(stride) {
        if written >= cap {
            break;
        }
        let Some((old, new)) = patch_for(&src, m) else {
            skipped += 1;
            continue;
        };
        let name = format!("{:03}-{}.patch", written, m.id.replace(['@', ':'], "_"));
        let body = format!("@@@\n{old}\n--->\n{new}\n");
        if fs::write(dir.join(&name), body).is_ok() {
            written += 1;
        }
    }

    println!(
        "{written} defect(s) written to {} from {} mutable site(s); {skipped} skipped as \
         un-anchorable",
        dir.display(),
        all.len()
    );
    if written == 0 {
        eprintln!("every mutant was un-anchorable -- no defect set written");
        return 3;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use farmerbob_core::mutate::MutationKind;

    fn m(offset: usize, before: &str, after: &str) -> Mutant {
        Mutant {
            id: format!("ComparisonBoundary@{offset}"),
            kind: MutationKind::ComparisonBoundary,
            byte_offset: offset,
            before: before.into(),
            after: after.into(),
        }
    }

    #[test]
    fn a_patch_carries_the_whole_line_so_it_anchors_uniquely() {
        let src = "fn a() {}\nif x < y { g() }\nfn b() {}\n";
        let off = src.find('<').expect("site");
        let (old, new) = patch_for(src, &m(off, "<", "<=")).expect("anchorable");
        assert_eq!(old, "if x < y { g() }");
        assert_eq!(new, "if x <= y { g() }");
    }

    /// The anchor must identify ONE site. fb-defects applies with replace(.., 1), so a
    /// duplicated line would silently mutate the earlier occurrence and measure nothing.
    #[test]
    fn a_duplicated_line_is_skipped_rather_than_anchored_ambiguously() {
        let src = "if x < y {}\nlet q = 1;\nif x < y {}\n";
        let second = src.rfind('<').expect("site");
        assert!(patch_for(src, &m(second, "<", "<=")).is_none());
    }

    #[test]
    fn line_at_handles_the_first_and_last_lines() {
        let src = "alpha\nbeta";
        assert_eq!(line_at(src, 0).map(|(s, e)| &src[s..e]), Some("alpha"));
        assert_eq!(line_at(src, 7).map(|(s, e)| &src[s..e]), Some("beta"));
        assert_eq!(line_at(src, 999), None);
    }

    #[test]
    fn a_mutation_that_changes_nothing_on_the_line_is_not_a_patch() {
        let src = "let n = 0;\n";
        // An offset with no real edit produces no patch rather than an empty one.
        assert!(patch_for(src, &m(0, "zzz", "zzz")).is_none());
    }
}
