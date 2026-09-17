<!-- fb:creates crates/fb/src/crossx.rs -->
<!-- fb:reads fb-crossx.sh fb-verdict.sh -->
# Task: port fb-crossx.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/crossx.rs` and declare it from `crates/fb/src/main.rs` with
`mod crossx;`. Do not change any other file, and do not delete the shell script.

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
# fb-crossx — cross-examination: run every candidate's test suite against every candidate's
# implementation. N^2 local executions, zero tokens, no model opinions.
#
#   suite_A on impl_B fails  =>  B has a bug, OR A's test is wrong. The column says which:
#     a test failing on 1 of N   -> real defect in that one
#     a test failing on N-1 of N -> the test is wrong (over-fitted to its author's internals)
#
#   survival  = fraction of OTHER arms' discriminating suites this impl passes
#   discovery = number of other impls this arm's suite breaks (discounted if it breaks ~all)
#
# Runs on copies; never touches a candidate worktree.        (bead farmerbob-xle)
set -uo pipefail
export PATH="$HOME/.cargo/bin:$PATH"
. /home/gabe/Documents/farmerbob/fb-verdict.sh
BEAD="${1:?bead}"; CRATE="${2:-farmerbob-core}"; FILE="${3:?relative path of the file under test}"
WT_ROOT="$HOME/.local/share/farmerbob/worktrees"
REPO=/home/gabe/Documents/farmerbob
OUT="$HOME/.local/share/farmerbob/logs/$BEAD.crossx.json"
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# Only candidates that PASSED the gate are cross-examined. A candidate whose build failed
# cannot run its own suite against its own code, which would trip the diagonal invariant and
# void a matrix that is otherwise sound -- ifm-k2-think did exactly that on adjudicate, where
# the other three candidates were fine.  (bead farmerbob-74l)
PASSED=$(python3 -c "
import json,sys
try: d=json.load(open('$HOME/.local/share/farmerbob/logs/$BEAD.score.json'))
except Exception: sys.exit(0)
print(' '.join(r['source'] for r in d if r.get('verdict')=='PASS'))" 2>/dev/null)

ARMS=(); for W in "$WT_ROOT/$BEAD--"*; do
  [ -d "$W" ] || continue
  [ -f "$W/.fb-task.md" ] && continue          # never score a live run
  [ -f "$W/$FILE" ] || continue
  a="${W##*/$BEAD--}"
  if [ -n "$PASSED" ]; then
    case " $PASSED " in *" $a "*) ;; *) echo "  skip $a (did not pass the gate)"; continue ;; esac
  fi
  ARMS+=("$a")
done
echo "candidates: ${ARMS[*]}"
[ "${#ARMS[@]}" -lt 2 ] && { echo "need >=2"; exit 1; }

