// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::{DiagnosticsHierarchy, InspectData};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct CoredumpReport {
    pub idx: usize,
    pub thread_koid: u64,
    pub process_koid: u64,
    pub pid: i64,
    pub argv: String,
    pub thread_name: String,
    pub uptime: i64,
    pub signal: String,
}

impl CoredumpReport {
    /// Returns `None` if the snapshot seems partially-written, indicating a retry is warranted.
    ///
    /// # Panics
    ///
    /// If the snapshot has the same rough structure as Starnix's inspect output but was incorrectly
    /// recorded.
    pub fn extract_from_snapshot(data: &InspectData) -> Option<Vec<Self>> {
        let DiagnosticsHierarchy { name, properties, children, .. } = data
            .payload
            .as_ref()?
            .get_child_by_path(&["container", "kernel", "coredumps"])
            .cloned()?;
        assert_eq!(name, "coredumps");
        assert_eq!(properties, vec![]);

        // Examine all of the properties and children so that we are alerted that this test code may
        // need to be updated if coredump reports gain new information.
        let mut reports = vec![];
        for DiagnosticsHierarchy {
            name: idx_str,
            properties: coredump_properties,
            children: coredump_children,
            ..
        } in children
        {
            assert!(
                coredump_children.is_empty(),
                "coredump reports aren't expected to have children, found {coredump_children:?}"
            );

            let mut argv = None;
            let mut process_koid = None;
            let mut thread_koid = None;
            let mut thread_name = None;
            let mut pid = None;
            let mut uptime = None;
            let mut signal = None;
            for property in coredump_properties {
                match property.name() {
                    "argv" => {
                        argv = Some(
                            property
                                .string()
                                .expect("getting argv string from report node")
                                .to_string(),
                        )
                    }
                    "thread_name" => {
                        thread_name = Some(
                            property
                                .string()
                                .expect("getting thread_name string from report node")
                                .to_string(),
                        )
                    }
                    "signal" => {
                        signal = Some(
                            property
                                .string()
                                .expect("getting signal string from report node")
                                .to_string(),
                        )
                    }
                    "thread_koid" => {
                        thread_koid =
                            Some(property.uint().expect("getting thread koid from report node"))
                    }
                    "process_koid" => {
                        process_koid =
                            Some(property.uint().expect("getting process koid from report node"))
                    }

                    // TODO(https://fxbug.dev/42081072) i64/int in kernel shows up as u64/uint here
                    "pid" => {
                        pid = Some(property.uint().expect("getting pid from report node") as i64)
                    }
                    // TODO(https://fxbug.dev/42081072) i64/int in kernel shows up as u64/uint here
                    "uptime" => {
                        uptime =
                            Some(property.uint().expect("getting uptime from report node") as i64)
                    }
                    other => panic!("unrecognized coredump report property `{other}`"),
                }
            }

            reports.push(Self {
                idx: idx_str.parse().expect("starnix coredump node names should be integers"),
                thread_koid: thread_koid.expect("retrieving thread koid property"),
                process_koid: process_koid.expect("retrieving process koid property"),
                pid: pid.expect("retrieving pid property"),
                argv: argv.expect("retrieving argv property"),
                thread_name: thread_name.expect("retrieving thread_name property"),
                uptime: uptime.expect("retrieving uptime property"),
                signal: signal.expect("retrieving signal property"),
            });
        }
        reports.sort();

        Some(reports)
    }
}
