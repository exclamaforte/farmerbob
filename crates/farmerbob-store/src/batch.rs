//! All-or-nothing batch inserts for runs.
//!
//! [`Store::insert_run`] writes one row and returns. A wave dispatches
//! three to six runs and the caller loops; if the fourth insert fails —
//! a duplicate id, a constraint, a disk error — the first three are
//! already committed, and the wave exists in the database as a partial
//! record no later reader can distinguish from a wave that only ever
//! had three arms. [`insert_runs`] closes that hole: either every run
//! in the batch is committed, or the database is left exactly as it
//! was found.

use farmerbob_core as core;

use crate::{Store, StoreError};

use core::run::Run;

/// What a batch write did.
///
/// There is deliberately no partial-success variant: `committed` is
/// either the full input length or zero, never anything between, and a
/// caller that receives `Ok` knows every row landed while a caller that
/// receives `Err` knows none did. The count exists because callers log
/// it, not because zero and N are not the only values it may hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// How many rows were committed. Equals the input length, or zero.
    pub committed: usize,
}

/// Which element of a batch rejected the write, and why.
///
/// Nothing from a rejected batch was written: the whole transaction was
/// rolled back before this is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Zero-based position in the input slice.
    pub index: usize,
    /// The store's own error, unchanged.
    pub cause: String,
}

