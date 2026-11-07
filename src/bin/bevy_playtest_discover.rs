//! `bevy_playtest discover` — the authoring-time tier-0 sweep CLI.
//!
//! One command drafts a game's input surface: taps the candidate
//! key matrix through probe subprocesses, diffs each run against a
//! control, and writes `surface.draft.json` with live AND inert
//! entries, findings, and the honest-coverage report. Exit 0 on
//! success — including an all-inert surface (inert is an answer,
//! not an error); non-zero only on a failed sweep or artifact write.
//!
//! ```console
//! bevy_playtest_discover --binary ./my_game [--keys KeyW,Digit1] \
//!   [--cache-dir .bevy_playtest/cache] [--out surface.draft.json] \
//!   [--fixture-name my_game]
//! ```
//!
//! The draft is a DRAFT: `draft: true` in its header, awaiting
//! curation — never the committed `surface.json` contract (that
//! lifecycle is a later step). See
//! `docs/how-to/discover-input-surface.md`.

/// Exit codes, matching the plugin contract's CONFIG_ERROR for CLI
/// misuse (the closest existing convention).
#[cfg(feature = "takeover")]
const EXIT_CONFIG_ERROR: u8 = 3;
#[cfg(feature = "takeover")]
const EXIT_FAILURE: u8 = 1;

#[cfg(feature = "takeover")]
fn main() -> std::process::ExitCode {
    match run() {
        Ok(code) => code,
        Err(usage_msg) => {
            // Usage errors already printed their specific message by
            // the time they surface here; the bare usage reminder is
            // the caller-friendly suffix.
            eprintln!("{usage_msg}");
            std::process::ExitCode::from(EXIT_CONFIG_ERROR)
        }
    }
}

#[cfg(feature = "takeover")]
fn run() -> Result<std::process::ExitCode, String> {
    use bevy_playtest::discovery::draft::{draft_surface, write_artifact};
    use bevy_playtest::discovery::flow::DiscoveryConfig;
    use bevy_playtest::discovery::probe_scenarios::ProbePlan;
    use bevy_playtest::scenario::normalize_key_name;
    use std::path::PathBuf;

    const USAGE: &str = "usage: bevy_playtest_discover --binary <path> \
                         [--keys comma,list] [--cache-dir path] \
                         [--out path] [--fixture-name name]";

    // Manual parsing (five flags, no positionals): the harness keeps
    // its dependency set deliberately narrow — a grammar this small
    // does not justify an arg-parsing crate. Repeated flags are
    // last-wins (each occurrence simply overwrites the previous
    // value; for `--keys` the last list replaces any earlier one).
    let mut binary: Option<PathBuf> = None;
    let mut keys: Option<Vec<String>> = None;
    let mut cache_dir: Option<PathBuf> = None;
    let mut out: PathBuf = PathBuf::from("surface.draft.json");
    let mut fixture_name: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut take_value = || -> Result<String, String> {
            args.next()
                .ok_or_else(|| format!("bevy_playtest_discover: missing value for {arg}\n{USAGE}"))
        };
        match arg.as_str() {
            "--binary" => binary = Some(PathBuf::from(take_value()?)),
            "--keys" => {
                let raw = take_value()?;
                let mut parsed: Vec<String> = Vec::new();
                for candidate in raw.split(',') {
                    let trimmed = candidate.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match normalize_key_name(trimmed) {
                        // Same canonicalization discipline as the
                        // ENV_DISCOVER_KEYS path in the flow: aliases
                        // never fragment the surface vocabulary or
                        // the cache.
                        Some(canonical) => {
                            if !parsed.iter().any(|k| k == canonical) {
                                parsed.push(canonical.to_string());
                            }
                        }
                        None => {
                            return Err(format!(
                                "bevy_playtest_discover: unknown key \
                                 '{trimmed}' — use a canonical key name or \
                                 alias (e.g. KeyW, Digit1, Space)\n{USAGE}"
                            ));
                        }
                    }
                }
                if parsed.is_empty() {
                    // An empty --keys must NOT silently fall back to
                    // the full default matrix (same refusal as the
                    // empty-ENV_DISCOVER_KEYS case in the flow).
                    return Err(format!(
                        "bevy_playtest_discover: --keys is set but empty \
                         (or whitespace-only) — refusing to silently fall \
                         back to the full default matrix; omit --keys to use it\n{USAGE}"
                    ));
                }
                keys = Some(parsed);
            }
            "--cache-dir" => cache_dir = Some(PathBuf::from(take_value()?)),
            "--out" => out = PathBuf::from(take_value()?),
            "--fixture-name" => fixture_name = Some(take_value()?),
            other => {
                return Err(format!(
                    "bevy_playtest_discover: unknown argument '{other}'\n{USAGE}"
                ));
            }
        }
    }
    let Some(binary) = binary else {
        return Err(format!(
            "bevy_playtest_discover: --binary is required\n{USAGE}"
        ));
    };

    let mut config = DiscoveryConfig::new(&binary);
    if let Some(keys) = keys {
        config.plan = ProbePlan {
            candidate_keys: keys,
            ..ProbePlan::default()
        };
    }
    if let Some(dir) = cache_dir {
        config.cache_dir = dir;
    }
    config.fixture_name = fixture_name;

    // The sweep itself. Errors here are infrastructure (probe
    // crashes, unreadable binary) — non-zero exit, and the message
    // says what failed rather than a bare code.
    match draft_surface(&config) {
        Ok(draft) => {
            // Artifact write failure is reported after the run's
            // cost, never swallowed.
            if let Err(e) = write_artifact(&draft, &out) {
                eprintln!(
                    "bevy_playtest_discover: cannot write draft artifact \
                     '{}': {e}",
                    out.display()
                );
                return Ok(std::process::ExitCode::from(EXIT_FAILURE));
            }
            for line in draft.summary_lines() {
                println!("{line}");
            }
            println!(
                "draft written to {} (draft: true — a machine-drafted \
                 surface awaiting curation, not the committed contract)",
                out.display()
            );
            // Exit 0 even for an all-inert surface: inert is an
            // answer, not an error.
            Ok(std::process::ExitCode::SUCCESS)
        }
        Err(e) => {
            eprintln!("bevy_playtest_discover: {e}");
            Ok(std::process::ExitCode::from(EXIT_FAILURE))
        }
    }
}

#[cfg(not(feature = "takeover"))]
fn main() {}
