#!/usr/bin/env bash
# Fetch Bevy showcase games and prepare them as fixtures for bevy_playtest.
#
# FEATURE-GATED TRANSFORM (self-dev-dependency pattern): each fixture
# is upstream source with two cfg-gated mechanical edits:
#   1) `DefaultPlugins` rewritten to a call to `default_plugins()` —
#      a helper pair that returns bevy_playtest::PlaytestDefaultPlugins
#      under the `playtest` feature (drops WinitPlugin when a playtest
#      env var is set; bevy_winit 0.20 builds its event loop EAGERLY at
#      plugin-build time) and plain DefaultPlugins otherwise.
#   2) `.add_plugins(playtest_plugins())` inserted before the final
#      `.run()` — an empty tuple without the feature, the PlaytestPlugin
#      with it.
#
# Plus a Cargo.toml with an optional bevy_playtest dep, a `playtest`
# feature, and a SELF-dev-dependency that activates the feature only
# for test builds — so:
#   - plain `cargo build` / `--release`: ZERO bevy_playtest code
#   - plain `cargo test`: feature on, runs tests/playtest.rs (the
#     battery), the exact user-facing integration pattern
#
# No lib.rs, no build_app(), no wrapper module, no fixture-runner.
#
# Run from the bevy_playtest repo root.

set -euo pipefail

die() { echo "ERROR: $*" >&2; exit 1; }

# Immutable pin: fetch by commit SHA, not the v0.20.0 tag (tags are
# mutable refs and can be deleted/recreated; commit SHAs cannot).
BEVY_REV="b3bdd75becaffe9a16677cf5bae5bd6d198f052b" # = tag v0.20.0
FIXTURES_DIR="fixtures"
# Local Bevy checkout override — OPT-IN. The default is always the
# immutable pinned revision fetched from GitHub; a local checkout at a
# different rev would silently produce different fixtures than CI.
# Set BEVY_PLAYTEST_USE_LOCAL_BEVY=1 (and optionally LOCAL_BEVY=path).
USE_LOCAL_BEVY="${BEVY_PLAYTEST_USE_LOCAL_BEVY:-0}"
LOCAL_BEVY="${LOCAL_BEVY:-$HOME/src/github.com/bevyengine/bevy}"

echo "Fetching Bevy ${BEVY_REV} showcase games..."

# The fixture set. Enumerated explicitly (not globbed from a local
# checkout) so the script works identically on CI, where no Bevy repo
# exists. Keep in sync with .github/workflows/fixtures.yml matrix.
SHOWCASE_GAMES=(
    alien_cake_addict
    breakout
    contributors
    desk_toy
    game_menu
    loading_screen
    mines
)

# Fetch Bevy's dev-dependencies once (for CI fallback)
CACHE_TOML="${FIXTURES_DIR}/.bevy-cargo-toml"
if [ ! -f "$CACHE_TOML" ]; then
    mkdir -p "${FIXTURES_DIR}"
    curl -sfL --retry 3 --retry-delay 2 \
        "https://raw.githubusercontent.com/bevyengine/bevy/${BEVY_REV}/Cargo.toml" \
        -o "$CACHE_TOML"
fi

dev_dep_line() {
    sed -n '/^\[dev-dependencies\]/,/^\[/p' "$CACHE_TOML" \
        | grep -E "^${1} ?=" | head -1
}

fixture_extra_deps() {
    local src="$1"
    grep -oE '(^|[ ,(.])[a-z0-9_]+::' "$src" | sed 's/[ ,(.]//; s/::$//' \
        | grep -v -e '^std$' -e '^bevy$' -e '^core$' -e '^alloc$' \
                  -e '^pipelines_ready$' -e '^stepping$' -e '^super$' -e '^crate$' -e '^self$' \
        | sort -u \
        | while read -r crate; do
            line=$(dev_dep_line "$crate")
            [ -n "$line" ] && echo "$line"
        done
}

