#!/usr/bin/env bash
# fb-prices — refresh OpenRouter prices in sources.toml from the live catalogue.
# Prices drift: deepseek-v4-flash-0731 moved 0.055/0.110 -> 0.060/0.120 inside one session.
# A cost-normalised leaderboard computed from stale figures is silently wrong, so the
# registry stores a `price_checked` date and this refetches.   (bead farmerbob-gyh)
set -uo pipefail
J=$(mktemp); trap 'rm -f "$J"' EXIT
curl -s --max-time 60 'https://openrouter.ai/api/v1/models' -o "$J" || { echo "fetch failed"; exit 1; }
python3 - "$J" "${1:-}" <<'PY'
import json,sys,tomllib,re,datetime
cat={m['id']:m for m in json.load(open(sys.argv[1]))['data']}
apply = sys.argv[2] == '--apply'
p='sources.toml'; s=open(p).read()
d=tomllib.load(open(p,'rb'))['source']
today=datetime.date.today().isoformat()
changed=0
for k,v in d.items():
    m=v.get('model','')
    if not m.startswith('openrouter/'): continue
    slug=m[len('openrouter/'):]
    if slug not in cat: print(f"  {k:<24} SLUG GONE: {slug}"); continue
    pr=cat[slug].get('pricing',{})
    ni=round(float(pr.get('prompt') or 0)*1e6,4); no=round(float(pr.get('completion') or 0)*1e6,4)
    oi=v.get('price_in'); oo=v.get('price_out')
    if oi is None: continue
    if abs(ni-oi)>1e-4 or abs(no-oo)>1e-4:
        changed+=1
        print(f"  {k:<24} {oi}/{oo} -> {ni}/{no}")
        if apply:
            blk=re.search(rf'(\[source\.{re.escape(k)}\].*?)(?=\n\[source\.|\Z)', s, re.S)
            if blk:
                b=blk.group(1)
                nb=re.sub(r'price_in\s*=\s*[\d.]+', f'price_in = {ni}', b)
                nb=re.sub(r'price_out\s*=\s*[\d.]+', f'price_out = {no}', nb)
                nb=re.sub(r'price_checked\s*=\s*"[^"]*"', f'price_checked = "{today}"', nb)
                s=s.replace(b,nb)
if apply and changed:
    open(p,'w').write(s); print(f"applied {changed} price change(s)")
elif changed: print(f"{changed} price(s) drifted — rerun with --apply to update")
else: print("all prices current")
PY
