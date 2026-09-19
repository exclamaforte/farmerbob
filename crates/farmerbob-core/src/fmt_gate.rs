//! Attribution of formatter damage: may this base be dispatched, and when a
//! run departs its scope, who does each departure belong to?
//!
//! `cargo fmt` is ordinary Rust practice. When the base a run started from was
//! already rustfmt-dirty, an agent that runs the formatter rewrites files it
//! never meant to touch, and the scope check counts each one as a departure
//! that is not the agent's fault. This module provides the two decisions that
//! distinction needs: whether a base may be dispatched at all, and — after a
//! run — which departures belong to the agent and which to the base.
//!
//! The module runs no formatter and reads no files. A caller runs
//! `cargo fmt --check` and brings the answer here as a list of paths. Paths
//! are compared as exact strings, the same way [`crate::scope`] compares them;
//! nothing is normalised, canonicalised or resolved.

/// Whether a base may be handed to an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatchable {
    /// Nothing on this base would be rewritten by the formatter. An agent may
    /// run it and touch only its own file.
    Yes,
    /// These paths would be rewritten by the formatter, in the order given.
    /// An agent that runs it departs its scope through no fault of its own.
    No {
        /// The unformatted paths, in the order the caller gave them.
        dirty: Vec<String>,
    },
}

/// Whether a base is safe to dispatch against.
///
/// `dirty` is the set of paths `cargo fmt --check` reports, as the caller
/// obtained it. This module does not run the formatter.
pub fn dispatchable(dirty: &[String]) -> Dispatchable {
    if dirty.is_empty() {
        Dispatchable::Yes
    } else {
        Dispatchable::No {
            dirty: dirty.to_vec(),
        }
    }
}

/// Who a scope departure belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blame {
    /// The agent changed this file for its own reasons.
    Agent,
    /// The file was already unformatted on the base the agent was given.
    /// Running an ordinary formatter rewrites it, so the change is the
    /// base's doing, not the agent's.
    BaseWasUnformatted,
}

/// One departure, with its blame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attributed {
    /// The departed path, exactly as the caller gave it.
    pub path: String,
    /// Who it belongs to.
    pub blame: Blame,
}

/// Split a run's scope departures between the agent and the base it was given.
///
/// `departures` are the paths the scope check flagged. `dirty_on_base` are the
/// paths the formatter would have rewritten on the base BEFORE the run.
pub fn attribute(departures: &[String], dirty_on_base: &[String]) -> Vec<Attributed> {
    departures
        .iter()
        .map(|path| Attributed {
            path: path.clone(),
            blame: if dirty_on_base.contains(path) {
                Blame::BaseWasUnformatted
            } else {
                Blame::Agent
            },
        })
        .collect()
}

