//! Build the deterministic-randomness shim (Linux only).
//!
//! Compiles `detrng/shim.rs` into a shared library in OUT_DIR with the
//! same `rustc` that builds this crate, and exposes its path to the
//! crate as `BEVY_PT_DETRNG_SHIM` (empty when unavailable: not
//! Linux, `takeover` feature off, or the compile failed — the battery
//! then reports randomness as not intercepted, with the reason).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=detrng/shim.rs");
    println!("cargo:rerun-if-changed=build.rs");

    let path = build_shim().unwrap_or_else(|reason| {
        println!(
            "cargo:warning=bevy_playtest: deterministic-randomness shim unavailable: {reason}"
        );
        String::new()
    });
    // Deliberately NOT a `BEVY_PLAYTEST_*` name: cargo also exports
    // build-script `rustc-env` values into the environment of
    // `cargo test`/`run`, children inherit them, and the takeover
    // rejects unknown `BEVY_PLAYTEST_*` variables.
    println!("cargo:rustc-env=BEVY_PT_DETRNG_SHIM={path}");
}

fn build_shim() -> Result<String, String> {
    if std::env::var_os("CARGO_FEATURE_TAKEOVER").is_none() {
        return Ok(String::new()); // only the battery (takeover) uses it
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return Ok(String::new()); // LD_PRELOAD interception is Linux-only
    }
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let out = out_dir.join("libbevy_playtest_detrng.so");
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let target = std::env::var("TARGET").map_err(|e| e.to_string())?;
    let status = Command::new(rustc)
        .args([
            "--edition",
            "2024",
            "--crate-type",
            "cdylib",
            "--crate-name",
            "bevy_playtest_detrng",
            "-C",
            "opt-level=2",
            "-C",
            "panic=abort",
            "--target",
            &target,
            "-o",
        ])
        .arg(&out)
        .arg("detrng/shim.rs")
        .status()
        .map_err(|e| format!("could not run rustc: {e}"))?;
    if !status.success() {
        return Err(format!("rustc exited with {status}"));
    }
    Ok(out.to_string_lossy().into_owned())
}
