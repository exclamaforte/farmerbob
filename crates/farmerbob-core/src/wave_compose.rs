//! Validation and accounting for a wave manifest.

/// One row of a wave manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The task this row dispatches.
    pub task: String,
    /// The crate this row dispatches against.
    pub crate_name: String,
    /// Arms in manifest order. Repeats are retained for validation and
    /// accounting.
    pub arms: Vec<String>,
    /// The file this task declares, when it declares one.
    pub target: Option<String>,
}

/// A reason a manifest may not be dispatched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// The manifest has no rows.
    Empty,
    /// Two rows name the same task.
    DuplicateTask {
        /// The shared task name.
        task: String,
        /// The matching row indexes, lower first.
        rows: (usize, usize),
    },
    /// Two rows declare the same target.
    TargetCollision {
        /// The shared target.
        target: String,
        /// The matching row indexes, lower first.
        rows: (usize, usize),
    },
    /// One row lists an arm more than once.
    RepeatedArm {
        /// The row containing the repeated arm.
        row: usize,
        /// The repeated arm name.
        arm: String,
    },
    /// A row lists no arms.
    NoArms {
        /// The row without arms.
        row: usize,
    },
}

/// Return every fault in a manifest, in the specified row and kind order.
pub fn faults(rows: &[Row]) -> Vec<Fault> {
    if rows.is_empty() {
        return vec![Fault::Empty];
    }

    let mut result = Vec::new();

    for (row_index, row) in rows.iter().enumerate() {
        for (other_index, other) in rows.iter().enumerate().skip(row_index + 1) {
            if row.task == other.task {
                result.push(Fault::DuplicateTask {
                    task: row.task.clone(),
                    rows: (row_index, other_index),
                });
            }
        }

        for (other_index, other) in rows.iter().enumerate().skip(row_index + 1) {
            if let (Some(target), Some(other_target)) = (&row.target, &other.target)
                && target == other_target
            {
                result.push(Fault::TargetCollision {
                    target: target.clone(),
                    rows: (row_index, other_index),
                });
            }
        }

        let mut repeated_arms = Vec::new();
        for (arm_index, arm) in row.arms.iter().enumerate() {
            if row.arms[..arm_index].iter().any(|previous| previous == arm)
                && !repeated_arms.contains(&arm)
            {
                repeated_arms.push(arm);
            }
        }
        for arm in repeated_arms {
            result.push(Fault::RepeatedArm {
                row: row_index,
                arm: arm.clone(),
            });
        }

        if row.arms.is_empty() {
            result.push(Fault::NoArms { row: row_index });
        }
    }

    result
}

/// Whether a manifest has no faults and may therefore be dispatched.
pub fn dispatchable(rows: &[Row]) -> bool {
    faults(rows).is_empty()
}

/// Count all arms in all rows, including repeated arms.
pub fn run_count(rows: &[Row]) -> usize {
    rows.iter().map(|row| row.arms.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(task: &str, arms: &[&str], target: Option<&str>) -> Row {
        Row {
            task: task.to_string(),
            crate_name: "crate".to_string(),
            arms: arms.iter().map(|arm| (*arm).to_string()).collect(),
            target: target.map(str::to_string),
        }
    }

    #[test]
    fn validates_and_accounts_for_a_manifest() {
        let rows = vec![
            row("same", &["a", "a", "b"], Some("one")),
            row("same", &["c"], Some("one")),
            row("third", &[], None),
        ];

        assert_eq!(
            faults(&rows),
            vec![
                Fault::DuplicateTask {
                    task: "same".to_string(),
                    rows: (0, 1),
                },
                Fault::TargetCollision {
                    target: "one".to_string(),
                    rows: (0, 1),
                },
                Fault::RepeatedArm {
                    row: 0,
                    arm: "a".to_string(),
                },
                Fault::NoArms { row: 2 },
            ]
        );
        assert!(!dispatchable(&rows));
        assert_eq!(run_count(&rows), 4);
    }

    #[test]
    fn reports_all_target_pairs_and_keeps_none_distinct() {
        let rows = vec![
            row("one", &["a"], Some("target")),
            row("two", &["b"], Some("target")),
            row("three", &["c"], Some("target")),
            row("four", &["d"], None),
        ];

        assert_eq!(
            faults(&rows),
            vec![
                Fault::TargetCollision {
                    target: "target".to_string(),
                    rows: (0, 1),
                },
                Fault::TargetCollision {
                    target: "target".to_string(),
                    rows: (0, 2),
                },
                Fault::TargetCollision {
                    target: "target".to_string(),
                    rows: (1, 2),
                },
            ]
        );
    }

    #[test]
    fn empty_manifest_is_empty_and_not_dispatchable() {
        assert_eq!(faults(&[]), vec![Fault::Empty]);
        assert!(!dispatchable(&[]));
        assert_eq!(run_count(&[]), 0);
    }
}