/// How many departures the agent is answerable for.
///
/// This is the number a verdict should key on. `attribute(..).len()` is the
/// number to report, and they are different figures on purpose.
pub fn agent_departures(attributed: &[Attributed]) -> u32 {
    attributed
        .iter()
        .filter(|entry| entry.blame == Blame::Agent)
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    #[test]
    fn dispatchable_empty_dirty_set_is_yes() {
        assert_eq!(dispatchable(&[]), Dispatchable::Yes);
    }

    #[test]
    fn dispatchable_one_dirty_path_is_no_with_that_path() {
        assert_eq!(
            dispatchable(&strings(&["a.rs"])),
            Dispatchable::No {
                dirty: vec!["a.rs".to_string()],
            }
        );
    }

    #[test]
    fn dispatchable_carries_dirty_unchanged_in_caller_order() {
        // "Unchanged and unsorted" pins the caller's list verbatim: the order
        // is not sorted and a repeated path is not merged away.
        let dirty = strings(&["b.rs", "a.rs", "b.rs"]);
        assert_eq!(
            dispatchable(&dirty),
            Dispatchable::No {
                dirty: dirty.clone()
            }
        );
    }

    #[test]
    fn attribute_distinguishes_base_blame_from_agent_blame_on_one_input() {
        // The headline, pinned on a single call whose departures include both
        // kinds: one path the base left unformatted, one it did not.
        let attributed = attribute(
            &strings(&["base_left_dirty.rs", "agent_chose.rs"]),
            &strings(&["base_left_dirty.rs"]),
        );
        assert_eq!(attributed.len(), 2);
        assert_eq!(attributed[0].path, "base_left_dirty.rs");
        assert_eq!(attributed[0].blame, Blame::BaseWasUnformatted);
        assert_eq!(attributed[1].path, "agent_chose.rs");
        assert_eq!(attributed[1].blame, Blame::Agent);
    }

    #[test]
    fn agent_departures_is_zero_when_every_departure_belongs_to_the_base() {
        // The two figures differ on purpose: the total is non-zero while the
        // agent is answerable for nothing.
        let departures = strings(&["one.rs", "two.rs"]);
        let attributed = attribute(&departures, &departures);
        assert!(!attributed.is_empty());
        assert_eq!(agent_departures(&attributed), 0);
    }

    #[test]
    fn agent_departures_counts_only_agent_entries() {
        let attributed = attribute(
            &strings(&["base.rs", "mine.rs", "also_mine.rs"]),
            &strings(&["base.rs"]),
        );
        assert_eq!(agent_departures(&attributed), 2);
    }

    #[test]
    fn dirty_path_the_agent_never_touched_contributes_nothing() {
        // "b.rs" is dirty on the base but not departed: not an entry, not a
        // count, not an error.
        let attributed = attribute(&strings(&["a.rs"]), &strings(&["a.rs", "b.rs"]));
        assert_eq!(attributed.len(), 1);
        assert_eq!(attributed[0].blame, Blame::BaseWasUnformatted);
        assert_eq!(agent_departures(&attributed), 0);
    }

    #[test]
    fn no_departures_means_nothing_to_attribute() {
        assert!(attribute(&[], &strings(&["a.rs"])).is_empty());
    }

    #[test]
    fn empty_base_dirty_set_blames_every_departure_on_the_agent() {
        let attributed = attribute(&strings(&["a.rs"]), &[]);
        assert_eq!(attributed.len(), 1);
        assert_eq!(attributed[0].path, "a.rs");
        assert_eq!(attributed[0].blame, Blame::Agent);
        assert_eq!(agent_departures(&attributed), 1);
    }

    #[test]
    fn both_inputs_empty_gives_nothing_and_zero() {
        let attributed = attribute(&[], &[]);
        assert!(attributed.is_empty());
        assert_eq!(agent_departures(&attributed), 0);
    }

    #[test]
    fn departure_listed_twice_yields_two_entries_attributed_alike() {
        // One entry PER DEPARTURE, not per distinct path.
        let attributed = attribute(&strings(&["a.rs", "a.rs"]), &strings(&["a.rs"]));
        assert_eq!(
            attributed,
            vec![
                Attributed {
                    path: "a.rs".to_string(),
                    blame: Blame::BaseWasUnformatted,
                },
                Attributed {
                    path: "a.rs".to_string(),
                    blame: Blame::BaseWasUnformatted,
                },
            ]
        );
        assert_eq!(agent_departures(&attributed), 0);
    }

    #[test]
    fn dirty_on_base_listed_twice_is_still_just_membership() {
        let departures = strings(&["a.rs"]);
        assert_eq!(
            attribute(&departures, &strings(&["a.rs", "a.rs"])),
            attribute(&departures, &strings(&["a.rs"])),
        );
    }

    #[test]
    fn paths_differing_by_case_or_dot_slash_prefix_are_different_paths() {
        // Exact string comparison: neither "./a.rs" nor "A.rs" is "a.rs", so
        // neither is blamed on the base.
        let attributed = attribute(&strings(&["./a.rs", "A.rs"]), &strings(&["a.rs"]));
        assert_eq!(attributed[0].blame, Blame::Agent);
        assert_eq!(attributed[1].blame, Blame::Agent);
        assert_eq!(agent_departures(&attributed), 2);
    }

    #[test]
    fn empty_string_is_an_ordinary_path() {
        let attributed = attribute(&strings(&[""]), &strings(&[""]));
        assert_eq!(attributed.len(), 1);
        assert_eq!(attributed[0].path, "");
        assert_eq!(attributed[0].blame, Blame::BaseWasUnformatted);
        assert_eq!(agent_departures(&attributed), 0);
    }

    #[test]
    fn repeated_calls_with_the_same_inputs_give_the_same_answers() {
        let dirty = strings(&["b.rs", "a.rs"]);
        let departures = strings(&["a.rs", "c.rs"]);
        assert_eq!(dispatchable(&dirty), dispatchable(&dirty));
        assert_eq!(
            attribute(&departures, &dirty),
            attribute(&departures, &dirty),
        );
        let attributed = attribute(&departures, &dirty);
        assert_eq!(agent_departures(&attributed), agent_departures(&attributed),);
    }
}
