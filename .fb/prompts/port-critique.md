<!-- fb:creates crates/fb/src/critique.rs -->
<!-- fb:reads fb-critique.sh fb-isolate.sh -->
# Task: port fb-critique.sh to Rust, behaviour-for-behaviour

Rust workspace, already builds. Work only inside `crates/fb`.
Create `crates/fb/src/critique.rs` and declare it from `crates/fb/src/main.rs` with
`mod critique;`. Do not change any other file, and do not delete the shell script.

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
# fb-critique — cross-review. Each implementer critiques ANOTHER implementer's patch.
#
# The critic is the SAME ARM that implemented this task, run in ITS OWN WORKTREE. Two reasons:
# it already knows the spec and the repository, so the review is cheaper and better grounded;
# and running in its own tree keeps the provider-side context warm rather than paying to
# re-establish it.
#
# Assignment is a derangement -- nobody reviews themselves -- and the patch is frozen, so
# every finding refers to the artefact actually being selected.
#
# The critique is SUBJECTIVE ONLY. Everything measurable is already in the objective table
# (fb-objective.sh); a claim about behaviour is a hypothesis that gets executed, not a finding.
#   (beads farmerbob-ops, farmerbob-ljq)
#
#   fb-critique.sh <bead> <crate> <target-rel>
set -uo pipefail
export PATH="$HOME/.cargo/bin:$HOME/.local/bin:$PATH"
BEAD="${1:?bead}"; CRATE="${2:?crate}"; TARGET="${3:?target}"
REPO=/home/gabe/Documents/farmerbob
WT="$HOME/.local/share/farmerbob/worktrees"
LOGS="$HOME/.local/share/farmerbob/logs"
SLOTS=${FB_SLOTS:-6}
. "$REPO/fb-eligible.sh"
. "$REPO/fb-launch.sh"

# candidates that actually produced an implementation
ARMS=()
for d in "$WT/$BEAD--"*; do
  [ -d "$d" ] || continue; a="${d##*/$BEAD--}"
  [ -f "$d/.fb-task.md" ] && continue
  [ -s "$d/$TARGET" ] || continue
  fb_eligible "$a" 2>/dev/null || continue
  ARMS+=("$a")