for name in "${SHOWCASE_GAMES[@]}"; do
    rel="examples/showcase/${name}.rs"
    target_dir="${FIXTURES_DIR}/${name}"

    echo "Processing: $name"
    mkdir -p "${target_dir}/src"

    # 1. Fetch pristine upstream source + sibling modules
    fetch_raw() { # $1 = repo-relative path, $2 = dest
        if [ "${USE_LOCAL_BEVY}" = "1" ] && [ -f "${LOCAL_BEVY}/${1}" ]; then
            cp "${LOCAL_BEVY}/${1}" "$2"
        else
            # Percent-encode the path: raw.githubusercontent.com rejects
            # unencoded spaces (e.g. "textures/Game Icons/right.png").
            url_path=$(python3 -c 'import urllib.parse, sys; print(urllib.parse.quote(sys.argv[1]))' "$1")
            curl -sfL --retry 3 --retry-delay 2 \
                "https://raw.githubusercontent.com/bevyengine/bevy/${BEVY_REV}/${url_path}" -o "$2"
        fi
    }
    fetch_raw "${rel}" "${target_dir}/src/main.rs"
    # Copy sibling module files referenced by `mod X;` declarations
    while read -r modname; do
        if fetch_raw "examples/showcase/${modname}.rs" "${target_dir}/src/${modname}.rs" 2>/dev/null; then
            echo "    (also copying ${modname}.rs)"
        else
            rm -f "${target_dir}/src/${modname}.rs"
        fi
    done < <(sed -n 's/^mod \([a-z_]*\);/\1/p' "${target_dir}/src/main.rs")
    echo "    (source: local checkout or github)"

    # 2. Feature-gated transform (self-dev-dependency pattern):
    #    a) `DefaultPlugins` -> `default_plugins()` call + cfg'd helper
    #       pair returning PlaytestDefaultPlugins (playtest) / plain
    #       DefaultPlugins (otherwise).
    #    b) `.add_plugins(playtest_plugins())` before `.run()` — cfg'd
    #       helper returning the PlaytestPlugin (playtest) / empty
    #       tuple (otherwise).
    #    Without the feature, plain `cargo build` compiles the ORIGINAL
    #    plugin graph unchanged; only `cargo test` (via the self-dev-dep)
    #    activates the feature. Games that construct DefaultPlugins
    #    differently will die loudly here — never silently ship a
    #    windowed fixture.
    python3 - "${target_dir}/src/main.rs" "${target_dir}/src/main.rs" << 'PYEOF'
import re, sys
path = sys.argv[1]
src = open(path).read()
# Step a: alias DefaultPlugins via a cfg'd helper fn (rustfmt-stable:
# no import alias pair fighting alphabetical sort). Keeps `.set(...)`
# modifier chains intact — the call yields the group, `.set()` on the
# returned group works identically.
helper = '''#[cfg(feature = "playtest")]
fn default_plugins() -> bevy_playtest::PlaytestDefaultPlugins {
    bevy_playtest::PlaytestDefaultPlugins
}
#[cfg(not(feature = "playtest"))]
fn default_plugins() -> bevy::DefaultPlugins {
    bevy::DefaultPlugins
}
'''
# Rewrite the single DefaultPlugins reference to default_plugins().
out, n = re.subn(
    r"(?<![A-Za-z_:.])DefaultPlugins(?!\()",
    "default_plugins()",
    src,
)
if n != 1:
    sys.exit(f"DefaultPlugins appears {n} times (need exactly 1)")
# Place the helper just before `fn main`.
if "fn default_plugins()" in out:
    sys.exit("default_plugins helper already present")
out = out.replace("fn main()", helper + "\nfn main()", 1)
# Step b: anchor to single `.run();` line; insert cfg'd PlaytestPlugin
# before it. `#[cfg]` cannot be attached to an expression inside a
# builder chain, so gate the *name*: `playtest_plugins()` resolves to
# a tuple of plugins, empty without the feature.
helper = '''#[cfg(feature = "playtest")]
fn playtest_plugins() -> (bevy_playtest::PlaytestPlugin,) {
    (bevy_playtest::PlaytestPlugin,)
}
#[cfg(not(feature = "playtest"))]
fn playtest_plugins() -> () {}
'''
# Place the helper just before `fn main`.
if "fn playtest_plugins()" in out:
    sys.exit("playtest_plugins helper already present")
out = out.replace("fn main()", helper + "\nfn main()", 1)
pat = re.compile(r"^([ \t]*)\.run\(\);$", re.M)
matches = pat.findall(out)
if len(matches) != 1:
    sys.exit(f"anchor '.run();' appears {len(matches)} times (need exactly 1)")
