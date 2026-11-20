//! Source-derived chaos surface (D1.5 slice 5): a milliseconds-cost
//! text pass over the GAME's Rust sources collecting every
//! `KeyCode::X` variant token the code mentions, and aiming the
//! default chaos surface with what the game actually reads — out
//! of the box, before any probing and without authoring a scenario.
//!
//! The scan is a CANDIDATE extractor, never a truth source: "the
//! code mentions KeyE" earns a key a place in the chaos LIST (where
//! a wrong guess costs one inert keypress), and nothing more.
//! Meaning stays discovery's behavioral job; this layer attaches
//! zero semantics to any key. That division is the design rule the
//! adversarial reviews froze: source candidates → runtime
//! verification is the funnel. The scan's candidates feed the
//! default battery's scan-derived surface (`layered_chaos_keys`). Emitting
//! them as schema entries (`Evidence::SourceRef` for
//! [`crate::discovery::provenance::ProvenanceGrade::SourceCited`])
//! is the planned next stage of the funnel.
//!
//! Layering contract (see [`layered_chaos_keys`]):
//!
//! - the WASD/arrows [`BACKBONE_KEYS`] is a FALLBACK tier, not an
//!   always-on floor: when the scan yields any usable key, the
//!   scan-derived surface (the scan's FULL unique-key set,
//!   backbone keys included) REPLACES the blind default, so the
//!   battery's ~200 chaos presses concentrate on keys the game's
//!   own code reads instead of spreading over inert WASD/arrows;
//!   when the scan is empty or poison-only, the backbone stands —
//!   exactly the surface that existed before the scan (ignorance
//!   fallback), and the notes say which arm fired;
//! - a short POISON list (`Escape`, `Tab`) is removed — the two
//!   keys a blind chaos bot can use to derail the run into a
//!   different context (pause menu, focus cycling) and thereby
//!   convert the remaining coverage into menu coverage;
//! - key→meaning mapping appears NOWHERE here; the layer replaces
//!   only where the default LIST comes from.
//!
//! Purity: `std` only, no game execution, no I/O beyond reading
//! `.rs` files under a given root. Determinism: files are walked in
//! sorted order, output order is canonical, so two scans of the
//! same tree yield identical summaries.

use std::path::{Path, PathBuf};

use crate::scenario::{normalize_key_name, BACKBONE_KEYS};

/// Parent-side opt-outs and overrides (consumed by the battery /
/// discovery PARENT process — the game child never sees these; the
/// takeover env contract is unaffected).
pub const ENV_SOURCE_ROOT: &str = "BEVY_PLAYTEST_SOURCE_ROOT";
/// Set to `1` to disable the source scan even when a root applies.
pub const ENV_SOURCE_SCAN_DISABLE: &str = "BEVY_PLAYTEST_SOURCE_SCAN_DISABLE";

/// Keys a blind chaos bot must never press: each one derails the
/// run into a different context (Escape: pause menu; Tab: focus
/// cycling), systematically converting the rest of the run's
/// coverage into menu coverage. Removed from the layered CHAOS
/// surface only — probing a poison key is fine (a probe run is a
/// dedicated controlled experiment, not a shared chaos budget).
pub const POISON_KEYS: &[&str] = &["Escape", "Tab"];

/// Hard cap on the TOTAL scan-derived chaos surface (the keys the
/// blind bot actually samples with its ~200 chaos presses —
/// 20 s at 10 Hz; presses spread thinner as the surface grows).
/// Beyond this many keys the per-key depth stops paying for the
/// breadth. Most-referenced keys win (then alphabetical), and the
/// drop is recorded as a note.
pub const MAX_SCAN_KEYS: usize = 24;

/// One `KeyCode::X` mention in the game's sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceScanHit {
    /// Canonical key name (validated against the scenario's
    /// emittable universe — `normalize_key_name` accepted it).
    pub key: String,
    /// File path, relative to the scan root (stable across
    /// machines; the shape `Evidence::SourceRef` wants).
    pub file: String,
    /// 1-based line number of the token.
    pub line: u32,
    /// Enclosing item, when identifiable: nearest preceding
    /// `fn NAME` line, else nearest `impl`/`struct`/`enum`/`mod`.
    pub enclosing: Option<String>,
}

/// What a scan found.
#[derive(Clone, Debug, Default)]
pub struct SourceScanResult {
    /// Every valid, canonicalized `KeyCode::` hit, in deterministic
    /// file-then-line order.
    pub hits: Vec<SourceScanHit>,
    /// Variants seen but outside the emittable universe (e.g.
    /// `F1`, `NumpadEnter`): recorded, never guessed at — the
    /// driver cannot emit what it cannot name.
    pub unsupported: Vec<String>,
    /// Number of `.rs` files read.
    pub files_scanned: usize,
    /// Files that failed to read (path + why); the scan continues
    /// past them (fail-open, but never silently).
    pub unreadable: Vec<(String, String)>,
}

impl SourceScanResult {
    /// Unique canonical keys among the hits, sorted (stable across
    /// scans of the same tree).
    pub fn unique_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.hits.iter().map(|h| h.key.clone()).collect();
        keys.sort();
        keys.dedup();
        keys
    }
}

/// The layered chaos surface plus its provenance notes (refusal
/// over silence: every filtering decision is reported).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LayeredKeys {
    /// The full chaos key list the bot samples, canonical spelling:
    /// the scan-hit arm emits ALPHABETICALLY; the fallback arm
    /// keeps the backbone's declared order.
    pub keys: Vec<String>,
    /// Provenance and filtering notes for the report.
    pub notes: Vec<String>,
}

/// Poison-filtered unique keys with reference counts, in
/// deterministic alphabetical order — the substrate the default
/// layering selects from.
/// Poison keys are excluded here (their drop is a note, made by
/// the callers).
fn usable_key_freq(
    result: &SourceScanResult,
) -> (std::collections::BTreeMap<String, usize>, Vec<String>) {
    let mut poisoned: Vec<String> = Vec::new();
    let mut freq: std::collections::BTreeMap<String, usize> = Default::default();
    for key in result.unique_keys() {
        if POISON_KEYS.contains(&key.as_str()) {
            poisoned.push(key);
        } else {
            let n = result.hits.iter().filter(|h| h.key == key).count();
            freq.insert(key, n);
        }
    }
    (freq, poisoned)
}

