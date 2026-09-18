
// ESCALATED: 2 confirmed finding(s) by critic glm-53-flash, found on ifm-k2-horizon.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_backtick_quotes_ifm_k2_horizon {
    use super::*;

    #[test]
    fn claim_1() {
        // "The mixed boundary is unenforced: an absent backtick fragment beside a present double-quoted one is not refused."
        let assertion = Assertion::Claim {
            target: String::from("target"),
            input: String::from("input"),
            expect: String::from("refused"),
            actual: String::from("\"registry.cmd.get(arm)\" then `Source::launcher`"),
        };
        assert_eq!(
            verify_quotes(
                &assertion,
                "registry.cmd.get(arm)",
                "critic mentions Source::launcher"
            ),
            Some(Rejection::Projected {
                quote: String::from("Source::launcher")
            })
        );
    }

    #[test]
    fn claim_2() {
        // "`verify_all` likewise returns no refusal for a backtick-only projection."
        let assertion = Assertion::Claim {
            target: String::from("target"),
            input: String::from("input"),
            expect: String::from("refused"),
            actual: String::from("\"registry.cmd.get(arm)\" then `Source::launcher`"),
        };
        assert_eq!(
            verify_all(
                &[assertion],
                "registry.cmd.get(arm)",
                "critic mentions Source::launcher"
            ),
            vec![(
                0,
                Rejection::Projected {
                    quote: String::from("Source::launcher")
                }
            )]
        );
    }
}
