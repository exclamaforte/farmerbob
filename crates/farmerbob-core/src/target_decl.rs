//! Deliverable declaration parsing for task specifications.
//!
//! Every task specification declares its deliverable file in an HTML comment:
//!
//! ```text
//! <!-- fb:creates crates/farmerbob-core/src/window.rs -->
//! <!-- fb:modifies crates/fb/src/status.rs -->
//! ```
//!
//! This module answers what a spec declares by querying
//! [`crate::precondition`].

use crate::precondition::{self, Requirement};

/// What a spec declares about its deliverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    /// The spec declares a file it CREATES. Carries the path as written.
    Creates(String),
    /// The spec declares a file it MODIFIES. Carries the path as written.
    Modifies(String),
}

/// Why a spec has no usable declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoDeclaration {
    /// No `fb:` marker anywhere outside fenced code.
    Absent,
    /// More than one declaration. Carries every path declared, in the order
    /// they appear. A spec with two deliverables has no single deliverable.
    Ambiguous(Vec<String>),
    /// A marker was found and refused. Carries the 1-based line and the reason.
    Refused {
        /// 1-based line number of the refused marker.
        line: usize,
        /// Human-readable explanation of why the marker was refused.
        reason: String,
    },
}

/// The one deliverable a spec declares.
///
/// Returns `Ok(Declaration)` if exactly one valid deliverable marker is declared
/// outside fenced code blocks and no markers are refused.
///
/// Returns `Err(NoDeclaration)` if no declaration is found, if multiple declarations
/// create ambiguity, or if a marker was refused by precondition validation.
/// Every deliverable a spec declares, in the order written.
///
/// A task may declare several files. [`declared`] answers only for the single-deliverable
/// case and reports two markers as [`NoDeclaration::Ambiguous`], which was correct while a
/// task could declare exactly one file and is not any more: `scope::Declared` has held a
/// SET since 2026-09-19.
///
/// A spec with two markers is two deliverables here, not an error. `Ambiguous` survives in
/// `declared` for callers that genuinely need one path and must refuse rather than pick.
///
/// Returns `Err(NoDeclaration::Absent)` when nothing is declared, and propagates a refused
/// marker unchanged -- a malformed declaration is still malformed however many there are.
pub fn declared_all(spec: &str) -> Result<Vec<Declaration>, NoDeclaration> {
    let rejections = precondition::rejections(spec);
    if let Some((line, rejected)) = rejections.first() {
        return Err(NoDeclaration::Refused {
            line: *line as usize,
            reason: reason_for(*rejected),
        });
    }
    let decls = precondition::declarations(spec);
    if decls.is_empty() {
        return Err(NoDeclaration::Absent);
    }
    Ok(decls
        .iter()
        .map(|d| match d.requirement {
            Requirement::Absent => Declaration::Creates(d.path.clone()),
            Requirement::Present => Declaration::Modifies(d.path.clone()),
        })
        .collect())
}

pub fn declared(spec: &str) -> Result<Declaration, NoDeclaration> {
    let rejections = precondition::rejections(spec);
    if let Some((line, rejected)) = rejections.first() {
        return Err(NoDeclaration::Refused {
            line: *line as usize,
            reason: reason_for(*rejected),
        });
    }

    let decls = precondition::declarations(spec);
    match decls.len() {
        0 => Err(NoDeclaration::Absent),
        1 => {
            let d = &decls[0];
            match d.requirement {
                Requirement::Absent => Ok(Declaration::Creates(d.path.clone())),
                Requirement::Present => Ok(Declaration::Modifies(d.path.clone())),
            }
        }
        _ => {
            let paths = decls.into_iter().map(|d| d.path).collect();
            Err(NoDeclaration::Ambiguous(paths))
        }
    }
}

/// The path, whichever verb carried it.
pub fn path(d: &Declaration) -> &str {
    match d {
        Declaration::Creates(p) => p.as_str(),
        Declaration::Modifies(p) => p.as_str(),
    }
}

/// Whether the declaration requires the file to be ABSENT from the base.
///
/// `Creates` requires absence; `Modifies` requires presence. This is the
/// question `fb-status.sh` asks when it marks a queued spec STALE.
pub fn requires_absent(d: &Declaration) -> bool {
    match d {
        Declaration::Creates(_) => true,
        Declaration::Modifies(_) => false,
    }
}

