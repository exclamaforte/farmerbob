//! The launcher: how to invoke each arm, read from `sources.toml`.
//!
//! The registry is the one launch table. A hand-written `match arm { ... }` used to live
//! here beside it, and the two disagreed: `sources.toml` gave codex-luna `--strict-config`
//! and `-c model_reasoning_effort="xhigh"` while the table's copy dropped both, so the arm
//! ran at default effort while the bandit attributed its every cost and ability figure to a
//! configuration that never ran. Nothing compared the two, so nothing reported it -- the
//! same shape as gemini-38-flash before it (beads farmerbob-ozv0, farmerbob-0919). The
//! table is deleted: an arm launches only the way its registry entry declares, and an arm
//! with no entry is refused by name rather than given a guess, because a wrong launcher
//! runs the wrong model and bills the wrong account.
//!
//! [`recipes`] and [`argv_from_recipe`] are pure: text in, argv out, no file system, no
//! clock, no environment. Only [`argv_for`] reads the registry from disk, so the whole
//! launch decision can be tested without spawning anything and without the real registry.

use farmerbob_core::measurement::Measurement;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One arm's launch recipe, as `sources.toml` declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The `cmd` key: the program to run.
    pub cmd: String,
    /// The `args` key, verbatim, placeholders unexpanded.
    pub args: Vec<String>,
    /// The `continue_args` key, or empty when the arm declares none.
    ///
    /// A session-resuming arm needs different arguments, and which ones is the arm's
    /// business, not this module's.
    pub continue_args: Vec<String>,
}

/// Every arm's recipe, by arm name, read from a `sources.toml` text.
///
/// One entry per `[source.*]` table that declares a `cmd`, keyed by the table's name byte
/// for byte. An entry with no `cmd` has no recipe and is absent from the map: a recipe
/// without a program is not a recipe, and inventing one is what [`argv_for`] refuses to do.
/// The same holds for an entry whose `args` or `continue_args` is present but is not an
/// array of strings -- arguments that cannot be read verbatim are an unreadable recipe, and
/// launching a fraction of a recipe silently would drop flags by omission.
///
/// Text that does not parse as TOML, or that holds no `[source.*]` tables at all, yields an
/// empty map and no error: no recipes means [`argv_for`] refuses every arm, which fails
/// closed, where an error would tempt a caller into a fallback.
pub fn recipes(sources_toml: &str) -> BTreeMap<String, Recipe> {
    let Ok(doc) = toml::from_str::<toml::Value>(sources_toml) else {
        return BTreeMap::new();
    };
    let Some(sources) = doc.get("source").and_then(toml::Value::as_table) else {
        return BTreeMap::new();
    };
    sources
        .iter()
        .filter_map(|(name, entry)| recipe_of(entry).map(|r| (name.clone(), r)))
        .collect()
}

/// The argv for one arm, from its recipe.
///
/// Placeholders are substituted in every argument: `{prompt}` becomes `prompt`, `{worktree}`
/// becomes `wd`. An argument containing neither is passed through byte for byte; the empty
/// argument survives, because dropping it would shift every argument after it. A placeholder
/// this module does not know -- `{model}`, say -- passes through unexpanded: a silently
/// emptied argument is indistinguishable from one the registry omitted.
///
/// `continue_session` appends `continue_args` after `args`; it does not replace them, since
/// an arm resuming a session still needs its model and its approval flags. An arm that
/// declares no `continue_args` resumes to the same argv it starts with, which is correct for
/// launchers that resume without a flag.
pub fn argv_from_recipe(
    recipe: &Recipe,
    wd: &Path,
    prompt: &str,
    continue_session: bool,
) -> Vec<String> {
    let worktree = wd.to_string_lossy().into_owned();
    let mut argv = Vec::with_capacity(recipe.args.len() + recipe.continue_args.len() + 1);
    argv.push(recipe.cmd.clone());
    argv.extend(
        recipe
            .args
            .iter()
            .map(|a| expand_placeholders(a, prompt, &worktree)),
    );
    if continue_session {
        argv.extend(
            recipe
                .continue_args
                .iter()
                .map(|a| expand_placeholders(a, prompt, &worktree)),
        );
    }
    argv
}