indent = matches[0] or "    "
plugin_line = f"{indent}.add_plugins(playtest_plugins())\n"
out, n = pat.subn(lambda m: plugin_line + m.group(0), out, count=1)
assert n == 1
open(path, "w").write(out)
PYEOF

    # Loud verification: both transformations MUST have landed exactly once.
    [ "$(grep -c 'add_plugins(playtest_plugins())' "${target_dir}/src/main.rs")" = "1" ] \
        || die "fixture ${name}: plugin insertion did not apply exactly once"
    [ "$(grep -c 'fn playtest_plugins()' "${target_dir}/src/main.rs")" = "2" ] \
        || die "fixture ${name}: playtest_plugins helper cfg pair incomplete (want 2 definitions)"
    grep -q 'bevy_playtest::PlaytestDefaultPlugins' "${target_dir}/src/main.rs" \
        || die "fixture ${name}: PlaytestDefaultPlugins swap did not apply"
    grep -q 'WinitPlugin' "${target_dir}/src/main.rs" \
        && die "fixture ${name}: raw WinitPlugin reference remains in patched main"

    # 2b. POINTER-TIER transform (game_menu only): makes the main-menu
    #     Play button a synthetic_pointer target. Three auditable edits:
    #     i)  Insert `Name::new("menu_play_button")` into the Play
    #         button's spawn bundle (upstream buttons are unnamed).
    #     ii) Append a self-contained audit module: a world-level
    #         Activate observer that logs `menu play button activated`
    #         (the scenario's `observes` target) when the named button
    #         fires.
    #     iii) Wire the observer with `.add_observer(...)` beside the
    #         PlaytestPlugin insertion.
    if [ "${name}" = "game_menu" ]; then
        python3 - "${target_dir}/src/main.rs" << 'GAME_MENU_PYEOF'
import sys
path = sys.argv[1]
src = open(path).read()

# (i) Name two buttons: the Play button (instrumented below — the
# pointer tier's positive target) and the Settings button (left
# uninstrumented: it works — transitions to the settings screen — but
# emits no log, exercising the coverage-miss failure mode with ZERO
# fault injection; upstream behavior is healthy, just silent).
play_anchor = "                        MenuButtonAction::Play,\n"
play_repl = play_anchor + "                        Name::new(\"menu_play_button\"),\n"
if src.count(play_anchor) != 1:
    sys.exit(f"game_menu: Play-button anchor appears {src.count(play_anchor)} times (need exactly 1)")
src = src.replace(play_anchor, play_repl, 1)
settings_anchor = "                        MenuButtonAction::Settings,\n"
settings_repl = settings_anchor + "                        Name::new(\"menu_settings_button\"),\n"
if src.count(settings_anchor) != 1:
    sys.exit(f"game_menu: Settings-button anchor appears {src.count(settings_anchor)} times (need exactly 1)")
src = src.replace(settings_anchor, settings_repl, 1)

# (iii) Wire the observer beside the playtest plugin insertion.
# Compiles in both cfgs (plain builds keep upstream behavior: the
# handler early-returns via compile-time cfg!), so no cfg gymnastics
# are needed on the wiring site itself.
plug_anchor = ".add_plugins(playtest_plugins())\n"
wire = plug_anchor + "        .add_observer(bevy_playtest_pointer_audit::on_button_activate)\n"
if src.count(plug_anchor) != 1:
    sys.exit(f"game_menu: PlaytestPlugin anchor appears {src.count(plug_anchor)} times (need exactly 1)")
src = src.replace(plug_anchor, wire, 1)

