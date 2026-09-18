



// ESCALATED: 1 confirmed finding(s) by critic unknown, found on or-nemotron-ultra.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_precondition_or_nemotron_ultra {
    use super::*;

    #[test]
    fn claim_1() {
        // "A tab between the marker word and the path silently turns a well-formed declaration into an unknown marker, so a runnable queued spec is reported `Undeclared` instead of checked."
        assert_eq!(
            check("<!-- fb:creates\ta.rs -->", &[]),
            Precondition::Satisfied
        );
    }
}
