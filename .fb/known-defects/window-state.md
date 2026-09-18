
## 2026-09-18T14:20:00-07:00 -- ifm-k2-horizon on or-ling-30-flash
ifm-k2-horizon on or-ling-30-flash: two of the three sub-second tests assert `seconds_left >= 1` where the specification pins exactly the value reported when truncation yields zero
WHERE: crates/farmerbob-core/src/window.rs, the sub-second boundary tests
TRIGGER: an implementation that clamps to 1 and then decrements, or that reports 2
EXPECT: the tests fail
ACTUAL: `>= 1` accepts both; only the 1500ms case pins `== 1`
