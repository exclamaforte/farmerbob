<!-- fb:creates crates/fb/src/promote.rs -->
<!-- fb:reads fb-promote.sh -->
# Task: port fb-promote.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/promote.rs` and declare it from `crates/fb/src/main.rs` with
`mod promote;`. Do not change any other file, and do not delete the shell script.

## Why this is being ported

This harness measures AI coding agents. Eleven of the sixteen defects found in it last week
were in its own shell scripts, and every repair that came BACK came back in bash: three
repairs each caused a second occurrence of the bug they fixed. Repairs that changed a *type*
in the Rust crate have not recurred once. Bash cannot express the distinction this harness
depends on most -- "measured zero" versus "not measured" -- because it has an empty string and
a `0` and nothing else.

So this is not a tidying exercise. The specification below is the existing script, and your
job is to reproduce what it does while making the states it confuses *unrepresentable*.

## The specification IS the script

It is reproduced in full at the bottom. Its behaviour is the contract, including its exit
codes and its stdout format, because other scripts parse both. Where a comment in the script
explains WHY something is the way it is, that reason is part of the contract -- those comments
record real incidents and the behaviour they describe must survive the port.

## Non-negotiable: distinguish absent from zero

The crate already has the types for this and you MUST use them rather than invent your own:

- `farmerbob_core::measurement::Measurement<T>` -- `Observed(T)` or `Missing(Absent)`, where
  `Absent` is `NotAttempted`, `InstrumentFailed`, `NothingToMeasure` or `Untrusted`. It has no
  `unwrap`, no `unwrap_or`, no `Default` and no `From<Option>`, deliberately.
- `farmerbob_core::gate::{judge, Observation, Verdict}` for any pass/fail verdict.

Wherever the script computes a number from a command that can fail, the Rust must return
`Measurement`, not a bare value with a sentinel. Every `|| echo 0`, every `2>/dev/null`
swallowing an error, every `${x:-0}` in the script below is a place where a failure currently
becomes a zero. Find them and make each one `Missing` with a stated reason.

## Required: a differential test

Add `#[cfg(test)]` tests, and include at least one that pins the OUTPUT FORMAT byte-for-byte
against a worked example taken from the script -- the exact column widths, the exact literal
words. Other scripts grep this output; a format change is a silent breakage.

You cannot run the shell script from a unit test, so do not try. Pin the format with literal
expected strings.

## Rules

- Public entry point: `pub fn run_cmd(...) -> i32`, returning the process exit code, with
  arguments matching the script's positional parameters in order.
- No `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on any path reachable from
  input, outside `#[cfg(test)]`.
- Available deps in `crates/fb`: farmerbob-core, clap, anyhow, serde, serde_json, toml,
  directories, rusqlite. Add none.
- `cargo build` and `cargo test` must pass for the whole workspace. Run them yourself.
- Do not make parameters generic. Concrete types only.

When done, briefly state what you implemented and which failure-becomes-zero paths you found.

## The script

