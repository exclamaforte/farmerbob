use chrono::{DateTime, NaiveDateTime, Utc};

/// One time-boxed arm, as the registry holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boxed {
    /// The arm's registry name.
    pub arm: String,
    /// The `expires_at` field exactly as written, unparsed.
    pub expires_at: String,
    /// Whether the registry currently has this arm enabled.
    pub enabled: bool,
}

/// What a window is doing at one instant. Exactly these variants and no
/// others. Says nothing about whether the arm is enabled: that is a fact
/// about the registry, not about the clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// The instant has not been reached.
    Open {
        /// Whole seconds remaining. Always strictly positive.
        seconds_left: i64,
    },
    /// The instant has been reached or passed.
    Expired {
        /// Whole seconds since it passed. Zero at the instant itself.
        seconds_over: i64,
    },
    /// `expires_at` could not be read as an instant.
    Unparseable,
}

/// Read one window against a clock. This module has no clock of its own;
/// `now` is the caller's.
pub fn read(expires_at: &str, now: DateTime<Utc>) -> State {
    let deadline = match parse(expires_at) {
        Some(dt) => dt,
        None => return State::Unparseable,
    };
    if deadline < now {
        State::Expired {
            seconds_over: (now - deadline).num_seconds(),
        }
    } else if deadline == now {
        State::Expired { seconds_over: 0 }
    } else {
        let seconds_left = (deadline - now).num_seconds();
        State::Open {
            seconds_left: if seconds_left == 0 { 1 } else { seconds_left },
        }
    }
}

fn parse(s: &str) -> Option<DateTime<Utc>> {
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(ndt, Utc));
    }
    if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(DateTime::from_naive_utc_and_offset(ndt, Utc));
    }
    if let Ok(ndt) = s.parse::<NaiveDateTime>() {
        return Some(DateTime::from_naive_utc_and_offset(ndt, Utc));
    }
    None
}

/// Whether this arm must be disabled now.
///
/// `enabled` is the registry's current setting. An arm that is already
/// disabled needs no action however long ago its window closed.
pub fn must_disable(s: &State, enabled: bool) -> bool {
    enabled && !matches!(s, State::Open { .. })
}

/// One line per arm, for a human reading `fb-status`.
pub fn report(boxed: &[Boxed], now: DateTime<Utc>) -> Vec<String> {
    boxed
        .iter()
        .map(|b| {
            let state = read(&b.expires_at, now);
            format!("{}: {}", b.arm, state_short(&state))
        })
        .collect()
}

