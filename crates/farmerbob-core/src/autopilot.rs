//! Pure decision logic for the autopilot's next action.
//!
//! [`Sighting`] is only the subset of the orchestrator's observations needed by
//! this decision. It does not include memory pressure, provider caps, or the
//! clock, so [`decide`] is the policy over what it is given, not "the"
//! scheduling policy.

/// What the orchestrator can see when it decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sighting {
    /// Agent scopes currently alive.
    pub live_agents: u32,
    /// Dispatcher processes still working, which outlive their agents.
    pub live_dispatchers: u32,
    /// Queued wave filenames, exactly as they appear on disk, in any order.
    pub queued_waves: Vec<String>,
    /// Tasks that have been scored but not carried through the subjective tier.
    pub pipelines_pending: Vec<String>,
}

/// What to do next. This is the closed set of branches in the shell
/// orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Something is still running; do not disturb it.
    Wait {
        /// Number of live agent scopes.
        live_agents: u32,
        /// Number of live dispatcher processes.
        live_dispatchers: u32,
    },
    /// Launch this wave file.
    LaunchWave {
        /// Filename of the wave to launch.
        file: String,
    },
    /// Run this task's pipeline.
    RunPipeline {
        /// Task whose remaining subjective stages should run.
        task: String,
    },
    /// Nothing to do, and nothing running.
    Idle,
}

/// Decide the next action. Pure: no I/O, no clock, no randomness.
pub fn decide(s: &Sighting) -> Action {
    if s.live_agents > 0 || s.live_dispatchers > 0 {
        return Action::Wait {
            live_agents: s.live_agents,
            live_dispatchers: s.live_dispatchers,
        };
    }

    // A pipeline call can block this loop for ten minutes or more. Observed:
    // heartbeat 638s stale with wave21 sitting queued, so a queued wave wins.
    if let Some(file) = s.queued_waves.iter().min_by(|left, right| {
        if natural_less(left, right) {
            std::cmp::Ordering::Less
        } else if natural_less(right, left) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }) {
        return Action::LaunchWave { file: file.clone() };
    }

    match s.pipelines_pending.first() {
        Some(task) => Action::RunPipeline { task: task.clone() },
        None => Action::Idle,
    }
}

