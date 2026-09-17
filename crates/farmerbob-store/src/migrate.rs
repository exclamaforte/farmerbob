//! Pure planning for forward-only schema migrations.
//!
//! This module decides WHAT to run and in what order; it performs no I/O
//! and opens no database. The caller applies the returned [`Plan`] inside
//! a single transaction so that the DDL and the recorded version land
//! together or not at all.
//!
//! Version 0 means "never migrated" (a fresh database), which is why a
//! migration may not claim it. Versions are strictly positive, unique,
//! and applied in ascending order; a forward-only plan never revisits a
//! version, so a database whose recorded history skips a version is in a
//! state no run of this module's migrations could have produced.

/// One forward-only schema step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// Strictly positive, unique, and applied in ascending order.
    pub version: u32,
    /// Human-readable name, recorded alongside the version when applied.
    pub name: String,
    /// The DDL. Opaque here; this module never parses or executes it.
    pub sql: String,
}

/// Why a migration plan could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// Two migrations share a version.
    DuplicateVersion(u32),
    /// A version of 0, which cannot be distinguished from "never migrated".
    ZeroVersion,
    /// The database is newer than this binary knows how to handle.
    DatabaseIsNewer {
        /// The database's recorded version.
        db: u32,
        /// The highest version this binary knows.
        binary: u32,
    },
    /// A migration is missing from the middle of the sequence the database
    /// has applied.
    GapInHistory {
        /// The lowest version missing from the applied history.
        missing: u32,
    },
}

/// What the caller should execute, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The migrations to apply, ascending by version.
    pub steps: Vec<Migration>,
    /// The version the database is recorded at when the plan starts.
    pub from_version: u32,
    /// The version the database will be at once every step is applied.
    pub to_version: u32,
}

