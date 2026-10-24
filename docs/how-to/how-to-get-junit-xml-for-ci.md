# Get JUnit XML for CI

Collect named reports and convert:

```rust
let xml = bevy_playtest::junit::reports_to_junit(
    &[("smoke".to_string(), report)],
    "playtest-suite",
);
std::fs::write("playtest-junit.xml", xml)?;
```

Most CI systems (Jenkins, GitLab, GitHub Actions) can ingest JUnit XML
through their native test-result support or standard report-upload
actions.
