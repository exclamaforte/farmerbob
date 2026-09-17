<!-- fb:creates crates/fb/src/defects.rs -->
<!-- fb:reads fb-defects.sh -->
# Task: port fb-defects.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/defects.rs` and declare it from `crates/fb/src/main.rs` with
`mod defects;`. Do not change any other file, and do not delete the shell script.

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

## What this script measures, and why it has been broken

fb-defects.sh measures DEFECT SENSITIVITY: it injects known defects into a correct
implementation and counts how many each candidate's test suite catches. That is the
adjudicator's third criterion, ranked above survival, and the only DIRECT evidence of suite
quality -- everything else is a proxy. It was available for one task in forty-eight until
today, because of two grafting bugs that are now fixed in the shell and that your port must
not reintroduce:

1. **The conformance suite is grafted into the TARGET MODULE, not lib.rs.** A suite is written
   as a child of the module it tests, so it says `use super::Admission`. Appended to lib.rs,
   `super::` resolves to the crate root and every import fails, every defect reads
   `nocompile`, and every run ends "0 validated defects, nothing to measure".

2. **The grafted module is RENAMED.** The merged reference already contains that suite --
   escalated into the file when the task was adjudicated -- so appending it verbatim is "the
   name `cx_budget_codex_luna` is defined multiple times", which also reads as nocompile.

## The three outcomes, which are the heart of this script

A defect run against a suite yields exactly one of three results, and conflating any two of
them destroys the measurement:

- `fail`      the suite DETECTED the defect. Good.
- `pass`      the suite MISSED it.
- `nocompile` the mutation is a syntax error, not a defect at all.

And validation against the reference gives the defect one of three classes:

- **valid** -- the reference conformance suite detects it. A real, spec-relevant defect.
- **beyond the reference** -- the reference MISSES it. Kept, and scored separately: a
  candidate suite that catches one has proved itself strictly better than the reference. This
  is the only column that can separate a field where everyone catches the obvious defects, and
  an earlier version of this script discarded exactly these as "inert".
- **not a defect** -- it does not compile.

`nocompile` must never be counted as "caught". A suite that cannot build has not detected
anything, and the whole family of bugs in this harness is a check that cannot run reporting
the same value as a check that ran and found nothing wrong. Use
`farmerbob_core::measurement::Measurement` for anything that might not have been measured.

## Required: a differential test

Add `#[cfg(test)]` tests, and include at least one that pins the OUTPUT FORMAT byte-for-byte
against a worked example taken from the script -- the exact column widths, the exact literal
words. Other scripts grep this output; a format change is a silent breakage.

You cannot run the shell script from a unit test, so do not try. Pin the format with literal
expected strings.

## Rules

- Public entry point: `pub fn run_cmd(...) -> i32`, returning the process exit code, with
  arguments matching the script's positional parameters in order.