fn state_short(s: &State) -> &'static str {
    match s {
        State::Open { .. } => "open",
        State::Expired { .. } => "expired",
        State::Unparseable => "unparseable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn test_read_open() {
        let now = dt("2024-01-01T00:00:00Z");
        assert_eq!(
            read("2024-01-01T00:00:10Z", now),
            State::Open { seconds_left: 10 }
        );
    }

    #[test]
    fn test_read_expired() {
        let now = dt("2024-01-01T00:00:10Z");
        assert_eq!(
            read("2024-01-01T00:00:00Z", now),
            State::Expired { seconds_over: 10 }
        );
    }

    #[test]
    fn test_read_at_instant() {
        let now = dt("2024-01-01T00:00:00Z");
        assert_eq!(
            read("2024-01-01T00:00:00Z", now),
            State::Expired { seconds_over: 0 }
        );
    }

    #[test]
    fn test_read_one_second_before() {
        let now = dt("2024-01-01T00:00:01Z");
        assert_eq!(
            read("2024-01-01T00:00:02Z", now),
            State::Open { seconds_left: 1 }
        );
    }

    #[test]
    fn test_read_one_second_after() {
        let now = dt("2024-01-01T00:00:02Z");
        assert_eq!(
            read("2024-01-01T00:00:01Z", now),
            State::Expired { seconds_over: 1 }
        );
    }

    #[test]
    fn test_read_open_never_zero_seconds_left() {
        let now = dt("2024-01-01T00:00:00.500Z");
        let future = dt("2024-01-01T00:00:01Z");
        let state = read(&future.to_rfc3339(), now);
        assert!(matches!(&state, State::Open { seconds_left } if *seconds_left >= 1));
    }

    #[test]
    fn test_read_unparseable_empty() {
        let now = dt("2024-01-01T00:00:00Z");
        assert_eq!(read("", now), State::Unparseable);
    }

    #[test]
    fn test_read_unparseable_garbage() {
        let now = dt("2024-01-01T00:00:00Z");
        assert_eq!(read("not-a-timestamp", now), State::Unparseable);
    }

    #[test]
    fn test_read_naive_as_utc() {
        let now = dt("2024-01-01T00:00:10Z");
        assert!(matches!(
            &read("2024-01-01T00:00:00", now),
            State::Expired { seconds_over: 10 }
        ));
    }

    #[test]
    fn test_read_with_offset() {
        let now = dt("2024-01-01T05:00:00Z");
        assert_eq!(
            read("2024-01-01T10:00:00+05:00", now),
            State::Expired { seconds_over: 0 }
        );
    }

    #[test]
    fn test_read_past_expired_full_elapsed() {
        let now = dt("2024-01-02T00:00:00Z");
        assert_eq!(
            read("2024-01-01T00:00:00Z", now),
            State::Expired {
                seconds_over: 86400
            }
        );
    }

    #[test]
    fn test_must_disable_enabled_expired() {
        assert!(must_disable(&State::Expired { seconds_over: 5 }, true));
    }

    #[test]
    fn test_must_disable_enabled_open() {
        assert!(!must_disable(&State::Open { seconds_left: 5 }, true));
    }

    #[test]
    fn test_must_disable_enabled_unparseable() {
        assert!(must_disable(&State::Unparseable, true));
    }

    #[test]
    fn test_must_disable_disabled_always_false() {
        assert!(!must_disable(&State::Expired { seconds_over: 0 }, false));
        assert!(!must_disable(&State::Open { seconds_left: 1 }, false));
        assert!(!must_disable(&State::Unparseable, false));
    }

    #[test]
    fn test_report_empty() {
        let now = dt("2024-01-01T00:00:00Z");
        let result: Vec<String> = report(&[], now);
        assert!(result.is_empty());
    }

    #[test]
    fn test_report_one_per_element() {
        let now = dt("2024-01-01T00:00:00Z");
        let boxed = vec![
            Boxed {
                arm: "alpha".into(),
                expires_at: "2024-01-01T00:00:00Z".into(),
                enabled: true,
            },
            Boxed {
                arm: "beta".into(),
                expires_at: "2024-01-02T00:00:00Z".into(),
                enabled: true,
            },
        ];
        let result = report(&boxed, now);
        assert_eq!(result.len(), 2);
        assert!(result[0].contains("alpha"));
        assert!(result[1].contains("beta"));
        assert!(!result[0].is_empty());
        assert!(!result[1].is_empty());
    }

    #[test]
    fn test_report_empty_arm_name() {
        let now = dt("2024-01-01T00:00:00Z");
        let boxed = vec![Boxed {
            arm: "".into(),
            expires_at: "2024-01-01T00:00:00Z".into(),
            enabled: true,
        }];
        let result = report(&boxed, now);
        assert_eq!(result.len(), 1);
        assert!(!result[0].is_empty());
    }

    #[test]
    fn test_read_and_must_disable_together_at_instant() {
        let now = dt("2024-01-01T00:00:00Z");
        let state = read("2024-01-01T00:00:00Z", now);
        assert!(must_disable(&state, true));
        assert!(!must_disable(&state, false));
    }

    #[test]
    fn test_read_and_must_disable_together_open() {
        let now = dt("2024-01-01T00:00:00Z");
        let state = read("2024-01-01T00:01:00Z", now);
        assert!(!must_disable(&state, true));
        assert!(!must_disable(&state, false));
    }

    #[test]
    fn test_state_closed_three_variants() {
        assert!(matches!(
            &State::Open { seconds_left: 0 },
            State::Open { .. }
        ));
        assert!(matches!(
            &State::Expired { seconds_over: 0 },
            State::Expired { .. }
        ));
        assert!(matches!(&State::Unparseable, State::Unparseable));
    }

    #[test]
    fn test_read_no_panic_on_extremes() {
        let now = dt("2024-01-01T00:00:00Z");
        let _ = read("0000-01-01T00:00:00Z", now);
        let _ = read("9999-12-31T23:59:59Z", now);
    }

    #[test]
    fn test_read_sub_second_before_minutes_to_one() {
        let now = dt("2024-01-01T00:00:00.5Z");
        let future = dt("2024-01-01T00:00:01Z");
        let state = read(&future.to_rfc3339(), now);
        assert!(matches!(&state, State::Open { seconds_left } if *seconds_left >= 1));
    }

    #[test]
    fn test_read_1500ms_before_yields_one() {
        let now = dt("2024-01-01T00:00:00.5Z");
        let future = dt("2024-01-01T00:00:02Z");
        let state = read(&future.to_rfc3339(), now);
        if let State::Open { seconds_left } = state {
            assert_eq!(seconds_left, 1);
        } else {
            panic!("expected Open, got {:?}", state);
        }
    }

    #[test]
    fn test_read_500ms_before_is_open_not_expired() {
        let now = dt("2024-01-01T00:00:00.5Z");
        let future = dt("2024-01-01T00:00:01Z");
        let state = read(&future.to_rfc3339(), now);
        assert!(matches!(&state, State::Open { .. }));
        assert!(!matches!(&state, State::Expired { .. }));
    }
}
