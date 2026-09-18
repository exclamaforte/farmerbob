
// ESCALATED: 2 confirmed finding(s) by critic gemini-38-flash, found on glm-53-flash.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_marker_grammar_glm_53_flash {
    use super::*;

    #[test]
    fn claim_1() {
        // "A line containing an unknown marker word followed by trailing text after the closer reports Rejected::UnknownWord instead of higher-priority Rejected::TrailingText."
        assert_eq!(
            rejections("<!-- fb:deletes a.rs --> trailing text"),
            vec![(1, Rejected::TrailingText)],
        );
    }

    #[test]
    fn claim_2() {
        // "A line containing an unknown marker word and a tab in the post-word separator reports Rejected::UnknownWord instead of higher-priority Rejected::Tab."
        assert_eq!(
            rejections("<!-- fb:deletes\ta.rs -->"),
            vec![(1, Rejected::Tab)],
        );
    }
}

// ESCALATED: 3 confirmed finding(s) by critic glm-53-flash, found on or-inkling.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_marker_grammar_or_inkling {
    use super::*;

    #[test]
    #[ignore = "the claimed Rejected and rejections API does not exist"]
    fn claim_1() {
        // "The Exact API additions — `Rejected` and `rejections(&str) -> Vec<(u32, Rejected)>` — do not exist."
        let prompt = "<!--fb:creates a.rs -->";
        let _ = prompt;
        // No assertion can be compiled because the claimed reporting API is absent.
    }

    #[test]
    fn claim_2() {
        // "A tab in any of the three separator positions yields a live declaration."
        let prompt = "<!-- fb:creates\ta.rs -->";
        assert!(declarations(prompt).is_empty());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn claim_3() {
        // "The tight form — no space after the opener — is accepted."
        let prompt = "<!--fb:creates a.rs -->";
        assert!(declarations(prompt).is_empty());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }

    #[test]
    fn claim_4() {
        // "Tilde fences are unrecognised; markers inside them are live."
        let prompt = "~~~\n<!-- fb:creates x.rs -->\n~~~";
        assert!(declarations(prompt).is_empty());
        assert_eq!(check(prompt, &[]), Precondition::Undeclared);
    }
}


