// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{TestData, TestEvent};

// If we don't include at least a {} then the reader library re-fetches instantly
// and spoils the test.
const INSPECT_EMPTY: &str = "[ {} ]";

const TRIAGE_CONFIG: &str = r#"
{
    select: {
        // Need a selector or it won't try to get Inspect data and the test won't terminate.
        meaningless: "INSPECT:missing:root:missing",
    },
    act: {
        always: {
            trigger: "3 > 2",
            type: "Snapshot",
            repeat: "Seconds(1)",
            signature: "yes"
        }
    }
}
"#;

pub(crate) fn test_with_enable() -> TestData {
    TestData::new("With Enable")
        .set_program_config("{enable_filing: true}")
        .add_inspect_data(INSPECT_EMPTY)
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![
            TestEvent::CrashReportingProductRegistration {
                product_name: "FuchsiaDetect".to_string(),
                program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::CrashReport {
                crash_signature: "fuchsia-detect-yes".to_string(),
                crash_program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::Done,
        ])
}

pub(crate) fn test_bad_enable() -> TestData {
    TestData::new("Bad Program Config Format")
        .set_program_config("{enable_filing: 1}")
        .add_inspect_data(INSPECT_EMPTY)
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![TestEvent::Bail])
}

pub(crate) fn test_false_enable() -> TestData {
    TestData::new("With False Enable")
        .set_program_config("{enable_filing: false}")
        .add_inspect_data(INSPECT_EMPTY)
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![
            TestEvent::CrashReportingProductRegistration {
                product_name: "FuchsiaDetect".to_string(),
                program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::DiagnosticFetch,
            TestEvent::Done,
        ])
}

pub(crate) fn test_no_enable() -> TestData {
    TestData::new("Empty Program Config")
        .set_program_config("{}")
        .add_inspect_data(INSPECT_EMPTY)
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![
            TestEvent::CrashReportingProductRegistration {
                product_name: "FuchsiaDetect".to_string(),
                program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::DiagnosticFetch,
            TestEvent::Done,
        ])
}

pub(crate) fn test_without_file() -> TestData {
    TestData::new("Without Program Config File")
        .add_inspect_data(INSPECT_EMPTY)
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![
            TestEvent::CrashReportingProductRegistration {
                product_name: "FuchsiaDetect".to_string(),
                program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::DiagnosticFetch,
            TestEvent::Done,
        ])
}
