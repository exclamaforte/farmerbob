# Follow-up turn: name-collisions

Your implementation scored PASS, 1938 tests, clippy clean. gemini-38-flash reviewed it and
raised one defect and two weaknesses. All three are in your own new file. Your worktree and
session are intact; continue from where you stopped.

## 1. A tuple struct keeps its parameter list in the name (CLAIM, confirmed)

```
public_types("pub struct Foo(u32);")
  expected: ["Foo"]
  actual:   ["Foo(u32)"]
```

The delimiter set at `name_collision.rs:105` is `'<' | '{' | ';' | '=' | ':'`. It has no
`'('`, so the scan runs past the parameter list to the trailing `;`.

This defeats the module's purpose, not just its tidiness: a tuple struct `Foo` never matches
`pub enum Foo` in another module, so a real collision is reported as none. Confirm with the
critic's second trigger:

```
collisions(&[
    Module { stem: "a".into(), source: "pub struct Foo(u32);".into() },
    Module { stem: "b".into(), source: "pub enum Foo {}".into() },
])
  expected: one Collision { name: "Foo", modules: ["a", "b"] }
  actual:   []
```

Add `'('` to the delimiter set, and pin both cases.

## 2. `brace_depth` counts braces inside strings and comments

The depth tracker counts raw `{` and `}` without lexing string literals, char literals or
comments. One unmatched brace inside a string or a comment in a test module skews the depth
permanently, and every public item after it in that file is silently dropped.

Silently is the problem. A file whose items vanish reports no collisions, which is
indistinguishable from a file that genuinely has none.

You do not need a full lexer. Handle the common cases — a `//` comment to end of line, and
`"` string literals with `\"` escapes — and add a test with a brace inside each.

## 3. Stacked attributes defeat the `#[cfg(test)]` exclusion

`pending_test_cfg` is reset by any non-comment line, so this is not excluded:

```rust
#[cfg(test)]
#[allow(dead_code)]
mod tests { ... }
```

Attributes stack in ordinary Rust. Let another `#[...]` line keep the flag set rather than
clearing it, and pin it.

## Rules

Change only `crates/farmerbob-core/src/name_collision.rs`. Keep every clause of the original
spec satisfied — the existing tests must still pass. Keep the workspace rustfmt-clean and
clippy-clean. Run `timeout 120 cargo test -q -p farmerbob-core` while you work.