/// Decision notes for the scan: unsupported variants, unreadable
/// files, poison drops. Cap notes are reported by the caller that
/// applies the cap.
fn shared_decision_notes(result: &SourceScanResult, poisoned: &[String]) -> Vec<String> {
    let mut notes = Vec::new();
    if !result.unsupported.is_empty() {
        let mut u = result.unsupported.clone();
        u.sort();
        u.dedup();
        notes.push(format!(
            "unsupported variants (outside the emittable universe, skipped): {}",
            u.join(", ")
        ));
    }
    if !result.unreadable.is_empty() {
        notes.push(format!(
            "{} files unreadable (skipped)",
            result.unreadable.len()
        ));
    }
    if !poisoned.is_empty() {
        notes.push(format!(
            "poison-filtered (never given to the blind chaos bot): {}",
            poisoned.join(", ")
        ));
    }
    notes
}

/// Build the default battery's chaos surface from a source scan —
/// the fallback-tier contract (see the module docs):
///
/// - Scan yielded at least one usable key: the scan's FULL
///   poison-filtered unique-key set (backbone keys INCLUDED, so
///   scan-visible live keys like alien_cake's arrows survive)
///   REPLACES the blind WASD/arrows default, capped at
///   [`MAX_SCAN_KEYS`] TOTAL. The battery's ~200 chaos presses then
///   concentrate on keys the game's own code reads instead of
///   spreading over inert defaults.
/// - Empty or poison-only scan: the backbone stands — exactly the
///   surface that existed before the scan (ignorance fallback).
///
/// Either arm is recorded in the notes; note, though, that the
/// default-battery consumer only STAMPS notes for the scan-hit
/// arm (a set-equal-to-backbone run keeps the declared default
/// surface, so no rewrite and no stamp — the None honesty
/// contract; key order is load-bearing in the driver, so
/// refusing to rewrite IS the "no change" guarantee), so the
/// fallback arm's note is visible only to direct callers of
/// this function.
pub fn layered_chaos_keys(result: &SourceScanResult) -> LayeredKeys {
    let mut notes = Vec::new();
    notes.push(format!(
        "source scan: {} files, {} KeyCode hits, {} unique keys",
        result.files_scanned,
        result.hits.len(),
        result.unique_keys().len()
    ));
    // Shared substrate ([`usable_key_freq`]/[`shared_decision_notes`]): unsupported,
    // unreadable, and poison decisions read identically whichever
    // consumer reports them.
    let (freq, poisoned) = usable_key_freq(result);
    notes.extend(shared_decision_notes(result, &poisoned));

    if freq.is_empty() {
        notes.push(
            "no usable scan keys; WASD/arrows backbone fallback (surface unchanged)".to_string(),
        );
        let keys: Vec<String> = BACKBONE_KEYS.iter().map(|s| s.to_string()).collect();
        return LayeredKeys { keys, notes };
    }

    // Scan-hit arm: the scan's full poison-filtered surface,
    // capped at MAX_SCAN_KEYS TOTAL.
    let mut keys: Vec<(String, usize)> = freq.into_iter().collect();
    if keys.len() > MAX_SCAN_KEYS {
        // Most-referenced first, then alphabetical: deterministic
        // SELECTION (who survives the cap)...
        let dropped: Vec<String> = keys.iter().map(|(k, _)| k.clone()).collect();
        keys.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        keys.truncate(MAX_SCAN_KEYS);
        // ...and the drop is a reported decision, in the same
        // alphabetical order the pre-cap list held.
        let dropped: Vec<String> = dropped
            .into_iter()
            .filter(|k| !keys.iter().any(|(kept, _)| kept == k))
            .collect();
        notes.push(format!(
            "capped at {MAX_SCAN_KEYS} total scan-derived keys (dropped, least-referenced: {})",
            dropped.join(", ")
        ));
    }
    // Canonical EMIT order: alphabetical, so scan-derived surfaces
    // stay comparable across runs.
    let mut keys: Vec<String> = keys.into_iter().map(|(k, _)| k).collect();
    keys.sort();
    notes.push(format!(
        "scan-derived surface replaced the WASD/arrows default: {} keys: {}",
        keys.len(),
        keys.join(", ")
    ));
    LayeredKeys { keys, notes }
}

/// Resolve the scan root for THIS process: explicit
/// [`ENV_SOURCE_ROOT`] override wins; otherwise the runtime
/// `CARGO_MANIFEST_DIR` (in a battery parent — the game's own test
/// binary — that IS the game crate directory, zero-config);
/// otherwise no scan.
///
/// Self-skip: when the runtime manifest dir equals the HARNESS's
/// own baked-in source dir, the parent is bevy_playtest testing
/// itself — scanning our own sources would layer harness demo
/// keys onto every self-test battery run. Skip (unless the
/// explicit override names another root).
pub fn scan_root_from_env() -> Option<PathBuf> {
    if let Ok(root) = std::env::var(ENV_SOURCE_ROOT) {
        if !root.trim().is_empty() {
            return Some(PathBuf::from(root));
        }
        // An explicitly empty override is a deliberate disable.
        return None;
    }
    let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") else {
        return None;
    };
    // The harness's own crate dir, baked in at compile time.
    if manifest == env!("CARGO_MANIFEST_DIR") {
        return None; // self-test: no scan.
    }
    Some(PathBuf::from(manifest))
}

/// Whether the caller disabled the scan wholesale.
pub fn scan_disabled() -> bool {
    std::env::var(ENV_SOURCE_SCAN_DISABLE).as_deref() == Ok("1")
}