/// The argv for one arm, read from the registry beside the repository.
///
/// The registry declares a launch in one of two forms. The primary form is a recipe --
/// `cmd` and `args`, optionally `continue_args` -- and it always wins. The legacy form
/// declares only a `launcher` program plus `kind` and `model`, the shape `fb-dispatch.sh`
/// launched before the registry grew recipes; the opencode and zcode arms still declare
/// that way (`launcher = "opencode"   # NOT `ori opencode` -- ori replaces the env`), so
/// they launch from what those keys say rather than being refused for lacking a `cmd` they
/// were never given. An arm with NO entry in the registry is refused:
/// [`NoLauncher::UnknownArm`], naming it, never a default. A wrong launcher runs the wrong
/// model and bills the wrong account.
///
/// `model` is accepted to keep the signature callers already hold and is not consulted:
/// the model comes from the registry, either spelled inside the recipe's arguments or read
/// from the entry's own `model` key.
pub fn argv_for(
    arm: &str,
    _model: &str,
    wd: &Path,
    prompt: &str,
    continue_session: bool,
) -> Result<Vec<String>, NoLauncher> {
    let text = registry_text().ok_or_else(|| NoLauncher::UnknownArm(arm.to_string()))?;
    if let Some(recipe) = recipes(&text).remove(arm) {
        return Ok(argv_from_recipe(&recipe, wd, prompt, continue_session));
    }
    // No recipe. The entry may still declare the legacy `launcher` form; an arm with no
    // entry at all -- or an entry that declares neither form -- is refused by name.
    declared_launch_argv(&text, arm, wd, prompt, continue_session)
        .ok_or_else(|| NoLauncher::UnknownArm(arm.to_string()))
}

/// Why an arm cannot be launched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoLauncher {
    /// The arm name matches no entry in the table. NOT a guess at a default: a wrong
    /// launcher runs the wrong model and bills the wrong account.
    UnknownArm(String),
}

/// The registry text, read from the repository root.
///
/// Lookup starts at `paths::repo()` and walks upward, the way git finds `.git`: cargo runs
/// a test binary from the package directory, so the repository root can be one level above
/// what `paths::repo()` reports at test time.
fn registry_text() -> Option<String> {
    registry_text_from(&crate::paths::repo())
}

/// The registry text at `start`, or in the nearest ancestor that holds one.
///
/// ABSENT IS NOT UNREADABLE. Only `ErrorKind::NotFound` sends the walk to the parent. A
/// `sources.toml` that exists but cannot be read -- permissions, a bad mount, a directory
/// where the file belongs -- stops the walk and returns `None`: an operational failure that
/// [`argv_for`] answers by refusing the arm by name, never by silently launching from
/// whatever registry an ancestor happens to hold. `FB_REPO` is the isolation override, and
/// a fallback that escapes it makes a misconfigured run indistinguishable from a correct
/// one. A registry that reads but does not parse is a different case and stays the one that
/// was named: [`recipes`] will read no recipes out of it and every arm will be refused,
/// which also fails closed.
fn registry_text_from(start: &Path) -> Option<String> {
    let mut dir = start.to_path_buf();
    loop {
        match std::fs::read_to_string(dir.join("sources.toml")) {
            Ok(text) => return Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
        dir = dir.parent()?.to_path_buf();
    }
}

/// One registry entry as a [`Recipe`], or `None` when the entry declares no `cmd` or its
/// argument keys cannot be read as string arrays.
fn recipe_of(entry: &toml::Value) -> Option<Recipe> {
    let cmd = entry.get("cmd")?.as_str()?.to_string();
    let args = match entry.get("args") {
        Some(v) => string_list(v)?,
        None => Vec::new(),
    };
    let continue_args = match entry.get("continue_args") {
        Some(v) => string_list(v)?,
        None => Vec::new(),
    };
    Some(Recipe {
        cmd,
        args,
        continue_args,
    })
}

/// A TOML value that must be an array whose every element is a string.
fn string_list(value: &toml::Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|item| item.as_str().map(str::to_string))
        .collect()
}

