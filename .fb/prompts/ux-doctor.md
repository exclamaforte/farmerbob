# Task: implement `fb doctor` — environment preflight

Rust workspace, already builds. Work only inside `crates/fb`.

Create `crates/fb/src/doctor.rs`, declare it from `main.rs` with `mod doctor;`, and
call it when the binary is run with the argument `doctor`.

`fb doctor` checks whether this machine can actually run farmerbob, and prints an
actionable report. farmerbob runs many AI coding agents in parallel, confines each in
a cgroup, and serializes their access to one GPU — so these checks are exactly the
things that silently make it not work.

## Checks to implement

Each check returns a status of `Ok`, `Warn`, or `Fail`, a message, and an optional
`fix` string (a command the user can run).

1. **cgroup v2** — `/sys/fs/cgroup/cgroup.controllers` exists.
2. **Delegated controllers** — read
   `/sys/fs/cgroup/user.slice/user-<uid>.slice/cgroup.controllers`. Require `cpu`,
   `memory`, `pids`. If `cpuset` is missing, that is a **Warn**, not a Fail, with fix:
   write `Delegate=cpu cpuset io memory pids` into
   `/etc/systemd/system/user@.service.d/delegate.conf` then `systemctl daemon-reexec`.
3. **systemd** — `systemd-run --version` runs.
4. **nvidia** — `nvidia-smi` runs; report driver version and GPU name. Missing is a Warn.
5. **git worktree support** — `git --version` >= 2.5.
6. **disk headroom** — free bytes on the home filesystem; Warn under 10 GiB.
7. **agent CLIs** — for each of `claude`, `codex`, `zcode`, `agy`, `opencode`, `ori`:
   is it on `PATH`? Report found/missing. Missing ones are Warn.

## Rules

- Get uid without extra crates: read `/proc/self/status` (the `Uid:` line) or use
  the `USER`/`HOME` env plus `/proc/self/loginuid`. Do NOT add a dependency.
- Never panic. A check that cannot run reports `Fail` with the error text.
- Exit code 0 if no `Fail`, 1 if any `Fail`.
- Support `--json`: print a JSON array of `{name, status, message, fix}` instead of
  the human table. `serde_json` is already available.
- Available deps (already in Cargo.toml, do not add more): clap, anyhow, serde,
  serde_json, toml, directories.
- Write unit tests for the pure parsing logic — split parsing (e.g. "given these
  controller-file contents, which are missing") into functions that take `&str` and
  test those. Do not write tests that require a GPU.
- `cargo build -p fb` and `cargo test -p fb` must both pass. Run them yourself and fix
  any failures before finishing.

When done, briefly state what you implemented.