/// Compare filenames using natural ordering, so `wave9` precedes `wave10`.
///
/// Non-digit runs are compared lexically. Digit runs are compared by their
/// numeric value without parsing: leading zeroes are removed, then the
/// significant lengths and digit strings are compared. This both defines an
/// order for names without digits and avoids overflow for arbitrarily long
/// digit runs. If numeric values tie, the original digit runs and finally the
/// complete filenames break the tie lexically; consequently `wave9a` precedes
/// `wave9b`.
pub fn natural_less(a: &str, b: &str) -> bool {
    let mut left = Chunks::new(a);
    let mut right = Chunks::new(b);

    loop {
        match (left.next(), right.next()) {
            (None, None) => return a < b,
            (None, Some(_)) => return true,
            (Some(_), None) => return false,
            (Some(left_chunk), Some(right_chunk)) => {
                let ordering = left_chunk.cmp(&right_chunk);
                if ordering != std::cmp::Ordering::Equal {
                    return ordering == std::cmp::Ordering::Less;
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Chunk<'a> {
    Text(&'a str),
    Digits(&'a str),
}

impl<'a> Chunk<'a> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
            (Self::Digits(left), Self::Digits(right)) => compare_digit_runs(left, right),
            (Self::Text(_), Self::Digits(_)) => std::cmp::Ordering::Less,
            (Self::Digits(_), Self::Text(_)) => std::cmp::Ordering::Greater,
        }
    }
}

fn compare_digit_runs(left: &str, right: &str) -> std::cmp::Ordering {
    let left_significant = trim_leading_zeroes(left);
    let right_significant = trim_leading_zeroes(right);

    left_significant
        .len()
        .cmp(&right_significant.len())
        .then_with(|| left_significant.cmp(right_significant))
        .then_with(|| left.cmp(right))
}

fn trim_leading_zeroes(digits: &str) -> &str {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() { "0" } else { trimmed }
}

struct Chunks<'a> {
    rest: &'a str,
}

impl<'a> Chunks<'a> {
    fn new(input: &'a str) -> Self {
        Self { rest: input }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = Chunk<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }

        let is_digit = self.rest.as_bytes()[0].is_ascii_digit();
        let end = match self
            .rest
            .as_bytes()
            .iter()
            .position(|byte| byte.is_ascii_digit() != is_digit)
        {
            Some(index) => index,
            None => self.rest.len(),
        };
        let (chunk, rest) = self.rest.split_at(end);
        self.rest = rest;

        if is_digit {
            Some(Chunk::Digits(chunk))
        } else {
            Some(Chunk::Text(chunk))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Sighting, decide, natural_less};

    fn sighting() -> Sighting {
        Sighting {
            live_agents: 0,
            live_dispatchers: 0,
            queued_waves: Vec::new(),
            pipelines_pending: Vec::new(),
        }
    }

    #[test]
    fn live_agents_block_everything_and_preserve_both_counts() {
        let action = decide(&Sighting {
            live_agents: 3,
            live_dispatchers: 7,
            queued_waves: vec![String::from("wave10.tsv")],
            pipelines_pending: vec![String::from("task")],
        });

        assert_eq!(
            action,
            Action::Wait {
                live_agents: 3,
                live_dispatchers: 7
            }
        );
    }

    #[test]
    fn live_dispatcher_alone_is_busy() {
        let mut sighting = sighting();
        sighting.live_dispatchers = 1;
        sighting.queued_waves.push(String::from("wave1.tsv"));
        sighting.pipelines_pending.push(String::from("task"));

        assert_eq!(
            decide(&sighting),
            Action::Wait {
                live_agents: 0,
                live_dispatchers: 1
            }
        );
    }

    #[test]
    fn max_live_agents_is_still_waiting() {
        let mut sighting = sighting();
        sighting.live_agents = u32::MAX;

        assert_eq!(
            decide(&sighting),
            Action::Wait {
                live_agents: u32::MAX,
                live_dispatchers: 0
            }
        );
    }

    #[test]
    fn quiet_queue_uses_natural_first_wave() {
        let mut sighting = sighting();
        sighting.queued_waves = vec![String::from("wave10.tsv"), String::from("wave9.tsv")];

        assert_eq!(
            decide(&sighting),
            Action::LaunchWave {
                file: String::from("wave9.tsv")
            }
        );
    }

    #[test]
    fn one_wave_is_launched() {
        let mut sighting = sighting();
        sighting.queued_waves.push(String::from("wave1.tsv"));

        assert_eq!(
            decide(&sighting),
            Action::LaunchWave {
                file: String::from("wave1.tsv")
            }
        );
    }

    #[test]
    fn wave_precedes_pipeline_to_avoid_the_638_second_stall() {
        let mut sighting = sighting();
        sighting.queued_waves.push(String::from("wave1.tsv"));
        sighting.pipelines_pending.push(String::from("task"));

        assert_eq!(
            decide(&sighting),
            Action::LaunchWave {
                file: String::from("wave1.tsv")
            }
        );
    }

    #[test]
    fn quiet_pending_pipeline_runs_the_first_task() {
        let mut sighting = sighting();
        sighting.pipelines_pending = vec![String::from("first"), String::from("second")];

        assert_eq!(
            decide(&sighting),
            Action::RunPipeline {
                task: String::from("first")
            }
        );
    }

    #[test]
    fn quiet_empty_sighting_is_idle() {
        assert_eq!(decide(&sighting()), Action::Idle);
    }

    #[test]
    fn natural_order_compares_numeric_runs_without_lexicographic_reordering() {
        assert!(natural_less("wave9.tsv", "wave10.tsv"));
        assert!(!natural_less("wave10.tsv", "wave9.tsv"));
    }

    #[test]
    fn names_without_digits_use_lexical_order() {
        assert!(natural_less("alpha.tsv", "beta.tsv"));
        assert!(!natural_less("beta.tsv", "alpha.tsv"));
    }

    #[test]
    fn equal_numeric_prefixes_compare_their_suffixes() {
        assert!(natural_less("wave9a.tsv", "wave9b.tsv"));
        assert!(!natural_less("wave9b.tsv", "wave9a.tsv"));
    }

    #[test]
    fn oversized_digit_runs_compare_by_length_and_digits() {
        let thirty_digit_nine = "wave999999999999999999999999999999.tsv";
        let thirty_digit_ten = "wave100000000000000000000000000000.tsv";

        assert!(natural_less(thirty_digit_ten, thirty_digit_nine));
        assert!(!natural_less(thirty_digit_nine, thirty_digit_ten));
    }

    #[test]
    fn empty_wave_name_is_safe_and_selectable() {
        let mut sighting = sighting();
        sighting.queued_waves.push(String::new());

        assert_eq!(
            decide(&sighting),
            Action::LaunchWave {
                file: String::new()
            }
        );
    }

    #[test]
    fn empty_name_precedes_a_nonempty_name() {
        assert!(natural_less("", "wave1.tsv"));
        assert!(!natural_less("wave1.tsv", ""));
    }
}