/// The argv for an arm whose entry declares the legacy form: a `launcher` program plus
/// `kind` and `model`, and no `cmd`. This is the shape `fb-dispatch.sh` launched, and the
/// argv reproduces it exactly -- `opencode run --dir <wd> --auto [-m model] prompt`, under
/// `ori` where the launcher names it, and `<launcher> [--prompt prompt]` for the rest --
/// so an arm that has not yet grown a recipe keeps the launch it has always had. `None`
/// when the arm has no entry or its entry declares neither form.
fn declared_launch_argv(
    text: &str,
    arm: &str,
    wd: &Path,
    prompt: &str,
    continue_session: bool,
) -> Option<Vec<String>> {
    let doc = toml::from_str::<toml::Value>(text).ok()?;
    let entry = doc.get("source")?.get(arm)?;
    let launcher = entry.get("launcher")?.as_str()?;
    let kind = entry
        .get("kind")
        .and_then(toml::Value::as_str)
        .unwrap_or("");
    let model = entry
        .get("model")
        .and_then(toml::Value::as_str)
        .unwrap_or("");
    let worktree = wd.to_string_lossy().into_owned();
    let mut argv: Vec<String> = launcher.split_whitespace().map(str::to_string).collect();
    if kind == "opencode" {
        argv.extend([
            "run".to_string(),
            "--dir".to_string(),
            worktree,
            "--auto".to_string(),
        ]);
        if continue_session {
            argv.push("--continue".to_string());
        }
        if !model.is_empty() {
            argv.extend(["-m".to_string(), model.to_string()]);
        }
        argv.push(prompt.to_string());
    } else {
        if continue_session {
            argv.push("--continue".to_string());
        }
        argv.extend(["--prompt".to_string(), prompt.to_string()]);
    }
    Some(argv)
}

/// Substitute `{prompt}` and `{worktree}` in one argument. Anything else in braces --
/// including an unterminated `{` -- is copied through untouched. Replacement text is never
/// rescanned, so a prompt that itself contains `{worktree}` reaches the arm verbatim rather
/// than expanding a second time.
fn expand_placeholders(arg: &str, prompt: &str, worktree: &str) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                match &after[..close] {
                    "prompt" => out.push_str(prompt),
                    "worktree" => out.push_str(worktree),
                    unknown => {
                        out.push('{');
                        out.push_str(unknown);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The confinement wrapper every arm is run under: this binary, as `fb isolated`.
///
/// It must be a real executable rather than a shell function, because systemd-run execs its
/// argument directly and reports "Failed to find executable" for a function. Two runs died
/// at 0s that way.
fn isolated() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| crate::paths::repo().join("target/debug/fb"))
}

/// The model an arm runs, from the registry. Empty when the registry does not say, which
/// the launchers that need it will reject for themselves.
pub fn model_for(arm: &str) -> String {
    crate::sources::Registry::load(&crate::paths::repo().join("sources.toml"))
        .ok()
        .and_then(|r| r.sources.get(arm).map(|s| s.model.clone()))
        .unwrap_or_default()
}

/// Run one arm in `wd`, capturing everything it says.
///
/// The output is RETURNED, not discarded. `fb critique` ran its launcher with
/// `Stdio::null()` on both streams and wrote a zero-byte log beside the prompt, so a critic
/// that crashed on startup and one that ran and chose to write nothing were byte-identical.
/// Three runs could not be diagnosed at all before that was fixed.
/// Resource limits for a confined run. `None` runs unconfined, which is what a critic got
/// until this port -- no scope, no memory cap, no timeout (bead farmerbob-vi7).
pub struct Scope<'a> {
    /// The systemd unit name. Must be unique per run.
    pub unit: &'a str,
    /// Hard memory cap, e.g. "3G".
    pub memory_max: String,
    /// Seconds before systemd terminates the whole scope -- which reaches the agent inside
    /// bwrap and everything it spawned, where a `timeout` wrapper would not.
    pub runtime_max_s: u64,
}

pub fn launch(
    arm: &str,
    prompt: &str,
    wd: &Path,
    continue_session: bool,
    env: &[(&str, PathBuf)],
) -> Measurement<String> {
    launch_in(arm, prompt, wd, continue_session, env, None)
}

