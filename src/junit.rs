//! JUnit XML output for reports.
//!
//! Hand-written XML (no dependency) with proper escaping; every
//! mainstream CI dashboard ingests the JUnit format. The full JSON
//! report goes in `<system-out>` CDATA for drill-down.
//!
//! Carries the PoC's RF2 hardening: control-char replacement (XML 1.0
//! forbids C0 refs), U+FFFE/U+FFFF replacement, and CDATA `]]>` splitting.

use crate::enums::PlaytestStatus;
use crate::state::PlaytestReport;

/// Escape a string for XML text/attribute content.
pub fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // XML 1.0 forbids C0 control chars (except \t\n\r) AND
            // character references to them, plus U+FFFE/U+FFFF. Replace
            // with U+FFFD (replacement char) so documents stay
            // well-formed regardless of input.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => o.push('\u{FFFD}'),
            '\u{FFFE}' | '\u{FFFF}' => o.push('\u{FFFD}'),
            c => o.push(c),
        }
    }
    o
}

/// CDATA-safe: filter control chars, then split any `]]>` into
/// `]]]]><![CDATA[>` so the embedded JSON cannot terminate the CDATA
/// section early.
fn cdata_wrap(content: &str) -> String {
    let filtered: String = content
        .chars()
        .map(|c| {
            if ((c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r'))
                || matches!(c, '\u{FFFE}' | '\u{FFFF}')
            {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect();
    format!("<![CDATA[{}]]>", filtered.replace("]]>", "]]]]><![CDATA[>"))
}

impl PlaytestReport {
    /// One `<testcase>`. Fail -> `<failure>`, Crash -> `<error>`;
    /// the full JSON report goes in `<system-out>` for drill-down.
    pub fn to_junit_testcase(&self, name: &str, classname: &str) -> String {
        let time = if self.wall_time_s > 0.0 {
            self.wall_time_s
        } else {
            self.simulated_duration_s
        };
        let mut out = format!(
            "      <testcase name=\"{}\" classname=\"{}\" time=\"{:.3}\">\n",
            xml_escape(name),
            xml_escape(classname),
            time
        );
        let json = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        match self.status {
            PlaytestStatus::Pass => {}
            PlaytestStatus::Crash => {
                let msg = self.error.as_deref().unwrap_or("crash");
                out.push_str(&format!(
                    "        <error message=\"{}\" type=\"crash\">{}\n        </error>\n",
                    xml_escape(msg),
                    xml_escape(msg)
                ));
            }
            PlaytestStatus::Fail => {
                // One summary line per violation: rule / target /
                // first..last frame / count / detail.
                let mut body = String::new();
                for v in &self.violations {
                    body.push_str(&format!(
                        "[{} @ {}] frames {}..{}, count {}, detail: {}\n",
                        v.rule, v.target, v.first_frame, v.last_frame, v.count, v.last_detail
                    ));
                }
                let headline = self
                    .violations
                    .first()
                    .map(|v| format!("{} @ {}", v.rule, v.target))
                    .unwrap_or_else(|| "failed".to_string());
                let ftype = self
                    .violations
                    .first()
                    .map(|v| v.rule.clone())
                    .unwrap_or_else(|| "failure".to_string());
                out.push_str(&format!(
                    "        <failure message=\"{}\" type=\"{}\">\n{}        </failure>\n",
                    xml_escape(&headline),
                    xml_escape(&ftype),
                    xml_escape(&body)
                ));
            }
        }
        out.push_str(&format!(
            "        <system-out>{}</system-out>\n",
            cdata_wrap(&json)
        ));
        out.push_str("      </testcase>\n");
        out
    }
}

/// Full JUnit XML document for a set of named reports.
pub fn reports_to_junit(reports: &[(String, PlaytestReport)], suite_name: &str) -> String {
    let tests = reports.len();
    let failures = reports
        .iter()
        .filter(|(_, r)| r.status == PlaytestStatus::Fail)
        .count();
    let errors = reports
        .iter()
        .filter(|(_, r)| r.status == PlaytestStatus::Crash)
        .count();
    let total_time: f64 = reports
        .iter()
        .map(|(_, r)| {
            if r.wall_time_s > 0.0 {
                r.wall_time_s
            } else {
                r.simulated_duration_s
            }
        })
        .sum();
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str("<testsuites>\n");
    out.push_str(&format!(
        "  <testsuite name=\"{}\" tests=\"{}\" failures=\"{}\" errors=\"{}\" time=\"{:.3}\">\n",
        xml_escape(suite_name),
        tests,
        failures,
        errors,
        total_time
    ));
    let coverage_holes: usize = reports
        .iter()
        .map(|(_, r)| {
            r.coverage
                .as_ref()
                .map(|c| c.observations.iter().filter(|o| !o.observed).count())
                .unwrap_or(0)
        })
        .sum();
    if coverage_holes > 0 {
        out.push_str(&format!(
            "    <properties>\n      <property name=\"coverage_holes\" value=\"{}\"/>\n    </properties>\n",
            coverage_holes
        ));
    }
    for (name, r) in reports {
        out.push_str(&r.to_junit_testcase(name, suite_name));
    }
    out.push_str("  </testsuite>\n");
    out.push_str("</testsuites>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ViolationEntry;

    fn pass_report() -> PlaytestReport {
        let mut r = PlaytestReport::crashed_empty();
        r.status = PlaytestStatus::Pass;
        r
    }

    fn fail_report() -> PlaytestReport {
        let mut r = PlaytestReport::crashed_empty();
        r.status = PlaytestStatus::Fail;
        r.violations.push(ViolationEntry {
            rule: "bounds_check".into(),
            target: "Ball <&\"quoted\">".into(),
            first_frame: 412,
            last_frame: 412,
            count: 1,
            detail: "out of bounds (10012.0)".into(),
            last_detail: "out of bounds (10012.0)".into(),
        });
        r
    }

    fn assert_well_formed(xml: &str) {
        // Minimal structural validation: balanced angle brackets OUTSIDE
        // CDATA sections, and tag open/close parity. CDATA content is
        // arbitrary text and may legitimately contain '<'/'>'. A real
        // XML parser would be stricter; this catches gross malformation.
        let outside: String = strip_cdata(xml);
        let opens = outside.matches('<').count();
        let closes = outside.matches('>').count();
        assert_eq!(
            opens, closes,
            "unbalanced angle brackets outside CDATA: {xml}"
        );
        for (open_tag, close_tag) in [
            ("<testcase", "</testcase>"),
            ("<testsuite ", "</testsuite>"),
            ("<system-out>", "</system-out>"),
            ("<root>", "</root>"),
        ] {
            let n_open = outside.matches(open_tag).count();
            let n_close = outside.matches(close_tag).count();
            assert_eq!(
                n_open, n_close,
                "tag {open_tag} unbalanced ({n_open}/{n_close}): {xml}"
            );
        }
        // No raw unescaped specials between elements (spot checks).
        assert!(!outside.contains("<&\""), "raw specials leaked: {xml}");
    }

    fn strip_cdata(xml: &str) -> String {
        let mut out = String::with_capacity(xml.len());
        let mut rest = xml;
        while let Some(start) = rest.find("<![CDATA[") {
            out.push_str(&rest[..start]);
            rest = &rest[start + "<![CDATA[".len()..];
            match rest.find("]]>") {
                Some(end) => rest = &rest[end + "]]>".len()..],
                None => panic!("unterminated CDATA: {xml}"),
            }
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn escape_specials() {
        assert_eq!(xml_escape("a<b>&\"'c"), "a&lt;b&gt;&amp;&quot;&apos;c");
        // C0 controls and U+FFFE/U+FFFF are REPLACED (character
        // references to them are illegal in XML 1.0).
        assert_eq!(xml_escape("\u{1}"), "\u{FFFD}");
        assert_eq!(xml_escape("\u{0}\u{7}\u{1b}"), "\u{FFFD}\u{FFFD}\u{FFFD}");
        assert_eq!(xml_escape("\u{FFFE}\u{FFFF}"), "\u{FFFD}\u{FFFD}");
        // allowed whitespace passes through
        assert_eq!(xml_escape("\t\n\r"), "\t\n\r");
    }

    #[test]
    fn passing_testcase_has_no_failure() {
        let tc = pass_report().to_junit_testcase("pass-case", "suite");
        assert_well_formed(&format!("<root>{tc}</root>"));
        assert!(tc.contains("<testcase name=\"pass-case\""));
        assert!(!tc.contains("<failure"), "{tc}");
        assert!(!tc.contains("<error"), "{tc}");
        assert!(tc.contains("<system-out>"));
    }

    #[test]
    fn failing_testcase_escapes_and_details() {
        let tc = fail_report().to_junit_testcase("fail-case", "suite");
        assert_well_formed(&format!("<root>{tc}</root>"));
        assert!(tc.contains("<failure"));
        assert!(tc.contains("type=\"bounds_check\""));
        // angle brackets + quotes in the target are escaped
        assert!(tc.contains("&lt;&amp;&quot;"), "{tc}");
        assert!(!tc.contains("<&\""), "{tc}");
        assert!(tc.contains("count 1"));
        assert!(tc.contains("frames 412..412"));
    }

    #[test]
    fn crash_testcase_uses_error() {
        let mut r = PlaytestReport::crashed_empty();
        r.error = Some("boom < boom".into());
        let tc = r.to_junit_testcase("crash-case", "suite");
        assert_well_formed(&format!("<root>{tc}</root>"));
        assert!(tc.contains("<error"));
        assert!(tc.contains("boom &lt; boom"));
    }

    #[test]
    fn control_chars_yield_replacement_chars() {
        let mut r = fail_report();
        r.violations[0].detail = "\u{1b}[31m red \u{0} bell \u{7}".into();
        r.violations[0].last_detail = "\u{1b}[31m red \u{0} bell \u{7}".into();
        r.error = Some("\u{FFFE} bad \u{FFFF}".into());
        let tc = r.to_junit_testcase("ctrl-case", "suite");
        assert!(
            !tc.contains("&#x"),
            "no char refs to forbidden codepoints: {tc}"
        );
        assert_well_formed(&format!("<root>{tc}</root>"));
    }

    #[test]
    fn cdata_split_protects_system_out() {
        let mut r = pass_report();
        r.error = Some("evil]]>injection".into());
        let tc = r.to_junit_testcase("cdata-case", "suite");
        let inner_start = tc.find("<system-out>").unwrap() + "<system-out>".len();
        let inner_end = tc.rfind("</system-out>").unwrap();
        let inner = &tc[inner_start..inner_end];
        assert!(
            inner.starts_with("<![CDATA["),
            "CDATA wrapper missing: {inner}"
        );
        let content = inner
            .strip_prefix("<![CDATA[")
            .and_then(|s| s.strip_suffix("]]>"))
            .expect("CDATA terminator");
        let rejoined = content.replace("]]]]><![CDATA[>", "]]>");
        let expected = serde_json::to_string_pretty(&r).unwrap();
        assert_eq!(rejoined, expected, "CDATA payload must round-trip");
        assert!(inner.contains("]]]]><![CDATA[>"), "split missing: {inner}");
    }

    #[test]
    fn document_balanced_and_counted() {
        let reports = vec![
            ("pass-variant".to_string(), pass_report()),
            ("fail-variant".to_string(), fail_report()),
        ];
        let doc = reports_to_junit(&reports, "playtest");
        assert_well_formed(&doc);
        assert!(doc.starts_with("<?xml"));
        assert!(doc.contains("tests=\"2\""));
        assert!(doc.contains("failures=\"1\""));
        assert!(doc.contains("errors=\"0\""));
        assert_eq!(doc.matches("<testcase").count(), 2);
        assert_eq!(doc.matches("</testcase>").count(), 2);
    }
    #[test]
    fn junit_suite_carries_coverage_hole_property() {
        let mut r = pass_report();
        r.coverage = Some(crate::state::Coverage {
            observations: vec![
                crate::state::ObservationRecord {
                    name: "seen".to_string(),
                    observed: true,
                },
                crate::state::ObservationRecord {
                    name: "unseen".to_string(),
                    observed: false,
                },
            ],
            unexercised_keys: vec![],
            surface_coverage: 1.0,
        });
        let xml = reports_to_junit(&[("case".to_string(), r)], "cov");
        assert!(
            xml.contains("<property name=\"coverage_holes\" value=\"1\"/>"),
            "{xml}"
        );
    }

    #[test]
    fn junit_suite_without_coverage_has_no_property() {
        let xml = reports_to_junit(&[("case".to_string(), pass_report())], "nc");
        assert!(!xml.contains("coverage_holes"), "{xml}");
    }
}
