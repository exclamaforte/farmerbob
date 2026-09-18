
## 2026-09-18T13:05:00-07:00 -- gemini-38-flash on glm-53-flash
gemini-38-flash on glm-53-flash: parse returns an EMPTY claim for an entry whose claim line lacks the `<critic> on <subject>: ` prefix, and says nothing
WHERE: crates/farmerbob-core/src/known_defect.rs, Entry::absorb
TRIGGER: parse("## s -- c on j\nThe claim line\nWHERE: x\n")
EXPECT: claim == "The claim line"
ACTUAL: claim == "", which the spec blesses as the degenerate empty-claim case, so the loss is indistinguishable from success

## 2026-09-18T13:05:00-07:00 -- or-nemotron-ultra on glm-53-flash
or-nemotron-ultra on glm-53-flash: a stamp containing ` -- ` misparses the critic, against the spec's own stated boundary
WHERE: crates/farmerbob-core/src/known_defect.rs, Entry::heading -- splits on the FIRST ` -- `
TRIGGER: render("t", "2026-09-18T11:08:04-07:00 -- rerun2", &d) then parse
EXPECT: critic == "or-luna-pro"
ACTUAL: critic == "rerun2 -- or-luna-pro"; rsplit_once(" -- ") is the fix, and both rivals used it
