# Changelog

## [Unreleased]

## [0.17.1](https://github.com/mcanevet/bevy_playtest/compare/v0.17.0...v0.17.1) - 2026-11-24

### Added

- *(judgement)* add the demon-stack litmus fixture

## [0.17.0](https://github.com/mcanevet/bevy_playtest/compare/v0.16.0...v0.17.0) - 2026-11-23

### Added

- *(judgement)* evaluate pairwise_separation at run time
- *(judgement)* add the pairwise_separation authoring grammar
- *(judgement)* add the shared episode-fold engine

### Fixed

- *(progress)* name the ECS role attributes in remediation texts

### Other

- crate-root the episode intra-doc links
- record the dogfooding roadmap vote for the judgement layer
- add the entity-scoped judgement-layer proposal
- disclose the zero-config battery's counts-only progress surface
- *(readme)* add a Bazel runfiles recipe next to the cargo one
- *(readme)* make the quick-start snippet clippy-clean

## [0.16.0](https://github.com/mcanevet/bevy_playtest/compare/v0.15.0...v0.16.0) - 2026-11-23

### Added

- *(lint)* probe progress-surface names in lint stage 2
- *(lint)* stage-2 type-registry probe of the booted App

### Fixed

- *(invariants)* suggest containers for generic-arg typos; rank own-segment matches first
- *(invariants)* suggest candidates for full-path typos too
- *(invariants)* refine driver rate-consistency keying
- *(lint)* tighten the probe gate; disclose it in the docs
- *(lint)* give crash reports an `error` narrative; pin the crash path e2e
- *(lint)* print findings to stderr once, after both stages
- *(invariants)* window-unit review fixes — rate-key aliasing, frozen-driver wording, boundary pins
- *(invariants)* parameterize resolve_registration nouns, upgrade did-you-mean
- *(lint)* contain warm-up panics, gate the probe, share oracle message builders
- *(invariants)* judge stagnant_resource windows in the driver's unit

### Other

- note the one-unit shortfall of exact budget windows

## [0.15.0](https://github.com/mcanevet/bevy_playtest/compare/v0.14.0...v0.15.0) - 2026-11-22

### Added

- *(scenario)* allow observe-only (passive) scenarios
- *(dx)* BEVY_PLAYTEST_LINT aggregates scenario rejections
- *(battery)* BatteryConfig::new constructor for the common path
- *(report)* trace passing invariants (held_invariants section)

### Fixed

- *(held)* scrub remaining internal spec references
- *(report)* recurrence annotation says recurred-across-frames, not present-every-frame
- *(lint)* describe lint mode honestly
- *(held)* describe the anticipated window rework in user-facing terms
- *(report)* drop deny_unknown_fields from HeldInvariant
- *(docs)* resolve intra-doc link errors caught by CI doc build
- *(invariants)* list similar registered types on unknown-name resolution
- *(invariants)* report oracle_error config failures once per error class

### Other

- *(it)* scrub the last internal spec reference
- *(it)* scrub stale spec reference from lint-mode module doc
- *(reference)* document lint-mode reports
- *(reference)* align Opaque leaves quote with the resource/component noun message
- *(changelog)* leave Unreleased entries to release-plz
- scrub internal working names from comments and test fixtures
- *(reference)* note the rendered operator spelling in report details
- *(it)* lint doc JSON examples against the current grammar
- *(it)* cover the documented engine clock driver rows
- *(reference)* document path and driver authoring for resource invariants

## [0.14.0](https://github.com/mcanevet/bevy_playtest/compare/v0.13.0...v0.14.0) - 2026-11-22

### Added

- *(invariants)* resource_check and stagnant_resource rules

### Other

- ci Update taiki-e/install-action action to v2.90.8
- align documentation with the post-removal code
- ci Update taiki-e/install-action action to v2.90.7

## [0.13.0](https://github.com/mcanevet/bevy_playtest/compare/v0.12.0...v0.13.0) - 2026-11-21

### Added

- *(explorer)* phase 2: prior keys, combos, drags, learned click targets
- *(sweep)* [**breaking**] seed sweep with fingerprint regression gate
- *(eval)* re-measure the baseline after the cursor fix and deterministic RNG
- *(explorer)* explore mode for the chaos bot, on by default in the battery
- *(battery)* deterministic OS randomness per seed (Linux, LD_PRELOAD)
- *(eval)* CI-measured baseline; document determinism findings
- *(eval)* commit the baseline; mark passive targets; measure M2
- *(takeover)* mute game audio during playtest runs
- *(eval)* state trace, exploration targets, and evaluator (phase 0)
- *(discovery)* add informational screen_liveness pHash oracle

### Fixed