done
N=${#ARMS[@]}
echo "$N candidates with an implementation"
[ "$N" -lt 2 ] && { echo "need >= 2 to cross-review"; exit 1; }

# derangement: arm i reviews arm i+1, wrapping. Nobody reviews themselves.
declare -A REVIEWS
for i in "${!ARMS[@]}"; do
  REVIEWS["${ARMS[$i]}"]="${ARMS[$(( (i + 1) % N ))]}"
done

mkdir -p "$LOGS/critiques/$BEAD"
for critic in "${ARMS[@]}"; do
  subject="${REVIEWS[$critic]}"
  while [ "$(jobs -rp | wc -l)" -ge "$SLOTS" ]; do sleep 2; done
  (
    cw="$WT/$BEAD--$critic"                 # critic works in its OWN worktree: warm context
    sw="$WT/$BEAD--$subject"
    # Show the DELIVERABLE, not "whatever git happens to call a change".
    #
    # This used to be `git diff HEAD -- crates/$CRATE`, with a fallback to reading $TARGET
    # if that came back empty. For a `creates` task the deliverable is UNTRACKED, so the
    # diff never contained it -- what it contained was the single tracked line the arm added
    # to lib.rs:
    #
    #     +pub mod matrix;
    #
    # The fallback existed for exactly this case and never fired, because it tested for an
    # EMPTY patch and one worthless line is not empty. The guard was defeated by the very
    # line that made the patch useless. On `matrix` this produced four reviews of which
    # three were worthless and two were fabricated: one critic invented line numbers for a
    # file it had never seen, another asserted a defect that the code plainly does not have.
    # Only glm-53-flash produced anything real, and only because it ignored the prompt and
    # read the file from disk itself.  (bead farmerbob-4ur)
    #
    # Rule: the patch is the declared target. If the target is tracked, its diff; if it is
    # untracked, its contents. Anything else the arm changed is reported separately as a
    # scope violation, because that is what it is.
    target_diff=$(cd "$sw" && git diff HEAD -- "$TARGET" 2>/dev/null)
    if [ -n "$target_diff" ]; then
      patch="$target_diff"
    elif [ -f "$sw/$TARGET" ]; then
      patch="=== NEW FILE: $TARGET ($(wc -l < "$sw/$TARGET") lines) ===
$(cat "$sw/$TARGET")"
    else
      patch=""
    fi

    # Files the arm changed that are NOT its deliverable. The critic should know: a review
    # of a one-file task by an arm that rewrote thirty-six files is reviewing the wrong
    # thing, and the scope violation is itself the finding.  (bead farmerbob-jxp)
    # Declaring the new module in its own crate's lib.rs is REQUIRED for a creates-task,
    # not a scope violation. Excluding it keeps the flag meaningful: every clean arm would
    # otherwise be reported as having strayed, and a warning that fires on everyone is
    # read by no one.
    own_lib="$(dirname "$TARGET")/lib.rs"
    # git may refuse the worktree entirely -- 103 of them have lost their admin directory
    # (farmerbob-13p). An empty `outside` would then read as "this arm stayed in scope",
    # which is a claim nobody measured. Say which it is.
    scope_readable=yes
    (cd "$sw" && git rev-parse --git-dir >/dev/null 2>&1) || scope_readable=no
    outside=$( { cd "$sw" && git diff --name-only HEAD -- crates/ 2>/dev/null
                 cd "$sw" && git ls-files --others --exclude-standard crates/ 2>/dev/null; } \
               | grep -vxF "$TARGET" | grep -vxF "$own_lib" | sort -u )
    # A deliverable may legitimately span several files. fnd-store's spec says "split into
    # modules if you like", and two arms did -- so the declared target became a 12-line list
    # of `mod` declarations and every substantive behaviour sat in files the critic never saw.
    # Two critics diagnosed it themselves rather than reviewing the header:
    #   or-deepseek-v4-flash: "every substantive behaviour lives in modules that are NOT part
    #     of the shown patch ... the reviewable artefact is a header."
    #   glm-53-flash: "if it is this candidate's output, the frozen patch has lost virtually
    #     all of its work and a patch-level comparison is invalid."
    # Showing the target alone was the fix for farmerbob-4ur; this is that fix's own next
    # failure. Carry the sibling sources too, bounded, so a split implementation is reviewable.
    #   (bead farmerbob-4ur, second occurrence)
    if [ "$scope_readable" = yes ] && [ -n "$outside" ]; then
      tdir=$(dirname "$TARGET")
      sibs=$(printf '%s\n' "$outside" | grep "^$tdir/" | grep '\.rs$' | head -12)
      for sib in $sibs; do
        [ -f "$sw/$sib" ] || continue
        patch="$patch

=== ALSO PART OF THIS DELIVERABLE: $sib ($(wc -l < "$sw/$sib") lines) ===
$(head -c 20000 "$sw/$sib")"
      done
    fi

    if [ "$scope_readable" = no ]; then
      patch="$patch

=== Scope could not be checked: git cannot read this worktree, so whether the arm stayed
=== within $TARGET is UNKNOWN, not confirmed."
    elif [ -n "$outside" ]; then
      patch="$patch

=== THIS ARM ALSO CHANGED $(printf '%s' "$outside" | grep -c .) FILE(S) OUTSIDE ITS DECLARED
=== DELIVERABLE ($TARGET). The task asked for that file only. The changes below are
=== out of scope and are shown as names, not content:
$outside"
    fi

    # An instrument that cannot perform its check must say so rather than produce a value.
    # A critique written against no patch is not a weak critique, it is a fabricated one,
    # and it costs a cycle and pollutes the critic's precision record.
    if [ -z "$patch" ]; then
      echo "  $critic: REFUSING -- $subject has no readable deliverable at $TARGET" >&2
      exit 3
    fi
    handoff=$(cat "$sw/.fb/handoff.md" 2>/dev/null || echo "(no handoff written)")

    p="$LOGS/critiques/$BEAD/$critic.prompt.md"
    python3 - "$REPO/.fb/prompts/_critique.md" "$p" <<PY
import sys
tpl = open(sys.argv[1]).read()
patch = """$(printf '%s' "$patch" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 60000)"""
handoff = """$(printf '%s' "$handoff" | sed 's/\\/\\\\/g; s/"/\\"/g' | head -c 4000)"""
open(sys.argv[2], "w").write(tpl.replace("{PATCH}", patch).replace("{HANDOFF}", handoff).replace("{OUT}", "$cw/.fb/critique.md"))
PY
    rm -f "$cw/.fb/critique.md"; mkdir -p "$cw/.fb"
    P="$(cat "$p")"
    ( fb_launch "$critic" "$P" "$cw" ) > "$LOGS/critiques/$BEAD/$critic.log" 2>&1
    if [ -s "$cw/.fb/critique.md" ]; then
      cp "$cw/.fb/critique.md" "$LOGS/critiques/$BEAD/$critic.on.$subject.md"
      printf '  %-22s reviewed %-22s %s claims, %s words\n' "$critic" "$subject" \
        "$(grep -c '^CLAIM:' "$cw/.fb/critique.md" 2>/dev/null)" "$(wc -w < "$cw/.fb/critique.md")"
    else
      printf '  %-22s reviewed %-22s (no critique written)\n' "$critic" "$subject"
    fi
  ) &
done
wait
echo "-> $LOGS/critiques/$BEAD/"

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
