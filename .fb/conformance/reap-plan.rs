


// ESCALATED: 1 confirmed finding(s) by critic gemini-38-flash, found on glm-53-flash.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_reap_plan_glm_53_flash {
    use super::*;

    #[test]
    fn claim_1() {
        // "A registered directory held by a live run is scheduled for unregistration and deletion instead of being skipped as InUse when may_unregister is true."
        let wts = [Worktree {
            dir: "t--a",
            registered: true,
        }];

        assert_eq!(
            plan(&wts, &["t--a"], &[], true),
            Plan {
                steps: vec![],
                skipped: vec![("t--a".to_string(), Skip::InUse)],
            }
        );
    }
}
