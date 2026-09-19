#!/usr/bin/env python3
"""Graft helper for fb-escalate crossx.

Kept as a file rather than a heredoc: embedding regexes full of backslashes inside a shell
heredoc inside a patch script mangled every one of them, twice.

  fb-escalate-graft.py <finder-src> <suite> <marker> <finder> <task>   extract + append
  fb-escalate-graft.py --into <suite> <target> <marker>                copy block into crate
  fb-escalate-graft.py --retract <suite> <target>                      remove last block
"""
import re
import sys

TAG = "\n// ESCALATED from cross-examination:"


def balanced(src, start):
    i = src.index("{", start)
    depth = 0
    for j in range(i, len(src)):
        if src[j] == "{":
            depth += 1
        elif src[j] == "}":
            depth -= 1
            if depth == 0:
                return src[start:j + 1]
    return None


def last_block(text):
    return re.search(re.escape(TAG) + r"(?:(?!" + re.escape(TAG) + r").)*$", text, re.S)


def main(argv):
    if argv[0] == "--retract":
        for p in argv[1:]:
            s = open(p).read()
            m = last_block(s)
            if m:
                open(p, "w").write(s[:m.start()] + "\n")
        return 0

    if argv[0] == "--into":
        suite, target, marker = argv[1:4]
        m = last_block(open(suite).read())
        if not m:
            return 0
        t = open(target).read()
        if marker not in t:
            open(target, "a").write("\n" + m.group(0))
        return 0

    src_p, suite, marker, finder, task = argv[:5]
    src = open(src_p).read()
    m = re.search(r"#\[cfg\(test\)\]\s*\n\s*mod tests\s*\{", src)
    if not m:
        print("  no test module in the finder's file")
        return 1
    body = balanced(src, m.start())
    if body is None:
        print("  unbalanced test module")
        return 1
    uses = "\n".join(l for l in src[:m.start()].splitlines() if l.startswith("use "))
    body = body.replace("mod tests", f"mod {marker}", 1)
    body = body.replace("{", "{\n" + uses, 1) if uses else body
    hdr = (f"{TAG} {finder}'s suite discriminated on {task}.\n"
           "// Not a CLAIM -- cross-examination found it directly. Kept only because it passes\n"
           "// against the merged winner, which is what separates a discovery from an\n"
           "// over-fitted suite.\n")
    open(suite, "a").write(hdr + body + "\n")
    print(f"  grafted {finder}'s suite as {marker}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
