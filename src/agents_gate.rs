//! The test `ekko agents merge` gates a branch on, derived from what the
//! repository already declares (task 1902, decision 1908), for a project
//! where the user set none: bors, mergetrain and GitHub's merge queue all
//! take their gate from what the project declares once, and none asks per
//! run. The rule is fixed, never a model's choice. Its sources, in order,
//! the first that names a command winning:
//!
//! 1. The CI: a step of a GitHub Actions workflow, `.github/workflows/*.yml`
//!    or `*.yaml` at the repository's top, whose `run` script has a line
//!    that runs a test runner (`TEST_RUNNERS`). It comes first because it is
//!    the gate the project already merges on.
//! 2. A flake that declares `checks`: `nix flake check`.
//! 3. A Makefile's `test` target, or else its `check` target: `make test`.
//! 4. A Cargo.toml: `cargo test`, `--locked` beside a Cargo.lock.
//! 5. A package.json whose `scripts.test` is not npm init's placeholder:
//!    `npm test`.
//!
//! A command from 1, 3, 4 or 5 runs in the flake's dev shell when the
//! repository has a flake that declares one, as the tools a CI step gets
//! from its setup actions come from there on a machine like this one,
//! which has no cargo on its PATH. A source that names two different
//! commands -- two CI steps that test differently -- derives nothing: the
//! user chooses, as when no source names any.
//!
//! The workflow is read with a reader of the YAML a workflow's steps are
//! written in -- block sequences and mappings, plain and quoted scalars,
//! `|` and `>` block scalars -- not a whole YAML parser, and not line by
//! line: a `run: |` script spans several lines.

use std::fs;
use std::path::{Path, PathBuf};

/// The commands that run a project's tests, by their first words.
const TEST_RUNNERS: &[&[&str]] = &[
    &["cargo", "test"],
    &["cargo", "nextest", "run"],
    &["nix", "flake", "check"],
    &["make", "test"],
    &["make", "check"],
    &["npm", "test"],
    &["npm", "run", "test"],
    &["yarn", "test"],
    &["pnpm", "test"],
    &["bun", "test"],
    &["deno", "test"],
    &["pytest"],
    &["python", "-m", "pytest"],
    &["python3", "-m", "pytest"],
    &["tox"],
    &["go", "test"],
    &["just", "test"],
    &["mix", "test"],
    &["dotnet", "test"],
    &["mvn", "test"],
    &["gradle", "test"],
    &["./gradlew", "test"],
];

/// What the rule found.
#[derive(Debug, PartialEq)]
pub enum Derived {
    /// The command, and where it came from, in words.
    Found { command: String, from: String },
    /// One source named several commands: the user's to choose.
    Disagree { from: String, commands: Vec<String> },
    /// No source named one.
    Nothing,
}

/// The gate for a project whose folder is `folder`, in the repository whose
/// top is `top`.
pub fn derive(top: &Path, folder: &Path) -> Derived {
    // The test runs in the project's folder; what the top declares runs there.
    let up = match folder.strip_prefix(top) {
        Ok(under) if under.components().count() > 0 => format!("cd {} && ", vec![".."; under.components().count()].join("/")),
        _ => String::new(),
    };
    let flake = fs::read_to_string(top.join("flake.nix")).ok();
    let shell = flake.as_deref().is_some_and(|text| declares(text, &["devShells", "devShell"]));
    let in_shell = |command: String| if shell && !command.starts_with("nix ") { format!("nix develop --command {}", wrapped(&command)) } else { command };

    let mut ci: Vec<(String, String)> = Vec::new();
    for workflow in workflows(top) {
        let Ok(text) = fs::read_to_string(&workflow) else { continue };
        let name = workflow.strip_prefix(top).unwrap_or(&workflow).display().to_string();
        for step in steps(&text) {
            let Some(run) = step.run else { continue };
            // A script of the runner's own -- its expressions, its folders -- runs only there.
            if ["${{", "GITHUB_", "RUNNER_"].iter().any(|runner| run.contains(runner)) || !run.lines().any(runs_tests) {
                continue;
            }
            let script = match &step.directory {
                Some(directory) => format!("cd {directory} && {}", if run.contains('\n') { wrapped(&run) } else { run.clone() }),
                None => run.clone(),
            };
            let label = match &step.name {
                Some(step) => format!("{name}, step {step}"),
                None => name.clone(),
            };
            if !ci.iter().any(|(seen, _)| *seen == script) {
                ci.push((script, label));
            }
        }
    }
    match ci.len() {
        0 => {}
        1 => {
            let (script, from) = ci.remove(0);
            return Derived::Found { command: format!("{up}{}", in_shell(script)), from };
        }
        _ => {
            return Derived::Disagree {
                from: ci.iter().map(|(_, from)| from.clone()).collect::<Vec<_>>().join("; "),
                commands: ci.into_iter().map(|(script, _)| script).collect(),
            }
        }
    }

    if flake.as_deref().is_some_and(|text| declares(text, &["checks"])) {
        return Derived::Found { command: format!("{up}nix flake check"), from: "flake.nix, which declares checks".into() };
    }
    let found = |command: &str, from: &str| Derived::Found { command: in_shell(command.to_string()), from: from.to_string() };
    for makefile in ["GNUmakefile", "makefile", "Makefile"] {
        let Ok(text) = fs::read_to_string(folder.join(makefile)) else { continue };
        for target in ["test", "check"] {
            if has_target(&text, target) {
                return found(&format!("make {target}"), &format!("{makefile}, target {target}"));
            }
        }
    }
    if folder.join("Cargo.toml").is_file() {
        return match folder.join("Cargo.lock").is_file() {
            true => found("cargo test --locked", "Cargo.toml, beside its Cargo.lock"),
            false => found("cargo test", "Cargo.toml"),
        };
    }
    let script = fs::read_to_string(folder.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|package| package["scripts"]["test"].as_str().map(str::to_string));
    if script.is_some_and(|script| !script.trim().is_empty() && !script.contains("no test specified")) {
        return found("npm test", "package.json, scripts.test");
    }
    Derived::Nothing
}

