//! Discovery cache (Phase 4).
//!
//! The discovered input surface is expensive to learn (~35s per game
//! at pool=8) and stable across scenarios: the same game binary,
//! built with the same features against the same bevy_playtest
//! version, has the same surface. The cache persists it, keyed by
//! everything that can change the answer:
//!
//! 1. **Game binary hash** — a code change rebuilds the binary and
//!    invalidates every entry for it. Computed with the crate's
//!    internal FNV-1a over the file bytes (same hasher as
//!    `digest.rs`/`replay.rs`): the cache key is a staleness token,
//!    not a security primitive, and the crate is deliberately
//!    dependency-narrow — sha2 would add five transitive crates for
//!    no functional gain.
//! 2. **bevy_playtest version** — schema/algorithm changes.
//! 3. **Feature fingerprint** — a different feature build is a
//!    different harness.
//! 4. **Fixture name** — distinguishes fixture games sharing one
//!    binary (the fixture battery compiles one binary for all its
//!    `tests/playtest.rs` cases).
//! 5. **Probe-plan fingerprint** — the matrix that produced the
//!    surface (sorted candidates, control key, tps, duration). A
//!    slim `BEVY_PLAYTEST_DISCOVER_KEYS` run and the full default
//!    matrix must never share an entry: the returned surface must
//!    match what the CURRENT plan can express, not what some earlier
//!    plan happened to learn.
//!
//! Cache lives in `.bevy_playtest/cache/` next to reports
//! (user-writable, gitignored; `rm -rf .bevy_playtest/cache` forces
//! fresh discovery). Entries are whole files: read → deserialize →
//! compare key → use or discard. A corrupt or unreadable entry is a
//! MISS (re-discover), never an error — a cache must never make
//! discovery FAIL, only make it slower.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::surface::DiscoveredSurface;

/// Everything that can change the discovered surface. Two surfaces
/// with equal keys are interchangeable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheKey {
    /// FNV-1a of the game binary's bytes (hex, for filename safety).
    pub game_binary_hash: String,
    /// `env!("CARGO_PKG_VERSION")` of the harness build.
    pub bevy_playtest_version: String,
    /// Sorted, comma-joined active feature names.
    pub feature_fingerprint: String,
    /// Fixture/game identifier; `None` for a plain game binary.
    pub fixture_name: Option<String>,
    /// Opaque fingerprint of the probe plan that produced the
    /// surface (candidates, control key, tps, duration). Change of
    /// any plan input ⇒ different key ⇒ miss. Computed by
    /// [`plan_fingerprint`] on the flow side.
    pub plan_fingerprint: String,
}

impl CacheKey {
    /// Compute the key for a concrete game binary + active feature
    /// set. `features` may be in any order (sorted internally).
    pub fn for_binary(
        binary: &Path,
        features: impl IntoIterator<Item = impl AsRef<str>>,
        fixture_name: Option<String>,
        plan_fingerprint: String,
    ) -> std::io::Result<Self> {
        let mut sorted: Vec<String> = features
            .into_iter()
            .map(|f| f.as_ref().to_string())
            .collect();
        sorted.sort();
        sorted.dedup();
        Ok(Self {
            game_binary_hash: hash_file(binary)?,
            bevy_playtest_version: env!("CARGO_PKG_VERSION").to_string(),
            feature_fingerprint: sorted.join(","),
            fixture_name,
            plan_fingerprint,
        })
    }

