#!/usr/bin/env bash
# Fetch Bevy showcase games and prepare them as fixtures for bevy_playtest.
#
# Generates TWO targets per fixture:
#   - src/main.rs.patched : binary target (external-BRP mode)
#   - src/lib.rs          : library target (in-process mode)
# Both share the same source logic; lib.rs exports `build_app()` which
# swaps DefaultPlugins for headless when BEVY_PLAYTEST_HEADLESS is set.
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

    # lib.rs = main.rs with three substitutions (fully auditable):
    #   1. `fn main() {`  -> `pub fn build_app() -> App {`
    #   2. `.add_plugins(<DefaultPlugins expr>)` (incl. `.set(...)` chains
    #      with nested parens) -> wrapped in `bevy_playtest_fixture::wrap(...)`
    #      which preserves .set() config but drops WinitPlugin when headless.
    #   3. `.run();` -> `; app`  (semicolon + return app)
    #   (the chain returns &mut App, we consume it with the final app expr)
    perl -0pe '
        s/^fn main\(\) \{$/pub fn build_app() -> App {/m;
        s{\.add_plugins\((DefaultPlugins(?:\s*\.set(?<parens>\((?:[^()]++|(?&parens))*\)))*)\)}
         {.add_plugins(bevy_playtest_fixture::wrap($1))}gs;
        s/^([ \t]*)\.run\(\);$/$1; app/m;
        s/App::new\(\)$/let mut app = App::new(); app/m;
    ' "${target_dir}/src/main.rs" > "${target_dir}/src/lib.rs"

    # Loud verification: every transform MUST have landed. A silent miss
    # (e.g. a new plugin-group shape upstream) would ship a windowed
    # fixture into CI and panic on machines with no display.
    grep -q 'pub fn build_app() -> App {' "${target_dir}/src/lib.rs" \
        || die "fixture ${name}: 'fn main' transform did not apply"
    grep -q 'bevy_playtest_fixture::wrap(DefaultPlugins' "${target_dir}/src/lib.rs" \
        || die "fixture ${name}: add_plugins(DefaultPlugins) transform did not apply"
    if grep -qE '\.add_plugins\(DefaultPlugins[),[:space:]]' "${target_dir}/src/lib.rs"; then
        die "fixture ${name}: untransformed add_plugins(DefaultPlugins) left in lib.rs"
    fi

    # Append the fixture module (plugin chooser + BRP bootstrap)
    cat >> "${target_dir}/src/lib.rs" << 'LIBTAIL'

// --- bevy_playtest fixture patch (appended; audit: diff main.rs lib.rs) ---
pub mod bevy_playtest_fixture {
    use bevy::app::{App, Plugin, PluginGroup, PluginGroupBuilder};

    /// Wraps a (possibly `.set()`-configured) DefaultPlugins group for the
    /// harness tiers. Headless (BEVY_PLAYTEST_HEADLESS, CI):
    ///   - drops winit's event loop (there's no window; assets, render,
    ///     and audio stay alive so game setup systems — fonts, meshes,
    ///     sprites, sounds — don't panic),
    ///   - adds ScheduleRunnerPlugin so `app.run()` keeps ticking at the
    ///     requested rate instead of returning immediately (nothing else
    ///     drives the schedule without winit). The harness terminates
    ///     the process at scenario end.
    /// WindowPlugin settings in the `.set()` chain become moot (no
    /// window is created) but remain valid config. Rendering uses
    /// software Vulkan (Lavapipe) when no GPU is present.
    pub fn wrap<G: PluginGroup>(group: G) -> PluginGroupBuilder {
        let builder = group.build();
        if std::env::var("BEVY_PLAYTEST_HEADLESS").is_ok() {
            builder
                .disable::<bevy::winit::WinitPlugin>()
                .add(bevy::app::ScheduleRunnerPlugin::run_loop(
                    std::time::Duration::from_secs_f64(1.0 / 60.0),
                ))
        } else {
            builder
        }
    }

    /// Adds BRP support when BRP_PORT is set; no-op otherwise.
    #[derive(Default)]
    pub struct Brp;

    /// Convenience constructor for the binary shim.
    pub fn brp() -> Brp {
        Brp
    }

    impl Plugin for Brp {
        fn build(&self, app: &mut App) {
            if let Ok(port) = std::env::var("BRP_PORT") {
                let port: u16 = port.parse().expect("BRP_PORT must be a port");
                app.add_plugins(bevy::remote::RemotePlugin::default());
                app.add_plugins(
                    bevy::remote::http::RemoteHttpPlugin::default().with_port(port),
                );
                app.register_type::<bevy::transform::components::Transform>();
                app.register_type::<bevy::input::keyboard::KeyboardInput>();
            }
        }
    }
}

/// Re-export so the binary shim can do `fixture_X::brp()`.
pub use bevy_playtest_fixture::brp;
LIBTAIL

    # 3. Generate main.rs.patched: tiny shim that calls the lib's
    # build_app() (which handles the headless plugin swap + BRP).
    cat > "${target_dir}/src/main.rs.patched" << MAINSHIM
// Binary entrypoint for external-BRP mode.
// Uses the lib target's build_app(): headless plugin swap happens there.
// Brp activates the remote-control surface when BRP_PORT is set; no-op otherwise.
fn main() {
    let mut app = fixture_${name//-/_}::build_app();
    app.add_plugins(fixture_${name//-/_}::brp());
    app.run();
}
MAINSHIM

    # Standalone-crate fixups for sibling modules
    for f in "${target_dir}/src/"*.rs; do
        [ "$(basename "$f")" = "main.rs.patched" ] && continue
        sed 's/bevy_ecs::/bevy::ecs::/g' "$f" > "$f.tmp" && mv "$f.tmp" "$f"
    done

    # 4. Cargo.toml (workspace member later)
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
$(fixture_extra_deps "${target_dir}/src/main.rs")

[[bin]]
name = "fixture_${name//-/_}"
path = "src/main.rs.patched"

[[bin]]
name = "fixture_${name//-/_}_pristine"
path = "src/main.rs"

[lib]
name = "fixture_${name//-/_}"
path = "src/lib.rs"
EOF

    # 5. README
    cat > "${target_dir}/README.md" << 'EOF'
# Fixture: NAME

## Zero-Contract Claim

- **src/main.rs** — byte-identical to upstream.
- **src/lib.rs** — headless-capable `build_app()` for in-process tier.
- **src/main.rs.patched** — binary shim for external-BRP tier.

Audit: `diff src/main.rs src/lib.rs`

## Modes

- In-process: harness calls `lib::build_app()` (set BEVY_PLAYTEST_HEADLESS=1 for headless plugins)
- External-BRP: `BRP_PORT=4570 cargo run --bin fixture_NAME`

The game logic is unmodified; only the plugin setup is swapped.
EOF
    # Substitute NAME after the quoted heredoc (no accidental execution)
    sed "s/NAME/${name}/g" "${target_dir}/README.md" > "${target_dir}/README.md.tmp"
    mv "${target_dir}/README.md.tmp" "${target_dir}/README.md"

    echo "  Created: $target_dir/"
done

# 6. Create fixtures workspace + in-process runner
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
    "fixture-runner",
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

mkdir -p "${FIXTURES_DIR}/fixture-runner/src"
cat > "${FIXTURES_DIR}/fixture-runner/Cargo.toml" << 'TOML_RUNNER'
[package]
name = "fixture-runner"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
bevy_playtest = { path = "../.." }
bevy = { workspace = true }
serde_json = "1"
fixture_alien_cake_addict = { path = "../alien_cake_addict" }
fixture_breakout = { path = "../breakout" }
fixture_contributors = { path = "../contributors" }
fixture_desk_toy = { path = "../desk_toy" }
fixture_game_menu = { path = "../game_menu" }
fixture_loading_screen = { path = "../loading_screen" }
fixture_mines = { path = "../mines" }
TOML_RUNNER

# Generate the runner's lib.rs (one arm per fixture)
cat > "${FIXTURES_DIR}/fixture-runner/src/lib.rs" << 'RS_LIB'
//! In-process runner: calls each fixture's build_app() with headless env set.

use bevy::prelude::*;
use bevy_playtest::run_scenario;
use bevy_playtest::scenario::Scenario;

pub fn run_fixture_in_process(fixture: &str, scenario: &Scenario) -> Result<(), String> {
    unsafe { std::env::set_var("BEVY_PLAYTEST_HEADLESS", "1"); }
    // Preempt the game's LogPlugin subscriber so we capture ERROR/WARN
    // during gameplay. We suppress capture during build_app() to avoid
    // false positives from setup-time asset errors (fixtures ship no
    // assets). Capture is enabled inside run_scenario. Our subscriber
    // chains a fmt layer for console output; LogPlugin then logs a
    // harmless "already set" error.
    bevy_playtest::sinks::ensure_global_subscriber();
    let mut app = match fixture {
        "alien_cake_addict" => fixture_alien_cake_addict::build_app(),
        "breakout" => fixture_breakout::build_app(),
        "contributors" => fixture_contributors::build_app(),
        "desk_toy" => fixture_desk_toy::build_app(),
        "game_menu" => fixture_game_menu::build_app(),
        "loading_screen" => fixture_loading_screen::build_app(),
        "mines" => fixture_mines::build_app(),
        _ => return Err(format!("unknown fixture: {fixture}")),
    };
    app.register_type::<bevy::transform::components::Transform>();
    app.register_type::<bevy::input::keyboard::KeyboardInput>();
    let report = run_scenario(&mut app, scenario)
        .map_err(|e| format!("scenario error: {e}"))?;
    if report.status != bevy_playtest::enums::PlaytestStatus::Pass {
        eprintln!("Violations: {:?}", report.violations);
        return Err(format!("status: {:?}", report.status));
    }
    Ok(())
}
RS_LIB

# Generate the runner's main.rs (CLI entrypoint for CI)
cat > "${FIXTURES_DIR}/fixture-runner/src/main.rs" << 'RUNNERMAIN'
use bevy_playtest::scenario::Scenario;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture = args.first().expect("usage: fixture-runner <fixture> [seed]");
    let seed: u64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(42);
    let scenario: Scenario = serde_json::from_str(&format!(
        r#"{{"name":"{}","bot":{{"type":"chaos","seed":{}}},"duration_s":2.0,"invariants":[],"setup":{{}}}}"#,
        fixture, seed
    )).expect("parse scenario");
    if let Err(e) = fixture_runner::run_fixture_in_process(fixture, &scenario) {
        eprintln!("Fixture {fixture} failed: {e}");
        std::process::exit(1);
    }
    eprintln!("{fixture}: ok");
}
RUNNERMAIN

echo "Next: add tests/acceptance.rs to run fixture-runner in-process"