/// Scan a source tree for `KeyCode::` mentions. Reads every `.rs`
/// file under `root` recursively (sorted, deterministic), skipping
/// `target/`, `.git/`, and hidden directories. Fails open: an
/// unreadable file or a missing root yields fewer hits and a
/// record, never an error — the layered default degrades to the
/// backbone, which is what existed before the scan.
pub fn scan_from_root(root: &Path) -> SourceScanResult {
    let mut files = Vec::new();
    let mut visited = Vec::new();
    collect_rs_files(root, &mut files, &mut visited);
    drop(visited);
    files.sort();
    let mut result = SourceScanResult {
        files_scanned: files.len(),
        ..Default::default()
    };
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let Ok(bytes) = std::fs::read(&path) else {
            result.unreadable.push((rel, "read failed".to_string()));
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            result.unreadable.push((rel, "not UTF-8".to_string()));
            continue;
        };
        scan_text(&text, &rel, &mut result);
    }
    result
}

/// Recursive `.rs` collector; skip `target`, dot-directories.
///
/// Symlink safety: directories are tracked by CANONICAL path in a
/// visited set — a symlink cycle (e.g. `src/loop -> ..`) would
/// otherwise recurse forever, and a symlink FAN (several links to
/// the same real tree) would multiply-scan the same files. The set
/// is local to one `scan_from_root` walk; sorting happens in the
/// caller, so traversal order stays irrelevant to determinism.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>, visited: &mut Vec<PathBuf>) {
    // One canonicalization per directory: symlinked dirs collapse
    // to their real path, and repeated visits bail. An
    // uncanonicalizable path (permission-dependent metadata) is
    // walked as-is at most once — the visited set still catches
    // repeats of the exact same unresolved path.
    let canon = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if visited.contains(&canon) {
        return;
    }
    visited.push(canon);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // missing/unreadable dir: fail open, no record
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            collect_rs_files(&path, out, visited);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

const KEY_CODE_TOKEN: &[u8] = b"KeyCode::";