- *(ui-surface)* Pickable::IGNORE nodes are not click targets
- *(fixtures)* pin thetawave with the pause menu's sound stand-in
- *(oracles)* default bounds oracle flags runaways, not far geometry
- *(driver)* bot cursor drives bevy_ui Interaction

### Other

- seed sweep findings after the rebase
- evaluation triggers, quick-run budget, baseline provenance
- *(eval)* quick evaluation on relevant PRs, full on demand, no schedule
- drop disk cleanup, pin the toolchain deterministically, pin nextest
- [**breaking**] remove probe-based input-surface discovery (BEVY_PLAYTEST_DISCOVER) ([#107](https://github.com/mcanevet/bevy_playtest/pull/107))
- cut CI from ~42 min — parallel jobs, rust-cache, nextest partitions, single test binary ([#105](https://github.com/mcanevet/bevy_playtest/pull/105))
- ci Update Rust crate wgpu-types to 30.0.2

## [0.12.0](https://github.com/mcanevet/bevy_playtest/compare/v0.11.0...v0.12.0) - 2026-11-18

### Added

- *(oracles)* add informational render-liveness structural rule
- *(battery)* run_and_expect_scoped_findings — scoped, double-sided expected findings
- *(driver)* multi-slot concurrent chaos keyboard with drawn hold durations
- *(liveness)* progress-heartbeat, bounds-selectors, phantom-write oracles
- report log_bridge_available to expose compound log-capture loss
- *(discovery)* focus-walker probe
- *(discovery)* periodic ui-surface re-enumeration

### Fixed

- make battery temp dirs unique across same-tick sibling runs
- detect persistent log-max-level tampering, correct defense docs
- defend log-crate capture against a foreign set_max_level(Off)
- *(oracles)* refine frozen_world detail with digest stepping evidence
- *(discovery)* address adversarial review of periodic ui-surface

### Other

- *(battery)* fix rustdoc splice, deterministic same-tick regression test
- *(discovery)* clarify is_first guard intent in ui-surface observe

## [0.11.0](https://github.com/mcanevet/bevy_playtest/compare/v0.10.0...v0.11.0) - 2026-11-17

### Added

- *(discovery)* demote the WASD/arrows backbone to a fallback tier

### Fixed

- *(discovery)* address adversarial review of the fallback-tier change
- *(log-capture)* bridge the log crate to tracing in takeover mode

### Other

- *(discovery)* pin the backbone-equal scan corner from round-2 review
- ci Update Rust crate log to 0.4.35

## [0.10.0](https://github.com/mcanevet/bevy_playtest/compare/v0.9.0...v0.10.0) - 2026-11-16

### Added

- add ui-surface feature enumerating the declared UI interactable surface
- *(invariants)* component_check authored-intent oracle (#round 1-3 review-hardened)
- *(discovery)* union source-scan extras into the injected surface
- *(discovery)* source-derived chaos surface layering

### Fixed

- *(discovery)* make lw-reader-only test builds compile again
- *(discovery)* close the two gaps from the follow-up review
- *(discovery)* address adversarial review of the inject-union

### Other

- drop unused bevy_sprite dev-dependency
- *(fixtures)* split loading_screen battery into zero-config and injected tests

## [0.9.0](https://github.com/mcanevet/bevy_playtest/compare/v0.8.0...v0.9.0) - 2026-11-14

### Added

- *(takeover)* mouse-motion parity and probing for input-surface discovery
- *(discovery)* mouse-grid probing for input-surface discovery

### Other

- ci Update Rust to v1.100.0
- ci Update dependency rust to 1.100
- single source of truth for toolchain version

## [0.8.0](https://github.com/mcanevet/bevy_playtest/compare/v0.7.0...v0.8.0) - 2026-11-12

### Added

- *(discovery)* load-time capability census and structural contract suggestions
- *(fixtures)* port thetawave to bevy 0.20 as real-world Leafwing fixture
- *(discovery)* Leafwing reader, intent-tap, and declared-vs-measured reconciliation
- *(discovery)* tier-0 discover sweep, draft artifact, and measured surface
- *(fixtures)* known-answer planted-truth fixture and measurement harness
- *(discovery)* SurfaceEntry IR, provenance grades, and finding vocabulary

### Fixed

- [**breaking**] isolate battery child cwd/HOME and name GPU-adapter panics
- *(fixtures)* refresh Cargo.lock for ron 0.12.3 bump
- *(discovery)* isolate probe-pool child cwd and HOME

### Other

- add headless-CI GPU how-to
- *(src)* repair doc-debt trio
- bump stale bevy_playtest version pins 0.4 -> 0.8 in quick-start snippets

## [0.7.0](https://github.com/mcanevet/bevy_playtest/compare/v0.6.0...v0.7.0) - 2026-11-06

### Added

- *(discovery)* feed the discovered surface into the battery chaos bot
- *(discovery)* [**breaking**] orchestrating flow, battery opt-in, and docs
- *(discovery)* [**breaking**] probe scenarios, subprocess pool, and surface cache
- *(discovery)* [**breaking**] static device-reader scan
- *(discovery)* [**breaking**] surface types and wire layer
- add broadened per-frame state digest module

### Other

- ci Update release-plz/action action to v0.5.148
- pin the no-default-features in-process compile check

## [0.6.0](https://github.com/mcanevet/bevy_playtest/compare/v0.5.0...v0.6.0) - 2026-11-01

### Added

- emit wrapped KeyboardInput events with populated logical_key/text
- teach frozen_world oracle about UI activity (ui-liveness)
- [**breaking**] remove external-BRP mode
- emit wrapped WindowEvent messages for pointer-driven games

### Fixed

- deliver just_pressed under DefaultPlugins takeover
- *(ci)* derive Bevy fixture rev from Cargo.lock

### Other

- refresh fixtures Cargo.lock to Bevy 0.20.1
- key target cache on fetch-script hash

## [0.5.0](https://github.com/mcanevet/bevy_playtest/compare/v0.4.0...v0.5.0) - 2026-10-31

### Other

- fix review findings on battery-first restructure
- lead with the zero-config battery in README and tutorial
- ci Update rust dependencies to 0.20.1
- expand runner disk-space cleanup
- [**breaking**] remove reset/cheat hooks, observes, and strict_coverage ([#44](https://github.com/mcanevet/bevy_playtest/pull/44))
- ci Update release-plz/action action to v0.5.147

## [0.4.0](https://github.com/mcanevet/bevy_playtest/compare/v0.3.0...v0.4.0) - 2026-10-26

### Added

- chaos mouse bot + scripted mouse_clicks

### Other

- ci Update release-plz/action action to v0.5.146
- restructure docs/ into diátaxis framework

## [0.3.0](https://github.com/mcanevet/bevy_playtest/compare/v0.2.0...v0.3.0) - 2026-10-23

### Added

- close the log_error oracle gap — cross-thread capture with honest loss reporting

### Fixed

- *(fetch)* fetch fixture runtime assets to silence bevy_asset Path-not-found
- avoid rustdoc private-intra-doc-link warning in try_acquire_run docs
- mock-game source write raced rustc in brp_robustness

### Other

- single-job PR gate + nightly, mirror workaround, build-once-test-many
- ci Update release-plz/action action to v0.5.145
- [**breaking**] remove contract::Gameplay marker — zero-annotation invariants
- drop --locked from fixtures workspace commands

## [0.2.0](https://github.com/mcanevet/bevy_playtest/compare/v0.1.0...v0.2.0) - 2026-10-23

### Added

- [**breaking**] run playtests via plain `cargo test` (self-dev-dependency pattern)

### Other

- ci Update taiki-e/install-action digest to a4d3f3f
- fix unit job — skip acceptance test (needs fixture binaries) ([#21](https://github.com/mcanevet/bevy_playtest/pull/21))
- fix release-plz workflow (add Linux deps + pin toolchain)
- minimize wall-clock via parallel jobs + binary artifacts
- ci Update actions/checkout action to v7
- add release-plz workflow and config

### Added
- In-process playtest takeover: `BEVY_PLAYTEST_SCENARIO` turns the game
  binary into its own headless runner (exit codes 0 Pass / 1 Fail / 2
  Crash / 3 config error).
- `PlaytestDefaultPlugins`: pass-through `DefaultPlugins` that swaps to a
  headless runner under `BEVY_PLAYTEST_*` env vars.
- Deterministic scenario schema with strict validation
  (`deny_unknown_fields`, did-you-mean suggestions, loud rejection of
  unsupported fields).
- Oracles: frozen-world liveness, NaN transforms, bounds invariants,
  panic/log capture, frame-timing anomaly counting.
- Replay-divergence verification (`verify_replay: true`) proves determinism
  via two-pass fingerprint comparison.
- Cheat postcondition contracts (`expects`) catch silently no-op hooks.
- Behavioral coverage (`observes` + `strict_coverage`) reports unobserved
  events and unexercised chaos-surface keys.
- External-BRP tier: drive a game binary over Bevy Remote Protocol with a
  reduced, parity-labeled oracle set; `BRP_PORT` auto-enables it.
- CLI: `init`, `run` (with auto-discovery), `doctor`, JUnit XML output.
- `docs/` (Diátaxis): tutorial, how-to, reference, explanation, roadmap.
