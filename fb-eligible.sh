# fb-eligible — refuse to dispatch an arm that must not be used. Sourced by the dispatcher.
#
# Every dispatch in this project was cost-blind: matrices were hand-written lists of arm
# names and fired, with nothing consulting price. That paid for Gemini-3.8-Flash and
# GLM-5.3-Flash through OpenRouter while IDENTICAL models were available free on a plan --
# and on verify-router the free agy route PASSED with 867 lines while the paid twin no-opped.
#
# The router's `- dollar_weight*cost` term exists to prevent exactly this. Until the router
# is doing the dispatching, this is the guard.   (bead farmerbob-15p)
#
#   fb_eligible <arm>  -> 0 if dispatchable, 1 otherwise (reason on stderr)
fb_eligible() {
  local arm="$1" repo=/home/gabe/Documents/farmerbob
  python3 - "$arm" "$repo/sources.toml" <<'PY'
import sys, tomllib
arm, path = sys.argv[1], sys.argv[2]
d = tomllib.load(open(path, 'rb'))['source']
v = d.get(arm)
if v is None:
    print(f"{arm}: not in the registry", file=sys.stderr); raise SystemExit(1)
st = v.get('status')
if st == 'disabled':
    print(f"{arm}: disabled -- {v.get('disabled_reason','no reason recorded')}", file=sys.stderr)
    raise SystemExit(1)
if st not in ('verified', 'untested'):
    print(f"{arm}: status={st}, not dispatchable", file=sys.stderr); raise SystemExit(1)
# An arm whose provider has refused it is not dispatchable until the stated reset. Without
# this, the scheduler keeps firing runs into a wall and the harness scores each refusal as
# an arm failure: gemini-38-flash read 50% complete while three of its runs never started.
# (bead farmerbob-h04)
parked = v.get('parked_until')
if parked:
    import datetime
    try:
        until = datetime.datetime.fromisoformat(parked)
        now = datetime.datetime.now(datetime.timezone.utc)
        if until > now:
            left = until - now
            print(f"{arm}: parked until {parked} ({left.days}d{left.seconds // 3600}h left) -- "
                  f"quota exhausted, not an arm failure", file=sys.stderr)
            raise SystemExit(1)
    except ValueError:
        print(f"{arm}: parked_until is not a valid timestamp: {parked!r}", file=sys.stderr)
        raise SystemExit(1)

# a paid arm may not run while its free equivalent is healthy
free = v.get('redundant_with')
if free and (v.get('price_in') or 0) > 0:
    fv = d.get(free, {})
    if fv.get('status') == 'verified':
        print(f"{arm}: ${v['price_in']}/{v['price_out']} but {free} is the same model, free and "
              f"verified -- use that (set FB_ALLOW_PAID_DUPES=1 to compare harnesses deliberately)",
              file=sys.stderr)
        raise SystemExit(1)
raise SystemExit(0)
PY
}
