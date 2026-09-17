
// ESCALATED from cross-examination: codex-luna's suite discriminated on resume.
// Not a CLAIM -- cross-examination found it directly. Kept only because it passes
// against the merged winner, which is what separates a discovery from an
// over-fitted suite.
#[cfg(test)]
mod cx_resume_codex_luna {
use std::collections::BTreeMap;
    use super::*;

    fn run(id: &str, bucket: &str, since_ms: u64) -> Parked {
        Parked {
            run_id: id.into(),
            bucket: bucket.into(),
            since_ms,
            resume_at_ms: Some(10),
            attempts: 0,
        }
    }

    fn policy(batch: usize) -> Policy {
        Policy {
            batch,
            stagger_ms: 5,
            default_window_ms: 10,
            max_attempts: 3,
        }
    }

    #[test]
    fn two_buckets_each_release_their_own_batch() {
        let parked = [
            run("a1", "a", 1),
            run("b1", "b", 2),
            run("a2", "a", 3),
            run("b2", "b", 4),
        ];
        assert_eq!(due_now(&parked, policy(1), 10), vec!["a1", "b1"]);
    }

    #[test]
    fn kth_release_is_staggered() {
        let parked = [run("a", "x", 1), run("b", "x", 2)];
        assert_eq!(
            plan(&parked, policy(2), 10),
            vec![
                Decision::Resume {
                    run_id: "a".into(),
                    at_ms: 10
                },
                Decision::Resume {
                    run_id: "b".into(),
                    at_ms: 15
                },
            ]
        );
    }

    #[test]
    fn oldest_first_ordering_has_a_deterministic_tie_break() {
        let parked = [run("z", "x", 2), run("b", "x", 1), run("a", "x", 1)];
        assert_eq!(due_now(&parked, policy(3), 10), vec!["a", "b", "z"]);
    }

    #[test]
    fn exhausted_run_does_not_consume_a_slot() {
        let mut exhausted = run("old", "x", 1);
        exhausted.attempts = 3;
        let parked = [exhausted, run("live", "x", 2)];
        assert_eq!(due_now(&parked, policy(1), 10), vec!["live"]);
    }

    #[test]
    fn future_since_does_not_underflow_and_is_not_due() {
        let parked = [Parked {
            resume_at_ms: None,
            ..run("future", "x", u64::MAX)
        }];
        assert_eq!(
            plan(&parked, policy(1), 0),
            vec![Decision::Wait {
                run_id: "future".into(),
                until_ms: u64::MAX
            }]
        );
    }

    #[test]
    fn due_now_agrees_with_plan() {
        let parked = [run("a", "x", 1), run("b", "x", 2), run("c", "y", 3)];
        let mut expected: Vec<(u64, String)> = plan(&parked, policy(2), 10)
            .into_iter()
            .filter_map(|d| match d {
                Decision::Resume { run_id, at_ms } => Some((at_ms, run_id)),
                _ => None,
            })
            .collect();
        expected.sort_by_key(|release| release.0);
        assert_eq!(
            due_now(&parked, policy(2), 10),
            expected
                .into_iter()
                .map(|(_, run_id)| run_id)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn next_wakeup_is_none_when_all_are_exhausted() {
        let mut parked_run = run("done", "x", 1);
        parked_run.attempts = 3;
        assert_eq!(next_wakeup(&[parked_run], policy(1), 0), None);
    }

    #[test]
    fn batch_zero_behaves_as_one() {
        let parked = [run("a", "x", 1), run("b", "x", 2)];
        assert_eq!(due_now(&parked, policy(0), 10), vec!["a"]);
    }
}
