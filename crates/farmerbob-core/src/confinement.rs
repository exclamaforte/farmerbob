//! Confinement verified from observation, never assumed.
//!
//! A confinement *request* proves nothing: a scope that never materialised is
//! invisible, because its absence looks exactly like its presence from anywhere
//! except the cgroup itself. These types decide confinement from what was
//! actually seen — one process at a time, with the cgroup path it was really
//! in. The cgroup is the authority on membership; identifying a run's processes
//! by name or command line matches things that are not the run.
//!
//! Pure logic over supplied observations: no `/proc`, no `/sys`, no process
//! inspection happens here.

/// What the harness asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// The systemd unit (scope) the run was supposed to be placed in.
    pub unit: String,
    /// Memory cap requested for the unit, in MiB, when one was set.
    pub memory_max_mib: Option<u64>,
    /// CPU quota requested for the unit, in percent of one CPU, when set.
    pub cpu_quota_pct: Option<u32>,
    /// Maximum task (pid) count requested for the unit, when set.
    pub tasks_max: Option<u32>,
}

/// One process seen during the run, with the cgroup path it was actually in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The observed process id.
    pub pid: u32,
    /// Full unified-hierarchy path, e.g. "/user.slice/.../fb-task--arm-123.scope".
    pub cgroup: String,
}

/// The verdict on a run's confinement, decided only from what was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confinement {
    /// Every observed process was inside the requested unit.
    Verified { unit: String, pids: usize },
    /// Processes were observed, none inside the requested unit. The refactor case.
    Absent { unit: String, found_in: Vec<String> },
    /// Some inside, some outside. Worse than Absent: the budget applied to part of the run.
    Partial { unit: String, inside: usize, outside: usize },
    /// Nothing was observed at all, so nothing can be concluded.
    Unobserved,
}

impl Confinement {
    /// Only `Verified`. `Unobserved` is NOT confined and NOT unconfined.
    pub fn is_verified(&self) -> bool {
        matches!(self, Confinement::Verified { .. })
    }

    /// True when the run's resource figures may be attributed to it. False for everything
    /// except `Verified`, because a figure from a foreign cgroup is not this run's.
    pub fn may_attribute_resources(&self) -> bool {
        self.is_verified()
    }
}

/// A process counts as inside when its cgroup path contains the unit name as a whole
/// path component, delimited by `/` or the ends of the string. An empty unit name
/// matches nothing, so nothing can ever be inside it.
fn inside_unit(unit: &str, cgroup: &str) -> bool {
    !unit.is_empty() && cgroup.split('/').any(|component| component == unit)
}

/// Decide from what was seen. A process counts as inside when its cgroup path CONTAINS the
/// unit name as a path component.
pub fn verify(req: &Request, seen: &[Seen]) -> Confinement {
    if seen.is_empty() {
        return Confinement::Unobserved;
    }
    let inside = seen
        .iter()
        .filter(|process| inside_unit(&req.unit, &process.cgroup))
        .count();
    if inside == seen.len() {
        Confinement::Verified {
            unit: req.unit.clone(),
            pids: seen.len(),
        }
    } else if inside == 0 {
        Confinement::Absent {
            unit: req.unit.clone(),
            found_in: foreign_cgroups(req, seen),
        }
    } else {
        Confinement::Partial {
            unit: req.unit.clone(),
            inside,
            outside: seen.len() - inside,
        }
    }
}

/// Cgroup paths observed that are not the requested unit, deduplicated and sorted. These
/// are the foreign cgroups, and a resource figure read from one is not this run's.
pub fn foreign_cgroups(req: &Request, seen: &[Seen]) -> Vec<String> {
    let mut foreign: Vec<String> = seen
        .iter()
        .filter(|process| !inside_unit(&req.unit, &process.cgroup))
        .map(|process| process.cgroup.clone())
        .collect();
    foreign.sort_unstable();
    foreign.dedup();
    foreign
}