/// Extract `KeyCode::X` hits from one file's text (comment-, string-,
/// and char-literal-stripped first). Line numbers refer to the
/// ORIGINAL text: the stripper preserves byte offsets exactly.
fn scan_text(text: &str, rel_file: &str, result: &mut SourceScanResult) {
    let stripped = strip_rust_noise(text);
    let orig = text.as_bytes();
    debug_assert_eq!(stripped.len(), orig.len());

    // Line starts for byte-offset → line-number conversion.
    let mut line_starts: Vec<usize> = vec![0];
    for (i, b) in orig.iter().enumerate() {
        if *b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let line_of = |offset: usize| -> u32 {
        match line_starts.binary_search(&offset) {
            Ok(idx) => idx as u32 + 1,
            Err(next) => next as u32, // between starts: line index
        }
    };

    // Enclosing-item tracking over the stripped text: nearest
    // preceding `fn NAME(` line, else nearest `impl`/item line.
    // (Heuristic — `Evidence::SourceRef::enclosing` is documented
    // as best-effort.)
    //
    // Defense in depth: the stripper is byte-level, so an
    // unforeseen UTF-8 edge (a fix for one was itself found by
    // adversarial review) must fail OPEN here, recorded — never
    // silently zero the file's findings and never panic. Recover
    // per-invariant: skip the file with an `unreadable` record.
    let Some(stripped_str) = std::str::from_utf8(&stripped).ok().or_else(|| {
        result
            .unreadable
            .push((rel_file.to_string(), "strip corruption".to_string()));
        None
    }) else {
        return;
    };
    let mut enclosing_by_line: std::collections::HashMap<u32, Option<String>> = Default::default();
    {
        let mut last_item: Option<String> = None;
        for (line_idx, line) in stripped_str.lines().enumerate() {
            let lineno = line_idx as u32 + 1;
            if let Some(name) = leading_word_after(line, "fn ") {
                last_item = Some(format!("fn {name}"));
            } else if let Some(name) = leading_word_after(line, "impl") {
                last_item = Some(format!("impl {name}"));
            } else if let Some(name) = leading_word_after(line, "struct ") {
                last_item = Some(format!("struct {name}"));
            } else if let Some(name) = leading_word_after(line, "enum ") {
                last_item = Some(format!("enum {name}"));
            } else if let Some(name) = leading_word_after(line, "mod ") {
                last_item = Some(format!("mod {name}"));
            }
            enclosing_by_line.insert(lineno, last_item.clone());
        }
    }

    let mut i = 0usize;
    while i + KEY_CODE_TOKEN.len() <= stripped.len() {
        if &stripped[i..i + KEY_CODE_TOKEN.len()] == KEY_CODE_TOKEN {
            let mut j = i + KEY_CODE_TOKEN.len();
            while j < stripped.len() && (stripped[j].is_ascii_alphanumeric() || stripped[j] == b'_')
            {
                j += 1;
            }
            let variant = &stripped_str[i + KEY_CODE_TOKEN.len()..j];
            if variant.is_empty() {
                // `use KeyCode::*` and friends: nothing to name.
                i = j;
                continue;
            }
            match normalize_key_name(&variant.to_ascii_lowercase()) {
                Some(canonical) => {
                    let line = line_of(i);
                    result.hits.push(SourceScanHit {
                        key: canonical.to_string(),
                        file: rel_file.to_string(),
                        line,
                        enclosing: enclosing_by_line.get(&line).cloned().flatten(),
                    });
                }
                None => {
                    result.unsupported.push(variant.to_string());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
}

/// Best-effort item-name extraction: is `keyword` present as a word
/// in `line` (heuristic: substring), and what identifier follows?
fn leading_word_after(line: &str, keyword: &str) -> Option<String> {
    let kw = keyword.trim_end();
    let pos = line.find(kw)?;
    // Word-start check: preceded by whitespace or line start.
    let before_ok = pos == 0
        || line[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| !c.is_alphanumeric() && c != '_');
    if !before_ok {
        return None;
    }
    let after = line[pos + kw.len()..].trim_start();
    let name: String = after
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Blank Rust noise (comments, strings, char literals) with SPACES,
/// preserving the input's exact byte length and every `\n` so byte
/// offsets in the stripped text map 1:1 onto original line numbers.
/// Identifiers and punctuation survive; heuristic by design — this
/// is a candidate extractor, not a parser.
///
/// Char-literal vs lifetime (the subtlest split): the LOOKAHEAD
/// never crosses bytes the main loop has not yet classified, and
/// the classification follows rustc's literal shapes — `'` opens a
/// char literal only when the very next bytes are ONE character
/// (or one backslash escape) followed by `'` on the same line;
/// anything else (`'a`, `'static`, a lone apostrophe in prose) is
/// left alone as a lifetime-or-junk. No distance window, no
/// crossing into later strings/comments: `'static` next to
/// `"it's"` can never blank through the string.
fn strip_rust_noise(src: &str) -> Vec<u8> {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        // Line comment (keep the newline).
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
            continue;
        }
        // Block comment (nested; Rust allows it).
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            let mut depth = 1usize;
            while i + 1 < b.len() && depth > 0 {
                if b[i] == b'/' && b[i + 1] == b'*' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    depth += 1;
                    i += 2;
                    continue;
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    depth -= 1;
                    i += 2;
                    continue;
                }
                if b[i] != b'\n' {
                    out[i] = b' ';
                }
                i += 1;
            }
            // Unterminated comment runs to EOF: consume the final
            // byte too. Without this, a last UTF-8 continuation
            // byte (lead already blanked) survives as a dangling
            // byte and the stripped output stops being valid
            // UTF-8 — a WIP `/* TODO é` at EOF would corrupt the
            // whole file's strip.
            if depth > 0 && i < b.len() && b[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
            continue;
        }
        // Byte-prefix check: `b"..."` (byte string) or `b'x'` (byte
        // char) before the normal-string / char branches get a
        // chance to see the quote.
        let mut bskip = 0usize;
        if c == b'b' && i + 1 < b.len() && (b[i + 1] == b'"' || b[i + 1] == b'\'') {
            bskip = 1;
        }
        let q = i + bskip; // index of the quote itself
                           // Normal (and byte) string literal. Backslash escaping
                           // handled byte-by-byte; a backslash-newline CONTINUATION
                           // consumes the newline (legal Rust: the string resumes on
                           // the next line). The newline byte itself is KEPT so
                           // line counting survives; string content spans lines but
                           // each line keeps its terminator.
        if b[q] == b'"' {
            out[q] = b' ';
            i = q + 1;
            let mut esc = false;
            while i < b.len() {
                if esc {
                    esc = false;
                    // Line continuation: keep the newline for line
                    // numbering, keep consuming after it.
                    if b[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                    continue;
                }
                match b[i] {
                    b'\\' => {
                        esc = true;
                        out[i] = b' ';
                        i += 1;
                    }
                    b'\n' => break, // unterminated: bail, worst case under-strip
                    b'"' => {
                        out[i] = b' ';
                        i += 1;
                        break;
                    }
                    _ => {
                        out[i] = b' ';
                        i += 1;
                    }
                }
            }
            continue;
        }
        // Raw string: r"..." / r#"..."# / br#..."#. Matched only at a
        // WORD START for the `r`/`br` prefix — preceded by line
        // start or a non-identifier byte — so identifiers like
        // `root` or `xbr` never enter this branch.
        let prefix_len = if c == b'r' && is_word_start(b, i) {
            Some(1usize)
        } else if c == b'b' && is_word_start(b, i) && i + 2 < b.len() && b[i + 1] == b'r' {
            Some(2usize)
        } else {
            None
        };
        if let Some(prefix) = prefix_len {
            let raw_start = i + prefix;
            if raw_start < b.len() && (b[raw_start] == b'"' || b[raw_start] == b'#') {
                let mut j = raw_start;
                let mut hashes = 0usize;
                while j < b.len() && b[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
                if j < b.len() && b[j] == b'"' {
                    out[i..=j].fill(b' ');
                    i = j + 1;
                    // Close rule (verified against rustc): a `"`
                    // closes the raw string iff followed by AT
                    // LEAST `hashes` consecutive `#`s; exactly
                    // `hashes` are consumed, surplus hashes re-lex
                    // as ordinary tokens. Fewer hashes = content
                    // (`r##"z"#w"##` stays open through `"#`).
                    'raw_close: while i < b.len() {
                        if b[i] == b'"' {
                            let mut followed = 0usize;
                            while i + 1 + followed < b.len()
                                && followed < hashes
                                && b[i + 1 + followed] == b'#'
                            {
                                followed += 1;
                            }
                            if followed == hashes {
                                out[i..=i + hashes].fill(b' ');
                                i += hashes + 1;
                                break 'raw_close;
                            }
                        }
                        if b[i] != b'\n' {
                            out[i] = b' ';
                        }
                        i += 1;
                    }
                    continue;
                }
            }
        }
        // Char / byte-char literal vs lifetime: rustc's shape rule.
        // `'` opens a literal only when followed by ONE char (or
        // one escape) and a closing `'` on the same line; the
        // lookahead reads at most 3 classified content bytes and
        // never crosses a newline or a quote.
        let ch_q = i + bskip;
        if b[ch_q] == b'\'' {
            let close = char_literal_close(b, ch_q);
            if let Some(end) = close {
                out[ch_q..=end].fill(b' ');
                i = end + 1;
            } else {
                i = ch_q + 1; // lifetime / apostrophe junk: leave it
            }
            continue;
        }
        i += 1;
    }
    out
}

/// Is the byte at `i` the start of an identifier-like word (line
/// start, or preceded by a non-identifier ASCII byte)? Byte-level;
/// UTF-8 continuation bytes (0x80–0xBF) count as identifier-ish so
/// a multibyte char never spawns a phantom raw-string prefix.
fn is_word_start(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_' || b[i - 1] & 0x80 != 0)
}

/// Does the `'` at `start` open a char/byte-char literal, and
/// where does it close? Shape rule, mirroring rustc: exactly one
/// content unit — a single byte for simple literals (`'a'`,
/// multi-byte chars are handled by their lead byte: `'中'` closes
/// after the full UTF-8 sequence) or one backslash escape
/// (`'\n'`, `'\\''`, `'\u{1F600}'`) — then a closing `'` on the
/// same line. Anything else is NOT a literal: no bounded window,
/// no scanning into unclassified text.
fn char_literal_close(b: &[u8], start: usize) -> Option<usize> {
    if start + 1 >= b.len() {
        return None;
    }
    let mut j = start + 1;
    if b[j] == b'\\' {
        // Escape: consume the escaped indicator byte, plus a
        // `{...}` payload for the `\u{...}` / `\x{...}` forms
        // (bounded, same line, closed-brace required).
        j += 1;
        if j >= b.len() || b[j] == b'\n' {
            return None;
        }
        let indicator = b[j];
        j += 1;
        if (indicator == b'u' || indicator == b'x') && j < b.len() && b[j] == b'{' {
            j += 1;
            while j < b.len() && b[j] != b'\n' && b[j] != b'}' {
                j += 1;
            }
            if j >= b.len() || b[j] != b'}' {
                return None; // malformed: treat as junk, not literal
            }
            j += 1;
        }
    } else {
        // One UTF-8 scalar: consume the lead byte (ASCII counts as
        // its own lead), then any continuation bytes.
        if j >= b.len() || b[j] == b'\n' {
            return None;
        }
        j += 1;
        while j < b.len() && (b[j] & 0xC0) == 0x80 {
            j += 1;
        }
    }
    if j < b.len() && b[j] == b'\'' {
        Some(j)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys_of(text: &str) -> Vec<String> {
        let mut r = SourceScanResult::default();
        scan_text(text, "t.rs", &mut r);
        r.unique_keys()
    }

    #[test]
    fn finds_plain_variants_and_dedups() {
        let text = "fn a() { if k.just_pressed(KeyCode::KeyW) {} \
                    if k.just_pressed(KeyCode::KeyW) {} \
                    match k.just_pressed(KeyCode::Digit1) { true => {} _ => {} } }";
        assert_eq!(
            keys_of(text),
            vec!["Digit1".to_string(), "KeyW".to_string()]
        );
    }

    #[test]
    fn comments_are_ignored() {
        let text = "// KeyCode::KeyW doc\n\
                    /* KeyCode::KeyA block */\n\
                    /* outer /* KeyCode::Space nested */ still comment */\n\
                    let x = 1; // KeyCode::Escape";
        assert!(keys_of(text).is_empty(), "{:?}", keys_of(text));
    }

    #[test]
    fn strings_and_raw_strings_are_ignored() {
        let text = "let s = \"KeyCode::KeyW in a string\";\n\
                    let r = r#\"KeyCode::Digit9 in raw\"#;\n\
                    let r2 = r\"KeyCode::KeyZ plain raw\";\n\
                    let b = b\"KeyCode::KeyX bytes\";";
        assert!(keys_of(text).is_empty(), "{:?}", keys_of(text));
    }

    #[test]
    fn char_literals_and_lifetimes_do_not_confuse_the_stripper() {
        let text = "let c = '\\'';\n\
                    let d = 'a';\n\
                    fn f<'a>(x: &'a str) {}\n\
                    let hit = k.just_pressed(KeyCode::KeyQ);";
        assert_eq!(keys_of(text), vec!["KeyQ".to_string()]);
    }

    #[test]
    fn unsupported_variants_are_recorded_not_crashed() {
        let mut r = SourceScanResult::default();
        scan_text(
            "let a = KeyCode::F1; let b = KeyCode::KeyW;",
            "t.rs",
            &mut r,
        );
        assert_eq!(r.unique_keys(), vec!["KeyW".to_string()]);
        assert_eq!(r.unsupported, vec!["F1".to_string()]);
    }

    #[test]
    fn use_statements_with_wildcards_yield_nothing() {
        assert!(keys_of("use KeyCode::*;").is_empty());
    }

    #[test]
    fn line_numbers_survive_stripping() {
        // Hit on line 3: two noise lines above shift nothing.
        let mut r = SourceScanResult::default();
        scan_text(
            "// comment line 1 with padding padding padding padding\n\
                   /* block on line 2 */\n\
                   if k.just_pressed(KeyCode::KeyW) {}",
            "t.rs",
            &mut r,
        );
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.hits[0].line, 3);
        assert_eq!(r.hits[0].enclosing.as_deref(), None);
    }

    #[test]
    fn enclosing_fn_is_tracked() {
        let mut r = SourceScanResult::default();
        scan_text(
            "fn movement_system(\n  k: Res<ButtonInput<KeyCode>>,\n) {\n  if k.pressed(KeyCode::KeyW) {} }\n",
            "t.rs",
            &mut r,
        );
        assert_eq!(r.hits.len(), 1);
        assert_eq!(r.hits[0].enclosing.as_deref(), Some("fn movement_system"));
    }

    #[test]
    fn layered_keys_use_the_full_scan_surface_drop_poison() {
        // Scan-hit arm: the scan's full poison-filtered key set
        // REPLACES the default (backbone included only when the
        // scan itself cites it). Poison keys never enter, and the
        // arm + drop decisions are reported notes.
        let mut r = SourceScanResult::default();
        for key in ["KeyE", "KeyF", "KeyG", "Space"] {
            for _ in 0..3 {
                r.hits.push(SourceScanHit {
                    key: key.into(),
                    file: "t.rs".into(),
                    line: 1,
                    enclosing: None,
                });
            }
        }
        // Escape mentioned plenty: still never given to the bot.
        for _ in 0..9 {
            r.hits.push(SourceScanHit {
                key: "Escape".into(),
                file: "t.rs".into(),
                line: 1,
                enclosing: None,
            });
        }
        let layered = layered_chaos_keys(&r);
        assert_eq!(
            layered.keys,
            vec![
                "KeyE".to_string(),
                "KeyF".to_string(),
                "KeyG".to_string(),
                "Space".to_string(),
            ]
        );
        assert!(!layered.keys.contains(&"Escape".to_string()));
        assert!(layered.notes.iter().any(|n| n.contains("poison-filtered")));
        assert!(layered
            .notes
            .iter()
            .any(|n| n.contains("scan-derived surface replaced the WASD/arrows default")));
    }

    #[test]
    fn layered_keys_include_scan_visible_backbone_keys() {
        // The replacement surface is the scan's FULL key set — a
        // game citing its own arrows (alien_cake's case) keeps them:
        // they enter as scan evidence, not as blind default.
        let mut r = SourceScanResult::default();
        for key in ["ArrowUp", "ArrowDown"] {
            r.hits.push(SourceScanHit {
                key: key.into(),
                file: "t.rs".into(),
                line: 1,
                enclosing: None,
            });
        }
        let layered = layered_chaos_keys(&r);
        assert_eq!(
            layered.keys,
            vec!["ArrowDown".to_string(), "ArrowUp".to_string()]
        );
    }

    #[test]
    fn layered_keys_fall_back_to_backbone_on_empty_or_poison_only_scans() {
        // Ignorance fallback: no usable keys (nothing scanned, or
        // poison-only) keeps exactly the pre-scan default surface,
        // and the note says which arm fired.
        let layered = layered_chaos_keys(&SourceScanResult::default());
        assert_eq!(
            layered.keys,
            BACKBONE_KEYS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        assert!(layered
            .notes
            .iter()
            .any(|n| n.contains("backbone fallback")));

        let mut poison_only = SourceScanResult::default();
        for key in ["Escape", "Tab"] {
            poison_only.hits.push(SourceScanHit {
                key: key.into(),
                file: "t.rs".into(),
                line: 1,
                enclosing: None,
            });
        }
        let layered = layered_chaos_keys(&poison_only);
        assert_eq!(
            layered.keys,
            BACKBONE_KEYS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
        assert!(layered.notes.iter().any(|n| n.contains("poison-filtered")));
        assert!(layered
            .notes
            .iter()
            .any(|n| n.contains("backbone fallback")));
    }

    #[test]
    fn layered_keys_backbone_eight_scan_hits_scan_arm_but_set_equal() {
        // The corner the battery gate exists for: a scan citing
        // exactly the backbone 8. The scan-HIT arm fires (>=1
        // usable key), so the emit is ALPHABETICAL — NOT the
        // backbone's declared order. Consumers comparing in order
        // would falsely treat this as a changed surface; the
        // contract is SET equality. Pin both facts: the arm note
        // is present, and the key SET equals BACKBONE_KEYS.
        let mut r = SourceScanResult::default();
        for key in BACKBONE_KEYS {
            r.hits.push(SourceScanHit {
                key: (*key).into(),
                file: "t.rs".into(),
                line: 1,
                enclosing: None,
            });
        }
        let layered = layered_chaos_keys(&r);
        // Scan-hit arm, not fallback.
        assert!(layered
            .notes
            .iter()
            .any(|n| n.contains("scan-derived surface replaced the WASD/arrows default")));
        // Alphabetical emit — provably different ORDER from the
        // declared backbone (load-bearing in the driver), yet the
        // same SET.
        let mut sorted = layered.keys.clone();
        sorted.sort();
        assert_eq!(layered.keys, sorted);
        let mut backbone_sorted: Vec<String> =
            BACKBONE_KEYS.iter().map(|s| s.to_string()).collect();
        backbone_sorted.sort();
        assert_eq!(sorted, backbone_sorted);
    }

    #[test]
    fn cap_keeps_most_referenced_keys_and_reports_the_drop() {
        // Distinct keys: 30 > MAX_SCAN_KEYS triggers the cap on the
        // TOTAL scan-derived surface; the most-referenced keys must
        // survive it.
        let mut r = SourceScanResult::default();
        let mut keys: Vec<String> = Vec::new();
        // All 26 letters (WASD included — scan-visible backbone
        // keys compete on their own citations now), plus digits
        // 0-9, plus the named keys — all canonical.
        for letter in (b'A'..=b'Z').map(|c| c as char) {
            keys.push(format!("Key{letter}"));
        }
        for d in 0..=9 {
            keys.push(format!("Digit{d}"));
        }
        keys.extend(
            [
                "Space",
                "Enter",
                "Backspace",
                "ShiftLeft",
                "ControlLeft",
                "AltLeft",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        assert!(keys.len() > MAX_SCAN_KEYS, "setup must exceed the cap");
        // A, B, C are referenced most and must survive the cap.
        for key in &keys {
            let n = if matches!(key.as_str(), "KeyA" | "KeyB" | "KeyC") {
                5
            } else {
                1
            };
            for _ in 0..n {
                r.hits.push(SourceScanHit {
                    key: key.clone(),
                    file: "t.rs".into(),
                    line: 1,
                    enclosing: None,
                });
            }
        }
        let layered = layered_chaos_keys(&r);
        assert_eq!(layered.keys.len(), MAX_SCAN_KEYS);
        // Most-referenced survivors made the cut.
        for letter in ['A', 'B', 'C'] {
            assert!(
                layered.keys.contains(&format!("Key{letter}")),
                "{:?}",
                layered.keys
            );
        }
        assert!(layered
            .notes
            .iter()
            .any(|n| n.contains("capped at") && n.contains("total scan-derived keys")));
    }

    #[test]
    fn scan_from_root_is_deterministic_and_skips_target_dirs() {
        let root = tempdir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/main.rs"), "KeyCode::KeyW KeyCode::Digit1").unwrap();
        std::fs::write(root.join("src/lib.rs"), "KeyCode::KeyW").unwrap();
        std::fs::write(root.join("target/debug/main.rs"), "KeyCode::KeyZ").unwrap();
        std::fs::write(root.join(".git/config.rs"), "KeyCode::KeyX").unwrap();
        let r1 = scan_from_root(&root);
        let r2 = scan_from_root(&root);
        assert_eq!(r1.hits, r2.hits);
        assert_eq!(r1.files_scanned, 2);
        assert_eq!(
            r1.unique_keys(),
            vec!["Digit1".to_string(), "KeyW".to_string()]
        );
    }

    fn tempdir() -> std::path::PathBuf {
        // Counter separates same-tick sibling tests sharing one pid
        // (the clock alone is not unique enough — see battery.rs).
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_source_scan_test_{}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            seq,
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // -----------------------------------------------------------------
    // Adversarial regressions: every case below is a confirmed or
    // suspected lexer defect found in review; each encodes the
    // actual rustc semantics it exercises.
    // -----------------------------------------------------------------

    fn keys_of_result(text: &str) -> Vec<String> {
        let mut r = SourceScanResult::default();
        scan_text(text, "t.rs", &mut r);
        r.unique_keys()
    }

    fn assert_strip_utf8(text: &str, label: &str) {
        let stripped = strip_rust_noise(text);
        assert_eq!(stripped.len(), text.len(), "{label}: length not preserved");
        std::str::from_utf8(&stripped).unwrap_or_else(|e| {
            panic!("{label}: stripped output is not valid UTF-8: {e}\ninput: {text:?}")
        });
    }

    #[test]
    fn apostrophe_in_string_no_longer_wipes_real_code() {
        // C1 false negative: `'static` + a string containing "it's".
        // The old 32-byte lookahead paired the lifetime's `'` with
        // the string's apostrophe, blanking through real code.
        let src = "let v: Vec<&'static str> = vec![\"it's fine\"]; let k = (KeyCode::KeyR, 'c');";
        assert!(
            keys_of_result(src).contains(&"KeyR".to_string()),
            "real code mention wiped by apostrophe mispairing: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn apostrophe_in_comment_no_longer_promotes_comment_to_code() {
        // C1 false positive: the wipe crossed a `//` marker, and the
        // comment's KeyCode mention became a scan hit.
        let src = "fn f<'a>(x: &'a u8) {} // don't bind KeyCode::Space here";
        assert!(
            !keys_of_result(src).contains(&"Space".to_string()),
            "comment mention promoted to a scan hit: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn lifetimes_on_both_sides_of_a_mention_survive() {
        // Mention pinched between two lifetimes on one line.
        let src = "let a: &'a K = &'a KeyCode::KeyY;";
        assert!(
            keys_of_result(src).contains(&"KeyY".to_string()),
            "mention between two lifetimes wiped: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn apostrophe_display_string_before_binding_survives() {
        // Realistic false negative: dialogue/UI text on the same
        // line before a real binding mention.
        let src = "text.sections.push(\"Don't stop\".into()); if kb.just_pressed(KeyCode::Space) { fire(); }";
        assert!(
            keys_of_result(src).contains(&"Space".to_string()),
            "binding mention lost: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn backslash_newline_continuation_inside_string() {
        // C2: `\<newline>` is a legal line continuation; the old
        // string branch broke at the newline and re-synced inside
        // string content, eating real code after the close.
        let src = "let s = \"a\\\nb\"; KeyCode::KeyO;";
        assert_eq!(
            keys_of_result(src),
            vec!["KeyO".to_string()],
            "spliced string ate real code: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn byte_string_with_escapes_is_stripped() {
        // `b"..."` now routes through the escape-aware string
        // branch; escaped quotes must not leak as openers.
        let src = "let b = b\"KeyCode::KeyQ in bytes \\\\\"; if k.p(KeyCode::KeyW) {}";
        assert_eq!(
            keys_of_result(src),
            vec!["KeyW".to_string()],
            "b-string content leaked: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn raw_string_closes_only_with_at_least_n_hashes() {
        // C3, probe-verified against rustc:
        // - `r#"title="#` — `"` + exactly 1 hash CLOSES (content
        //   is `title=`);
        // - `r##"z"#w"##` — `"` + 1 hash < 2 is CONTENT;
        // - `r#"x"##y"#` — `"` + 2 hashes closes a 1-hash string,
        //   consuming exactly 1 hash; surplus re-lexes.
        let exact = "let a = r#\"title=\"#; KeyCode::KeyW;";
        assert!(keys_of_result(exact).contains(&"KeyW".to_string()));
        let fewer = "let s = r##\"a \"# b \"##; KeyCode::Digit4;";
        assert_eq!(
            keys_of_result(fewer),
            vec!["Digit4".to_string()],
            "inner \"# wrongly closed a 2-hash raw string: {:?}",
            keys_of_result(fewer)
        );
        let more = "let b = r#\"x\"##y\"#; KeyCode::KeyP;";
        // After the surplus `#` re-lexes as tokens, `y"#;` etc. is
        // ordinary code: `y` identifier, `"` opens a string that
        // swallows the rest of the line. KeyP must not be eaten by
        // any confusion — but it is inside that re-opened string,
        // so it legitimately disappears. What MUST hold: no panic,
        // UTF-8 stays valid, and `x`'s content is gone.
        let more_res = keys_of_result(more);
        assert!(!more_res.contains(&"KeyP".to_string()));
        assert_strip_utf8(more, "raw-more-hashes");
    }

    #[test]
    fn raw_string_css_and_html_shapes() {
        // Real-world raw content: HTML attributes (`color="#ff0000"`)
        // and CSS hex colors (`"#fff"`) contain `"#` sequences that
        // must not close a 1-hash raw string early... wait, they DO
        // close it per rustc (that code uses r#\"...\"#). The legal
        // form puts the `"` adjacent: these are content only when
        // the quote ISN'T followed by hashes. `color=\"#...` — the
        // `"` IS followed by `#`. Valid Rust requires escaping via
        // more hashes (r##\"...\"##). So a 1-hash raw string with
        // `"#` inside genuinely closes there in rustc — and here.
        // Assert our behavior matches: the mention after the close
        // is real code and must be found.
        let src = "let html = r#\"<font color=\"#ff0000\">\"#; if k.p(KeyCode::KeyW) {}";
        assert!(
            keys_of_result(src).contains(&"KeyW".to_string()),
            "code after genuine raw close was lost: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn raw_string_content_never_yields_hits() {
        // Content of a properly-closed raw string must produce NO
        // hits, even when it embeds a `"#`-adjacent payload (2
        // hashes keep the inner `"#` in content).
        let src = "let s = r##\"title=\"#KeyCode::Space was here\"##; if k.p(KeyCode::KeyW) {}";
        assert_eq!(
            keys_of_result(src),
            vec!["KeyW".to_string()],
            "raw-string content produced hits: {:?}",
            keys_of_result(src)
        );
    }

    #[test]
    fn multibyte_chars_near_quotes_stay_utf8() {
        assert_strip_utf8("let s = \"abc\"é\"def\";", "quote-adjacent-multibyte");
        assert_strip_utf8("let c = '中'; KeyCode::KeyW;", "multibyte-char-literal");
        assert_strip_utf8("let s = r#\"raw é\"#é", "raw-adjacent-multibyte");
        assert_strip_utf8("s.push('\\u{1F600}')é", "unicode-escape-char");
        assert_strip_utf8("let c = '\u{274c}'; x();", "emoji-char-literal");
    }

    #[test]
    fn unterminated_literals_at_eof_stay_utf8() {
        assert_strip_utf8("let s = \"unterminated", "unterminated-string");
        assert_strip_utf8("/* unterminated block", "unterminated-block");
        assert_strip_utf8("let c = 'x", "unterminated-char");
        assert_strip_utf8("let s = r#\"unterminated raw", "unterminated-raw");
        assert_strip_utf8("let s = r##\"incomplete #\"#", "partial-raw-close-at-eof");
    }

    #[test]
    fn char_literals_with_all_escape_shapes() {
        let src = "let a = '\\''; let b = '\\\\'; let c = '\\u{1F600}'; let d = '\u{274c}'; \
                   let e = '中'; let f = 'x'; let g = b'y'; \
                   fn f2<'a, 'b>(x: &'a str) -> &'b str { x } KeyCode::KeyQ;";
        assert_eq!(
            keys_of_result(src),
            vec!["KeyQ".to_string()],
            "char escape handling corrupted the stream: {:?}",
            keys_of_result(src)
        );
        // And the escape payloads are ACTUALLY blanked — the
        // literal forms must not survive in the stripped text
        // (regression: the `\\u{...}` branch was once dead code).
        let stripped = strip_rust_noise(src);
        let text = std::str::from_utf8(&stripped).unwrap();
        for needle in ["1F600", "u{274c}", "\\\\"] {
            assert!(
                !text.contains(needle),
                "char-literal payload {needle:?} leaked into stripped output: {text:?}"
            );
        }
    }

    #[test]
    fn unterminated_block_comment_at_eof_with_multibyte_tail() {
        // C1 regression (found in review round 2): a WIP comment
        // ending in a multibyte char at EOF (no trailing newline)
        // used to leave a dangling continuation byte — invalid
        // UTF-8, silently zeroing the file's hits.
        let src = "fn f(k: Res<ButtonInput<KeyCode>>) { if k.pressed(KeyCode::KeyW) {} } /* WIP é";
        assert_strip_utf8(src, "eof-multibyte-comment");
        assert_eq!(
            keys_of_result(src),
            vec!["KeyW".to_string()],
            "real code before the WIP comment was lost: {:?}",
            keys_of_result(src)
        );
        // The pure two-byte trigger as well.
        assert_strip_utf8("/*é", "minimal-utf8-tail");
        assert_strip_utf8("/*中", "three-byte-tail");
        assert_strip_utf8("/*❌", "four-byte-tail");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_cycles_terminate() {
        // C4: a symlink pointing at a parent must not hang or
        // multiply the scan. Bounded by the visited-set guard.
        let root = tempdir();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "KeyCode::KeyW").unwrap();
        std::os::unix::fs::symlink("..", root.join("src/loop")).unwrap();
        // A symlink fan: two links to the same real directory.
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/a.rs"), "KeyCode::Digit1").unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link_a")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link_b")).unwrap();
        let r = scan_from_root(&root);
        // src/main.rs + real/a.rs exactly once each: 2 files.
        assert_eq!(
            r.files_scanned, 2,
            "cycle/fan must not duplicate or hang: {r:?}"
        );
        let mut keys = r.unique_keys();
        keys.sort();
        assert_eq!(keys, vec!["Digit1".to_string(), "KeyW".to_string()]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fuzz_strip_length_and_utf8_invariants() {
        // Ported from review: combinatorial snippets over lexer
        // atoms must always preserve length and UTF-8 validity.
        let atoms: Vec<&str> = vec![
            "\"a\"",
            "\"it's\"",
            "// don't",
            "/* c'ment */",
            "'c'",
            "'中'",
            "'\\u{274c}'",
            "&'a x",
            "&'static x",
            "r#\"raw\"#",
            "r##\"r\"#x\"##",
            "b\"bytes\"",
            "KeyCode::KeyW",
            "\\\n",
            "é",
            "中",
            "\"unclosed",
            "/* unclosed",
            "'unclosed",
            "r#\"unclosed",
            "\"\",",
            "''",
            "'\\''",
            "r\"",
            "b'y'",
            "'\\u{1F600}'",
            "x'#y",
            "r###\"z\"##w\"###",
        ];
        // Deterministic LCG so failures reproduce.
        let mut state: u64 = 0x1234_5678_9abc_def0;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        for iter in 0..20000u64 {
            let n = 1 + rng() % 6;
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(atoms[rng() % atoms.len()]);
                s.push(if rng() % 3 == 0 { '\n' } else { ' ' });
            }
            let stripped = strip_rust_noise(&s);
            if stripped.len() != s.len() {
                panic!("iter {iter}: length changed for {s:?}");
            }
            if std::str::from_utf8(&stripped).is_err() {
                panic!(
                    "iter {iter}: stripped not UTF-8 for input {s:?}\nstripped bytes: {stripped:?}"
                );
            }
        }
    }

    #[test]
    fn fuzz_newline_count_is_invariant() {
        // Stripping must preserve EVERY newline (line mapping
        // depends on it).
        let atoms: Vec<&str> = vec![
            "\"a\nb\"",
            "// line\n",
            "/* x\ny */",
            "'c\n'",
            "'\\n'",
            "&'a",
            "r#\"a\nb\"#",
            "real code;\n",
            "KeyCode::KeyW\n",
            "\"a\\\nb\"",
            "'a\n'b",
        ];
        let mut state: u64 = 42;
        let mut rng = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        for iter in 0..20000u64 {
            let n = 1 + rng() % 6;
            let mut s = String::new();
            for _ in 0..n {
                s.push_str(atoms[rng() % atoms.len()]);
            }
            let stripped = strip_rust_noise(&s);
            let nl_orig = s.bytes().filter(|b| *b == b'\n').count();
            let nl_strip = stripped.iter().filter(|b| **b == b'\n').count();
            if nl_orig != nl_strip {
                panic!(
                    "iter {iter}: newline count changed {nl_orig}->{nl_strip} for {s:?}\nstripped: {:?}",
                    std::str::from_utf8(&stripped)
                );
            }
        }
    }
}