# (ii) Append the audit module (activation logging).
audit = '''
// --- bevy_playtest pointer-tier addition (appended, not modifying upstream) ---
/// World-level observer for the pointer acceptance tier: logs when the
/// named Play button activates, so a synthetic_pointer scenario can
/// `observe` the click landing through the real picking chain. The
/// sibling Settings button (also named) intentionally has NO observer:
/// clicking it is healthy upstream behavior that emits nothing — the
/// natural negative case for observation coverage.
mod bevy_playtest_pointer_audit {
    use bevy::prelude::*;
    use bevy::ui_widgets::Activate;

    pub fn on_button_activate(event: On<Activate>, named: Query<&Name>) {
        #[cfg(feature = "playtest")]
        if let Ok(name) = named.get(event.entity) {
            if name.as_str() == "menu_play_button" {
                info!("menu play button activated");
            }
        }
        let _ = (&event, &named);
    }
}
'''
src += audit
open(path, "w").write(src)
GAME_MENU_PYEOF
        echo "    (pointer-tier edits applied: menu_play_button + audit observer)"
    fi

    # Standalone-crate fixups for sibling modules
    for f in "${target_dir}/src/"*.rs; do
        sed 's/bevy_ecs::/bevy::ecs::/g' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
    done

    # 2b. Collect referenced assets (generic scan): every string the
    #     game loads at runtime must exist under the fixture's assets/
    #     dir or bevy_asset logs ERROR Path-not-found in CI. Scan:
    #     - direct literals in `.load("...")`
    #     - `&str` consts referenced by `.load(CONST)`
    #     - `.from_asset("...")` for self-contained .glb files.
    #     .gltf is SKIPPED on purpose: it references sibling .bin /
    #     texture files, so a single-file fetch would still be broken;
    #     the battery tolerates the missing handle (asset loads fail
    #     asynchronously, they do not crash the app).
    python3 - "${target_dir}/src" > "${target_dir}/.assets.list" << 'PYEOF'
import re, sys, pathlib

consts = {}
loads = set()
for f in pathlib.Path(sys.argv[1]).glob("*.rs"):
    src = f.read_text()
    # &str consts usable as .load(CONST)
    for name, val in re.findall(r'const\s+(\w+)\s*:\s*&str\s*=\s*"([^"]+)"', src):
        consts[name] = val
    # .load(<arg>) up to the matching close paren on the same call
    for m in re.finditer(r'\.load\(([^)]*)\)', src):
        arg = m.group(1).strip()
        lit = re.match(r'^"([^"]+)"$', arg)
        if lit:
            loads.add(lit.group(1))
        elif arg in consts:
            loads.add(consts[arg])
    # self-contained glb scenes from GltfAssetLabel::...().from_asset("...")
    for m in re.finditer(r'from_asset\("([^"]+\.glb)"\)', src):
        loads.add(m.group(1))

for p in sorted(loads):
    print(p)
PYEOF

    # Fetch each referenced asset from the Bevy repo's assets/ root.
    while read -r asset; do
        [ -n "$asset" ] || continue
        dest="${target_dir}/assets/${asset}"
        mkdir -p "$(dirname "$dest")"
        if fetch_raw "assets/${asset}" "$dest"; then
            echo "    (asset: ${asset})"
        else
            echo "    WARNING: asset ${asset} not found at pinned Bevy rev; skipping"
            rm -f "$dest"
        fi
    done < "${target_dir}/.assets.list"
    rm -f "${target_dir}/.assets.list"

    # 3. Cargo.toml (workspace member later) — feature-gated
    #    self-dev-dependency pattern: plain `cargo build` compiles the
    #    game WITHOUT bevy_playtest; plain `cargo test` activates the
    #    `playtest` feature (via the self-dev-dep) and runs the
    #    battery from tests/playtest.rs.
    cat > "${target_dir}/Cargo.toml" << EOF
# Auto-generated by scripts/fetch_bevy_examples.sh — do not edit by hand.
[package]
name = "fixture_${name//-/_}"
version = "0.1.0"
edition = "2024"
publish = false

[lints.rust]
# Upstream showcase code pokes these cfgs; expected, not an error.
unexpected_cfgs = { level = "allow" }

[features]
playtest = [
    "dep:bevy_playtest",
    "bevy_playtest/takeover",
    "bevy_playtest/brp-plugin",
    "bevy_playtest/takeover-winit",
    "bevy_playtest/pointer-input",
]

[dependencies]
bevy = { version = "0.20.0", features = ["bevy_remote"] }
bevy_playtest = { path = "../..", optional = true }
$(fixture_extra_deps "${target_dir}/src/main.rs")

