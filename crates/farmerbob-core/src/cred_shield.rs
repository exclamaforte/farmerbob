//! Whether an agent launcher's credentials survive XDG isolation.
//!
//! Every dispatched run gets private `XDG_DATA_HOME`, `XDG_STATE_HOME` and
//! `XDG_CACHE_HOME` directories, so a launcher that keeps its provider keys
//! under one of those bases finds them gone. A missing credential is
//! indistinguishable from a remote outage once the launcher is running, so
//! this module answers the question before launch: which credentials does
//! the launcher need, and will the private environment actually contain
//! them?
//!
//! The module performs no I/O. [`check`] takes a `present` closure that
//! answers whether a credential exists in the environment the run will
//! actually get, so the caller decides how and where to look.

/// A credential file an agent launcher reads, expressed relative to the XDG
/// base directory it lives under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// Which XDG base the path is relative to.
    pub base: XdgBase,
    /// Path beneath that base, e.g. `"opencode/auth.json"`. Never absolute.
    pub rel: String,
}

/// The XDG base directory a credential lives under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdgBase {
    /// `$XDG_DATA_HOME`
    Data,
    /// `$XDG_CONFIG_HOME`
    Config,
    /// `$XDG_STATE_HOME`
    State,
}

/// What a launcher needs in order to authenticate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Needs {
    /// The launcher reads these files, all of which must be present.
    Files(Vec<Credential>),
    /// The launcher authenticates from the process environment, not from a
    /// file, so isolating XDG cannot break it.
    Environment,
    /// This module does not know how the launcher authenticates. NOT a claim
    /// that it needs nothing.
    Unknown,
}

/// What the named launcher needs.
///
/// Known launchers: `"opencode"` reads `XDG_DATA_HOME/opencode/auth.json`;
/// `"ifm"` and `"ori"` are `Environment`. Any other name is `Unknown`.
/// At least these; a caller may not assume the list is closed.
pub fn needs(launcher: &str) -> Needs {
    match launcher {
        "opencode" => Needs::Files(vec![Credential {
            base: XdgBase::Data,
            rel: String::from("opencode/auth.json"),
        }]),
        "ifm" | "ori" => Needs::Environment,
        _ => Needs::Unknown,
    }
}

/// Whether an isolated environment will actually satisfy a launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shielded {
    /// Every needed credential is present. Safe to launch.
    Satisfied,
    /// Credentials are needed and these are missing. Carries them in the
    /// order `needs` listed them. Launching WILL fail, and it will fail
    /// looking like something else.
    Missing(Vec<Credential>),
    /// Nothing is known about this launcher, so nothing is claimed. A caller
    /// must not read this as `Satisfied`.
    Unchecked,
}