- **Wire it as an `fb` SUBCOMMAND in `main.rs`** -- a `Command` variant with the script's
  parameters as arguments, and a dispatch arm calling `run_cmd`. A module that only declares
  `mod x;` compiles, passes its own tests, and is unreachable from the binary, which is the
  exact condition this whole migration exists to end.

  This requirement is here because two critics found its absence independently, reviewing
  different subjects: "the module is shipped as `#[allow(dead_code)] mod crossx;` with no
  subcommand wired -- the feature is unreachable except by calling `run_cmd` directly, a merge
  hazard", and "the implementation can compile and have unit coverage while remaining
  inaccessible to users of the public `fb` interface". An earlier version of this spec asked
  only for the module declaration, so every candidate complied and every candidate was
  unreachable. The defect was mine.
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
# fb-defects — measure each frozen test suite's SENSITIVITY against a validated defect set.
#
# Cross-examination found no behavioural disagreement among 13 implementations, and every
# candidate passes conformance. Neither measures whether a suite can CATCH A BUG, because
# nothing in the corpus was buggy. This injects known defects and counts detections.
#
# A defect counts only if the reference conformance suite detects it -- otherwise the
# mutation is inert or the spec is silent, and it belongs outside the denominator.
#
#   fb-defects.sh <bead> <crate> <target-rel>
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target}"
REPO=/home/gabe/Documents/farmerbob
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
DEFECTS="$REPO/.fb/defects/$BEAD"
OUT="$HOME/.local/share/farmerbob/logs/$BEAD.defects.json"
SLOTS=${FB_CX_SLOTS:-4}
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# reference = the merged implementation on master
REF="$TMP/ref"; mkdir -p "$REF"
(cd "$REPO" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$REF" && tar xf -)

apply() {  # apply <patchfile> <destdir>
  python3 - "$1" "$2/$TARGET" <<'PY'
import sys
spec = open(sys.argv[1]).read()
path = sys.argv[2]
src = open(path).read()
for block in spec.split('@@@')[1:]:
    old, new = block.split('--->')
    old, new = old.strip('\n'), new.strip('\n')
    if old not in src:
        print("ANCHOR-MISSING", file=sys.stderr); sys.exit(2)
    src = src.replace(old, new, 1)
open(path, 'w').write(src)
PY
}

run_suite() {  # run_suite <dir> <suitefile> -> pass|fail|nocompile
  local d="$1" suite="$2"
  local sd="$d/crates/$CRATE/src"
  # Graft into the TARGET MODULE, not lib.rs.
  #
  # A conformance suite is written as a child of the module it tests, so it says
  # `use super::Admission`. Appended to lib.rs, `super::` resolves to the CRATE ROOT and
  # every import fails: "unresolved imports super::Admission, super::Machine, super::Policy".
  # So every defect came back `nocompile`, every defect was EXCLUDED as inert, and the run
  # ended "0 validated defects, nothing to measure" -- for any task whose deliverable is a
  # module rather than lib.rs itself, which is all but a handful.
  #
  # That is why DefectSensitivity, the adjudicator's only DIRECT measure of suite quality and
  # its third criterion, has been available for one task in forty-eight. Not because nobody
  # wrote defect sets: because the validation step could not compile the suite it validates
  # against.
  #
  # fb-crossx.sh already solved exactly this and carries the bead for it. One rule, two
  # implementations, and only one of them got the fix.  (beads farmerbob-74l, farmerbob-jd2.11)
  local host="$sd/$(basename "$TARGET")"
  [ -f "$host" ] || host="$sd/lib.rs"
  cp "$host" "$host.bak"
  # Rename the grafted module. The merged reference already contains this very suite -- it was
  # escalated into the file when the task was adjudicated -- so appending it verbatim is
  # "the name `cx_budget_codex_luna` is defined multiple times", which reads as nocompile and
  # excludes the defect exactly like the import failure did. fb-crossx renames to xtests_N for
  # the same reason.
  sed 's/^\( *\)mod \([a-z_0-9]*\)/\1mod dfx_\2/' "$suite" >> "$host"
  local o="$d/out"
  local r
  if (cd "$d" && timeout 300 cargo test -p "$CRATE" >"$o" 2>&1); then r=pass
  elif grep -qE '^error\[E[0-9]+\]:|could not compile' "$o"; then r=nocompile
  else r=fail; fi
  mv "$host.bak" "$host"
  echo "$r"
}

# ---- 1. validate each defect against the reference conformance suite -------------------
echo "validating defects against the reference implementation"
VALID=(); MISSED=()
for p in "$DEFECTS"/*.patch; do
  [ -f "$p" ] || continue
  name=$(basename "$p" .patch)
  d="$TMP/v.$name"; rm -rf "$d"; cp -r "$REF" "$d"
  if ! apply "$p" "$d" 2>/dev/null; then printf '  %-28s SKIP (anchor missing)\n' "$name"; continue; fi
  r=$(run_suite "$d" "$REPO/.fb/conformance/$BEAD.rs")
  # THREE outcomes, not two.
  #
  #   fail       the reference suite detects it -> a valid, spec-relevant defect
  #   pass       the reference suite MISSES it -> either semantically inert, or a real defect
  #              the reference is blind to. These were discarded, and they are the only
  #              mutants that can DISCRIMINATE between candidate suites.
  #   nocompile  a syntax error, not a defect
  #
  # Keeping only `fail` selects for defects every competent suite catches, and guarantees the
  # measure reads ~100% for everyone. Measured: on `budget` all four suites caught 4 of 4, on
  # `gate` all four caught 12 of 12. The filter was removing exactly the evidence that would
  # have separated them -- 2 of gate's 24 mutants were excluded this way.
  #
  # A mutant the reference misses but SOME candidate catches proves that candidate's suite is
  # strictly better than the reference. That is the most informative result available here, so
  # it is now measured instead of thrown away.  (bead farmerbob-jd2.11)
  case "$r" in
    fail) printf '  %-28s valid (conformance detects it)\n' "$name"; VALID+=("$name") ;;
    pass) printf '  %-28s BEYOND the reference (it misses this one)\n' "$name"; MISSED+=("$name") ;;
    *)    printf '  %-28s excluded (%s) -- not a defect\n' "$name" "$r" ;;
  esac
  rm -rf "$d"
done
echo "  ${#VALID[@]} validated defects, ${#MISSED[@]} beyond the reference"
[ "${#VALID[@]}" -eq 0 ] && [ "${#MISSED[@]}" -eq 0 ] && { echo "nothing to measure"; exit 1; }

# ---- 2. every candidate suite against every validated defect ---------------------------
ARMS=()
for W in "$WT_ROOT/$BEAD--"*; do
  [ -d "$W" ] || continue; a="${W##*/$BEAD--}"
  [ -f "$W/.fb-task.md" ] && continue
  [ -s "$W/$TARGET" ] || continue
  # freeze the candidate's own suite
  awk '/#\[cfg\(test\)\]/{f=1} f' "$W/$TARGET" \
    | sed -e 's/^\( *\)mod tests/\1mod frozen/' \
          -e "s|use super::\*;|use crate::$(basename "$TARGET" .rs)::*;|" > "$TMP/s.$a.rs"
  [ "$(grep -c '#\[test\]' "$TMP/s.$a.rs")" -gt 0 ] && ARMS+=("$a")
