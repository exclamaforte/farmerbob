//! Pure descriptions of the commands a target repository can run.

use crate::measurement::Measurement;

/// Something the harness must be able to do to a target repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Compile it.
    Build,
    /// Run its tests.
    Test,
    /// Run its linter in the mode that fails on a warning.
    Lint,
    /// Rewrite its sources into canonical form.
    Format,
    /// Report whether formatting would rewrite anything, changing nothing.
    FormatCheck,
}

/// How much of the repository a step applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// The whole repository.
    Whole,
    /// One unit within it, carrying the unit's toolchain name.
    Unit(String),
}

/// A command to run, represented as an argument vector rather than a shell string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The program to execute.
    pub program: String,
    /// The program's arguments, in execution order.
    pub args: Vec<String>,
}

/// How one target repository is built, tested, linted, and formatted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    /// A short name for reports.
    pub name: String,
    /// The steps this toolchain can perform and their base invocations.
    pub steps: Vec<(Step, Invocation)>,
    /// The argument that narrows a step to one unit, when the toolchain supports that.
    pub unit_flag: Option<String>,
}

/// The toolchain this harness has always assumed.
pub fn cargo() -> Toolchain {
    Toolchain {
        name: "cargo".to_string(),
        steps: vec![
            (Step::Build, invocation_for(&["build"])),
            (Step::Test, invocation_for(&["test"])),
            (
                Step::Lint,
                invocation_for(&["clippy", "--all-targets", "--", "-D", "warnings"]),
            ),
            (Step::Format, invocation_for(&["fmt", "--all"])),
            (
                Step::FormatCheck,
                invocation_for(&["fmt", "--all", "--", "--check"]),
            ),
        ],
        unit_flag: Some("-p".to_string()),
    }
}

/// The command for one step at one scope.
pub fn invocation(t: &Toolchain, step: Step, scope: &Scope) -> Measurement<Invocation> {
    let base = match t.steps.iter().find(|(declared, _)| *declared == step) {
        Some((_, invocation)) => invocation,
        None => return Measurement::nothing_to_measure("the toolchain does not support this step"),
    };

    let unit = match scope {
        Scope::Whole => None,
        Scope::Unit(name) => match &t.unit_flag {
            Some(flag) => Some((flag.as_str(), name.as_str())),
            None => {
                return Measurement::nothing_to_measure("the toolchain cannot narrow to a unit");
            }
        },
    };

    let mut args = base.args.clone();
    if let Some((flag, name)) = unit {
        let insertion = match args.iter().position(|arg| arg == "--") {
            Some(index) => index,
            None => args.len(),
        };
        args.splice(insertion..insertion, [flag.to_string(), name.to_string()]);
    }

    Measurement::observed(Invocation {
        program: base.program.clone(),
        args,
    })
}

/// Which steps this toolchain can perform, in declaration order.
pub fn supported(t: &Toolchain) -> Vec<Step> {
    [
        Step::Build,
        Step::Test,
        Step::Lint,
        Step::Format,
        Step::FormatCheck,
    ]
    .into_iter()
    .filter(|step| t.steps.iter().any(|(declared, _)| declared == step))
    .collect()
}

fn invocation_for(args: &[&str]) -> Invocation {
    Invocation {
        program: "cargo".to_string(),
        args: args.iter().map(|arg| (*arg).to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measurement::Absent;

    #[test]
    fn cargo_commands_and_scopes_are_distinct() {
        let toolchain = cargo();
        let whole = invocation(&toolchain, Step::Test, &Scope::Whole);
        let unit = invocation(&toolchain, Step::Test, &Scope::Unit("fb".to_string()));

        assert_eq!(
            whole,
            Measurement::observed(Invocation {
                program: "cargo".to_string(),
                args: vec!["test".to_string()],
            })
        );
        assert_eq!(
            unit,
            Measurement::observed(Invocation {
                program: "cargo".to_string(),
                args: vec!["test".to_string(), "-p".to_string(), "fb".to_string()],
            })
        );
    }

    #[test]
    fn missing_steps_and_unit_support_are_reported() {
        let no_lint = Toolchain {
            name: "minimal".to_string(),
            steps: vec![(
                Step::Test,
                Invocation {
                    program: "tool".to_string(),
                    args: Vec::new(),
                },
            )],
            unit_flag: None,
        };

        assert!(matches!(
            invocation(&no_lint, Step::Lint, &Scope::Whole),
            Measurement::Missing(Absent::NothingToMeasure { .. })
        ));
        assert!(matches!(
            invocation(&no_lint, Step::Test, &Scope::Unit("x".to_string())),
            Measurement::Missing(Absent::NothingToMeasure { .. })
        ));
        assert_eq!(supported(&no_lint), vec![Step::Test]);
    }
}