    /// Filename-safe rendering of the key for the cache entry file.
    ///
    /// EVERY component passes through [`sanitize`] — not just the
    /// human-authored ones: `CacheKey` is public with public fields,
    /// so a direct constructor can put path separators or `..`
    /// segments into ANY of them, and an unsanitized interpolation
    /// into the file name could steer `entry_path` OUTSIDE the
    /// requested cache directory (path traversal). The hex-shaped
    /// fields (`game_binary_hash`, `plan_fingerprint`) are hex when
    /// produced by the crate's own constructors, but that is a
    /// convention, not a type guarantee — sanitize unconditionally.
    fn slug(&self) -> String {
        // Truncate AFTER sanitizing, never the raw field: `sanitize`
        // emits ASCII-only, so slicing the CLEANED string can never
        // cut a multi-byte UTF-8 boundary (a raw byte slice like
        // `&hash[..16]` panics when a directly constructed or
        // deserialized key puts a multi-byte character across that
        // offset — CacheKey fields carry no type-level hex guarantee).
        let fixture = sanitize(self.fixture_name.as_deref().unwrap_or("game"));
        let sanitized_hash = sanitize(&self.game_binary_hash);
        let binary_hash = truncate_char_safe(&sanitized_hash, 16);
        // COLLISION DISCRIMINATORS (round 13 + review round 2):
        // sanitize is LOSSY, so distinct raw values can collapse
        // onto the same readable prefix — fixtures/a.json vs
        // fixtures_a.json, `None` vs Some("game") via the default,
        // or plan fingerprints differing only in sanitized-away
        // characters. The embedded-key check turns slug collisions
        // into PERMANENT cache misses (each run overwrites the
        // other's entry, then fails its own read-back) — never
        // corruption, but perpetual re-probing and a broken
        // per-key isolation contract. So every lossy-derived
        // segment also carries a short digest of its RAW value:
        // distinct raw identities get distinct slugs while the
        // human-readable prefixes stay for eyeballing the cache
        // directory. Injective by construction: each discriminator
        // hashes the raw value, not the sanitized one. (The
        // readable plan prefix is truncated to 8 chars for tidier
        // filenames — its FULL uniqueness lives in the
        // discriminator, unlike before, where truncation alone
        // decided identity to 32 bits.)
        let sanitized_plan = sanitize(&self.plan_fingerprint);
        let plan_fp = truncate_char_safe(&sanitized_plan, 8);
        let plan_discriminator = hex16(&fnv1a(&self.plan_fingerprint));
        let raw_fixture_tag = match &self.fixture_name {
            Some(name) => format!("some\u{1}{name}"),
            None => "none".to_owned(),
        };
        let fixture_discriminator = hex16(&fnv1a(&raw_fixture_tag));
        format!(
            "{fixture}_{fixture_discriminator}_{binary_hash}_{}_p{plan_fp}_{plan_discriminator}_f{}",
            // Short prefixes keep filenames tame; the full values are
            // inside the file and compared exactly on read.
            sanitize(&self.bevy_playtest_version),
            // A different feature set writes a DIFFERENT FILE (not
            // just a different embedded key), so stale entries
            // accumulate visibly instead of thrashing one name.
            sanitize(&hex16(&fnv1a(&self.feature_fingerprint))),
        )
    }
}

/// First `n` BYTES of `s`, cut back to a char boundary. Sanitized
/// input is ASCII so the walk is a no-op there; the boundary walk
/// keeps the helper panic-free on ANY input, so a future call site
/// cannot reintroduce the multi-byte slice panic by passing raw text
/// (defense in depth — the type system does not enforce hex here).
fn truncate_char_safe(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The probe-plan fingerprint ingredient of a [`CacheKey`]:
/// everything in [`super::probe_scenarios::ProbePlan`] that can
/// change the learned surface — candidate set (order-insensitive),
/// control key, tps, duration bit pattern — folded into one opaque
/// hex string with the internal FNV-1a. Pure and public so the flow
/// layer (which owns the plan) computes it exactly once per run.
pub fn plan_fingerprint(
    candidates: &[String],
    control_key: &str,
    tps: u32,
    duration_s: f32,
) -> String {
    let mut sorted: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
    sorted.sort();
    sorted.dedup();
    let mut acc = String::with_capacity(sorted.len() * 8);
    for key in sorted {
        acc.push_str(key);
        acc.push('\u{1}');
    }
    acc.push_str(control_key);
    acc.push('\u{1}');
    acc.push_str(&tps.to_string());
    acc.push('\u{1}');
    acc.push_str(&duration_s.to_bits().to_string());
    hex16(&fnv1a(&acc))
}

/// FNV-1a over bytes, hex-encoded (same hasher constants as
/// `hash_file`/`digest.rs` — the cache is a staleness token, not a
/// security primitive).
fn fnv1a(s: &str) -> u64 {
    let mut hasher: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hasher ^= *b as u64;
        hasher = hasher.wrapping_mul(0x100000001b3);
    }
    hasher
}

/// A u64 as 16 lowercase hex chars.
fn hex16(v: &u64) -> String {
    format!("{v:016x}")
}

/// The persisted discovery result: the canonical
/// [`DiscoveredSurface`] plus the key it was learned under.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CachedSurface {
    /// The key this entry was written under. Compared exactly on
    /// read: any mismatch is a stale entry.
    pub key: CacheKey,
    /// The discovered input surface (canonical report shape, Phase 5).
    pub surface: DiscoveredSurface,
}

/// Default cache root: `.bevy_playtest/cache/` under the current
/// directory.
pub fn default_cache_dir() -> PathBuf {
    Path::new(".bevy_playtest").join("cache")
}

/// Where the entry for `key` lives under `dir`.
fn entry_path(dir: &Path, key: &CacheKey) -> PathBuf {
    dir.join(format!("{}.json", key.slug()))
}

/// Read the cached surface for `key`, if a fresh entry exists.
///
/// Misses return `Ok(None)`; so do unreadable, corrupt, and stale
/// entries (refusal applies to INFRASTRUCTURE, not to caches — a
/// broken cache merely costs a re-discovery: permission denied,
/// transient IO, mid-write races, none of them may fail discovery).
pub fn read_cached(dir: &Path, key: &CacheKey) -> std::io::Result<Option<CachedSurface>> {
    let path = entry_path(dir, key);
    // Every read error is a miss — NotFound (no entry), permission
    // denied, transient IO failure, mid-write race: none of them say
    // anything about the surface, and none may fail the run.
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(None),
    };
    let entry: CachedSurface = match serde_json::from_slice(&bytes) {
        Ok(entry) => entry,
        Err(_) => return Ok(None), // Corrupt entry == miss.
    };
    // Exact-key check: same slug can race a rebuild mid-write; the
    // embedded key is authoritative.
    Ok(if entry.key == *key { Some(entry) } else { None })
}