/// Insert every run, or none.
///
/// The batch runs inside a single transaction on the store's own
/// connection, and each element is written by the existing
/// [`Store::insert_run`], so batch rows and single-row writes share one
/// table and one row shape; each is immediately visible to the other.
///
/// If any element is rejected — a duplicate id, a failed foreign key,
/// any store error — the transaction is rolled back and the database is
/// left exactly as it was found. No row of the batch remains, including
/// the rows before the failing element, and no transaction is left open
/// on the connection: the store is fully usable afterwards.
///
/// [`Written::committed`] equals `runs.len()` on success, and the error
/// return carries the failing position and cause rather than a count,
/// so there is no partial count anywhere in this API. An empty batch is
/// `Ok(Written { committed: 0 })`: nothing to write is not a failure.
pub fn insert_runs(store: &Store, runs: &[Run]) -> Result<Written, Rejected> {
    if runs.is_empty() {
        return Ok(Written { committed: 0 });
    }

    // The signature takes `&Store`, so `Connection::transaction` (which
    // needs `&mut` to prevent nesting) is not available. Nothing in the
    // crate's public API can hold an open transaction across a call to
    // this function — the only other transactions live inside `open`,
    // `apply_migrations`, and `update_run_state` — so this is always
    // the outermost transaction and the nesting check is not needed.
    let tx = store.conn.unchecked_transaction().map_err(|e| Rejected {
        index: 0,
        cause: StoreError::from(e).to_string(),
    })?;

    // `insert_run` executes on the same connection the transaction
    // borrows, so every statement joins it — which is also what keeps
    // the batch from becoming a second table or a second row shape.
    for (index, run) in runs.iter().enumerate() {
        if let Err(e) = store.insert_run(run) {
            let cause = e.to_string();
            // Roll back explicitly; if the ROLLBACK itself fails, the
            // transaction's default drop behavior rolls it back again,
            // so no transaction is left open on the connection.
            let _ = tx.rollback();
            return Err(Rejected { index, cause });
        }
    }

    tx.commit().map_err(|e| Rejected {
        index: 0,
        cause: StoreError::from(e).to_string(),
    })?;

    Ok(Written {
        committed: runs.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, TimeZone, Utc};
    use core::agent::{Agent, AgentKind};
    use core::ids::{AgentId, RunId, TaskId};
    use core::run::RunState;
    use core::task::Task;
    use farmerbob_core as core;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn make_agent() -> Agent {
        Agent {
            id: AgentId::new(),
            kind: AgentKind::Glm,
            model: "glm-5.3-flash".into(),
            provider: "zai".into(),
        }
    }

    fn make_task() -> Task {
        Task {
            id: TaskId::new(),
            name: "wave-test".into(),
            repo_path: std::path::PathBuf::from("/srv/repo"),
            task_dir: std::path::PathBuf::from("/srv/repo/task"),
        }
    }

    fn make_run(agent_id: AgentId, task_id: TaskId) -> Run {
        Run {
            id: RunId::new(),
            task_id,
            agent_id,
            worktree: std::path::PathBuf::from("/srv/worktrees/r1"),
            branch: "fb/test".into(),
            slot_index: None,
            state: RunState::Queued,
            created_at: ts(0),
            started_at: None,
            ended_at: None,
        }
    }

    /// A run whose `task_id` references no row, so any insert of it
    /// violates the foreign key on `runs.task_id`.
    fn orphan_run(agent_id: AgentId, task_id: TaskId) -> Run {
        Run {
            task_id: TaskId::new(),
            ..make_run(agent_id, task_id)
        }
    }

    fn store_with_agent_and_task() -> (Store, AgentId, TaskId) {
        let store = Store::open_in_memory().unwrap();
        let agent = make_agent();
        let task = make_task();
        store.insert_agent(&agent).unwrap();
        store.insert_task(&task).unwrap();
        (store, agent.id, task.id)
    }

    fn assert_absent(store: &Store, runs: &[Run]) {
        for run in runs {
            assert_eq!(
                store.get_run(run.id).unwrap(),
                None,
                "run {} must not be present after the rollback",
                run.id
            );
        }
    }

    #[test]
    fn empty_batch_is_ok_with_zero_committed() {
        let store = Store::open_in_memory().unwrap();
        let written = insert_runs(&store, &[]).unwrap();
        assert_eq!(written, Written { committed: 0 });
    }

    #[test]
    fn valid_batch_commits_every_row_and_reads_back() {
        let (store, agent_id, task_id) = store_with_agent_and_task();

        let runs = vec![
            make_run(agent_id, task_id),
            Run {
                branch: "fb/wave/1".into(),
                slot_index: Some(1),
                state: RunState::Failed {
                    reason: "boom".into(),
                },
                ..make_run(agent_id, task_id)
            },
            Run {
                branch: "fb/wave/2".into(),
                state: RunState::Succeeded,
                started_at: Some(ts(1)),
                ended_at: Some(ts(2)),
                ..make_run(agent_id, task_id)
            },
        ];

        let written = insert_runs(&store, &runs).unwrap();
        assert_eq!(
            written,
            Written {
                committed: runs.len()
            }
        );

        for run in &runs {
            assert_eq!(store.get_run(run.id).unwrap().as_ref(), Some(run));
        }
    }

    #[test]
    fn single_element_batch_commits_one_row() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let run = make_run(agent_id, task_id);

        let written = insert_runs(&store, std::slice::from_ref(&run)).unwrap();
        assert_eq!(written, Written { committed: 1 });
        assert_eq!(store.get_run(run.id).unwrap(), Some(run));
    }

    #[test]
    fn single_invalid_element_rejects_at_index_zero_and_writes_nothing() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let run = orphan_run(agent_id, task_id);

        let rejected = insert_runs(&store, std::slice::from_ref(&run)).unwrap_err();
        assert_eq!(rejected.index, 0);
        assert!(!rejected.cause.is_empty());
        assert_absent(&store, &[run]);
    }

    #[test]
    fn failure_at_first_element_commits_nothing() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let runs = vec![
            orphan_run(agent_id, task_id),
            make_run(agent_id, task_id),
            make_run(agent_id, task_id),
        ];

        let rejected = insert_runs(&store, &runs).unwrap_err();
        assert_eq!(rejected.index, 0);
        assert_absent(&store, &runs);
    }

    #[test]
    fn failure_at_last_element_leaves_the_valid_rows_before_it_absent() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let runs = vec![
            make_run(agent_id, task_id),
            make_run(agent_id, task_id),
            orphan_run(agent_id, task_id),
        ];

        let rejected = insert_runs(&store, &runs).unwrap_err();
        assert_eq!(rejected.index, runs.len() - 1);
        assert_absent(&store, &runs);
    }

    #[test]
    fn duplicate_run_in_batch_commits_neither_copy() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let run = make_run(agent_id, task_id);

        let rejected = insert_runs(&store, &[run.clone(), run.clone()]).unwrap_err();
        assert_eq!(rejected.index, 1);
        assert_absent(&store, &[run]);
    }

    #[test]
    fn cause_matches_the_single_row_duplicate_error_unchanged() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let run = make_run(agent_id, task_id);
        store.insert_run(&run).unwrap();

        let direct = store.insert_run(&run).unwrap_err().to_string();
        let rejected = insert_runs(&store, std::slice::from_ref(&run)).unwrap_err();
        assert_eq!(rejected.index, 0);
        assert_eq!(rejected.cause, direct);
    }

    #[test]
    fn cause_matches_the_single_row_foreign_key_error_unchanged() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let good = make_run(agent_id, task_id);
        let bad = orphan_run(agent_id, task_id);

        let direct = store.insert_run(&bad).unwrap_err().to_string();
        let rejected = insert_runs(&store, &[good, bad]).unwrap_err();
        assert_eq!(rejected.index, 1);
        assert_eq!(rejected.cause, direct);
    }

    #[test]
    fn store_stays_usable_after_a_rejection() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let runs = vec![make_run(agent_id, task_id), orphan_run(agent_id, task_id)];

        assert_eq!(insert_runs(&store, &runs).unwrap_err().index, 1);

        // The same store accepts a single-row write immediately after.
        let survivor = make_run(agent_id, task_id);
        store.insert_run(&survivor).unwrap();
        assert_eq!(store.get_run(survivor.id).unwrap(), Some(survivor));

        // And another batch.
        let second_wave = vec![make_run(agent_id, task_id)];
        assert_eq!(
            insert_runs(&store, &second_wave).unwrap(),
            Written { committed: 1 }
        );
    }

    #[test]
    fn earlier_batch_rows_survive_a_later_batch_rejection() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let first = vec![make_run(agent_id, task_id), make_run(agent_id, task_id)];
        assert_eq!(
            insert_runs(&store, &first).unwrap(),
            Written {
                committed: first.len()
            }
        );

        let second = vec![make_run(agent_id, task_id), orphan_run(agent_id, task_id)];
        assert_eq!(insert_runs(&store, &second).unwrap_err().index, 1);

        for run in &first {
            assert!(store.get_run(run.id).unwrap().is_some());
        }
        assert_absent(&store, &second);
    }

    #[test]
    fn batch_rows_and_single_row_writes_see_the_same_table() {
        let (store, agent_id, task_id) = store_with_agent_and_task();
        let r1 = make_run(agent_id, task_id);
        let r2 = make_run(agent_id, task_id);

        insert_runs(&store, &[r1.clone(), r2.clone()]).unwrap();

        // The existing single-row API sees batch rows, conflicts included.
        assert!(
            store.insert_run(&r1).is_err(),
            "insert_run must see the row the batch wrote"
        );
        assert_eq!(store.list_runs().unwrap().len(), 2);

        // And a batch sees rows written by the single-row API.
        let r3 = make_run(agent_id, task_id);
        store.insert_run(&r3).unwrap();
        let rejected = insert_runs(&store, std::slice::from_ref(&r3)).unwrap_err();
        assert_eq!(rejected.index, 0);
        assert_eq!(store.get_run(r3.id).unwrap(), Some(r3));
    }
}