```bash
#!/usr/bin/env bash
# fb-promote — turn critics' CLAIMs into executed evidence.
#
# A claim is a hypothesis, not a finding. Each is classified before any test is written:
#
#   CONTRADICTED  two independent critics assert OPPOSITE expectations of the same behaviour
#                 -> the SPEC is underdetermined, not the code. Routes to task authoring.
#                    Neither arm is scored. This is SpecAmbiguous, and by the rule the router
#                    itself implements, it outranks a demonstrated defect.
#   CONFIRMATORY  EXPECT == ACTUAL: the critic documented correct behaviour rather than
#                 alleging a defect. Costs a cycle to confirm something that already passes.
#   TESTABLE      a genuine single-sided allegation -> generate a test, run it, record the
#                 outcome and credit or debit the critic.
#
#   fb-promote.sh <bead>
set -uo pipefail
BEAD="${1:?bead}"
C="$HOME/.local/share/farmerbob/logs/critiques/$BEAD"
OUT="$HOME/.local/share/farmerbob/logs/$BEAD.claims.json"
python3 - "$C" "$OUT" <<'PY'
import glob, json, os, re, sys
cdir, out = sys.argv[1], sys.argv[2]

claims = []
_reviews = sorted(glob.glob(f"{cdir}/*.on.*.md"))
for f in _reviews:
    base = os.path.basename(f)[:-3]
    critic, subject = base.split(".on.")
    txt = open(f).read()
    # split on CLAIM: markers, keep each block's fields
    for blk in re.split(r'(?m)^CLAIM:', txt)[1:]:
        one = blk.strip().splitlines()
        title = one[0].strip() if one else ""
        def field(name):
            m = re.search(rf'{name}:\s*(.*?)(?=\n[A-Z]+:|\n\n|\Z)', blk, re.S)
            return " ".join(m.group(1).split()) if m else ""
        where, trig = field("WHERE"), field("TRIGGER")
        exp, act = field("EXPECT"), field("ACTUAL")
        # critics inline ACTUAL on the EXPECT line as often as not; split it out first or
        # the comparison reads an expect-string that already contains the actual
        if "ACTUAL" in exp:
            head, tail = re.split(r'ACTUAL:?', exp, 1)
            exp = head.strip()
            if not act:
                act = tail.strip()
        exp = re.sub(r'^\(.*?\)\s*', '', exp).strip()   # drop parentheticals like "(plain reading of ...)"
        claims.append({"critic": critic, "subject": subject, "claim": title,
                       "where": where, "trigger": trig,
                       "expect": exp.strip(), "actual": act.strip()})

def norm(s):
    return re.sub(r'[^a-z0-9]+', ' ', s.lower()).strip()

# confirmatory: the critic documented correct behaviour
for c in claims:
    a = norm(c["actual"])
    c["kind"] = "CONFIRMATORY" if (a in ("same", "same as expected", "as expected")
                                   or (a and a == norm(c["expect"]))) else "TESTABLE"

# contradiction: two critics, same subject area, opposite expectations
def topic(c):
    t = norm(c["claim"] + " " + c["where"])
    for k in ("excluded", "family", "budget", "shadow", "sensitivity", "ambiguous", "ladder"):
        if k in t:
            return k
    return None

by_topic = {}
for c in claims:
    if c["kind"] != "TESTABLE":
        continue
    t = topic(c)
    if t:
        by_topic.setdefault(t, []).append(c)

contradictions = []
for t, group in by_topic.items():
    if len(group) < 2:
        continue
    for i in range(len(group)):
        for j in range(i + 1, len(group)):
            a, b = group[i], group[j]
            if a["critic"] == b["critic"]:
                continue
            # opposite readings: one expects X where the other reports X as the defect
            ea, eb = norm(a["expect"]), norm(b["expect"])
            aa, ab = norm(a["actual"]), norm(b["actual"])
            # both must be real allegations, and one's EXPECTED behaviour must be the other's
            # COMPLAINT. Merely differing wording is not a contradiction.
            if not (ea and eb and aa and ab):
                continue
            if ea == aa or eb == ab:        # confirmatory, not an allegation
                continue
            if (ea and ea == ab) or (eb and eb == aa):
                for c in (a, b):
                    c["kind"] = "CONTRADICTED"
                contradictions.append({"topic": t, "a": a["critic"], "b": b["critic"],
                                       "a_expects": a["expect"][:90],
                                       "b_expects": b["expect"][:90]})
                break

# "No critic found a defect" and "no critic produced a review" are DIFFERENT facts, and
# writing an empty claims file for both makes the second invisible: fb-status then reports
# the task as critiqued and ready to adjudicate. prior, bandit-route and crossx were all
# recorded as 0-claims when in truth zero critiques were ever written -- the critics had
# been told to write a relative .fb/critique.md, which resolved to /.fb/critique.md and was
# refused.  Refuse to write the artefact rather than assert a measurement that was not made.
#   (beads farmerbob-k9f, farmerbob-1bd)
if not _reviews:
    print("NO critiques were written -- refusing to emit an empty claims file")
    raise SystemExit(3)
json.dump({"claims": claims, "contradictions": contradictions}, open(out, "w"), indent=1)

from collections import Counter
k = Counter(c["kind"] for c in claims)
print(f"  {len(claims)} claims from {len({c['critic'] for c in claims})} critics")
for kind in ("CONTRADICTED", "TESTABLE", "CONFIRMATORY"):
    print(f"    {kind:<14}{k.get(kind, 0)}")
if contradictions:
    print("\n  SPEC AMBIGUITY — independent critics read the spec differently:")
    seen = set()
    for c in contradictions:
        key = (c["topic"], c["a"], c["b"])
        if key in seen: continue
        seen.add(key)
        print(f"    topic '{c['topic']}': {c['a']} vs {c['b']}")
        print(f"      {c['a']} expects: {c['a_expects']}")
        print(f"      {c['b']} expects: {c['b_expects']}")
print(f"\n-> {out}")
PY

```


## How this will be scored

farmerbob re-runs everything itself; your self-report is not used. Stated so you can
optimise for the real bar rather than guess at it.

**Gate (all required, else the run scores as failed):**
- `cargo build -p <crate>` succeeds
- `cargo test -p <crate>` passes, with at least one test that actually executes
- only the crate named in the task is modified

**Scored, in this order:**
1. **Conformance** — a test suite you will not see, derived from this spec, is run against
   your implementation. The assertions are hidden; the criteria are exactly what this
   document states.