/// Launch, optionally inside a systemd scope.
pub fn launch_in(
    arm: &str,
    prompt: &str,
    wd: &Path,
    continue_session: bool,
    env: &[(&str, PathBuf)],
    scope: Option<Scope<'_>>,
) -> Measurement<String> {
    let model = model_for(arm);
    let argv = match argv_for(arm, &model, wd, prompt, continue_session) {
        Ok(argv) => argv,
        Err(NoLauncher::UnknownArm(a)) => {
            return Measurement::instrument_failed(&format!(
                "no launcher for arm {a}: sources.toml declares no launch recipe for it"
            ));
        }
    };
    // ifm-* arms authenticate from a secrets file the launcher reads itself; every other
    // family authenticates from a credential the isolated environment must carry.
    let mut cmd = match &scope {
        Some(s) => {
            let mut c = Command::new("systemd-run");
            c.args([
                "--user",
                "--scope",
                "--quiet",
                &format!("--unit={}", s.unit),
                "-p",
                &format!("MemoryMax={}", s.memory_max),
                "-p",
                &format!("RuntimeMaxSec={}", s.runtime_max_s),
                "-p",
                "TasksMax=2048",
                "--",
            ]);
            c.arg(isolated()).arg("isolated");
            c
        }
        None => {
            let mut c = Command::new(isolated());
            c.arg("isolated");
            c
        }
    };
    cmd.arg(wd).args(&argv).current_dir(wd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.is_empty() {
                text.push_str("\n=== stderr ===\n");
                text.push_str(&err);
            }
            if out.status.success() {
                Measurement::observed(text)
            } else {
                Measurement::instrument_failed(&format!(
                    "{arm} exited with {}: {}",
                    out.status,
                    text.chars().take(400).collect::<String>()
                ))
            }
        }
        Err(e) => Measurement::instrument_failed(&format!("cannot spawn {arm}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A list of arguments as the registry would hold them.
    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// codex-luna's recipe exactly as `sources.toml` declares it. This is the entry the
    /// deleted hardcoded table disagreed with: `--strict-config` and the `-c` pair were
    /// dropped, so the arm ran at default effort while the registry claimed xhigh.
    fn codex_luna() -> Recipe {
        Recipe {
            cmd: "codex".to_string(),
            args: list(&[
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--strict-config",
                "-c",
                "model_reasoning_effort=\"xhigh\"",
                "-m",
                "gpt-5.6-luna",
                "{prompt}",
            ]),
            continue_args: Vec::new(),
        }
    }

    /// An arm that resumes by flag, with its own spelling of how. Which arguments resume a
    /// session is the registry's business now; launch.rs once decided `resume --last` for
    /// codex and `--continue` for everyone else, by prefix, in a table of its own.
    fn resuming() -> Recipe {
        Recipe {
            cmd: "opencode".to_string(),
            args: list(&["run", "--auto", "-m", "vendor/model", "{prompt}"]),
            continue_args: list(&["--continue", "-m", "vendor/model", "{prompt}"]),
        }
    }

    /// The recipe is emitted in declared order, command at index 0, every arg in the
    /// position the registry gives it -- including the two the old table lost.
    #[test]
    fn the_recipe_is_emitted_in_declared_order_with_the_command_first() {
        let argv = argv_from_recipe(&codex_luna(), Path::new("/w"), "fix the gate", false);
        assert_eq!(
            argv,
            list(&[
                "codex",
                "exec",
                "--dangerously-bypass-approvals-and-sandbox",
                "--strict-config",
                "-c",
                "model_reasoning_effort=\"xhigh\"",
                "-m",
                "gpt-5.6-luna",
                "fix the gate",
            ])
        );
    }

    /// `{prompt}` and `{worktree}` expand whether the argument is exactly the placeholder
    /// or embeds it (`--add-dir={worktree}` is how every agy arm gets its directory).
    #[test]
    fn placeholders_expand_alone_or_embedded() {
        let recipe = Recipe {
            cmd: "agy".to_string(),
            args: list(&["{prompt}", "--add-dir={worktree}", "before-{prompt}-after"]),
            continue_args: Vec::new(),
        };
        let argv = argv_from_recipe(&recipe, Path::new("/some/wt"), "P", false);
        assert_eq!(
            argv,
            list(&["agy", "P", "--add-dir=/some/wt", "before-P-after"])
        );
    }

    /// An argument with no placeholder passes through byte for byte. The pinned one is
    /// `-c model_reasoning_effort="xhigh"` itself: quote and equals intact, because
    /// re-quoting or splitting it is the defect this module exists to remove.
    #[test]
    fn an_argument_without_a_placeholder_passes_through_byte_for_byte() {
        let recipe = Recipe {
            cmd: "codex".to_string(),
            args: list(&["model_reasoning_effort=\"xhigh\"", "--odd=key=with=equals"]),
            continue_args: Vec::new(),
        };
        let argv = argv_from_recipe(&recipe, Path::new("/w"), "P", false);
        assert_eq!(
            argv,
            list(&[
                "codex",
                "model_reasoning_effort=\"xhigh\"",
                "--odd=key=with=equals"
            ])
        );
    }

    /// Continuing APPENDS `continue_args` and keeps every regular argument: a resumed arm
    /// still needs its model and its approval flags.
    #[test]
    fn continuation_appends_continue_args_and_keeps_every_regular_arg() {
        let resumed = argv_from_recipe(&resuming(), Path::new("/w"), "P", true);
        assert_eq!(
            resumed,
            list(&[
                "opencode",
                "run",
                "--auto",
                "-m",
                "vendor/model",
                "P",
                "--continue",
                "-m",
                "vendor/model",
                "P",
            ])
        );
    }

    /// The other state of the same recipe: a fresh turn carries none of the continuation.
    /// Resuming when a fresh turn was wanted makes an arm re-answer its previous prompt.
    #[test]
    fn a_fresh_turn_never_carries_the_continue_args() {
        let fresh = argv_from_recipe(&resuming(), Path::new("/w"), "P", false);
        assert_eq!(
            fresh,
            list(&["opencode", "run", "--auto", "-m", "vendor/model", "P"])
        );
    }

    /// An arm with no `continue_args` resumes to the argv it starts with. Not an error:
    /// many launchers resume without a flag, and codex-luna's registry entry declares none.
    #[test]
    fn continuation_without_declared_continue_args_changes_nothing() {
        let recipe = codex_luna();
        assert_eq!(
            argv_from_recipe(&recipe, Path::new("/w"), "P", true),
            argv_from_recipe(&recipe, Path::new("/w"), "P", false)
        );
    }

    /// The registry names every arm it launches, exactly. launch.rs once routed whole
    /// PREFIXES (or-* through ori, oc-* through opencode) from its own table; that table is
    /// gone because it disagreed with sources.toml about codex-luna's flags, so a name now
    /// launches only what the registry declares under that exact name.
    #[test]
    fn an_arm_name_matches_only_its_own_registry_entry() {
        let book = recipes("[source.oc-x]\ncmd = \"opencode\"\n");
        assert_eq!(book.get("oc-x").map(|r| r.cmd.as_str()), Some("opencode"));
        assert!(
            !book.contains_key("oc-x-and-more"),
            "a prefix is not a match"
        );
        assert!(
            !book.contains_key("OC-X"),
            "the key is the name, byte for byte"
        );
    }

    /// An arm the registry gives no recipe is REFUSED, by name, never defaulted. A wrong
    /// launcher runs the wrong model and bills the wrong account.
    #[test]
    fn an_arm_without_a_recipe_is_refused_by_name_never_defaulted() {
        let invented = "no-such-arm-in-any-registry";
        assert_eq!(
            argv_for(invented, "m", Path::new("/w"), "P", false),
            Err(NoLauncher::UnknownArm(invented.to_string()))
        );
    }

    /// The registry beside this repository is now the only launcher table, read from disk.
    /// codex-luna is its proof: the entry carries `--strict-config` and the effort pair the
    /// deleted table dropped, so argv_for must emit them in the registry's positions.
    #[test]
    fn argv_for_launches_codex_luna_the_way_the_registry_declares() {
        let argv = match argv_for(
            "codex-luna",
            "gpt-5.6-luna",
            Path::new("/tmp/wt"),
            "P",
            false,
        ) {
            Ok(a) => a,
            Err(e) => panic!("codex-luna is registered with a cmd, it must resolve: {e:?}"),
        };
        assert_eq!(argv[0], "codex");
        assert!(
            argv.contains(&"--strict-config".to_string()),
            "the flag the old table dropped must be back: {argv:?}"
        );
        let at = argv.iter().position(|a| a == "-c").expect("the -c pair");
        assert_eq!(argv[at + 1], "model_reasoning_effort=\"xhigh\"");
        assert!(argv.contains(&"P".to_string()), "the prompt must reach it");
    }

    /// ABSENT IS NOT UNREADABLE. A missing sources.toml sends the lookup to the parent --
    /// that is how the real registry is found from the package dir. But a registry that
    /// EXISTS and cannot be read stops the walk: falling through would launch from a
    /// registry the caller's FB_REPO never named and report success. The unreadable fixture
    /// is a directory where the file belongs, which read_to_string refuses without
    /// depending on process privileges.
    #[test]
    fn a_missing_registry_is_walked_past_but_an_unreadable_one_stops_the_walk() {
        let root = std::env::temp_dir().join("fb-launch-registry-walk");
        let _ = std::fs::remove_dir_all(&root);
        let inner = root.join("inner");
        std::fs::create_dir_all(&inner).expect("fixture dir");
        std::fs::write(root.join("sources.toml"), "[source.a]\ncmd = \"x\"\n")
            .expect("fixture registry");
        assert!(
            registry_text_from(&inner).is_some(),
            "a missing file still walks to the parent that holds one"
        );

        std::fs::create_dir(inner.join("sources.toml")).expect("unreadable fixture");
        assert!(
            registry_text_from(&inner).is_none(),
            "an unreadable registry must stop the walk, never fall through to the ancestor"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An entry with a `cmd` and no `args` yields just the command, and is not an error.
    /// Declaring no arguments is a choice a registry may make.
    #[test]
    fn declaring_no_arguments_yields_the_bare_command() {
        let book = recipes("[source.bare]\ncmd = \"codex\"\n");
        let recipe = book.get("bare").expect("cmd alone is a recipe");
        assert_eq!(
            argv_from_recipe(recipe, Path::new("/w"), "P", false),
            list(&["codex"])
        );
    }

    /// The EMPTY `args` list is the same answer as no `args` key: the command alone. Both
    /// spellings are the registry choosing to pass nothing.
    #[test]
    fn an_empty_args_list_yields_the_bare_command_too() {
        let book = recipes("[source.nothing]\ncmd = \"codex\"\nargs = []\n");
        let recipe = book
            .get("nothing")
            .expect("an empty args is still a recipe");
        assert_eq!(
            argv_from_recipe(recipe, Path::new("/w"), "P", false),
            list(&["codex"])
        );
    }

    /// A `sources.toml` with no `[source.*]` tables -- or no TOML at all -- holds no
    /// recipes, and that is an empty map rather than an error.
    #[test]
    fn sourceless_or_unparseable_toml_yields_no_recipes() {
        assert!(recipes("").is_empty());
        assert!(recipes("status = \"just a top-level key\"\n").is_empty());
        assert!(recipes("[this is not = toml\n").is_empty());
    }

    /// An entry with no `cmd` has no recipe and is absent from the map -- but only that
    /// entry: its neighbours with a `cmd` still launch. A recipe without a program is not a
    /// recipe, and inventing one is what argv_for refuses to do.
    #[test]
    fn an_entry_without_a_cmd_is_absent_while_its_neighbours_launch() {
        let book = recipes(
            "[source.shell-only]\nstatus = \"verified\"\nmodel = \"m\"\n\
             [source.real]\ncmd = \"codex\"\n",
        );
        assert!(!book.contains_key("shell-only"));
        assert_eq!(book.get("real").map(|r| r.cmd.as_str()), Some("codex"));
    }

    /// An argument that is the empty string survives. `codex` takes empty arguments in
    /// some positions; dropping one shifts every argument after it.
    #[test]
    fn an_empty_string_argument_survives() {
        let recipe = Recipe {
            cmd: "x".to_string(),
            args: list(&["", "{prompt}", ""]),
            continue_args: Vec::new(),
        };
        assert_eq!(
            argv_from_recipe(&recipe, Path::new("/w"), "P", false),
            list(&["x", "", "P", ""])
        );
    }

    /// A placeholder this module does not know passes through UNEXPANDED, never blanked:
    /// a silently emptied argument is indistinguishable from one the registry omitted.
    #[test]
    fn an_unknown_placeholder_passes_through_unexpanded() {
        let recipe = Recipe {
            cmd: "x".to_string(),
            args: list(&["--model={model}", "{prompt}"]),
            continue_args: Vec::new(),
        };
        assert_eq!(
            argv_from_recipe(&recipe, Path::new("/w"), "P", false),
            list(&["x", "--model={model}", "P"])
        );
    }

    // ---- the legacy `launcher` declaration: the compatibility bridge ------------------
    //
    // Most of the roster still declares `launcher` + `kind` + `model` with no `cmd`, so
    // this path launches more arms than the recipe path does. These tests pin its argv per
    // `kind` directly against synthetic registry text rather than through `argv_for` and
    // the real sources.toml, because the registry is data the operator will one day
    // migrate -- a test keyed to today's entries would break on the migration it ought to
    // welcome.

    /// The opencode spelling, with and without continuation: tokens from the `launcher`
    /// key, then `run --dir --auto`, continuation ahead of the model, the entry's model,
    /// the prompt last. `fb-dispatch.sh` launched exactly this, and the deleted hardcoded
    /// table reproduced it; the bridge must keep doing so until the registry migrates.
    #[test]
    fn the_legacy_opencode_declaration_launches_the_way_fb_dispatch_did() {
        let text = concat!(
            "[source.or-x]\n",
            "launcher = \"ori opencode\"\n",
            "kind = \"opencode\"\n",
            "model = \"vendor/model\"\n"
        );
        let fresh = declared_launch_argv(text, "or-x", Path::new("/w"), "P", false)
            .expect("a declared launcher launches");
        assert_eq!(
            fresh,
            list(&[
                "ori",
                "opencode",
                "run",
                "--dir",
                "/w",
                "--auto",
                "-m",
                "vendor/model",
                "P"
            ])
        );
        let resumed = declared_launch_argv(text, "or-x", Path::new("/w"), "P", true)
            .expect("continuation changes spelling, not launchability");
        assert_eq!(
            resumed,
            list(&[
                "ori",
                "opencode",
                "run",
                "--dir",
                "/w",
                "--auto",
                "--continue",
                "-m",
                "vendor/model",
                "P"
            ])
        );
    }

    /// An entry with no `model` key omits the whole `-m` pair. Emitting `-m` with an empty
    /// argument in its place would shift every argument after it -- the same shape as the
    /// dropped-empty-argument defect the recipe path refuses.
    #[test]
    fn a_legacy_entry_without_a_model_omits_the_whole_model_pair() {
        let text = "[source.oc-x]\nlauncher = \"opencode\"\nkind = \"opencode\"\n";
        let argv = declared_launch_argv(text, "oc-x", Path::new("/w"), "P", false)
            .expect("a declared launcher launches");
        assert_eq!(
            argv,
            list(&["opencode", "run", "--dir", "/w", "--auto", "P"])
        );
    }

    /// The second spelling: everything that is not opencode hands the prompt over with
    /// `--prompt`, continuation inserted before it. glm-53-flash's zcode entry is the live
    /// case.
    #[test]
    fn the_legacy_cli_declaration_hands_the_prompt_with_a_flag() {
        let text = "[source.glm]\nlauncher = \"zcode\"\nkind = \"cli\"\n";
        let fresh = declared_launch_argv(text, "glm", Path::new("/w"), "P", false)
            .expect("a declared launcher launches");
        assert_eq!(fresh, list(&["zcode", "--prompt", "P"]));
        let resumed = declared_launch_argv(text, "glm", Path::new("/w"), "P", true)
            .expect("continuation changes spelling, not launchability");
        assert_eq!(resumed, list(&["zcode", "--continue", "--prompt", "P"]));
    }

    /// A `kind` this module has never heard of is neither an error nor a refusal: the
    /// launcher still runs and the prompt still reaches it by the default spelling. Pinned
    /// here so that behaviour is a decision on record, not an accident discovered in a
    /// production log.
    #[test]
    fn an_unanticipated_kind_still_launches_by_the_default_prompt_spelling() {
        let text = "[source.next]\nlauncher = \"futurecli\"\nkind = \"holographic\"\n";
        let argv = declared_launch_argv(text, "next", Path::new("/w"), "P", false)
            .expect("an unanticipated kind launches by the default spelling");
        assert_eq!(argv, list(&["futurecli", "--prompt", "P"]));
    }

    /// An entry that declares NEITHER form -- no `cmd`, no `launcher` -- has no launch.
    /// `argv_for` turns this `None` into a refusal by name; it must never become a default.
    #[test]
    fn an_entry_declaring_neither_form_has_no_launch() {
        let text = "[source.ghost]\nstatus = \"verified\"\nmodel = \"m\"\n";
        assert!(
            declared_launch_argv(text, "ghost", Path::new("/w"), "P", false).is_none(),
            "no declared program, no launch"
        );
    }
}