# Collect each arm's tests from EVERY file in the crate, not just one. Implementations that
# split into modules keep their tests beside the code, and a test module living in run.rs
# that says `use super::*` means crate::run::*, which breaks when grafted elsewhere.
# Rewrite those imports to the crate root, which is what the pinned public API is exported
# from anyway.
SRCDIR_REL="crates/$CRATE/src"
TARGET_REL="$FILE"          # the module a suite is grafted into
slug() { printf '%s' "$1" | tr -c 'a-zA-Z0-9' '_'; }
for a in "${ARMS[@]}"; do
  # A test module inherits its parent FILE's imports. Grafted into lib.rs those are gone,
  # producing E0433 "cannot find type PathBuf/Utc/Uuid" -- tooling noise, not an API
  # mismatch. Re-supply the crate's common imports so that only genuine signature
  # divergence shows up as nocompile.
  rm -f "$TMP/suite.$a."*.rs; : > "$TMP/manifest.$a"
  # Which files hold this candidate's work. Prefer the task's own declaration -- the spec
  # states `fb:creates` / `fb:modifies`, which is precise and survives the candidate being
  # committed or merged (at which point a diff against HEAD is empty). Fall back to the
  # worktree diff, then to the single target file.
  #
  # Collecting EVERY file in the crate was wrong: it swept up the inherited domain-model
  # tests, which reference types from sibling modules (ExperimentId lives in crate::ids,
  # not crate::experiment) and cannot compile once grafted elsewhere.
  n=0
  DECLARED=$(grep -ohE '<!-- fb:(creates|modifies) [^ ]+ -->' "$REPO/.fb/prompts/$BEAD.md" 2>/dev/null | awk '{print $3}')
  CHANGED=$( { git -C "$WT_ROOT/$BEAD--$a" diff --name-only HEAD -- "$SRCDIR_REL" 2>/dev/null
               git -C "$WT_ROOT/$BEAD--$a" ls-files --others --exclude-standard "$SRCDIR_REL" 2>/dev/null
             } | sort -u )
  FILES="${DECLARED:-$CHANGED}"
  FILES="${FILES:-$FILE}"
  for rel in $FILES; do
    f="$WT_ROOT/$BEAD--$a/$rel"
    [ -f "$f" ] || continue
    case "$f" in *.rs) ;; *) continue ;; esac
    # `use super::*` resolves against the module the tests were WRITTEN in, not the one they
    # are grafted into. A suite from vrouter.rs means crate::vrouter::*; one from lib.rs means
    # crate::*. Rewriting every case to `crate::*` silently pointed at the wrong module and
    # made every suite look API-incompatible.
    # A test module inherits its parent FILE's top-level imports. The hand-maintained
    # xprelude guessed at which ones (PathBuf/Utc/Uuid) and missed serde_json::Value, so a
    # suite failed to compile against its OWN implementation -- an impossible result that
    # marks the instrument, not the candidate.  Carry the file's real imports instead of
    # predicting them.  (bead farmerbob-74l)
    USES=$(awk '/#\[cfg\(test\)\]/{exit} /^use /{print}' "$f")
    BODY=$(awk '/#\[cfg\(test\)\]/{f=1} f' "$f" | sed -e "s/^\( *\)mod tests/\1mod xtests_${n}/")
    # Do NOT inject a `use` the suite already declares for itself. Carrying the file's real
    # imports (above) fixed one impossible result and created another: a test module that
    # explicitly imports the same item as its parent file ends up with the import twice, and
    # a duplicate EXPLICIT import is E0252, a hard error. A glob is only shadowed, which is
    # why `use super::*` alongside an explicit import compiles fine and this does not.
    #
    # or-ling-30-flash's pricing suite compiled perfectly in its own worktree and failed
    # against its own implementation once grafted. The diagonal invariant caught it and
    # VOIDed the matrix, which is the invariant doing its job -- but the fault was the
    # instrument's, and one arm's whole cross-examination was lost to it.
    #   (bead farmerbob-74l, second occurrence)
    USES=$(printf '%s\n' "$USES" | while IFS= read -r u; do
             [ -n "$u" ] || continue
             printf '%s\n' "$BODY" | grep -qF "$(printf '%s' "$u" | sed 's/^ *//')" || printf '%s\n' "$u"
           done)
    printf '%s\n' "$BODY" \
      | awk -v uses="$USES" '{print} /^ *mod xtests_[0-9]+ *\{/ && !done {print uses; done=1}' \
      >> "$TMP/suite.$a.$(slug "$rel").rs"
    echo >> "$TMP/suite.$a.$(slug "$rel").rs"
    grep -qxF "$rel" "$TMP/manifest.$a" || echo "$rel" >> "$TMP/manifest.$a"
    n=$((n+1))
  done
  printf '  suite %-22s %s lines, %s tests, %s file(s)\n' "$a" \
    "$(cat "$TMP/suite.$a."*.rs 2>/dev/null | wc -l)" \
    "$(cat "$TMP/suite.$a."*.rs 2>/dev/null | grep -c '#\[test\]')" \
    "$(wc -l < "$TMP/manifest.$a")"
done

