// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{TestData, TestEvent};

const INSPECT: &str = r#"
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

// Ensure that the "repeat" lines here are consistent with CHECK_PERIOD_SECONDS.
const TRIAGE_CONFIG: &str = r#"
{
    select: {
        widgets: "INSPECT:foo/bar:root:widgets",
    },
    act: {
        messy_signature: {
            trigger: "widgets > 2",
            type: "Snapshot",
            repeat: "Seconds(1)",
            signature: "This_is $illy!, BᗷB♥B"
        },
        sentence_signature: {
            trigger: "widgets > 2",
            type: "Snapshot",
            repeat: "Seconds(1)",
            signature: "There was an error"
        }
    }
}
"#;

pub(crate) fn test() -> TestData {
    TestData::new("Snapshot sanitizing")
        .add_inspect_data(INSPECT)
        .set_program_config("{enable_filing: true}")
        .add_triage_config(TRIAGE_CONFIG)
        .expect_events(vec![
            TestEvent::CrashReportingProductRegistration {
                product_name: "FuchsiaDetect".to_string(),
                program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::CrashReport {
                crash_signature: "fuchsia-detect-there-was-an-error".to_string(),
                crash_program_name: "triage_detect".to_string(),
            },
            TestEvent::CrashReport {
                crash_signature: "fuchsia-detect-this-is--illy---b-b-b".to_string(),
                crash_program_name: "triage_detect".to_string(),
            },
            TestEvent::DiagnosticFetch,
            TestEvent::Done,
        ])
}