impl Plan {
    /// True when the database is already current. An empty plan is a
    /// legitimate, common outcome and not an error.
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Decide what to run.
///
/// `available` need not be sorted. `current` is the database's recorded
/// version, 0 meaning a fresh database. The plan contains every migration
/// with a version above `current`, ascending, regardless of the order in
/// `available`.
///
/// A `current` above this binary's target is [`PlanError::DatabaseIsNewer`],
/// never an empty plan: an old binary must refuse a newer schema, not
/// silently run against it. When several problems exist the error is
/// reported in this order: [`PlanError::ZeroVersion`], then
/// [`PlanError::DuplicateVersion`] (lowest duplicated version), then
/// [`PlanError::DatabaseIsNewer`].
pub fn plan(available: &[Migration], current: u32) -> Result<Plan, PlanError> {
    validate_set(available)?;
    let target = target_version(available).unwrap_or(0);
    if current > target {
        return Err(PlanError::DatabaseIsNewer {
            db: current,
            binary: target,
        });
    }

    let mut steps: Vec<Migration> = available
        .iter()
        .filter(|m| m.version > current)
        .cloned()
        .collect();
    steps.sort_unstable_by_key(|m| m.version);

    Ok(Plan {
        steps,
        from_version: current,
        to_version: target,
    })
}

/// The highest version this binary knows. `None` when it knows none.
pub fn target_version(available: &[Migration]) -> Option<u32> {
    available.iter().map(|m| m.version).max()
}

/// Verify a database's applied history against this binary's migration set.
///
/// `applied` is the sorted list of versions the database records as applied;
/// entries of 0 are ignored, since 0 records "never migrated". A version in
/// `applied` that this binary does not know is [`PlanError::DatabaseIsNewer`]
/// — with `db` set to the highest applied version — and is checked before
/// gaps, because no forward-only sequence explains such a history anyway.
/// Otherwise every version from 1 up to the highest applied must be present,
/// with [`PlanError::GapInHistory`] naming the lowest that is missing.
pub fn check_history(available: &[Migration], applied: &[u32]) -> Result<(), PlanError> {
    validate_set(available)?;

    let known: std::collections::BTreeSet<u32> = available.iter().map(|m| m.version).collect();
    let applied_set: std::collections::BTreeSet<u32> =
        applied.iter().copied().filter(|v| *v != 0).collect();

    if let Some(db) = applied_set.iter().next_back()
        && !known.contains(db)
    {
        let binary = target_version(available).unwrap_or(0);
        return Err(PlanError::DatabaseIsNewer { db: *db, binary });
    }

    let mut expected: u32 = 1;
    for &version in &applied_set {
        if version > expected {
            return Err(PlanError::GapInHistory { missing: expected });
        }
        expected = version.saturating_add(1);
    }

    Ok(())
}

/// Reject migration sets this module cannot reason about: any version of 0,
/// then any duplicated version (the lowest duplicate is reported).
fn validate_set(available: &[Migration]) -> Result<(), PlanError> {
    if available.iter().any(|m| m.version == 0) {
        return Err(PlanError::ZeroVersion);
    }

    let mut versions: Vec<u32> = available.iter().map(|m| m.version).collect();
    versions.sort_unstable();
    for window in versions.windows(2) {
        if window[0] == window[1] {
            return Err(PlanError::DuplicateVersion(window[0]));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mig(version: u32, name: &str) -> Migration {
        Migration {
            version,
            name: name.to_string(),
            sql: format!("-- {name}"),
        }
    }

    fn versions(steps: &[Migration]) -> Vec<u32> {
        steps.iter().map(|m| m.version).collect()
    }

    #[test]
    fn unsorted_input_plans_in_ascending_order() {
        let available = vec![
            mig(3, "c"),
            mig(1, "a"),
            mig(5, "e"),
            mig(2, "b"),
            mig(4, "d"),
        ];
        let p = plan(&available, 0).unwrap();
        assert_eq!(versions(&p.steps), vec![1, 2, 3, 4, 5]);
        assert_eq!(p.from_version, 0);
        assert_eq!(p.to_version, 5);
    }

    #[test]
    fn plan_carries_names_and_sql_intact() {
        let available = vec![mig(2, "indexes"), mig(1, "tables")];
        let p = plan(&available, 0).unwrap();
        assert_eq!(p.steps[0].name, "tables");
        assert_eq!(p.steps[0].sql, "-- tables");
        assert_eq!(p.steps[1].name, "indexes");
        assert_eq!(p.steps[1].sql, "-- indexes");
    }

    #[test]
    fn plan_only_includes_versions_above_current() {
        let available: Vec<Migration> = (1..=5).map(|v| mig(v, "m")).collect();
        let p = plan(&available, 2).unwrap();
        assert_eq!(versions(&p.steps), vec![3, 4, 5]);
        assert_eq!(p.from_version, 2);
        assert_eq!(p.to_version, 5);
        assert!(!p.is_noop());
    }

    #[test]
    fn already_current_database_plans_nothing() {
        let available: Vec<Migration> = (1..=3).map(|v| mig(v, "m")).collect();
        let p = plan(&available, 3).unwrap();
        assert!(p.steps.is_empty());
        assert!(p.is_noop());
        assert_eq!(p.from_version, 3);
        assert_eq!(p.to_version, 3);
    }

    #[test]
    fn empty_available_with_current_zero_is_noop() {
        let p = plan(&[], 0).unwrap();
        assert!(p.is_noop());
        assert_eq!(p.from_version, 0);
        assert_eq!(p.to_version, 0);
    }

    #[test]
    fn duplicate_version_errors_deterministically() {
        let available = vec![
            mig(5, "e"),
            mig(3, "c"),
            mig(1, "a"),
            mig(3, "c-again"),
            mig(5, "e-again"),
        ];
        assert_eq!(
            plan(&available, 0).unwrap_err(),
            PlanError::DuplicateVersion(3)
        );
    }

    #[test]
    fn zero_version_is_rejected() {
        let available = vec![mig(1, "a"), mig(0, "zero")];
        assert_eq!(plan(&available, 0).unwrap_err(), PlanError::ZeroVersion);
    }

    #[test]
    fn zero_version_rejected_even_when_alone() {
        assert_eq!(
            plan(&[mig(0, "zero")], 0).unwrap_err(),
            PlanError::ZeroVersion
        );
    }

    #[test]
    fn newer_database_is_refused_not_no_opped() {
        let available: Vec<Migration> = (1..=3).map(|v| mig(v, "m")).collect();
        assert_eq!(
            plan(&available, 4).unwrap_err(),
            PlanError::DatabaseIsNewer { db: 4, binary: 3 }
        );
        assert_eq!(
            plan(&available, 99).unwrap_err(),
            PlanError::DatabaseIsNewer { db: 99, binary: 3 }
        );
    }

    #[test]
    fn database_newer_than_empty_binary_set_is_refused() {
        assert_eq!(
            plan(&[], 1).unwrap_err(),
            PlanError::DatabaseIsNewer { db: 1, binary: 0 }
        );
    }

    #[test]
    fn target_version_is_highest_known() {
        let available = vec![mig(7, "g"), mig(3, "c"), mig(5, "e")];
        assert_eq!(target_version(&available), Some(7));
    }

    #[test]
    fn target_version_none_when_nothing_known() {
        assert_eq!(target_version(&[]), None);
    }

    #[test]
    fn complete_and_partial_histories_are_accepted() {
        let available: Vec<Migration> = (1..=3).map(|v| mig(v, "m")).collect();
        assert_eq!(check_history(&available, &[1, 2, 3]), Ok(()));
        assert_eq!(check_history(&available, &[1]), Ok(()));
        assert_eq!(check_history(&available, &[]), Ok(()));
    }

    #[test]
    fn gap_names_the_lowest_missing_version() {
        let available: Vec<Migration> = (1..=4).map(|v| mig(v, "m")).collect();
        assert_eq!(
            check_history(&available, &[1, 3]),
            Err(PlanError::GapInHistory { missing: 2 })
        );
        assert_eq!(
            check_history(&available, &[2]),
            Err(PlanError::GapInHistory { missing: 1 })
        );
        assert_eq!(
            check_history(&available, &[1, 2, 4]),
            Err(PlanError::GapInHistory { missing: 3 })
        );
    }

    #[test]
    fn unknown_applied_version_reads_as_newer_not_as_a_gap() {
        let available: Vec<Migration> = (1..=3).map(|v| mig(v, "m")).collect();
        assert_eq!(
            check_history(&available, &[1, 2, 9]),
            Err(PlanError::DatabaseIsNewer { db: 9, binary: 3 })
        );
    }

    #[test]
    fn unsorted_applied_history_is_still_checked_for_gaps() {
        let available: Vec<Migration> = (1..=3).map(|v| mig(v, "m")).collect();
        assert_eq!(
            check_history(&available, &[3, 1]),
            Err(PlanError::GapInHistory { missing: 2 })
        );
    }

    #[test]
    fn invalid_available_set_is_rejected_by_check_history_too() {
        assert_eq!(
            check_history(&[mig(0, "zero")], &[]),
            Err(PlanError::ZeroVersion)
        );
        assert_eq!(
            check_history(&[mig(2, "b"), mig(2, "b-again")], &[]),
            Err(PlanError::DuplicateVersion(2))
        );
    }
}