# N^2 cargo cycles. Measured: one cold cycle peaks at ~881M across rustc+cargo, so a machine
# with this much free memory runs several concurrently. Verification is cpu/memory-bound
# while the agents are network-bound, so the two are limited independently.
CX_SLOTS=${FB_CX_SLOTS:-4}
RES="$TMP/res"; mkdir -p "$RES"
run_one() {
  local impl="$1" suite="$2"
  local C="$TMP/run.$impl.$suite"; rm -rf "$C"; mkdir -p "$C"
  (cd "$WT_ROOT/$BEAD--$impl" && tar cf - Cargo.toml Cargo.lock crates rustfmt.toml 2>/dev/null) | (cd "$C" && tar xf -) 2>/dev/null
  for f in "$C/$SRCDIR_REL"/*.rs; do
    [ -f "$f" ] || continue
    awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" > "$f.stripped" && mv "$f.stripped" "$f"
  done
  # A test module is a CHILD of the module it tests and can see its private items.
  # Grafting it into lib.rs stripped that privilege, so a suite touching a private field
  # failed to compile against its OWN implementation (or-hy3 on speed: E0616 on
  # first_write_ms) and the matrix read "no signal". Graft into the module under test,
  # where `use super::*` resolves exactly as the author wrote it.  (bead farmerbob-74l)
  # A task may declare several files (fb:creates + fb:modifies). Concatenating their tests
  # into one graft put quota.rs's tests inside limit_signal.rs, where `use super::*` names
  # the wrong module and BOTH arms failed against their own code. Each file's tests go home.
  local grafted=0 rel
  while IFS= read -r rel; do
    [ -n "$rel" ] || continue
    local sfile="$TMP/suite.$suite.$(slug "$rel").rs"
    [ -f "$sfile" ] || continue
    if [ -f "$C/$rel" ]; then cat "$sfile" >> "$C/$rel"; grafted=$((grafted+1)); fi
  done < "$TMP/manifest.$suite"
  # The implementation does not have the file this suite tests: a genuine API divergence.
  [ "$grafted" -eq 0 ] && { echo nocompile > "$RES/$impl|$suite"; rm -rf "$C"; return; }
  local o="$C/out"
  if (cd "$C" && timeout 300 cargo test -p "$CRATE" >"$o" 2>&1); then echo pass > "$RES/$impl|$suite"
  elif grep -qE '^error\[E[0-9]+\]:|could not compile' "$o"; then echo nocompile > "$RES/$impl|$suite"
  else echo fail > "$RES/$impl|$suite"; fi
  rm -rf "$C"
}
for impl in "${ARMS[@]}"; do
  for suite in "${ARMS[@]}"; do
    while [ "$(jobs -rp | wc -l)" -ge "$CX_SLOTS" ]; do sleep 1; done
    run_one "$impl" "$suite" &
  done
done
wait
declare -A R
for impl in "${ARMS[@]}"; do
  for suite in "${ARMS[@]}"; do
    R["$impl|$suite"]=$(cat "$RES/$impl|$suite" 2>/dev/null || echo nocompile)
  done
done
# THE PARTITION SHAPE. A defect shows as 1-of-N; an over-fitted suite as N-1-of-N. A clean
# split into camps that pass within themselves and fail across is NEITHER -- it is the spec
# admitting two readings, with each camp implementing one correctly. compare produced exactly
# that and the harness reported four discriminating suites and survival 1/3 for everyone,
# which says every arm found two defects when none did.  (bead: partition shape)
detect_partition() {
  python3 - "$RJSON_TMP" "${ARMS[*]}" <<'PYP'
import json, sys
R = json.load(open(sys.argv[1])); arms = sys.argv[2].split()
if len(arms) < 4: raise SystemExit(1)
# camp(a) = the set of suites a's IMPLEMENTATION passes
camps = {}
for a in arms:
    camps[a] = frozenset(s for s in arms if R.get(f"{a}|{s}") == "pass")
groups = {}
for a, c in camps.items(): groups.setdefault(c, []).append(a)
if len(groups) < 2: raise SystemExit(1)
for c, members in groups.items():
    # internally all-pass, and fails every arm outside the camp
    if set(members) - set(c): raise SystemExit(1)
    for a in members:
        for other in arms:
            if other in c: continue
            if R.get(f"{a}|{other}") != "fail": raise SystemExit(1)
print(" | ".join("{" + ",".join(sorted(m)) + "}" for m in groups.values()))
PYP
}

# THE DIAGONAL INVARIANT. Every arm's own suite must pass against its own implementation:
# it demonstrably did so inside the candidate's worktree, so a failure here is the transplant,
# never the candidate. Twice this produced a matrix that read "no signal" -- once from dropped
# file-level imports, once from grafting into lib.rs and losing private-field access -- and
# both times the harness reported two candidates as indistinguishable while measuring nothing.
# A broken instrument must say so instead of returning a confident null.  (bead farmerbob-74l)
DIAG_BAD=()
for a in "${ARMS[@]}"; do
  [ "${R["$a|$a"]}" = pass ] || DIAG_BAD+=("$a(${R["$a|$a"]})")
done
if [ "${#DIAG_BAD[@]}" -gt 0 ]; then
  echo
  echo "VOID: the diagonal is not all-pass -- ${DIAG_BAD[*]}"
  echo "A suite that cannot run against the code it shipped with is a grafting failure."
  echo "Scores are NOT written; fix the transplant before trusting any cell."
  exit 3
fi

RJSON_TMP=$(mktemp)
python3 -c "
import json,sys
R={}
for k in sys.argv[1:]:
    a,b,v=k.split('|'); R[f'{a}|{b}']=v
json.dump(R,open('$RJSON_TMP','w'))" $(for impl in "${ARMS[@]}"; do for suite in "${ARMS[@]}"; do printf '%s|%s|%s ' "$impl" "$suite" "${R["$impl|$suite"]}"; done; done)
if PART=$(detect_partition 2>/dev/null); then
  echo
  echo "SPEC-AMBIGUOUS: the field partitions into self-consistent camps -- $PART"
  echo "Each camp passes within itself and fails across, which is neither a defect (1-of-N)"
  echo "nor an over-fitted suite (N-1-of-N). Two readings of the spec, both implemented"
  echo "correctly. Treat every cross-camp failure as vetoed and fix the specification."
fi
rm -f "$RJSON_TMP"

echo; printf '%-24s' 'impl \ suite'; for s in "${ARMS[@]}"; do printf '%-10s' "${s:0:9}"; done; echo
for impl in "${ARMS[@]}"; do
  printf '%-24s' "${impl:0:23}"
  for suite in "${ARMS[@]}"; do
    v=${R["$impl|$suite"]}
    case "$v" in pass) printf '%-10s' " ." ;; fail) printf '%-10s' " FAIL" ;; *) printf '%-10s' " nocomp" ;; esac
  done; echo
done

RJSON=$(for k in "${!R[@]}"; do printf '"%s":"%s",' "$k" "${R[$k]}"; done)
RJSON="{${RJSON%,}}"
export RJSON
python3 - "$OUT" "${ARMS[*]}" <<PY
import json,sys,os
arms=sys.argv[2].split()
R=json.loads(os.environ['RJSON'])
res={}
for a in arms:
    others=[o for o in arms if o!=a]
    # nocompile is an API MISMATCH, not evidence about correctness -- the suite was written
    # against a different surface. Only a genuine assertion failure counts as a discovery,
    # and only a compiling suite can say anything about survival.
    def verdict(i, s): return R.get(f"{i}|{s}")
    compiles = lambda s: [i for i in arms if i != s and verdict(i, s) != "nocompile"]
    # a suite discriminates if, among impls it COMPILES against, it fails some but not all
    disc = []
    for s in arms:
        c = compiles(s)
        if len(c) < 2: continue
        f = sum(1 for i in c if verdict(i, s) == "fail")
        if 0 < f < len(c): disc.append(s)
    rel = [s for s in disc if s != a and verdict(a, s) != "nocompile"]
    surv = sum(1 for s in rel if verdict(a, s) == "pass")
    broke = [i for i in others if verdict(i, a) == "fail"]
    incompat = [i for i in others if verdict(i, a) == "nocompile"]
    overfit = len(broke) == len(compiles(a)) and len(compiles(a)) > 1
    res[a]={"survival": (surv/len(rel)) if rel else None, "survived":surv, "of":len(rel),
            "discovery": 0 if overfit else len(broke), "broke":broke,
            "api_incompatible_with": len(incompat),
            "suite_discriminating": a in disc, "suite_overfitted": overfit}
json.dump(res, open(sys.argv[1],'w'), indent=1)
print()
print(f"{'ARM':<24}{'SURVIVAL':>10}{'DISCOVERY':>11}{'API-INCOMPAT':>14}  SUITE")
for a,v in sorted(res.items(), key=lambda kv: (-(kv[1]['survival'] or 0), -kv[1]['discovery'])):
    s = f"{v['survived']}/{v['of']}" if v['of'] else "n/a"
    tag = "OVER-FITTED" if v['suite_overfitted'] else ("discriminating" if v['suite_discriminating'] else "no signal")
    print(f"{a:<24}{s:>10}{v['discovery']:>11}{v['api_incompatible_with']:>14}  {tag}")
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