/// Whether the file at `path` is a VALID cache entry carrying exactly
/// `key`. This is the "concurrent publish?" decision for the Windows
/// rename-collision path (`std::fs::rename` may fail there when the
/// destination exists — see `write_cached`): a valid same-key entry
/// means some other writer already published this surface and the
/// collision is a SUCCESS; anything else — the file is absent,
/// torn mid-write, corrupt garbage, or carries a foreign key —
/// is simply `false` (the boolean deliberately conflates those
/// cases: ALL of them mean "not a publish we can trust", and the
/// caller heals or fails fast accordingly). Factored out — and unit
/// tested directly — because the collision itself cannot be provoked
/// deterministically on Unix (renames replace silently).
fn existing_entry_matches(path: &Path, key: &CacheKey) -> bool {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<CachedSurface>(&bytes).ok())
        .map(|entry| entry.key == *key)
        .unwrap_or(false)
}

/// Persist `surface` under its own key. Best-effort: a failure to
/// write the cache must not fail discovery (the surface is already
/// learned; only the NEXT run would pay again) — logged as the Err
/// for callers that care, swallowed by callers that don't.
pub fn write_cached(dir: &Path, surface: &CachedSurface) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(surface)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Write-then-rename: a reader never sees a half-written entry.
    // The temp name is unique PER WRITE (PID + nanoseconds + an
    // in-process monotonic counter): two concurrent discoveries
    // sharing a cache dir cannot clobber or steal each other's
    // in-flight bytes via one fixed `.tmp` name. The counter is
    // load-bearing: the OS clock alone is NOT enough — sibling
    // writes within one coarse clock tick would collide on the
    // nanos component (observed as a flake on macOS), and the PID
    // only separates PROCESSES, not threads.
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = entry_path(dir, &surface.key);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("json.tmp.{}.{}.{}", std::process::id(), nanos, seq));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, &path) {
        Err(e) => {
            // Windows portability: `std::fs::rename` MAY fail when the
            // destination exists — modern std uses
            // `MoveFileExW(REPLACE_EXISTING)` / POSIX rename info
            // (Win10 1607+), so replacement usually works there; it can
            // still fail when the destination is held open without
            // `FILE_SHARE_DELETE`, on filesystems lacking POSIX rename
            // support, or on pre-1607 Windows. On Unix a rename always
            // replaces. The collision handling below covers whichever
            // platform/filesystem actually refuses to replace.
            //
            // "Destination already exists" is a SUCCESS under our
            // semantics — some concurrent writer already published a
            // complete payload for this key — PROVIDED what sits there
            // is valid: a reader must never end up with garbage.
            //
            // HEAL GATING: the destructive heal (remove + retry) is for
            // DESTINATION problems only. Other rename failure kinds —
            // permission denied on the DIRECTORY, antivirus, IO errors,
            // source-side trouble — would make the retry fail the same
            // way, and an unconditional remove would needlessly destroy
            // a possibly-valid entry (turning a stale cache into an
            // EMPTIED one) for no gain. Only error kinds that plausibly
            // indicate a destination in place enter the heal; everything
            // else fails fast, leaving the existing entry intact.
            let destination_in_place = matches!(
                e.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
            );
            if destination_in_place && existing_entry_matches(&path, &surface.key) {
                // Never leave our unique temp file behind.
                let _ = std::fs::remove_file(&tmp);
                return Ok(());
            }
            // KNOWN WINDOW: the heal's remove-then-rename is not atomic —
            // between the remove and the retried rename, another writer
            // may claim the path. The retry below closes the visible
            // harm: a valid winning entry is recognized as a
            // concurrent-publish success instead of dropping OUR payload
            // on the floor. Residual imperfection: our own publication
            // is skipped (the winner's valid identical-key entry serves
            // readers equally), which is exactly the success semantics
            // of the "destination already exists" branch above.
            // Corruption is impossible in every interleaving — nothing
            // is ever read without the embedded-key check.
            //
            // Heal: retry with our whole file — but ONLY on
            // destination-plausible kinds (see HEAL GATING above). If
            // the destination was claimed between our validity probe and
            // this remove by yet another writer's VALID entry, the retry
            // fails with "already exists" — the same concurrent-publish
            // success as above, not a lost publication. Only if the
            // retry fails and the destination STILL isn't a valid
            // matching entry do we propagate the error (best-effort
            // contract; the next run re-discovers). The heal's
            // remove-then-rename is knowingly non-atomic — see the
            // KNOWN WINDOW note above — but every outcome stays inside
            // the no-corruption envelope: readers verify embedded keys,
            // writers never read garbage.
            if destination_in_place {
                let _ = std::fs::remove_file(&path);
                let retried = std::fs::rename(&tmp, &path);
                if retried.is_err() {
                    if existing_entry_matches(&path, &surface.key) {
                        // A concurrent writer won the heal race with a
                        // valid entry: success under concurrent-publish
                        // semantics.
                        let _ = std::fs::remove_file(&tmp);
                        return Ok(());
                    }
                    // Never leave our unique temp file behind on a
                    // failed rename.
                    let _ = std::fs::remove_file(&tmp);
                }
                return retried;
            }
            // Non-destination failure: fail fast. Remove only OUR temp
            // file; whatever entry sits at the destination (if any)
            // stays untouched — it may be perfectly valid.
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
        renamed => renamed,
    }
}