/// The workflows of GitHub Actions at `top`, by name.
fn workflows(top: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(top.join(".github/workflows")) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && path.extension().is_some_and(|extension| extension == "yml" || extension == "yaml"))
        .collect();
    found.sort();
    found
}

/// Whether a line of a script runs a test runner: its first words, past
/// variables it sets and a `nix develop ... --command`, are one of
/// `TEST_RUNNERS`.
fn runs_tests(line: &str) -> bool {
    let mut words: Vec<&str> = line.split_whitespace().collect();
    loop {
        let set = words.iter().take_while(|word| word.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))).count();
        words.drain(..set);
        if words.first() == Some(&"env") {
            words.remove(0);
            while let Some(flag) = words.first().copied().filter(|word| word.starts_with('-')) {
                let takes = if flag == "-u" { 2 } else { 1 };
                words.drain(..takes.min(words.len()));
            }
            continue;
        }
        if words.starts_with(&["nix", "develop"]) {
            match words.iter().position(|word| *word == "--command" || *word == "-c") {
                Some(at) => {
                    words.drain(..=at);
                    continue;
                }
                None => return false,
            }
        }
        break;
    }
    TEST_RUNNERS.iter().any(|runner| words.starts_with(runner))
}

/// `command` as one word for `nix develop --command` or `cd ... &&`, through
/// `sh -c` where the shell must read it.
fn wrapped(command: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || " -_./=+:@%,".contains(c);
    if command.chars().all(plain) {
        command.to_string()
    } else {
        format!("sh -c '{}'", command.replace('\'', r"'\''"))
    }
}

/// Whether a flake's text declares an output named one of `names`: the name
/// as a word, outside a comment, followed by `.` or `=`.
fn declares(flake: &str, names: &[&str]) -> bool {
    flake.lines().map(|line| line.split('#').next().unwrap_or("")).any(|line| {
        names.iter().any(|name| {
            line.match_indices(name).any(|(at, _)| {
                let before = line[..at].chars().next_back();
                let after = line[at + name.len()..].trim_start().chars().next();
                before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')) && matches!(after, Some('.' | '='))
            })
        })
    })
}

/// Whether a Makefile has a rule for `target`.
fn has_target(makefile: &str, target: &str) -> bool {
    makefile.lines().any(|line| {
        if line.starts_with([' ', '\t', '#']) {
            return false;
        }
        let Some((targets, rest)) = line.split_once(':') else { return false };
        !rest.starts_with('=') && targets.split_whitespace().any(|name| name == target)
    })
}

/// A step of a workflow, as far as the gate needs it.
#[derive(Debug, Default, PartialEq)]
struct Step {
    name: Option<String>,
    run: Option<String>,
    directory: Option<String>,
}

