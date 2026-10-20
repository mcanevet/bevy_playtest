//! `bevy_playtest` CLI front door.
//!
//! Subcommands:
//! - `init [path] [--force]` — write a starter `playtest.scenario.json`
//!   (validated before writing; refuses to overwrite without `--force`)
//! - `run <scenario-path-or-smoke> [--report <path>] [--package <name>]
//!   [--bin <name>]` — thin wrapper: `cargo run` the current workspace's
//!   game with `BEVY_PLAYTEST_SCENARIO` injected, propagating the
//!   game's exit code. The game must already integrate
//!   `bevy_playtest::PlaytestPlugin`.
//!
//! Exit codes: 0-3 pass through the takeover contract (0 Pass, 1 Fail,
//! 2 Crash, 3 harness/config error); 4 is a CLI usage error (bad args,
//! init target exists, cargo missing, ambiguous/absent game target).
//!
//! The worked invariants example lives in `--help`/README, not the
//! emitted file: the scenario schema is strict, so commented JSON in
//! the template would fail its own parse-back validation.
//!
//! `run` forwards the scenario argument verbatim to
//! `BEVY_PLAYTEST_SCENARIO` — the `smoke` magic literal is interpreted
//! only by the plugin, never by this CLI.
use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_INIT_PATH: &str = "playtest.scenario.json";
const EXIT_USAGE: i32 = 4;

const INIT_TEMPLATE: &str = r#"{
  "name": "starter",
  "bot": { "type": "chaos", "seed": 42 },
  "duration_s": 5.0,
  "tps": 60
}"#;

const HELP: &str = "\
bevy_playtest — playtesting harness for Bevy 0.20 games

USAGE:
    bevy_playtest <SUBCOMMAND> [ARGS]