[dev-dependencies]
# Self-dependency: activates OUR OWN \`playtest\` feature only for
# test/example/bench builds. Normal & release builds stay clean.
fixture_${name//-/_} = { path = ".", features = ["playtest"] }
EOF

    # 4. tests/playtest.rs — the per-game playtest assertions. This
    #    file is COMMITTED (hand-editable): regeneration seeds it only
    #    if missing, never overwrites, so per-game expectations (known
    #    findings, TODOs) survive upstream refreshes. Scenario
    #    selection lives HERE (Rust), not in env vars: the battery
    #    helper forwards BEVY_PLAYTEST_SCENARIO to the spawned game
    #    binary and isolates it from leaked shell vars.
    mkdir -p "${target_dir}/tests"
    if [[ ! -e "${target_dir}/tests/playtest.rs" ]]; then
    cat > "${target_dir}/tests/playtest.rs" << 'EOF'
//! Seeded by scripts/fetch_bevy_examples.sh — COMMITTED, edit freely.
//! Plain `cargo test` runs the default playtest battery against the
//! game binary (compiled with the playtest feature via the
//! self-dev-dependency). Per-game assertions belong here.
#[cfg(feature = "playtest")]
#[test]
fn playtest_battery() {
    bevy_playtest::battery::run_and_expect_clean(
        env!("CARGO_BIN_EXE_fixture_NAME"),
        bevy_playtest::battery::BUILTIN_BATTERY,
    );
}

// `cargo package` strips `[dev-dependencies]`, so the self-dev-dependency
// that auto-activates `playtest` is gone in an unpacked archive. Rather
// than fail to compile (missing `bevy_playtest`) or silently run zero
// playtests, surface it as an ignored test pointing at the fix.
#[cfg(not(feature = "playtest"))]
#[test]
#[ignore = "playtest feature inactive: self-dev-dependency stripped (e.g. cargo package). Run `cargo test --features playtest`."]
fn playtest_battery() {}
EOF
    # Substitute the fixture name portably (GNU sed treats `sed -i ''`
    # as a filename; python3 is available on macOS and Linux runners).
    python3 - "$name" "${target_dir}/tests/playtest.rs" << 'PYEOF'
import sys
name, path = sys.argv[1], sys.argv[2]
with open(path) as f:
    s = f.read()
with open(path, "w") as f:
    f.write(s.replace("fixture_NAME", "fixture_" + name.replace("-", "_")))
PYEOF
    fi

    # 5. README
    cat > "${target_dir}/README.md" << 'EOF'
# Fixture: NAME

## Zero-Contract Claim

- **src/main.rs** — upstream + TWO mechanical edits (both feature-gated
  via cfg'd helper pairs):
  1. `DefaultPlugins` rewritten to `default_plugins()` — returns
     `bevy_playtest::PlaytestDefaultPlugins` under the feature
     (headless-safe: drops `WinitPlugin` when a playtest env var is
     set), plain `bevy::DefaultPlugins` otherwise.
  2. `.add_plugins(playtest_plugins())` inserted before the final
     `.run()` — the PlaytestPlugin under the feature, an empty tuple
     otherwise.
- **tests/playtest.rs** — one generated test running the battery.
- **Cargo.toml** — optional `bevy_playtest` dep + `playtest` feature +
  self-dev-dependency (activates the feature only for test builds).

Plain `cargo build` / `cargo build --release` compile the game WITHOUT
any bevy_playtest code (the feature is off; the optional dep is not
even linked). Plain `cargo test` activates the feature via the
self-dev-dep and runs the default battery.

## Modes

- Battery (zero-config): `cargo test`
- Authored scenario: `bevy_playtest::battery::run_and_expect_clean(
  binary, "playtests/my_scenario.json")` in `tests/playtest.rs`
- Debugging only (live logs, quick re-runs): `BEVY_PLAYTEST_SCENARIO=
  <scenario> cargo run --features playtest` — the default build is
  playtest-free
- External-BRP: `BRP_PORT=4570 cargo run --features playtest`
EOF
    sed "s/NAME/${name}/g" "${target_dir}/README.md" > "${target_dir}/README.md.tmp"
    mv "${target_dir}/README.md.tmp" "${target_dir}/README.md"

    echo "  Created: $target_dir/"
done

# 5. Create fixtures workspace
cat > "${FIXTURES_DIR}/Cargo.toml" << 'WORKSPACE'
[workspace]
resolver = "2"
members = [
    "alien_cake_addict",
    "breakout",
    "contributors",
    "desk_toy",
    "game_menu",
    "loading_screen",
    "mines",
]

[workspace.dependencies]
bevy = { version = "0.20.0", features = ["bevy_remote"] }

# Standard Bevy CI practice: no debug info for dev/profile builds
# cuts linker memory and build time dramatically.
[profile.dev]
debug = 0

[profile.release]
debug = 0
WORKSPACE

echo "Done. Fixtures generated under ${FIXTURES_DIR}/."
echo "Test:  cargo test --manifest-path fixtures/<name>/Cargo.toml"