/// Every step under a `steps:` key of a workflow's text, in order.
fn steps(text: &str) -> Vec<Step> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let Some(("steps", value)) = key(line.trim_start()) else { continue };
        if !value.is_empty() {
            continue;
        }
        let at = indent(line);
        let mut dash: Option<usize> = None;
        let mut step: Option<(usize, Step)> = None;
        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                i += 1;
                continue;
            }
            let here = indent(line);
            let item = trimmed == "-" || trimmed.starts_with("- ");
            if here < at || (here == at && !item) {
                break;
            }
            i += 1;
            let (keys, rest) = if item && dash.is_none_or(|dash| dash == here) {
                dash = Some(here);
                found.extend(step.take().map(|(_, step)| step));
                let rest = trimmed[1..].trim_start();
                let keys = here + (trimmed.len() - rest.len());
                step = Some((keys, Step::default()));
                (keys, rest)
            } else {
                (here, trimmed)
            };
            let Some((keys_at, step)) = step.as_mut() else { continue };
            if keys != *keys_at {
                continue;
            }
            let Some((name, value)) = key(rest) else { continue };
            let value = scalar(&lines, &mut i, keys, value);
            match name {
                "name" => step.name = Some(value),
                "run" => step.run = Some(value),
                "working-directory" => step.directory = Some(value),
                _ => {}
            }
        }
        found.extend(step.take().map(|(_, step)| step));
    }
    found
}

/// The key and the rest of a `key: value` line.
fn key(line: &str) -> Option<(&str, &str)> {
    let (name, value) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.contains(' ') || !(value.is_empty() || value.starts_with([' ', '\t'])) {
        return None;
    }
    Some((name.trim_matches(['"', '\'']), value.trim()))
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// The value of a key at `keys` whose line holds `value`: a block scalar
/// takes the lines under it that `i` points at, and moves `i` past them.
fn scalar(lines: &[&str], i: &mut usize, keys: usize, value: &str) -> String {
    let value = strip_comment(value);
    // Its chomping indicator aside: a script ends with its last line.
    if let Some(style) = value.chars().next().filter(|c| matches!(c, '|' | '>')) {
        let mut block: Vec<&str> = Vec::new();
        while *i < lines.len() {
            let line = lines[*i];
            if !line.trim().is_empty() && indent(line) <= keys {
                break;
            }
            block.push(line);
            *i += 1;
        }
        let deep = block.iter().filter(|line| !line.trim().is_empty()).map(|line| indent(line)).min().unwrap_or(0);
        let body: Vec<String> = block.iter().map(|line| line.get(deep..).unwrap_or("").to_string()).collect();
        let text = if style == '|' {
            body.join("\n")
        } else {
            let mut folded = String::new();
            for (n, line) in body.iter().enumerate() {
                if n > 0 {
                    folded.push(if line.is_empty() || body[n - 1].is_empty() { '\n' } else { ' ' });
                }
                folded.push_str(line);
            }
            folded
        };
        return text.trim_end_matches('\n').to_string();
    }
    if let Some(quoted) = value.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')) {
        return quoted.replace("''", "'");
    }
    if let Some(quoted) = value.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        return quoted.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    value.to_string()
}

