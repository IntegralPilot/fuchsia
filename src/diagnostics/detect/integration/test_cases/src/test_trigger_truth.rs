// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{TestData, TestEvent};

const INSPECT_2: &str = r#"
[
    {
        "data_source": "Inspect",
        "metadata": {
          "errors": null,
          "filename": "namespace/whatever",
          "component_url": "some-component:///#meta/something.cm",
          "timestamp": 1233474285373
        },
        "moniker": "foo/bar",
        "payload": {
          "root": {
            "widgets": 2
          }
        },
        "version": 1
    }

]
"#;

const INSPECT_3: &str = r#"
[
    {
        "data_source": "Inspect",
        "metadata": {
          "errors": null,
          "filename": "namespace/whatever",
          "component_url": "some-component:///#meta/something.cm",
          "timestamp": 1233474285373
        },
        "moniker": "foo/bar",
        "payload": {
          "root": {
            "widgets": 3
          }
        },
        "version": 1
    }

]
"#;

const TRIAGE_CONFIG: &str = r#"
{
    select: {
        widgets: "INSPECT:foo/bar:root:widgets",
    },
    act: {
        should_fire: {
            trigger: "widgets > 2",
            type: "Snapshot",
            repeat: "Seconds(1)",
            signature: "widgets-over-two"
        }
    }
}
"#;

pub(crate) fn test() -> TestData {
    TestData::new("Trigger truth")
        .add_inspect_data(INSPECT_3)
        .add_inspect_data(INSPECT_2)
        .add_inspect_data(INSPECT_3)
        .set_program_config("{enable_filing: true}")
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![
            TestEvent::CrashReportingProductRegistration {
                product_name: "FuchsiaDetect".to_string(),
                program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::CrashReport {
                crash_signature: "fuchsia-detect-widgets-over-two".to_string(),
                crash_program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::DiagnosticFetch,
            TestEvent::CrashReport {
                crash_signature: "fuchsia-detect-widgets-over-two".to_string(),
                crash_program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::Done,
        ])
}
