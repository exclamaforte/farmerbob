
// ESCALATED: 1 confirmed finding(s) by critic glm-53-flash, found on ifm-k2-think.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_crossx_graft_ifm_k2_think {
    use super::*;

    // CLAIM: `test_body` renames only declarations of the exact form `mod tests`; any other module declared in the test section passes through unrenamed, so grafting a section containing `mod conformance_limit_detect` into a host that also declares it redeclares the module (E0428) — the exact incident this task specifies away.
    #[test]
    fn claim_1() {
        let src = r#"#[cfg(test)]
mod tests {
    // inner
}
mod conformance_limit_detect {
    // limit
}
pub mod x {
    // public
}
"#;
        let body = test_body(src, 3);
        // EXPECT: every module declared in the section renamed under `xtests_<idx>` scheme.
        assert!(body.contains("mod xtests_3_conformance_limit_detect"), "conformance_limit_detect should be renamed");
        assert!(body.contains("pub mod xtests_3_x"), "pub mod x should be renamed keeping pub");
        assert!(!body.contains("mod conformance_limit_detect\n"), "original module name should not appear verbatim");
        assert!(body.contains("mod xtests_3"), "mod tests should become mod xtests_3");
    }
}
