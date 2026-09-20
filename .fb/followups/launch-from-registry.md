# Follow-up turn: launch-from-registry

Scored PASS, +10 tests, clippy clean, 497 lines. The headline fix works: codex-luna's recipe
now carries `--strict-config` and `-c model_reasoning_effort="xhigh"`, and your test pins the
full vector. codex-luna critiqued it and filed one CLAIM plus one judgement. Both are right.
Your worktree and session are intact; continue from there.

## 1. An unreadable registry silently reads an ancestor's (CLAIM, confirmed)

`crates/fb/src/launch.rs:154`:

```rust
match std::fs::read_to_string(dir.join("sources.toml")) {
    Ok(text) => return Some(text),
    Err(_) => dir = dir.parent()?.to_path_buf(),
}
```

Every read error walks to the parent. So an `FB_REPO` whose `sources.toml` EXISTS but cannot
be read -- permissions, a bad mount, a truncated write -- silently launches from whatever
registry a parent directory happens to hold, and reports success.

`FB_REPO` is the isolation override. A fallback that escapes it defeats the one thing it is
for, and makes a misconfigured run indistinguishable from a correct one. I fixed the
identical defect in `critique.rs` this afternoon, where `repo_file` fell back to a hardcoded
path for the same reason.

**Absent is not unreadable.** Walk to the parent only when the file is genuinely not there
(`ErrorKind::NotFound`). Any other error is an operational failure: return it, and let
`argv_for` refuse the arm by name. Pin both — a missing file still walks, an unreadable one
does not.

## 2. Test the launcher fallback you kept

Your handoff is explicit that `declared_launch_argv` stays as a compatibility bridge for
entries with no `cmd`, and that is the right call — but the critic is right that it is
untested, and it matters more than it looks: **32 of the 38 entries in `sources.toml` have no
`cmd`**, so that path launches most of the roster.

Add tests for it: one entry per distinct `kind` you handle, asserting the argv, and one with
an unanticipated `kind` asserting what it does rather than leaving it to be discovered.

Do NOT migrate the registry — that is data work and it is mine.

## Rules

Change only `crates/fb/src/launch.rs`. Keep the workspace rustfmt-clean and clippy-clean and
every existing test passing. Run `timeout 120 cargo test -q -p fb` while you work.
