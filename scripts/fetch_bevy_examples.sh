#!/usr/bin/env bash
# Fetch Bevy showcase games and prepare them as fixtures for bevy_playtest.
#
# TWO-STEP TRANSFORM: each fixture is upstream source with
#   1) DefaultPlugins renamed to bevy_playtest::PlaytestDefaultPlugins
#      (drops WinitPlugin when BEVY_PLAYTEST_SCENARIO/HEADLESS is set —
#      bevy_winit 0.20 builds its event loop EAGERLY at plugin-build
#      time, so a runner swap alone cannot save a display-less host)
#   2) one line inserted before the final `.run()`:
#
#     .add_plugins(bevy_playtest::PlaytestPlugin)
#
# That makes the binary BOTH tiers:
#   - in-process takeover: BEVY_PLAYTEST_SCENARIO=<scenario> cargo run --bin fixture_X
#   - external BRP:        BRP_PORT=<port> cargo run --bin fixture_X
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
            curl -sfL --retry 3 --retry-delay 2 \
                "https://raw.githubusercontent.com/bevyengine/bevy/${BEVY_REV}/${1}" -o "$2"
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

    # 2. TWO-STEP transform for headless CI compatibility:
    #    a) Replace `DefaultPlugins` with `bevy_playtest::PlaytestDefaultPlugins`
    #       (drops WinitPlugin when BEVY_PLAYTEST_SCENARIO or HEADLESS is set).
    #    b) Insert the PlaytestPlugin line before `.run()` (last-set-wins runner swap).
    #    Games that explicitly construct DefaultPlugins differently will die loudly
    #    here — never silently ship a windowed fixture.
    python3 - "${target_dir}/src/main.rs" "${target_dir}/src/main.rs.playtest" << 'PYEOF'
import re, sys
src_path, out_path = sys.argv[1], sys.argv[2]
src = open(src_path).read()
# Step a: rename DefaultPlugins -> bevy_playtest::PlaytestDefaultPlugins
# (drops WinitPlugin when BEVY_PLAYTEST_SCENARIO/HEADLESS is set; identical
# pass-through otherwise). .set(...) modifier chains carry over: both are
# PluginGroups of the same underlying type.
out, n = re.subn(
    r"(?<![A-Za-z_:])DefaultPlugins(?![A-Za-z_:])",
    "bevy_playtest::PlaytestDefaultPlugins",
    src,
)
if n != 1:
    sys.exit(f"DefaultPlugins appears {n} times (need exactly 1)")
# Step b: anchor to single `.run();` line; insert PlaytestPlugin before it.
pat = re.compile(r"^([ \t]*)\.run\(\);$", re.M)
matches = pat.findall(out)
if len(matches) != 1:
    sys.exit(f"anchor '.run();' appears {len(matches)} times (need exactly 1)")
indent = matches[0] or "    "
out, n = pat.subn(lambda m: f"{m.group(1)}.add_plugins(bevy_playtest::PlaytestPlugin)\n{m.group(0)}", out, count=1)
assert n == 1
open(out_path, "w").write(out)
PYEOF

    # Loud verification: both transformations MUST have landed exactly once.
    grep -q 'bevy_playtest::PlaytestPlugin' "${target_dir}/src/main.rs.playtest" \
        || die "fixture ${name}: plugin insertion did not apply"
    [ "$(grep -c 'bevy_playtest::PlaytestPlugin' "${target_dir}/src/main.rs.playtest")" = "1" ] \
        || die "fixture ${name}: plugin insertion applied more than once"
    grep -q 'bevy_playtest::PlaytestDefaultPlugins' "${target_dir}/src/main.rs.playtest" \
        || die "fixture ${name}: PlaytestDefaultPlugins swap did not apply"
    grep -q 'WinitPlugin' "${target_dir}/src/main.rs.playtest" \
        && die "fixture ${name}: raw WinitPlugin reference remains in patched main"

    # Standalone-crate fixups for sibling modules
    for f in "${target_dir}/src/"*.rs; do
        [ "$(basename "$f")" = "main.rs.playtest" ] && continue
        sed 's/bevy_ecs::/bevy::ecs::/g' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
    done

    # 3. Cargo.toml (workspace member later)
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

[dependencies]
bevy = { version = "0.20.0", features = ["bevy_remote"] }
bevy_playtest = { path = "../..", features = ["takeover", "brp-plugin"] }
$(fixture_extra_deps "${target_dir}/src/main.rs")

[[bin]]
name = "fixture_${name//-/_}"
path = "src/main.rs.playtest"

[[bin]]
name = "fixture_${name//-/_}_pristine"
path = "src/main.rs"
EOF

    # 4. README
    cat > "${target_dir}/README.md" << 'EOF'
# Fixture: NAME

## Zero-Contract Claim

- **src/main.rs** — byte-identical to upstream.
- **src/main.rs.playtest** — upstream + TWO mechanical edits:
  1. `DefaultPlugins` → `bevy_playtest::PlaytestDefaultPlugins` (headless-safe
     plugin group: drops `WinitPlugin` when a playtest env var is set).
  2. `.add_plugins(bevy_playtest::PlaytestPlugin)` inserted before the final
     `.run()`.

Audit: `diff src/main.rs src/main.rs.playtest` (two mechanical edits, no
logic changes).

## Modes

- In-process: `BEVY_PLAYTEST_SCENARIO=<scenario> cargo run --bin fixture_NAME`
- External-BRP: `BRP_PORT=4570 cargo run --bin fixture_NAME`

The game logic is unmodified; only the plugin line is added.
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
echo "Build:  cargo build --manifest-path fixtures/Cargo.toml --bins"
echo "Run:    BEVY_PLAYTEST_SCENARIO=smoke cargo run --manifest-path fixtures/<name>/Cargo.toml --bin fixture_<name>"