2. **Panic-freedom** — no `unwrap()`, `expect()`, `panic!`, `todo!` or `unimplemented!` on
   any path reachable from input, outside `#[cfg(test)]`.
3. **`cargo clippy -- -D warnings` clean.**
4. **Test depth and generality** — measured directly where possible, by injecting known
   defects and by running your suite against rival implementations. Where neither
   measurement could be taken, the number of distinct behaviours you covered stands in for
   it. Count is the fallback, not the target:
   assertions. Your tests must be good enough to catch a bug in **any** correct-looking
   implementation of this spec, not only your own: test the behaviour the specification
   requires, not your particular implementation's internals. Asserting on exact error
   strings, private field names, or an output format the spec does not fix makes a test
   worthless.
5. **Documentation** — `///` on every public item.
6. **Structure** — coherent modules over one large file, where the crate warrants it.

**How to read a list in this spec.** Every enumerated list of keywords, formats or cases
states its own status, and you should read it literally:

- *"exactly these and no others"* — accepting anything further is a defect.
- *"at least these; accepting more is neither required nor penalised"* — a superset is fine,
  and **your tests may not assert on cases outside the list**, because another correct
  implementation may reasonably not handle them.
- *"at least these, plus the obvious morphological variants"* — the stemming rule is pinned
  where it says so.

If a list carries no such marker, treat it as the second form, and say so in your handoff.

**And to the composition of anything aggregate you return.** If a function returns a
table, a tuple, or a collection whose membership is not forced by its type, the spec states
exactly what is in it and in what order. Where it does not, say so in your handoff and do
not let your tests assert on it.

**The same applies to every numeric boundary.** Where a clause says "after N has elapsed",
"at least N", or "below N", the behaviour AT N and at the degenerate value (N = 0, an empty
collection, a timestamp that runs backwards) is part of the contract. If the spec does not
pin it, your tests may not assert on it either -- another correct implementation may
reasonably choose the other side. Say in your handoff which boundary you found unpinned and
which way you resolved it.

Three tasks have now been decided by candidates disagreeing about exactly this rather than
about anything either of them got wrong.
Three earlier tasks were decided by candidates disagreeing about exactly this, every time
because a test asserted a case the specification never fixed.

## A signature that cannot compute what the spec promises

If a clause in this spec describes a value that the API it also fixes makes **uncomputable**,
say so in your handoff and implement the closest honest thing. Do not silently return a
placeholder.

This is not hypothetical. A previous task's spec asked `status(resource)` to report "how long
the current holder has held the lease" while fixing a signature that takes no clock. Several
implementations returned `Duration::zero()` -- correct by necessity, indistinguishable from a
bug -- and the same spec's `holder_died(holder) -> Option<Grant>` could report only one grant
for a holder that may hold many, so its own "never left locked" invariant was unreportable.
Three critics found all of it, in three different implementations, which is how the fault was
traced to the spec rather than to any arm.

A defect that appears in nearly every implementation is evidence about the specification, not
about the field. Naming it in your handoff routes it where the fix belongs.

## Reuse the crate's existing types

If this spec names a type that already exists in `farmerbob-core` -- `Verdict`, `Grade`,
`RunState`, `Outcome`, `Measurement` and so on -- you **use that type**, imported, and you do
not define your own. A new public type whose name already exists in the crate is a defect,
scored as one, however good its internals are.

Forty-four modules have been merged, each written without sight of the others, and seven core
concepts now exist two or three times in mutually incompatible shapes. `Verdict` exists three
times. That happened one reasonable-looking local decision at a time. If you believe the
existing type genuinely cannot express what this task needs, say so in your handoff, name the
type and the clause it cannot express, and extend it rather than shadowing it.

**Not scored:** wallclock. Taking longer to produce better work is the preferred trade.
There is a generous resource budget; a run is cut early only if it stops making progress or
regresses past its own best error count.


## Handoff (required)

When you are done, write `.fb/handoff.md` in the repository root. Keep it under 300 words.

**Do not state anything the harness can check.** No test counts, no "all tests pass", no "this
handles empty input", no performance claims. Those are measured independently and a claim
about them adds nothing — the harness has already run them by the time anyone reads this.

Write only what cannot be measured:

- **Approach.** The shape of the solution and why this shape rather than an obvious alternative.
- **Trade-offs.** What you chose against, and what it would cost to choose differently.
- **Risk.** Where you think this is most likely to be wrong, or hardest to change later.
- **Deliberate omissions.** What the spec allows that you did not do, and why.

If you found the specification ambiguous or underdetermined, say exactly where. That is the
most valuable thing this file can contain: it routes back to the task author instead of
becoming a defect argued about later.
