//! Cwd/HOME-isolation child for the BATTERY regression test
//! (tests/battery.rs: battery_child_runs_in_its_own_cwd_and_home):
//! self-checks its working directory and HOME, writes two small
//! marker files — one RELATIVE to its working directory, one under
//! its HOME — then exits.
//!
//! Sibling of the probe pool's `takeover_demo_cwd_write` (same
//! isolation contract, opposite observation strategy: the pool's
//! workspace SURVIVES the run, so that child just WRITES markers
//! and the pool test inspects the directory afterwards; here the
//! battery's temp dir is RAII-removed before the test can look,
//! so THIS child polices the contract itself and reports
//! violations through its exit code, which the parent asserts):
//!
//! - exit 0: cwd is NOT the inherited crate/repo directory
//!   (`CARGO_MANIFEST_DIR` — where a non-isolated child would run,
//!   since `cargo test` executes from the package root), HOME
//!   EQUALS the cwd (the battery repoints it at the same
//!   directory), a `./`-relative READ resolves (through the
//!   symlink tree), and both markers were written.
//! - exit 2: cwd still IS the parent's working directory.
//! - exit 3: HOME was not repointed at the cwd.
//! - exit 4: cwd-relative reads were blinded by the isolation.
//!
//! Deliberately NOT an App run (same rationale as
//! `takeover_demo_noop`): the test observes WHERE the child writes,
//! not what it runs — the takeover/report path is irrelevant.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on — an arm for
//! the off case would be unreachable dead code (and would silently
//! mask a future removal of `required-features`).
fn main() {
    let cwd = std::env::current_dir().expect("child cwd");
    let repo_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR is inherited from cargo test");
    // Canonicalize both sides: temp dirs and the workspace can differ
    // in symlink resolution (`/var` vs `/private/var` on macOS).
    let cwd_real = std::fs::canonicalize(&cwd).expect("canonicalize child cwd");
    let repo_real = std::fs::canonicalize(&*repo_dir).expect("canonicalize CARGO_MANIFEST_DIR");
    if cwd_real == repo_real {
        eprintln!(
            "cwd_check FAILED: child cwd is the parent's directory ({cwd:?}); \
             the battery must run children in an isolated throwaway cwd"
        );
        std::process::exit(2);
    }
    let home = std::env::var_os("HOME").expect("the battery sets HOME on every child");
    // Canonicalize BOTH sides before comparing: on macOS the battery
    // sets HOME to the temp-dir spelling (`/var/folders/...`) while
    // `current_dir()` returns the resolved spelling
    // (`/private/var/folders/...`) — different STRINGS, same
    // directory, and a plain `!=` would false-fail. The cwd side
    // was already canonicalized above (`cwd_real`).
    let home_real = std::fs::canonicalize(&home).expect("canonicalize child HOME");
    if home_real != cwd_real {
        eprintln!(
            "cwd_check FAILED: HOME ({home:?}) is not the child cwd ({cwd:?}); \
             the battery must repoint HOME at the isolated cwd"
        );
        std::process::exit(3);
    }
    std::fs::write("cwd_marker.txt", b"cwd").expect("write cwd-relative marker");
    std::fs::write(cwd.join("home_marker.txt"), b"home").expect("write HOME-resolved marker");
    // Read-preservation half of the contract: `./`-relative READS
    // must keep resolving through the symlink tree (the write-
    // isolation must not blind real games that read `./assets/...`
    // with raw fs calls). The battery runs `cargo test` from the
    // crate root, so `./Cargo.toml` exists in the ORIGINAL cwd and
    // must be readable — through the symlink — in the isolated one.
    if !std::path::Path::new("./Cargo.toml").exists() {
        eprintln!(
            "cwd_check FAILED: ./Cargo.toml is not readable from the isolated \
             cwd — the symlink tree did not preserve cwd-relative reads"
        );
        std::process::exit(4);
    }
}
