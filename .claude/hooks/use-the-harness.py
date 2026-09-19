#!/usr/bin/env python3
"""PreToolUse hook: refuse Bash that reads harness state instead of asking the harness.

Installed because the orchestrator kept reaching for `cat`/`python3`/`ls` against
farmerbob's own artefacts rather than the commands farmerbob exists to provide. Reading
the files by hand means the harness is never exercised, so its gaps stay invisible -- the
whole point of dogfooding it.

Exit 0 = allow. Exit 2 = block, and stderr is returned to the model as feedback.

This does NOT block writing code, editing specs, git, cargo, or launching waves. It blocks
exactly one thing: inspecting harness OUTPUT with a shell instead of with `fb`.
"""
import json
import re
import sys

LOGS = r"(\.local/share/farmerbob/logs|\$LOGS|\$\{LOGS\})"

# (pattern, what to run instead)
RULES = [
    (rf"\b(cat|head|tail|less|more|bat)\b[^|;&]*{LOGS}[^|;&]*critiques",
     "fb brief <task>  -- prints every critique with the metrics beside them"),
    (rf"\b(cat|head|tail|less|more|bat)\b[^|;&]*{LOGS}[^|;&]*\.score\.json",
     "fb brief <task>  -- the score record, read and ranked"),
    (rf"\b(cat|head|tail|less|more|bat)\b[^|;&]*{LOGS}[^|;&]*\.(promoted|proved)\.json",
     "fb brief <task>  -- promote/prove results are part of the brief"),
    (rf"\b(cat|head|tail|less|more|bat)\b[^|;&]*{LOGS}[^|;&]*speccheck",
     "fb brief <task>  -- spec critiques are surfaced there too"),
    # No [^|;&] guard here: a python -c body legitimately contains `;` inside quotes,
    # and excluding it let `python3 -c "import json; json.load(open('...'))"` straight
    # through -- which is exactly the evasion this rule exists to catch.
    (rf"python3?\b.*(json\.load|open\().*{LOGS}",
     "fb brief <task>  -- do not re-parse the harness's own JSON by hand"),
    (rf"\bls\b[^|;&]*{LOGS}[^|;&]*(critiques|score|promoted|proved)",
     "fb next  -- says which stage is missing for which task, and why"),
    (rf"\bls\b[^|;&]*\.local/share/farmerbob/worktrees",
     "fb next  -- reports worktrees waiting to be measured"),
]

def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except Exception:
        return 0
    if payload.get("tool_name") != "Bash":
        return 0
    command = payload.get("tool_input", {}).get("command", "")
    if not command:
        return 0
    # An explicit override, for the case where the harness itself is the thing broken
    # and you genuinely need to look underneath it. Say so out loud by typing it.
    if "FB_BYPASS_HARNESS=1" in command:
        return 0
    for pattern, instead in RULES:
        if re.search(pattern, command):
            print(
                "BLOCKED: that reads farmerbob's own output with a shell.\n"
                f"Use instead:  {instead}\n"
                "The harness is the thing under test. Reading its files by hand means its\n"
                "gaps stay invisible -- `fb next` reported a stale stage today and only the\n"
                "command showed it, never the directory listing.\n"
                "If the harness itself is broken and you must look underneath it, prefix the\n"
                "command with FB_BYPASS_HARNESS=1 and say why.",
                file=sys.stderr,
            )
            return 2
    return 0

if __name__ == "__main__":
    sys.exit(main())
