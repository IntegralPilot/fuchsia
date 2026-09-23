// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::lock::Mutex;
use log::*;
use moniker::Moniker;
use std::sync::Arc;
use {fidl_fuchsia_sys2 as fsys, fuchsia_async as fasync};

/// Any stored data is removed after this amount of time
const CLEANUP_DEADLINE_SECONDS: i64 = 600;

#[derive(Debug, Clone, PartialEq)]
pub struct ComponentCrashInfo {
    pub url: String,
    pub moniker: Moniker,
}

impl Into<fsys::ComponentCrashInfo> for ComponentCrashInfo {
    fn into(self) -> fsys::ComponentCrashInfo {
        fsys::ComponentCrashInfo {
            url: Some(self.url),
            moniker: Some(self.moniker.to_string()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Record {
    // The point at which this record should be deleted
    deadline: zx::MonotonicInstant,
    // The koid of the thread a crash was observed on
    koid: zx::Koid,
    // The crash information
    crash_info: ComponentCrashInfo,
}

#[derive(Clone)]
pub struct CrashRecords {
    records: Arc<Mutex<Vec<Record>>>,
    _cleanup_task: Arc<fasync::Task<()>>,
}

/// This task will inspect `records`, and sleep until either `records[0].deadline` or for
/// `CLEANUP_DEADLINE_SECONDS` seconds. Upon waking, it removes anything from `records` whose
/// deadline has passed, and then repeats.
///
/// If `records` is not sorted by `Record::deadline` in ascending order then this task will not
/// behave correctly.
async fn record_cleanup_task(records: Arc<Mutex<Vec<Record>>>) {
    loop {
        let sleep_until = {
            let records_guard = records.lock().await;
            if records_guard.is_empty() {
                // If we have no records, then we can sleep for as long as the timeout and check
                // again
                zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(
                    CLEANUP_DEADLINE_SECONDS,
                ))
            } else {
                // If there's an upcoming record to delete, sleep until then
                records_guard[0].deadline.clone()
            }
        };
        let timer = fasync::Timer::new(sleep_until);
        timer.await;
        let mut records_guard = records.lock().await;
        while !records_guard.is_empty() && zx::MonotonicInstant::get() > records_guard[0].deadline {
            records_guard.remove(0);
        }
    }
}

impl CrashRecords {
    pub fn new() -> Self {
        let records = Arc::new(Mutex::new(vec![]));
        CrashRecords {
            records: records.clone(),
            _cleanup_task: Arc::new(fasync::Task::spawn(record_cleanup_task(records))),
        }
    }

    /// Adds a new report to CrashRecords, and schedules the new reports deletion in
    /// `CLEANUP_DEADLINE_SECONDS`.
    pub async fn add_report(&self, thread_koid: zx::Koid, report: ComponentCrashInfo) {
        self.records.lock().await.push(Record {
            deadline: zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(
                CLEANUP_DEADLINE_SECONDS,
            )),
            koid: thread_koid.clone(),
            crash_info: report,
        });
    }

    /// Removes the report (if any), and deletes it.
    pub async fn take_report(&self, thread_koid: &zx::Koid) -> Option<ComponentCrashInfo> {
        let mut records_guard = self.records.lock().await;
        let index_to_remove = records_guard
            .iter()
            .enumerate()
            .find(|(_, record)| &record.koid == thread_koid)
            .map(|(i, _)| i);
        if index_to_remove.is_none() {
            warn!(
                "crash introspection failed to provide attribution for the crashed thread with koid {:?}",
                thread_koid
            );
        }
        index_to_remove.map(|i| records_guard.remove(i).crash_info)
    }
}