/// Check a launcher against a private environment.
///
/// `present` answers whether a given credential exists in the environment the
/// run will actually get. This module never touches the filesystem.
pub fn check(launcher: &str, present: &impl Fn(&Credential) -> bool) -> Shielded {
    match needs(launcher) {
        Needs::Files(files) => {
            let missing: Vec<Credential> = files.into_iter().filter(|c| !present(c)).collect();
            if missing.is_empty() {
                Shielded::Satisfied
            } else {
                Shielded::Missing(missing)
            }
        }
        Needs::Environment => Shielded::Satisfied,
        Needs::Unknown => Shielded::Unchecked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opencode_credential() -> Credential {
        Credential {
            base: XdgBase::Data,
            rel: String::from("opencode/auth.json"),
        }
    }

    /// Clause 1: exactly one credential, under Data, at the documented path.
    #[test]
    fn needs_opencode_is_one_data_credential() {
        match needs("opencode") {
            Needs::Files(files) => {
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].base, XdgBase::Data);
                assert_eq!(files[0].rel, "opencode/auth.json");
            }
            other => panic!("opencode must need files, got {other:?}"),
        }
    }

    /// Clause 2: both environment-authenticated launchers are Environment.
    #[test]
    fn needs_ifm_and_ori_are_environment() {
        assert_eq!(needs("ifm"), Needs::Environment);
        assert_eq!(needs("ori"), Needs::Environment);
    }

    /// Clause 3: a name nothing registered is Unknown — not Environment and
    /// not `Files(vec![])`, both of which claim things this module cannot
    /// know. Equality with `Unknown` pins both exclusions.
    #[test]
    fn needs_unregistered_name_is_unknown() {
        assert_eq!(needs("something-nobody-registered"), Needs::Unknown);
    }

    /// Clause 4: with every credential absent, the missing list is exactly
    /// the one credential `needs` returned.
    #[test]
    fn check_opencode_all_absent_lists_the_credential() {
        let absent = |_c: &Credential| false;
        assert_eq!(
            check("opencode", &absent),
            Shielded::Missing(vec![opencode_credential()])
        );
    }

    /// Clause 5: with the credential present, the launcher is satisfied.
    #[test]
    fn check_opencode_all_present_is_satisfied() {
        let present = |_c: &Credential| true;
        assert_eq!(check("opencode", &present), Shielded::Satisfied);
    }

    /// Clause 6: the same always-false answer that condemns opencode leaves
    /// an environment-authenticated launcher satisfied. Paired with the
    /// clause 4 test deliberately — that pairing is the whole distinction.
    #[test]
    fn check_ifm_satisfied_despite_absent_environment() {
        let absent = |_c: &Credential| false;
        assert_eq!(check("ifm", &absent), Shielded::Satisfied);
        assert_eq!(check("ori", &absent), Shielded::Satisfied);
    }

    /// Clause 7: ignorance and safety are different answers. An unknown
    /// launcher is Unchecked even when every probe answers present.
    #[test]
    fn check_unregistered_is_unchecked_not_satisfied() {
        let present = |_c: &Credential| true;
        assert_eq!(check("unregistered", &present), Shielded::Unchecked);
    }

    /// Clause 8: for an Environment launcher `present` is never called —
    /// isolating XDG says nothing about process-environment authentication.
    #[test]
    fn present_never_called_for_environment_launcher() {
        let calls = std::cell::Cell::new(0);
        let probe = |_c: &Credential| {
            calls.set(calls.get() + 1);
            true
        };
        assert_eq!(check("ifm", &probe), Shielded::Satisfied);
        assert_eq!(calls.get(), 0);
    }

    /// Clause 8: `needs` returned no credentials for an unknown launcher, so
    /// `present` is never called there either.
    #[test]
    fn present_never_called_for_unknown_launcher() {
        let calls = std::cell::Cell::new(0);
        let probe = |_c: &Credential| {
            calls.set(calls.get() + 1);
            true
        };
        assert_eq!(
            check("something-nobody-registered", &probe),
            Shielded::Unchecked
        );
        assert_eq!(calls.get(), 0);
    }

    /// Clause 8, the "only" direction: every question `check` asks is about a
    /// credential `needs` listed — never about anything else.
    #[test]
    fn present_only_ever_asked_about_listed_credentials() {
        let expected = opencode_credential();
        let foreign = std::cell::Cell::new(0);
        let probe = |c: &Credential| {
            if *c != expected {
                foreign.set(foreign.get() + 1);
            }
            false
        };
        let verdict = check("opencode", &probe);
        assert_eq!(foreign.get(), 0);
        assert_eq!(verdict, Shielded::Missing(vec![expected]));
    }

    /// Boundary at zero: the empty launcher name is Unknown, and `check` on
    /// it is Unchecked rather than Satisfied.
    #[test]
    fn empty_launcher_name_is_unknown_and_unchecked() {
        assert_eq!(needs(""), Needs::Unknown);
        let present = |_c: &Credential| true;
        assert_eq!(check("", &present), Shielded::Unchecked);
    }
}
