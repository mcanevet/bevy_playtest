//! Doc-example lint: every strict JSON example in `docs/reference`
//! must parse and validate against the current scenario grammar.
//!
//! Blocks are picked up from ```json fences. Stray `//` comments are
//! stripped before parsing (house style annotates examples with
//! them). A fence may carry a directive in an HTML comment within the
//! few lines above it, so rendered markdown is unchanged:
//!
//! - `<!-- doc-lint: skip -->` — illustrative only, not strict JSON
//!   (the annotated grammar sketch);
//! - `<!-- doc-lint: rejected -->` — parses fine but MUST be rejected
//!   by load validation (teaching a rejection).
//!
//! A parsed object that is not scenario-shaped (no top-level `bot`,
//! no invariant `rule`) is left alone: the reference docs carry other
//! JSON shapes (e.g. the known-answer methods object) that belong to
//! different schemas. Scenario-shaped blocks are validated either as
//! a complete scenario (top-level `bot`) or — when the block is a
//! lone invariant object — wrapped into a minimal chaos scenario with
//! a duration large enough for any `window_s`/`eventually_s` the
//! block declares, so authoring snippets stay terse in prose.

use std::fs;
use std::path::{Path, PathBuf};

use bevy_playtest::scenario::{validate_scenario, Scenario, ScenarioError};

/// Lines above a fence to scan for a `doc-lint:` directive.
const DIRECTIVE_CONTEXT_LINES: usize = 6;

fn reference_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/reference")
}

/// One fenced ```json block: file, 1-based line of the opening
/// fence, and the raw block text (without the fences).
struct JsonBlock {
    file: PathBuf,
    line: usize,
    text: String,
    skip: bool,
    rejected: bool,
}

fn collect_blocks(file: &Path, text: &str) -> Vec<JsonBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim_start().starts_with("```json") {
            let mut body = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim() != "```" {
                body.push(lines[j]);
                j += 1;
            }
            if j >= lines.len() {
                panic!("{}:{}: unterminated ```json fence", file.display(), i + 1);
            }
            let context = lines[i.saturating_sub(DIRECTIVE_CONTEXT_LINES)..i].join("\n");
            blocks.push(JsonBlock {
                file: file.to_path_buf(),
                line: i + 1,
                text: body.join("\n"),
                skip: context.contains("doc-lint: skip"),
                rejected: context.contains("doc-lint: rejected"),
            });
            i = j + 1;
        } else {
            i += 1;
        }
    }
    blocks
}

/// Strip `//` line and trailing comments. Only cuts at a `//` that
/// starts a line or follows a space, so strings containing `//` are
/// left intact.
fn strip_comments(block: &str) -> String {
    block
        .lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let mut cut = None;
            for i in 0..bytes.len().saturating_sub(1) {
                if bytes[i] == b'/' && bytes[i + 1] == b'/' {
                    let at_line_start = line[..i].trim().is_empty();
                    let after_space = i > 0 && bytes[i - 1] == b' ';
                    if at_line_start || after_space {
                        cut = Some(i);
                        break;
                    }
                }
            }
            match cut {
                Some(i) => &line[..i],
                None => line,
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove commas orphaned by comment stripping (`value, // note`
/// directly before a closing brace).
fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1; // drop the comma; whitespace is harmless
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Wrap a lone invariant object into a minimal chaos scenario whose
/// duration accommodates any `window_s`/`eventually_s` in the block.
fn envelope_for_fragment(inv: &serde_json::Value) -> serde_json::Value {
    let mut needed = 0.0f64;
    for key in ["window_s", "eventually_s"] {
        if let Some(v) = inv.get(key).and_then(|v| v.as_f64()) {
            needed = needed.max(v);
        }
    }
    serde_json::json!({
        "bot": { "type": "chaos" },
        "duration_s": (needed + 1.0).max(1.0),
        "invariants": [inv],
    })
}

fn is_scenario_shaped(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            map.contains_key("bot") || map.get("rule").is_some_and(|r| r.is_string())
        }
        _ => false,
    }
}

#[test]
fn reference_doc_json_examples_match_the_grammar() {
    let dir = reference_dir();
    let mut md_files: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&dir).expect("docs/reference exists") {
        let path = entry.expect("read dir entry").path();
        if path.extension().is_some_and(|e| e == "md") {
            md_files.push(path);
        }
    }
    md_files.sort();
    assert!(
        !md_files.is_empty(),
        "no markdown found under docs/reference"
    );

    let mut checked = 0;
    for file in &md_files {
        let text = fs::read_to_string(file).expect("read markdown");
        for block in collect_blocks(file, &text) {
            let where_ = format!("{}:{}", block.file.display(), block.line);
            if block.skip {
                continue;
            }
            let cleaned = strip_trailing_commas(&strip_comments(&block.text));
            let parsed: serde_json::Value = match serde_json::from_str(&cleaned) {
                Ok(v) => v,
                Err(e) => panic!(
                    "{where_}: ```json block is not valid JSON after comment \
                     stripping ({e}); fix the example, or mark it with \
                     <!-- doc-lint: skip --> if it is illustrative"
                ),
            };
            if !is_scenario_shaped(&parsed) {
                // A different schema (e.g. the known-answer methods
                // object) — outside this lint's scope.
                continue;
            }
            checked += 1;
            let scenario_value = if parsed.get("bot").is_some() {
                parsed.clone()
            } else {
                envelope_for_fragment(&parsed)
            };
            let scenario: Scenario = match serde_json::from_value(scenario_value) {
                Ok(s) => s,
                Err(e) => panic!(
                    "{where_}: example does not deserialize against the \
                     current Scenario grammar ({e})"
                ),
            };
            match validate_scenario(&scenario) {
                Ok(()) => {
                    assert!(
                        !block.rejected,
                        "{where_}: marked doc-lint: rejected but load \
                         validation accepts it"
                    );
                }
                Err(ScenarioError::Rejected(msg)) => {
                    assert!(
                        block.rejected,
                        "{where_}: load validation rejects this example \
                         ({msg}); fix it, or mark it with \
                         <!-- doc-lint: rejected --> if it teaches a rejection"
                    );
                }
                Err(other) => panic!("{where_}: unexpected error kind {other:?}"),
            }
        }
    }
    assert!(
        checked > 0,
        "lint validated no scenario examples — did the fence markers change?"
    );
}