fn reason_for(rejected: precondition::Rejected) -> String {
    match rejected {
        precondition::Rejected::Tab => "tab in separator where spaces required".to_string(),
        precondition::Rejected::TrailingText => "trailing text after marker closer".to_string(),
        precondition::Rejected::UnknownWord => "unknown marker word".to_string(),
        precondition::Rejected::NoSpaceAfterOpener => {
            "no whitespace between opener and fb:".to_string()
        }
        precondition::Rejected::MultipleMarkers => "multiple markers on a single line".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clause_1_creates_on_first_line() {
        let spec = "<!-- fb:creates crates/a/src/b.rs -->\n# Task heading\nProse text.";
        assert_eq!(
            declared(spec),
            Ok(Declaration::Creates("crates/a/src/b.rs".to_string()))
        );
    }

    #[test]
    fn clause_2_modifies_marker() {
        let spec = "<!-- fb:modifies crates/a/src/b.rs -->\n# Task heading\nProse text.";
        assert_eq!(
            declared(spec),
            Ok(Declaration::Modifies("crates/a/src/b.rs".to_string()))
        );
    }

    #[test]
    fn clause_3_marker_on_later_line() {
        let spec = "# Task title\n\nSome introductory prose.\n\n<!-- fb:creates crates/foo/src/bar.rs -->\n\nMore prose.";
        assert_eq!(
            declared(spec),
            Ok(Declaration::Creates("crates/foo/src/bar.rs".to_string()))
        );
    }

    #[test]
    fn clause_4_fenced_marker_ignored() {
        let spec = "# Title\n\n```\n<!-- fb:creates crates/example/src/lib.rs -->\n```\n\nNo actual declaration.";
        assert_eq!(declared(spec), Err(NoDeclaration::Absent));
    }

    #[test]
    fn clause_5_markerless_spec_is_absent() {
        let spec = "# Title\n\nJust a document with no HTML comment markers anywhere.\n";
        assert_eq!(declared(spec), Err(NoDeclaration::Absent));
    }

    #[test]
    fn clause_6_two_markers_ambiguous() {
        let spec = "<!-- fb:creates crates/first.rs -->\n<!-- fb:modifies crates/second.rs -->\n";
        assert_eq!(
            declared(spec),
            Err(NoDeclaration::Ambiguous(vec![
                "crates/first.rs".to_string(),
                "crates/second.rs".to_string(),
            ]))
        );
    }

    #[test]
    fn clause_7_one_marker_base() {
        let spec = "<!-- fb:creates crates/a/src/b.rs -->\n# Details\n";
        assert_eq!(
            declared(spec),
            Ok(Declaration::Creates("crates/a/src/b.rs".to_string()))
        );
    }

    #[test]
    fn clause_7_two_markers_variant() {
        let spec = "<!-- fb:creates crates/a/src/b.rs -->\n# Details\n<!-- fb:modifies crates/c/src/d.rs -->\n";
        assert_eq!(
            declared(spec),
            Err(NoDeclaration::Ambiguous(vec![
                "crates/a/src/b.rs".to_string(),
                "crates/c/src/d.rs".to_string(),
            ]))
        );
    }

    #[test]
    fn clause_8_path_preserved_unchanged() {
        let d_creates = Declaration::Creates("crates/../a/./b.rs".to_string());
        assert_eq!(path(&d_creates), "crates/../a/./b.rs");

        let d_modifies = Declaration::Modifies("/absolute/path.rs".to_string());
        assert_eq!(path(&d_modifies), "/absolute/path.rs");
    }

    #[test]
    fn clause_9_requires_absent_boolean() {
        let creates = Declaration::Creates("foo.rs".to_string());
        let modifies = Declaration::Modifies("foo.rs".to_string());
        assert!(requires_absent(&creates));
        assert!(!requires_absent(&modifies));
    }

    #[test]
    fn clause_10_unknown_verb_refused_matching_precondition() {
        let spec = "<!-- fb:deletes foo.rs -->";
        let res = declared(spec);
        match res {
            Err(NoDeclaration::Refused { line, reason }) => {
                assert_eq!(line, 1);
                assert!(!reason.is_empty());
            }
            other => panic!("expected Refused, got {:?}", other),
        }
    }

    #[test]
    fn boundary_empty_spec() {
        assert_eq!(declared(""), Err(NoDeclaration::Absent));
    }

    #[test]
    fn boundary_spec_is_only_marker() {
        assert_eq!(
            declared("<!-- fb:creates single.rs -->"),
            Ok(Declaration::Creates("single.rs".to_string()))
        );
    }

    #[test]
    fn boundary_three_markers_ambiguous() {
        let spec =
            "<!-- fb:creates a.rs -->\n<!-- fb:modifies b.rs -->\n<!-- fb:creates c.rs -->\n";
        assert_eq!(
            declared(spec),
            Err(NoDeclaration::Ambiguous(vec![
                "a.rs".to_string(),
                "b.rs".to_string(),
                "c.rs".to_string(),
            ]))
        );
    }

    #[test]
    fn boundary_unterminated_fence() {
        let spec = "```\n<!-- fb:creates inside_unterminated.rs -->\n";
        assert_eq!(declared(spec), Err(NoDeclaration::Absent));
    }

    #[test]
    fn boundary_duplicate_paths_ambiguous() {
        let spec = "<!-- fb:creates a.rs -->\n<!-- fb:creates a.rs -->\n";
        assert_eq!(
            declared(spec),
            Err(NoDeclaration::Ambiguous(vec![
                "a.rs".to_string(),
                "a.rs".to_string(),
            ]))
        );
    }

    #[test]
    fn boundary_marker_with_no_path() {
        let spec = "<!-- fb:creates -->";
        assert_eq!(declared(spec), Err(NoDeclaration::Absent));
    }
}