/// A plain value without a comment after it: ` #` starts one.
fn strip_comment(value: &str) -> &str {
    if value.starts_with(['\'', '"']) {
        return value;
    }
    value.find(" #").map_or(value, |at| value[..at].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(files: &[(&str, &str)]) -> PathBuf {
        let dir = crate::paths::test_dir("ekko-agents-gate");
        for (name, text) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        dir
    }

    fn derived(files: &[(&str, &str)]) -> Derived {
        let dir = repo(files);
        let derived = derive(&dir, &dir);
        fs::remove_dir_all(&dir).ok();
        derived
    }

    fn found(command: &str, from: &str) -> Derived {
        Derived::Found { command: command.into(), from: from.into() }
    }

    const CI: &str = "name: CI
on: [push]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - name: Build
        run: cargo build --locked
      # The tests.
      - name: Test
        run: cargo test --locked
      - name: Smoke-test the binary
        run: |
          mkdir -p \"$RUNNER_TEMP/x\"
          cargo test --locked
";

    #[test]
    fn a_ci_step_that_runs_a_test_runner_is_the_gate() {
        assert_eq!(derived(&[(".github/workflows/ci.yml", CI)]), found("cargo test --locked", ".github/workflows/ci.yml, step Test"));
        // Without that step, nothing: Build runs no tests, and the smoke test is the runner's own.
        assert_eq!(derived(&[(".github/workflows/ci.yml", &CI.replace("cargo test --locked\n      - name: Smoke", "cargo build\n      - name: Smoke"))]), Derived::Nothing);
    }

    #[test]
    fn ekkos_own_ci_gives_its_test_step() {
        let ci = include_str!("../.github/workflows/ci.yml");
        let runs: Vec<String> = steps(ci).into_iter().filter_map(|step| step.run).collect();
        assert!(runs.contains(&"cargo test --locked".to_string()), "{runs:?}");
        assert_eq!(derived(&[(".github/workflows/ci.yml", ci)]), found("cargo test --locked", ".github/workflows/ci.yml, step Test"));
    }

    #[test]
    fn a_run_block_spanning_lines_is_read_whole() {
        let ci = "jobs:\n  t:\n    steps:\n    - name: Tests\n      working-directory: app\n      run: |\n        npm ci\n        npm test\n\n    - run: echo done\n";
        assert_eq!(derived(&[(".github/workflows/t.yaml", ci)]), found("cd app && sh -c 'npm ci\nnpm test'", ".github/workflows/t.yaml, step Tests"));
        // Its runner is on the block's second line: the first alone runs none.
        let flat = ci.replace("run: |\n        npm ci\n        npm test", "run: npm ci");
        assert_eq!(derived(&[(".github/workflows/t.yaml", &flat)]), Derived::Nothing);
    }

    #[test]
    fn two_ci_steps_that_test_differently_derive_nothing() {
        let two = CI.replace("run: cargo build --locked", "run: cargo test --release");
        assert_eq!(
            derived(&[(".github/workflows/ci.yml", &two), ("Cargo.toml", "")]),
            Derived::Disagree {
                from: ".github/workflows/ci.yml, step Build; .github/workflows/ci.yml, step Test".into(),
                commands: vec!["cargo test --release".into(), "cargo test --locked".into()]
            }
        );
        // The same command in two jobs is one.
        let same = CI.replace("run: cargo build --locked", "run: cargo test --locked");
        assert_eq!(derived(&[(".github/workflows/ci.yml", &same)]), found("cargo test --locked", ".github/workflows/ci.yml, step Build"));
    }

    #[test]
    fn a_flake_with_checks_is_checked_and_its_dev_shell_runs_the_rest() {
        let checks = "{ outputs = { self }: { checks.x86_64-linux.default = self; }; }";
        assert_eq!(derived(&[("flake.nix", checks), ("Cargo.toml", "")]), found("nix flake check", "flake.nix, which declares checks"));
        // Named in a comment only, it declares none.
        let commented = "{ outputs = { self }: { # checks.x = 1;\n packages.default = self; }; }";
        assert_eq!(derived(&[("flake.nix", commented), ("Cargo.toml", "")]), found("cargo test", "Cargo.toml"));
        let shell = "{ outputs = { self }: { devShells.default = self; }; }";
        assert_eq!(derived(&[("flake.nix", shell), ("Cargo.toml", "")]), found("nix develop --command cargo test", "Cargo.toml"));
        assert_eq!(
            derived(&[("flake.nix", shell), (".github/workflows/ci.yml", CI)]),
            found("nix develop --command cargo test --locked", ".github/workflows/ci.yml, step Test")
        );
    }

    #[test]
    fn the_sources_go_in_their_order() {
        let make = ("Makefile", ".PHONY: test\ntest: build\n\tcargo test\n");
        let cargo = ("Cargo.toml", "[package]\n");
        let npm = ("package.json", r#"{"scripts": {"test": "vitest"}}"#);
        let ci = (".github/workflows/ci.yml", CI);
        assert_eq!(derived(&[ci, make, cargo, npm]), found("cargo test --locked", ".github/workflows/ci.yml, step Test"));
        assert_eq!(derived(&[make, cargo, npm]), found("make test", "Makefile, target test"));
        assert_eq!(derived(&[cargo, npm, ("Cargo.lock", "")]), found("cargo test --locked", "Cargo.toml, beside its Cargo.lock"));
        assert_eq!(derived(&[npm]), found("npm test", "package.json, scripts.test"));
        assert_eq!(derived(&[("Makefile", "check:\n\ttrue\ntest := 1\n")]), found("make check", "Makefile, target check"));
    }

    #[test]
    fn nothing_declared_derives_nothing() {
        assert_eq!(derived(&[("readme.md", "# x\n")]), Derived::Nothing);
        assert_eq!(derived(&[("package.json", r#"{"scripts": {"test": "echo \"Error: no test specified\" && exit 1"}}"#)]), Derived::Nothing);
        assert_eq!(derived(&[("Makefile", "build:\n\ttest -f x\n")]), Derived::Nothing);
    }

    #[test]
    fn a_project_under_the_top_runs_what_the_top_declares_from_there() {
        let dir = repo(&[(".github/workflows/ci.yml", CI), ("sub/app/readme.md", "")]);
        assert_eq!(derive(&dir, &dir.join("sub/app")), found("cd ../.. && cargo test --locked", ".github/workflows/ci.yml, step Test"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn runs_tests_reads_past_variables_and_a_dev_shell() {
        assert!(runs_tests("RUST_LOG=debug cargo test -q"));
        assert!(runs_tests("env -u CLAUDECODE nix develop --command cargo test --locked"));
        assert!(!runs_tests("echo cargo test"));
        assert!(!runs_tests("cargo testing"));
        assert!(!runs_tests("nix develop"));
    }
}