done
echo "  ${#ARMS[@]} candidate suites"

for a in "${ARMS[@]}"; do
  for name in "${VALID[@]}" "${MISSED[@]}"; do
    [ -n "$name" ] || continue
    while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ]; do sleep 1; done
    (
      d="$TMP/r.$a.$name"; rm -rf "$d"; cp -r "$REF" "$d"
      apply "$DEFECTS/$name.patch" "$d" 2>/dev/null
      echo "$(run_suite "$d" "$TMP/s.$a.rs")" > "$TMP/o.$a.$name"
      rm -rf "$d"
    ) &
  done
done
wait

python3 - "$OUT" "$TMP" "${#VALID[@]}" "${ARMS[*]}" "${VALID[*]}" "${MISSED[*]}" <<'PY'
import json, os, sys
out, tmp, nvalid, arms, valid = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4].split(), sys.argv[5].split()
beyond = sys.argv[6].split() if len(sys.argv) > 6 else []

def verdict(a, n):
    p = os.path.join(tmp, f"o.{a}.{n}")
    return open(p).read().strip() if os.path.exists(p) else "nocompile"
res = {}
for a in arms:
    caught = comparable = 0
    missed = []
    for n in valid:
        p = os.path.join(tmp, f"o.{a}.{n}")
        v = open(p).read().strip() if os.path.exists(p) else "nocompile"
        if v == "nocompile":
            continue
        comparable += 1
        if v == "fail":
            caught += 1
        else:
            missed.append(n)
    # Defects the REFERENCE suite misses. Catching one proves this suite is strictly
    # better than the reference, and it is the only figure here that can separate a field
    # in which everyone catches the obvious defects.
    beyond_caught = sum(1 for n in beyond if verdict(a, n) == "fail")
    beyond_comparable = sum(1 for n in beyond if verdict(a, n) != "nocompile")
    res[a] = {"caught": caught, "comparable": comparable,
              "sensitivity": (caught / comparable) if comparable else None,
              "beyond_reference_caught": beyond_caught,
              "beyond_reference_of": beyond_comparable,
              "missed": missed}
json.dump({"validated_defects": nvalid, "beyond_reference_defects": len(beyond),
           "arms": res}, open(out, "w"), indent=1)
print()
print(f"{'SUITE':<24}{'CAUGHT':>9}{'OF':>5}{'SENSITIVITY':>13}{'BEYOND REF':>12}")
for a, v in sorted(res.items(),
                   key=lambda kv: (-kv[1]['beyond_reference_caught'], -(kv[1]['sensitivity'] or -1))):
    s = f"{v['sensitivity']:.0%}" if v['sensitivity'] is not None else "n/a"
    b = f"{v['beyond_reference_caught']}/{v['beyond_reference_of']}" if v['beyond_reference_of'] else "-"
    print(f"{a:<24}{v['caught']:>9}{v['comparable']:>5}{s:>13}{b:>12}")
if beyond:
    print()
    print("  BEYOND REF counts defects the reference conformance suite does NOT detect.")
    print("  Catching one proves a suite is strictly better than the reference; it is the")
    print("  only column here that can separate a field where everyone catches the rest.")
PY
echo "-> $OUT"

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

## Run the tests quietly

Use `cargo test -q` and `cargo build -q`. This workspace has over nine hundred tests and a
plain `cargo test` prints a line for every one of them; three arms on a recent task spent
their whole run reading their own test output and produced nothing at all. `-q` prints the
summary and any failures, which is the entire signal.

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
