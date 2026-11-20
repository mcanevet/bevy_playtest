//! Score a game's exploration against a committed targets file.
//!
//! ```text
//! bevy_playtest_eval --targets eval/targets/game_menu.json \
//!     --binary target/debug/fixture_game_menu [--seeds 5] [--frames 5000] \
//!     [--json-out results.json]
//! ```
//!
//! Prints a markdown table (for a CI step summary) and optionally
//! writes the full JSON result. Exit code 0 even when targets are
//! missed: this measures, it does not gate.

use std::path::PathBuf;

use bevy_playtest::evaluation::{run_evaluation, TargetsFile};

fn main() {
    let mut targets: Option<PathBuf> = None;
    let mut binary: Option<String> = None;
    let mut seeds: u64 = 5;
    let mut frames: u64 = 5000;
    let mut json_out: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .unwrap_or_else(|| usage(&format!("{arg} needs a value")))
        };
        match arg.as_str() {
            "--targets" => targets = Some(value().into()),
            "--binary" => binary = Some(value()),
            "--seeds" => {
                seeds = value()
                    .parse()
                    .unwrap_or_else(|_| usage("--seeds: integer"))
            }
            "--frames" => {
                frames = value()
                    .parse()
                    .unwrap_or_else(|_| usage("--frames: integer"))
            }
            "--json-out" => json_out = Some(value().into()),
            other => usage(&format!("unknown argument {other}")),
        }
    }
    let targets_path = targets.unwrap_or_else(|| usage("--targets is required"));
    let binary = binary.unwrap_or_else(|| usage("--binary is required"));
    let text = std::fs::read_to_string(&targets_path)
        .unwrap_or_else(|e| usage(&format!("read {}: {e}", targets_path.display())));
    let file: TargetsFile = serde_json::from_str(&text)
        .unwrap_or_else(|e| usage(&format!("parse {}: {e}", targets_path.display())));
    let repo_root = std::env::current_dir().expect("current dir");

    let seed_list: Vec<u64> = (1..=seeds).collect();
    let evaluation = run_evaluation(&file, &repo_root, &binary, &seed_list, frames);
    print!("{}", evaluation.to_markdown());
    if let Some(path) = json_out {
        let json = serde_json::to_string_pretty(&evaluation).expect("serialize evaluation");
        std::fs::write(&path, json)
            .unwrap_or_else(|e| usage(&format!("write {}: {e}", path.display())));
    }
}

fn usage(msg: &str) -> ! {
    eprintln!(
        "bevy_playtest_eval: {msg}\nusage: bevy_playtest_eval --targets FILE --binary PATH \
         [--seeds N] [--frames N] [--json-out FILE]"
    );
    std::process::exit(2);
}
