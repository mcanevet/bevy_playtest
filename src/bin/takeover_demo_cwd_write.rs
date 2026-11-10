//! Cwd/HOME-isolation child for probe-pool tests: writes two small
//! marker files — one RELATIVE to its working directory, one under
//! the HOME it inherited — then exits. The pool runs each probe
//! child in its own throwaway directory with HOME repointed there
//! (see `execute_job` in src/discovery/probe_pool.rs); the
//! regression test tests/probe_pool.rs:
//! probe_children_get_their_own_cwd_and_home asserts BOTH markers
//! land in that per-job directory, not in the parent's cwd or the
//! developer's real home.
//!
//! Deliberately NOT an App run (same rationale as
//! `takeover_demo_noop`): the test observes WHERE the child writes,
//! not what it runs — the takeover/report path is irrelevant, and
//! the pool's expected NoReport outcome for this child is asserted
//! (not ignored) as proof the child really ran to completion.
//!
//! No `#[cfg(not(feature = "takeover"))]` fallback arm: the
//! `[[bin]]` entry carries `required-features = ["takeover"]`, so
//! this binary is only ever BUILT with the feature on — an arm for
//! the off case would be unreachable dead code (and would silently
//! mask a future removal of `required-features`).
fn main() {
    std::fs::write("cwd_marker.txt", b"cwd").expect("write cwd-relative marker");
    let home = std::env::var_os("HOME").expect("the pool sets HOME on every child");
    std::fs::write(std::path::Path::new(&home).join("home_marker.txt"), b"home")
        .expect("write HOME-resolved marker");
}