/// Hash a file's bytes with the internal FNV-1a, hex-encoded.
///
/// The file's LENGTH is folded into the same accumulator after the
/// content (eight little-endian FNV rounds): the binary hash keys
/// cache identity, and while FNV-1a-64 collisions are already
/// negligible for realistic binary counts, two same-LENGTH binaries
/// colliding is the one structured case left — feeding length in
/// removes it, shrinking the practical collision class to
/// adversarial-only at zero cost. (A collision would otherwise
/// serve one game's discovered surface for another, marked Exact —
/// the one failure mode in this module that is NOT self-detected by
/// the embedded-key read-back, since the key embeds the same hash.)
fn hash_file(path: &Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher: u64 = 0xcbf29ce484222325;
    let mut buf = [0u8; 64 * 1024];
    let mut total_len: u64 = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        total_len += n as u64;
        for &b in &buf[..n] {
            hasher ^= b as u64;
            hasher = hasher.wrapping_mul(0x100000001b3);
        }
    }
    for b in total_len.to_le_bytes() {
        hasher ^= b as u64;
        hasher = hasher.wrapping_mul(0x100000001b3);
    }
    Ok(format!("{hasher:016x}"))
}

/// Filename-safe subset of a string. `/` never survives, so the
/// result is always a single path component; reserved components
/// (".", "..", empty) are additionally neutralized.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "_".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::diff::Effect;
    use crate::discovery::surface::{KeyConfidence, KeyFinding};
    fn key(binary_hash: &str, version: &str, features: &str, fixture: Option<&str>) -> CacheKey {
        key_with_plan(binary_hash, version, features, fixture, "a1b2c3d4e5f60718")
    }

    #[test]
    fn hostile_cache_keys_cannot_escape_the_cache_directory() {
        // Copilot r4468466211: `CacheKey` is public with public
        // fields, so a direct constructor can smuggle path
        // separators, absolute prefixes, or `..` segments into any
        // component. `slug()` interpolates every component into the
        // entry FILE NAME, so an unsanitized one could steer
        // `entry_path` outside the requested cache directory
        // (path traversal — write/read of arbitrary .json files).
        // Every component must be filename-sanitized, including the
        // conventionally-hex ones (`game_binary_hash`,
        // `plan_fingerprint`): hex is a constructor convention, not
        // a type guarantee.
        let hostile = key("deadbeef", "1.0.0", "default", None);
        let with_slash = key("../../etc/passwd", "1.0.0", "default", None);
        let abs_prefix = key("/etc/crontab", "1.0.0", "default", None);
        for k in [&hostile, &with_slash, &abs_prefix] {
            let slug = k.slug();
            assert!(
                !slug.contains('/') && !slug.contains('\\'),
                "slug must never embed a path separator: {slug}"
            );
            assert!(
                Path::new(&slug)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "slug must resolve as a plain relative file name: {slug}"
            );
        }
        // Distinct hostile inputs must still produce distinct,
        // deterministic slugs (sanitization is lossy but the
        // embedded full key compared on read still discriminates).
        assert_ne!(with_slash.slug(), abs_prefix.slug());
    }

    #[test]
    fn hostile_fixture_and_plan_fingerprints_stay_inside_the_dir() {
        // Same traversal class through the OTHER interpolatable
        // fields (r4468466211 mentions both the hash prefix and the
        // plan fingerprint; the fixture name and version were
        // already sanitized but pin them too).
        let hostile = CacheKey {
            game_binary_hash: "aaaa".to_string(),
            bevy_playtest_version: "../..\\evil".to_string(),
            feature_fingerprint: "brp,takeover".to_string(),
            fixture_name: Some("../../../secrets".to_string()),
            plan_fingerprint: "..\\/escape".to_string(),
        };
        let slug = hostile.slug();
        assert!(
            !slug.contains('/') && !slug.contains('\\'),
            "no path separators may survive: {slug}"
        );
        let dir = default_cache_dir();
        let path = entry_path(&dir, &hostile);
        // The entry must stay INSIDE the cache directory: the joined
        // path's components after the directory prefix must all be
        // Normal (no ParentDir escaping, no RootDir from an
        // absolute-looking slug).
        let rel = path
            .strip_prefix(&dir)
            .expect("entry path must stay under the cache dir");
        assert!(
            rel.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "entry must be a single file under the cache dir, got: {rel:?}"
        );
    }

    #[test]
    fn non_ascii_key_components_cannot_panic_the_slug() {
        // Copilot r4469158240: the slug used to truncate the RAW
        // fields with a byte slice (`&hash[..16]`). A directly
        // constructed or deserialized `CacheKey` can carry multi-byte
        // UTF-8 (the hex shapes are a constructor convention, not a
        // type guarantee), and a raw byte cut at a non-boundary
        // offset PANICS. These inputs place two-byte characters
        // exactly across byte 16 (hash prefix) and byte 8 (plan
        // fingerprint): 5 ASCII bytes then `é` pairs puts the cut
        // inside a character for BOTH offsets.
        let straddles_hash_cut = CacheKey {
            game_binary_hash: format!("abcde{}", "é".repeat(6)),
            bevy_playtest_version: "1.0.0".to_string(),
            feature_fingerprint: "default".to_string(),
            fixture_name: None,
            plan_fingerprint: format!("abcde{}", "é".repeat(6)),
        };
        // Reversed stress: non-ASCII across the whole field, plus
        // every separator-worthy hostile char for good measure.
        let all_multi_byte = CacheKey {
            game_binary_hash: "日本語ハッシュ値".to_string(),
            bevy_playtest_version: "版本💡".to_string(),
            feature_fingerprint: "特征".to_string(),
            fixture_name: Some("フィクスチャ".to_string()),
            plan_fingerprint: "指纹值".to_string(),
        };
        for k in [&straddles_hash_cut, &all_multi_byte] {
            // No panic is the primary pin; the separators and
            // single-component properties stay asserted too.
            let slug = k.slug();
            assert!(
                !slug.contains('/') && !slug.contains('\\'),
                "slug must never embed a path separator: {slug}"
            );
            assert!(
                Path::new(&slug)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
                "slug must resolve as a plain relative file name: {slug}"
            );
        }
        // Determinism across repeated calls (slugs feed file names).
        assert_eq!(straddles_hash_cut.slug(), straddles_hash_cut.slug());
        assert_ne!(straddles_hash_cut.slug(), all_multi_byte.slug());
    }

    fn key_with_plan(
        binary_hash: &str,
        version: &str,
        features: &str,
        fixture: Option<&str>,
        plan: &str,
    ) -> CacheKey {
        CacheKey {
            game_binary_hash: binary_hash.to_string(),
            bevy_playtest_version: version.to_string(),
            feature_fingerprint: features.to_string(),
            fixture_name: fixture.map(|f| f.to_string()),
            plan_fingerprint: plan.to_string(),
        }
    }

    fn surface(k: CacheKey) -> CachedSurface {
        CachedSurface {
            key: k,
            surface: DiscoveredSurface {
                keys: vec![KeyFinding {
                    key: "Digit1".to_string(),
                    effect: Some(Effect {
                        first_divergence_frame_delta: 1,
                    }),
                    confidence: KeyConfidence::Exact,
                }],
                probed_at: vec![60, 90],
                ..Default::default()
            },
        }
    }

    fn temp_dir() -> PathBuf {
        // Process id + nanosecond timestamp + a monotonic counter: the
        // clock alone is NOT enough — sibling tests can start within
        // the same coarse-granularity tick and would then share (and
        // race-cleanup) one directory.
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "bevy_playtest_cache_test_{}_{}_{}",
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

    #[test]
    fn round_trip_hits_on_identical_key() {
        let dir = temp_dir();
        let k = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
        );
        write_cached(&dir, &surface(k.clone())).unwrap();
        let loaded = read_cached(&dir, &k).unwrap().expect("hit");
        assert_eq!(loaded, surface(k));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn version_change_is_a_miss() {
        let dir = temp_dir();
        let k = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
        );
        write_cached(&dir, &surface(k.clone())).unwrap();
        // Different slug only if version differs in first 16 chars —
        // same file either way; the embedded key decides.
        let stale = key(
            "abcdef0123456789fedcba",
            "0.7.0",
            "in-process,takeover",
            None,
        );
        assert_eq!(read_cached(&dir, &stale).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn binary_change_is_a_miss() {
        let dir = temp_dir();
        let k = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
        );
        write_cached(&dir, &surface(k.clone())).unwrap();
        let rebuilt = key("ffffffffffffffffffff", "0.6.0", "in-process,takeover", None);
        assert_eq!(read_cached(&dir, &rebuilt).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn feature_change_is_a_miss() {
        let dir = temp_dir();
        let k = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
        );
        write_cached(&dir, &surface(k.clone())).unwrap();
        let different_build = key("abcdef0123456789fedcba", "0.6.0", "in-process", None);
        assert_eq!(read_cached(&dir, &different_build).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn fixture_change_is_a_miss() {
        let dir = temp_dir();
        let k = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
        );
        write_cached(&dir, &surface(k.clone())).unwrap();
        let other_fixture = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            Some("breakout"),
        );
        assert_eq!(read_cached(&dir, &other_fixture).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lossy_sanitize_collisions_get_distinct_entry_paths() {
        // Copilot #58 round 13: `sanitize` is LOSSY, so distinct raw
        // fixture identities used to collapse onto one slug —
        // `fixtures/a.json` vs `fixtures_a.json` (both sanitize to
        // `fixtures_a.json`), and `None` vs `Some("game")` (both use
        // the `game` prefix). The collision turned into PERPETUAL
        // cache misses: alternating runs overwrite each other's
        // entry, then fail the embedded-key check on read-back. The
        // raw-identity discriminator in `slug()` must give every
        // distinct raw identity its own entry path.
        let dir = temp_dir();
        let slash = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            Some("fixtures/a.json"),
        );
        let underscore = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            Some("fixtures_a.json"),
        );
        let none = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
        );
        let some_default = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            Some("game"),
        );
        assert_ne!(
            entry_path(&dir, &slash),
            entry_path(&dir, &underscore),
            "sanitize-equal fixture names must not share an entry file"
        );
        assert_ne!(
            entry_path(&dir, &none),
            entry_path(&dir, &some_default),
            "`None` and `Some(\"game\")` must not share the default entry file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn colliding_fixtures_coexist_instead_of_thrashing_one_entry() {
        // The user-visible contract of the round-13 fix: with the
        // discriminator in place, two sanitize-colliding fixtures
        // BOTH stay cached — the second write no longer clobbers the
        // first's file. Pre-fix, each write destroyed the other's
        // entry and every read after an alternating write was a
        // miss, i.e. perpetual re-probing despite a warm cache.
        let dir = temp_dir();
        let a = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            Some("fixtures/a.json"),
        );
        let b = key(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            Some("fixtures_a.json"),
        );
        write_cached(&dir, &surface(a.clone())).unwrap();
        write_cached(&dir, &surface(b.clone())).unwrap();
        assert_eq!(
            read_cached(&dir, &a)
                .unwrap()
                .expect("a must survive b's write"),
            surface(a)
        );
        assert_eq!(
            read_cached(&dir, &b)
                .unwrap()
                .expect("b must survive a's write"),
            surface(b)
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn lossy_plan_prefix_collisions_get_distinct_entry_paths() {
        // Review round 2 (slug segments): the plan-fingerprint
        // READABLE prefix is truncated to 8 sanitized chars, so two
        // distinct plans whose first 8 sanitizable characters agree
        // used to collapse onto one entry file — the same perpetual
        // re-probe thrash the fixture discriminator prevents, just
        // via a different segment. The plan discriminator (raw-value
        // digest) keeps them apart.
        let dir = temp_dir();
        let a = key_with_plan(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
            "abcdefghTail1",
        );
        let b = key_with_plan(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
            "abcdefgh!@#$/tail2",
        );
        // The readable prefixes really do collide (pin the premise).
        let slug_a = a.slug();
        let slug_b = b.slug();
        assert!(
            slug_a.contains("_pabcdefgh") && slug_b.contains("_pabcdefgh"),
            "test premise broken: readable prefixes no longer collide \
             ({slug_a} vs {slug_b})"
        );
        assert_ne!(
            entry_path(&dir, &a),
            entry_path(&dir, &b),
            "sanitize-colliding plan fingerprints must not share an entry file"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_dir_is_a_clean_miss() {
        let dir = temp_dir();
        let k = key("abcdef0123456789fedcba", "0.6.0", "", None);
        assert_eq!(read_cached(&dir, &k).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn corrupt_entry_is_a_miss_not_an_error() {
        let dir = temp_dir();
        let k = key("abcdef0123456789fedcba", "0.6.0", "", None);
        std::fs::create_dir_all(&dir).unwrap();
        let path = entry_path(&dir, &k);
        std::fs::write(&path, b"not json at all").unwrap();
        assert_eq!(read_cached(&dir, &k).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn slug_is_filename_safe() {
        let base = std::env::temp_dir();
        let k = key("abcdef0123456789", "0.6.0", "", Some("weird/../name!"));
        let path = entry_path(&base, &k);
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(!name.contains('/'), "single component only");
        assert!(name.ends_with(".json"));
        // The resolved entry lands inside the cache dir (no /, no
        // traversal-only components escaping it).
        let resolved = base.canonicalize().unwrap().join(name);
        assert!(resolved.starts_with(base.canonicalize().unwrap()));
        // The slug embeds the sanitized fixture name.
        assert!(name.starts_with("weird_"));
        // Reserved components neutralize.
        let dot = key("abcdef0123456789", "0.6.0", "", Some(".."));
        let dot_name = entry_path(&base, &dot)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert_ne!(dot_name, "..");
        assert!(!dot_name.starts_with(".."));
    }

    #[test]
    fn feature_fingerprint_is_order_insensitive() {
        let dir = temp_dir();
        std::fs::write(dir.join("bin"), b"contents don't matter").unwrap();
        let plan = "a1b2c3d4e5f60718".to_string();
        let a = CacheKey::for_binary(
            &dir.join("bin"),
            ["in-process", "takeover"],
            None,
            plan.clone(),
        )
        .unwrap();
        let b = CacheKey::for_binary(
            &dir.join("bin"),
            ["takeover", "in-process", "in-process"],
            None,
            plan,
        )
        .unwrap();
        assert_eq!(a, b, "feature order (and dupes) must not change the key");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn concurrent_writers_do_not_corrupt_each_other() {
        // Fixed `.tmp` names made concurrent discoveries sharing a
        // cache dir able to clobber each other's in-flight bytes (one
        // rename can steal the other's file). The write must use a
        // unique temp file per write so each writer renames its OWN
        // complete payload. Nonsense payload sizes make collisions
        // detectable: a stolen/truncated file never parses.
        //
        // Publication is replace-tolerant (Windows portability): the
        // loser of a rename race — `std::fs::rename` fails when the
        // destination exists on Windows — must treat a valid
        // existing destination as a successful concurrent publish,
        // never an error and never a torn file.
        let dir = temp_dir();
        let k = key("abcdef0123456789", "0.6.0", "in-process", None);
        let mut entries = Vec::new();
        for i in 0..16 {
            let mut s = surface(k.clone());
            // Distinct payloads: hundreds of keys, uniquely sized.
            s.surface.keys = (0..(i + 1) * 50)
                .map(|j| KeyFinding {
                    key: format!("Key{j}"),
                    effect: None,
                    confidence: KeyConfidence::Exact,
                })
                .collect();
            entries.push(s);
        }

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(entries.len()));
        let dir_shared = std::sync::Arc::new(dir.clone());
        let handles: Vec<_> = entries
            .into_iter()
            .map(|s| {
                let barrier = barrier.clone();
                let dir = dir_shared.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    write_cached(&dir, &s).expect("write must succeed")
                })
            })
            .collect();
        let mut written = 0;
        for h in handles {
            h.join().expect("writer thread");
            written += 1;
        }

        // Exactly one final entry exists, and it parses and carries
        // SOME writer's complete payload (last rename wins, and every
        // rename was of a whole self-consistent file).
        assert_eq!(written, 16);
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(files.len(), 1, "exactly the final entry: {files:?}");
        let read = read_cached(&dir, &k).unwrap().expect("entry must hit");
        let len = read.surface.keys.len();
        assert!(
            len.is_multiple_of(50) && (50..=800).contains(&len),
            "corrupted payload: {len} keys"
        );
        // No temp litter survives failed/interrupted writes.
        assert!(files
            .iter()
            .all(|f| !f.file_name().to_str().unwrap().contains(".tmp")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn garbage_destination_is_healed_not_trusted() {
        // The Windows replace-tolerance path: a rename racing an
        // existing destination must only count as a "successful
        // concurrent publish" when what sits there is a VALID entry
        // for the same key. Garbage at the destination (a torn
        // write, foreign file) must be HEALED — removed and replaced
        // by our whole file — never trusted as-is and never
        // allowed to fail the publication outright.
        //
        // The collision branch itself cannot be provoked on Unix
        // (renames replace silently), so the decision is pinned at
        // BOTH levels: the extracted validator directly (here), and
        // the heal outcome end-to-end below (a garbage file is
        // overwritten whole either way).
        let dir = temp_dir();
        let k = key("abcdef0123456789", "0.6.0", "in-process", None);
        let dest = entry_path(&dir, &k);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&dest, b"this is not json").unwrap();
        // Garbage is NOT a concurrent publish...
        assert!(!existing_entry_matches(&dest, &k));
        // ...while a real entry for this key IS.
        write_cached(&dir, &surface(k.clone())).unwrap();
        assert!(existing_entry_matches(&dest, &k));

        // And healing works end-to-end: publish over garbage again.
        std::fs::write(&dest, b"torn wr").unwrap();
        write_cached(&dir, &surface(k.clone())).expect("publication must heal a garbage file");

        // The destination now carries OUR valid payload...
        let read = read_cached(&dir, &k)
            .unwrap()
            .expect("healed entry must hit");
        assert_eq!(read.surface.keys.len(), 1);
        // ...and no temp litter survived.
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(files
            .iter()
            .all(|f| !f.file_name().to_str().unwrap().contains(".tmp")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rename_failure_over_a_valid_existing_entry_is_a_concurrent_publish_success() {
        // The core of the Windows fix, pinned through the extracted
        // collision decision: when publication finds a VALID entry
        // for the same key already sitting at the destination, it
        // must treat that as success without disturbing that entry.
        // The collision itself cannot be provoked on Unix (renames
        // replace silently), so we pin the DECISION plus the
        // end-to-end invariant it protects: whichever of two
        // writers' payloads lands, the destination stays a valid,
        // complete, parseable entry.
        let dir = temp_dir();
        let k = key("abcdef0123456789", "0.6.0", "in-process", None);
        let mut first = surface(k.clone());
        first.surface.keys = (0..50)
            .map(|j| KeyFinding {
                key: format!("Key{j}"),
                effect: None,
                confidence: KeyConfidence::Exact,
            })
            .collect();
        write_cached(&dir, &first).unwrap();
        // A second writer with a different payload for the same
        // key: the first entry is a valid same-key publish, so a
        // losing rename must count as success — pinned directly...
        assert!(existing_entry_matches(&entry_path(&dir, &k), &k));
        // ...and the second publication succeeds either way, the
        // destination remaining a whole payload from ONE writer.
        write_cached(&dir, &surface(k.clone())).expect("second publication must succeed");
        let read = read_cached(&dir, &k).unwrap().expect("entry must hit");
        assert!(
            read.surface.keys.len() == 50 || read.surface.keys.len() == 1,
            "whole payload from one of the writers: {}",
            read.surface.keys.len()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plan_change_is_a_miss_in_both_directions() {
        // A slim-matrix entry must NOT answer a full-matrix request,
        // and vice versa: the cached surface must match what the
        // CURRENT plan can express (review: "cache key omits probe
        // plan inputs" — cover both directions). Separate cache dirs
        // keep the two directions independent (entries for different
        // plans are different FILES and coexist in one dir).
        let slim = key_with_plan(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
            "1111111111111111",
        );
        let full = key_with_plan(
            "abcdef0123456789fedcba",
            "0.6.0",
            "in-process,takeover",
            None,
            "2222222222222222",
        );

        // Direction 1: a slim entry must not answer a full request.
        let dir = temp_dir();
        write_cached(&dir, &surface(slim.clone())).unwrap();
        assert_eq!(
            read_cached(&dir, &full).unwrap(),
            None,
            "full-matrix request must not consume a slim entry"
        );
        // The slim entry still answers ITSELF.
        assert!(read_cached(&dir, &slim).unwrap().is_some());
        std::fs::remove_dir_all(&dir).unwrap();

        // Direction 2: a full entry must not answer a slim request.
        let dir = temp_dir();
        write_cached(&dir, &surface(full.clone())).unwrap();
        assert_eq!(
            read_cached(&dir, &slim).unwrap(),
            None,
            "slim-matrix request must not consume a full entry"
        );
        assert!(read_cached(&dir, &full).unwrap().is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plan_fingerprint_is_candidate_order_insensitive() {
        let a = plan_fingerprint(&["Digit1".to_string(), "KeyQ".to_string()], "KeyQ", 60, 3.0);
        let b = plan_fingerprint(&["KeyQ".to_string(), "Digit1".to_string()], "KeyQ", 60, 3.0);
        assert_eq!(a, b, "candidate order must not change the fingerprint");
        // Every plan input independently changes it.
        assert_ne!(
            a,
            plan_fingerprint(&["Digit1".to_string()], "KeyQ", 60, 3.0),
            "candidate set"
        );
        assert_ne!(
            a,
            plan_fingerprint(
                &["Digit1".to_string(), "KeyQ".to_string()],
                "Escape",
                60,
                3.0
            ),
            "control key"
        );
        assert_ne!(
            a,
            plan_fingerprint(&["Digit1".to_string(), "KeyQ".to_string()], "KeyQ", 30, 3.0),
            "tps"
        );
        assert_ne!(
            a,
            plan_fingerprint(&["Digit1".to_string(), "KeyQ".to_string()], "KeyQ", 60, 5.0),
            "duration"
        );
        // Canonical shape: 16 lowercase hex chars.
        assert_eq!(a.len(), 16);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn slug_differs_across_plans_and_features() {
        // Both fingerprints embed in the FILENAME, so distinct plans
        // (and distinct feature builds) never thrash one entry name.
        let base = std::env::temp_dir();
        let a = key_with_plan(
            "abcdef0123456789",
            "0.6.0",
            "in-process",
            None,
            "1111111111111111",
        );
        let b = key_with_plan(
            "abcdef0123456789",
            "0.6.0",
            "in-process",
            None,
            "2222222222222222",
        );
        assert_ne!(
            entry_path(&base, &a).file_name().unwrap(),
            entry_path(&base, &b).file_name().unwrap()
        );
        let c = key_with_plan("abcdef0123456789", "0.6.0", "", None, "1111111111111111");
        assert_ne!(
            entry_path(&base, &a).file_name().unwrap(),
            entry_path(&base, &c).file_name().unwrap()
        );
    }

    #[test]
    fn file_hash_depends_on_contents() {
        let dir = temp_dir();
        std::fs::write(dir.join("a"), b"same length").unwrap();
        std::fs::write(dir.join("b"), b"dame length").unwrap();
        let ha = hash_file(&dir.join("a")).unwrap();
        let hb = hash_file(&dir.join("b")).unwrap();
        assert_ne!(ha, hb);
        // Stable across calls.
        assert_eq!(ha, hash_file(&dir.join("a")).unwrap());
        // Known vector: FNV-1a of empty input + length=0 folded in
        // (eight little-endian zero rounds — see hash_file).
        assert_eq!(hash_file_of_empty(), "a8c7f832281a39c5");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn hash_file_of_empty() -> String {
        let dir = temp_dir();
        std::fs::write(dir.join("empty"), b"").unwrap();
        let h = hash_file(&dir.join("empty")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        h
    }
}
