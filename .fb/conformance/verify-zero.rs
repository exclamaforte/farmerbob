
// ESCALATED: 1 confirmed finding(s) by critic glm-53-flash, found on or-nemotron-ultra.
// Promoted from an executed proof that passed the reference veto. Provenance is
// recorded so a bad test can be traced and retired.  (bead farmerbob-mqr)
#[cfg(test)]
mod escalated_verify_zero_or_nemotron_ultra {
    use super::*;

    // CLAIM: A passed count above u64::MAX is reported as `Unreadable` instead of saturating at `u32::MAX`, because `parse_u32_saturating` delegates to `u64::parse`, which rejects what it cannot fit.
    #[test]
    fn claim_1() {
        // TRIGGER: 26 digits exceed u64::MAX
        // EXPECT: The spec's saturation clause ("Counts larger than `u32::MAX`: SATURATE at `u32::MAX`. Pinned, not delegated") requires `Passed { ran: u32::MAX }` — the shape is recognised, only the count is huge.
        let output = "test result: ok. 99999999999999999999999999 passed; 0 failed; 0 ignored; 0 measured";
        assert_eq!(read(output), Verified::Passed { ran: u32::MAX });
        assert!(was_checked(&read(output)));
    }
}