SUBCOMMANDS:
    init [path] [--force]
        Write a starter playtest.scenario.json (default: ./playtest.scenario.json).
        The emitted file is validated before writing. --force overwrites.

    run <scenario-path-or-smoke> [--report <path>] [--package <name>] [--bin <name>]
        Run the current workspace's game via `cargo run`, injecting
        BEVY_PLAYTEST_SCENARIO (and BEVY_PLAYTEST_REPORT for --report).
        The game must already add bevy_playtest::PlaytestPlugin (see the
        crate docs for the one-line integration). `smoke` selects the
        built-in scenario. The child's exit code is propagated:
        0 Pass, 1 Fail, 2 Crash, 3 harness/config error.

        Target selection: if the current directory's cargo project has
        exactly one bin target (discovered via src/bin/*.rs), it is
        auto-selected; otherwise pass --package or --bin. Build output
        is passed through untouched.

WORKED INVARIANTS EXAMPLE (add to the scenario JSON):
    \"invariants\": [{
        \"name\": \"player_in_bounds\",
        \"rule\": \"nodes_in_bounds\",
        \"min_x\": -10.0, \"max_x\": 10.0,
        \"min_y\": -10.0, \"max_y\": 10.0
    }]

END-TO-END:
    bevy_playtest init
    # edit playtest.scenario.json (see invariants example above)
    bevy_playtest run playtest.scenario.json
    # or, with no file at all:
    bevy_playtest run smoke

EXIT CODES:
    0-3  takeover contract (see `run`)
    4    CLI usage error
";

fn usage_error(msg: &str) -> String {
    format!("bevy_playtest: {msg}\n\n{}", HELP.trim_end())
}

fn die(msg: String) -> ! {
    eprintln!("{msg}");
    std::process::exit(EXIT_USAGE);
}

#[derive(Debug, PartialEq, Eq)]
struct InitArgs {
    path: PathBuf,
    force: bool,
}

fn parse_init_args(args: &[String]) -> Result<InitArgs, String> {
    let mut path: Option<PathBuf> = None;
    let mut force = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--force" => force = true,
            "--" => {
                for positional in rest.by_ref() {
                    if path.replace(PathBuf::from(positional)).is_some() {
                        return Err(usage_error(&format!(
                            "unexpected extra argument '{positional}'"
                        )));
                    }
                }
            }
            other if other.starts_with('-') => {
                return Err(usage_error(&format!(
                    "unknown flag '{other}' (init takes [path] [--force])"
                )))
            }
            positional if path.is_none() => path = Some(PathBuf::from(positional)),
            positional => {
                return Err(usage_error(&format!(
                    "unexpected extra argument '{positional}'"
                )))
            }
        }
    }
    Ok(InitArgs {
        path: path.unwrap_or_else(|| PathBuf::from(DEFAULT_INIT_PATH)),
        force,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct RunArgs {
    scenario: String,
    report: Option<String>,
    package: Option<String>,
    bin: Option<String>,
}

fn parse_run_args(args: &[String]) -> Result<RunArgs, String> {
    let mut scenario: Option<String> = None;
    let mut out = RunArgs {
        scenario: String::new(),
        report: None,
        package: None,
        bin: None,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--report" => {
                i += 1;
                out.report = Some(value_of(args, i, "--report")?);
            }
            "--package" | "-p" => {
                i += 1;
                out.package = Some(value_of(args, i, "--package")?);
            }
            "--bin" | "-b" => {
                i += 1;
                out.bin = Some(value_of(args, i, "--bin")?);
            }
            "--" => {
                break;
            }
            other if other.starts_with('-') => {
                return Err(usage_error(&format!(
                    "unknown flag '{other}' (run takes <scenario> [--report <path>] [--package <name>] [--bin <name>])"
                )))
            }
            positional if scenario.is_none() => scenario = Some(positional.to_string()),
            positional => {
                return Err(usage_error(&format!(
                    "unexpected extra argument '{positional}'"
                )))
            }
        }
        i += 1;
    }
    let Some(scenario) = scenario else {
        return Err(usage_error(
            "run requires a scenario path (or the built-in 'smoke')",
        ));
    };
    out.scenario = scenario;
    Ok(out)
}

fn value_of(args: &[String], i: usize, flag: &str) -> Result<String, String> {
    args.get(i)
        .cloned()
        .ok_or_else(|| usage_error(&format!("flag '{flag}' requires a value")))
}

fn discover_bin_targets(base: &Path) -> Vec<String> {
    let src_bin = base.join("src/bin");
    let mut bins = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&src_bin) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "rs") {
                if let Some(stem) = entry.path().file_stem().and_then(|s| s.to_str()) {
                    bins.push(stem.to_string());
                }
            }
        }
    }
    bins.sort();
    bins
}

fn cmd_init(args: &[String], base: &Path) -> i32 {
    let init = match parse_init_args(args) {
        Ok(init) => init,
        Err(msg) => {
            eprintln!("{msg}");
            return EXIT_USAGE;
        }
    };
    let path = base.join(&init.path);
    if path.exists() && !init.force {
        eprintln!(
            "bevy_playtest: init target '{}' already exists (use --force to overwrite)",
            init.path.display()
        );
        return EXIT_USAGE;
    }
    let scenario: bevy_playtest::scenario::Scenario = match serde_json::from_str(INIT_TEMPLATE) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("bevy_playtest: internal error: init template invalid: {err}");
            return EXIT_USAGE;
        }
    };
    if let Err(err) = bevy_playtest::scenario::validate_scenario(&scenario) {
        eprintln!("bevy_playtest: internal error: init template invalid: {err}");
        return EXIT_USAGE;
    }
    match std::fs::write(&path, INIT_TEMPLATE) {
        Ok(()) => {
            println!("bevy_playtest: wrote {}", path.display());
            0
        }
        Err(err) => {
            eprintln!("bevy_playtest: cannot write {}: {err}", path.display());
            EXIT_USAGE
        }
    }
}

fn cmd_run(args: &RunArgs, base: &Path) -> i32 {
    if !base.join("Cargo.toml").is_file() {
        eprintln!("bevy_playtest: no Cargo.toml in current directory; `run` must be invoked in the game's cargo project");
        return EXIT_USAGE;
    }
    let mut bin = args.bin.clone();
    if bin.is_none() && args.package.is_none() {
        let bins = discover_bin_targets(base);
        match bins.len() {
            0 => {
                eprintln!("bevy_playtest: no src/bin/*.rs targets found; pass --package or --bin");
                return EXIT_USAGE;
            }
            1 => bin = Some(bins[0].clone()),
            _ => {
                eprintln!(
                    "bevy_playtest: multiple bin targets found ({}); pass --package or --bin",
                    bins.join(", ")
                );
                return EXIT_USAGE;
            }
        }
    }
    let mut cmd = Command::new("cargo");
    cmd.current_dir(base);
    cmd.arg("run");
    if let Some(package) = &args.package {
        cmd.arg("--package").arg(package);
    }
    if let Some(bin) = &bin {
        cmd.arg("--bin").arg(bin);
    }
    cmd.env("BEVY_PLAYTEST_SCENARIO", &args.scenario);
    if let Some(report) = &args.report {
        cmd.env("BEVY_PLAYTEST_REPORT", report);
    }
    match cmd.status() {
        Ok(status) => status.code().unwrap_or(EXIT_USAGE),
        Err(err) => {
            eprintln!("bevy_playtest: cannot run cargo: {err}");
            EXIT_USAGE
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cwd = PathBuf::from(".");
    let Some(subcommand) = args.first() else {
        eprint!("{}", HELP);
        std::process::exit(EXIT_USAGE);
    };
    match subcommand.as_str() {
        "init" => std::process::exit(cmd_init(&args[1..], &cwd)),
        "run" => match parse_run_args(&args[1..]) {
            Ok(run) => std::process::exit(cmd_run(&run, &cwd)),
            Err(msg) => die(msg),
        },
        "help" | "--help" | "-h" => {
            print!("{}", HELP);
        }
        other => die(usage_error(&format!("unknown subcommand '{other}'"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_cli_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn init_template_parses_as_scenario() {
        let scenario: bevy_playtest::scenario::Scenario =
            serde_json::from_str(INIT_TEMPLATE).unwrap();
        bevy_playtest::scenario::validate_scenario(&scenario).unwrap();
    }

    #[test]
    fn init_writes_validated_file_and_refuses_overwrite() {
        let dir = tmp_dir("init");
        let args: Vec<String> = vec![];
        assert_eq!(cmd_init(&args, &dir), 0);
        let written = std::fs::read_to_string(dir.join("playtest.scenario.json")).unwrap();
        let scenario: bevy_playtest::scenario::Scenario = serde_json::from_str(&written).unwrap();
        bevy_playtest::scenario::validate_scenario(&scenario).unwrap();
        assert_eq!(cmd_init(&[], &dir), EXIT_USAGE, "second init must refuse");
    }

    #[test]
    fn init_force_overwrites() {
        let dir = tmp_dir("init_force");
        assert_eq!(cmd_init(&[String::from("--force")], &dir), 0);
        assert_eq!(cmd_init(&[String::from("--force")], &dir), 0);
    }

    #[test]
    fn init_unknown_flag_is_usage_error() {
        assert!(parse_init_args(&[String::from("--turbo")]).is_err());
    }

    #[test]
    fn run_in_non_cargo_dir_is_usage_error() {
        let dir = tmp_dir("run_no_cargo");
        let run = RunArgs {
            scenario: "smoke".into(),
            report: None,
            package: None,
            bin: None,
        };
        assert_eq!(cmd_run(&run, &dir), EXIT_USAGE);
    }

    #[test]
    fn run_ambiguous_bins_is_usage_error() {
        let dir = tmp_dir("run_ambiguous");
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"g\"\n").unwrap();
        std::fs::create_dir_all(dir.join("src/bin")).unwrap();
        std::fs::write(dir.join("src/bin/a.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("src/bin/b.rs"), "fn main() {}\n").unwrap();
        let run = RunArgs {
            scenario: "smoke".into(),
            report: None,
            package: None,
            bin: None,
        };
        assert_eq!(cmd_run(&run, &dir), EXIT_USAGE);
    }

    #[test]
    fn run_arg_parsing_contract() {
        let run = parse_run_args(&[
            String::from("smoke"),
            String::from("--report"),
            String::from("out.json"),
        ])
        .unwrap();
        assert_eq!(run.scenario, "smoke");
        assert_eq!(run.report.as_deref(), Some("out.json"));
        assert!(parse_run_args(&[]).is_err(), "run without scenario rejects");
        assert!(
            parse_run_args(&[String::from("s.json"), String::from("--wat")]).is_err(),
            "unknown run flag rejects"
        );
    }
}
