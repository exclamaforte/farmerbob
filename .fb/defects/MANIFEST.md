# Validated defect set — verify-router

Each defect is a single, deliberate change to the MERGED implementation
(`crates/farmerbob-core/src/vrouter.rs`), chosen to violate a requirement the spec states
explicitly. Every one is validated two ways before it counts:

  1. the clean implementation passes farmerbob's conformance suite, and
  2. the mutant FAILS it

A mutation the conformance suite cannot detect is not a validated defect — it may be
semantically inert, or it may expose a gap in the spec. Either way it is excluded from the
denominator rather than counted as a miss.

Detection rate against this set is a VERIFIER's sensitivity. It is the number this project
has none of: with a 70% pass base rate, specificity is nearly free and sensitivity is the
scarce signal.