/// Which caps the request omitted. An uncapped dimension is unbounded, and a run that is
/// "confined" on memory alone can still take the machine down on pids.
pub fn uncapped(req: &Request) -> Vec<&'static str> {
    let mut omitted = Vec::new();
    if req.memory_max_mib.is_none() {
        omitted.push("memory_max_mib");
    }
    if req.cpu_quota_pct.is_none() {
        omitted.push("cpu_quota_pct");
    }
    if req.tasks_max.is_none() {
        omitted.push("tasks_max");
    }
    omitted
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: &str = "fb-run--arm-123.scope";
    /// The per-run scope path, had the wrapper actually been applied.
    const INSIDE: &str =
        "/user.slice/user-1000.slice/user@1000.service/app.slice/fb-run--arm-123.scope";
    /// The caller's own cgroup, where an unwrapped run ends up.
    const CALLER: &str = "/user.slice/user-1000.slice/user@1000.service/session-2.scope";

    fn req(unit: &str) -> Request {
        Request {
            unit: unit.to_string(),
            memory_max_mib: Some(2048),
            cpu_quota_pct: Some(200),
            tasks_max: Some(512),
        }
    }

    fn uncapped_req() -> Request {
        Request {
            unit: UNIT.to_string(),
            memory_max_mib: None,
            cpu_quota_pct: None,
            tasks_max: None,
        }
    }

    fn seen(pid: u32, cgroup: &str) -> Seen {
        Seen {
            pid,
            cgroup: cgroup.to_string(),
        }
    }

    #[test]
    fn fb_a_does_not_match_fb_abc_scope() {
        let req = req("fb-a");
        let seen = [seen(1, "/system.slice/fb-abc.scope")];
        assert_eq!(verify(&req, &seen), Confinement::Absent {
            unit: "fb-a".to_string(),
            found_in: vec!["/system.slice/fb-abc.scope".to_string()],
        });
    }

    #[test]
    fn unit_matches_only_as_whole_path_component() {
        let req = req("fb-a");
        let ends_string = [seen(1, "/sys/fs/cgroup/fb-a")];
        let after_leading_slash = [seen(2, "/fb-a")];
        let mid_path = [seen(3, "/user.slice/fb-a/workload")];
        assert_eq!(verify(&req, &ends_string), Confinement::Verified {
            unit: "fb-a".to_string(),
            pids: 1,
        });
        assert!(verify(&req, &after_leading_slash).is_verified());
        assert!(verify(&req, &mid_path).is_verified());
        // Substrings inside a larger component are not whole components.
        let extended = [seen(4, "/user.slice/fb-a-extra")];
        let prefixed = [seen(5, "/user.slice/xfb-a")];
        assert!(!verify(&req, &extended).is_verified());
        assert!(!verify(&req, &prefixed).is_verified());
    }

    #[test]
    fn empty_seen_is_unobserved_not_absent() {
        assert_eq!(verify(&req(UNIT), &[]), Confinement::Unobserved);
        assert_eq!(verify(&uncapped_req(), &[]), Confinement::Unobserved);
        assert!(!verify(&req(UNIT), &[]).is_verified());
    }

    #[test]
    fn mixed_run_is_partial_with_both_counts() {
        let seen = [
            seen(1, INSIDE),
            seen(2, INSIDE),
            seen(3, CALLER),
            seen(4, "/system.slice/stray.service"),
            seen(5, CALLER),
        ];
        assert_eq!(verify(&req(UNIT), &seen), Confinement::Partial {
            unit: UNIT.to_string(),
            inside: 2,
            outside: 3,
        });
    }

    #[test]
    fn may_attribute_resources_only_for_verified() {
        let partial = [seen(1, INSIDE), seen(2, CALLER)];
        assert!(verify(&req(UNIT), &[seen(1, INSIDE)]).may_attribute_resources());
        assert!(!verify(&req(UNIT), &[seen(1, CALLER)]).may_attribute_resources());
        assert!(!verify(&req(UNIT), &partial).may_attribute_resources());
        assert!(!verify(&req(UNIT), &[]).may_attribute_resources());
    }

    #[test]
    fn is_verified_true_only_for_verified() {
        let seen_one_inside = [seen(1, INSIDE)];
        let seen_one_outside = [seen(2, CALLER)];
        let mixed = [seen(1, INSIDE), seen(2, CALLER)];
        assert!(verify(&req(UNIT), &seen_one_inside).is_verified());
        assert!(!verify(&req(UNIT), &seen_one_outside).is_verified());
        assert!(!verify(&req(UNIT), &mixed).is_verified());
        assert!(!verify(&req(UNIT), &[]).is_verified());
    }

    #[test]
    fn verified_reports_unit_and_pid_count() {
        let seen = [seen(7, INSIDE), seen(8, INSIDE), seen(9, INSIDE)];
        assert_eq!(verify(&req(UNIT), &seen), Confinement::Verified {
            unit: UNIT.to_string(),
            pids: 3,
        });
    }

    #[test]
    fn absent_reports_where_processes_were_found() {
        // The refactor case: the wrapper never materialised, so the run sat in the
        // caller's cgroup. One distinct foreign cgroup, so dedup cannot matter here.
        let seen = [seen(10, CALLER), seen(11, CALLER)];
        assert_eq!(verify(&req(UNIT), &seen), Confinement::Absent {
            unit: UNIT.to_string(),
            found_in: vec![CALLER.to_string()],
        });
    }

    #[test]
    fn a_well_formed_request_alone_proves_nothing() {
        // Every cap set, unit name plausible — and no process ever entered the scope.
        let request = req(UNIT);
        let seen = [seen(1, CALLER)];
        let verdict = verify(&request, &seen);
        assert_eq!(verdict, Confinement::Absent {
            unit: UNIT.to_string(),
            found_in: vec![CALLER.to_string()],
        });
        assert!(!verdict.may_attribute_resources());
    }

    #[test]
    fn foreign_cgroups_dedupes_and_sorts() {
        let seen = [
            seen(1, CALLER),
            seen(2, "/system.slice/stray.service"),
            seen(3, CALLER),
            seen(4, "/system.slice/another.service"),
        ];
        assert_eq!(foreign_cgroups(&req(UNIT), &seen), vec![
            "/system.slice/another.service".to_string(),
            "/system.slice/stray.service".to_string(),
            CALLER.to_string(),
        ]);
    }

    #[test]
    fn foreign_cgroups_excludes_the_requested_unit() {
        let seen = [seen(1, INSIDE), seen(2, INSIDE)];
        assert!(foreign_cgroups(&req(UNIT), &seen).is_empty());
        assert!(foreign_cgroups(&req(UNIT), &[]).is_empty());
    }

    #[test]
    fn uncapped_lists_omissions_in_stated_order() {
        assert_eq!(uncapped(&uncapped_req()), vec![
            "memory_max_mib",
            "cpu_quota_pct",
            "tasks_max"
        ]);
        let partial = Request {
            memory_max_mib: Some(1024),
            cpu_quota_pct: None,
            tasks_max: None,
            ..uncapped_req()
        };
        assert_eq!(uncapped(&partial), vec!["cpu_quota_pct", "tasks_max"]);
    }

    #[test]
    fn uncapped_is_empty_when_every_dimension_is_capped() {
        assert!(uncapped(&req(UNIT)).is_empty());
    }

    #[test]
    fn empty_unit_with_processes_seen_is_absent() {
        let seen = [seen(1, INSIDE), seen(2, CALLER)];
        assert_eq!(verify(&req(""), &seen), Confinement::Absent {
            unit: String::new(),
            found_in: vec![INSIDE.to_string(), CALLER.to_string()],
        });
        assert_eq!(verify(&req(""), &[]), Confinement::Unobserved);
    }
}
