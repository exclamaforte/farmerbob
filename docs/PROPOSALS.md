# Spec critique, follow-up proposals, and the credit ledger

Two stages and one record, added 2026-09-18.

## Why

Every merged task in the last week produced at least one finding about the SPECIFICATION
rather than about any arm's code, and every one of them arrived too late to act on:

- `park-scope`: the pinned API said `registry: &Registry` and never said where `Registry`
  lived. Both arms invented one. The critic that found it had done the same in its own patch.
- `availability`: "state which one wins and pin it" read as a requirement rather than as
  delegation. The field split 2-2 and every failing cell in a 4x4 matrix was that one line.
- `park-wire`: clause 3 said `Park::Backoff { ms }`. The real type has no `ms`. Two arms found
  it independently, one explicitly declining to file a claim because the boundary was unpinned.
- `suite-witness`: the rubric forbids a duplicate type name, the Exact API ordered one,
  `gate::Verdict` exists, and the scope gate makes extending it `OutOfScope`. Three rules, no
  legal move.
- `due-reset`: the spec ordered `due` to stop mutating and froze a signature advertising
  mutation. The arm that fixed it was right and lost for breaking the rule.

Five specs, five defects, all found by models reading the spec — after three arms had each
spent a full run implementing against it. The information existed before the work did. Nothing
asked for it.

Separately, critics keep finding real work that is out of scope for the patch under review and
has nowhere to go. `or-ling` on `wt-conflict` found `safe_to_reap` filtering with
`Vec::contains` where its own doc claims set subtraction, on the function that gates deletion
across 327 directories. It survives only because the adjudicator happened to file it by hand.

## Stage 1: spec critique, before any implementation

    spec + code  ->  SPEC CRITIQUE  ->  adjudicate + revise  ->  implement  ->  cross-critique

Arms receive the spec and the existing code it will touch, and are asked for defects **in the
spec**. They do not implement. The output is `FINDINGS`, and `NO SPEC DEFECTS FOUND.` is a
legitimate answer.

The adjudicator rules on each finding. Accepted findings revise the spec before dispatch, so
the implementation wave is spent on a specification that has already survived review.

This is deliberately not a vote. Three arms agreeing that a clause is ambiguous is evidence;
three arms agreeing on what it should say is not, because they share a cause — the same
ambiguous text.

## Stage 2: follow-up proposals, during cross-critique

The existing critique already asks for defects in the patch. It now also accepts `FOLLOWUPS`:
work worth doing in the codebase that is **out of scope for the patch under review**. The
adjudicator accepts or rejects; accepted ones become beads.

This gives a critic somewhere to put a true observation that is not a defect in what it was
handed, which is currently either discarded or mis-filed as a claim.

## The ledger

Both stages produce proposals, and a proposal that is ACCEPTED is a positive signal about the
arm that made it — distinct from implementing well, and not currently measured by anything.

`farmerbob_core::ledger` holds the pure logic. One append-only record per proposal:

    arm, task, kind (SpecDefect | FollowUp), ruling (Accepted | Rejected | Duplicate), title

and `tally` reduces a slice of them to per-arm credits.

Three rules that follow the rest of this crate's doctrine:

- **`Duplicate` is not `Rejected`.** An arm that independently finds something already known
  was right; it simply was not first. Counting it as a rejection would punish being correct.
- **An arm that proposed nothing has NO acceptance rate**, not a rate of zero.
  `acceptance_rate` returns `Measurement::Missing(NothingToMeasure)`. A zero here would be
  indistinguishable from an arm that proposed ten things and had them all rejected.
- **Proposals are counted per arm per task**, so an arm cannot raise its score by splitting one
  finding into five. The adjudicator rules `Duplicate` on the splits.

## What this does not do

It does not let an arm's proposals change what the arm is scored on for the task at hand. The
gate, the crossx matrix and the critique are unchanged. Credits accumulate beside them and are
read by the adjudicator and by `fb pareto`, not folded into a single number — this project does
not have a defensible weighting between "wrote the better implementation" and "found the flaw
in the question", and inventing one would hide the trade-off rather than inform it.
