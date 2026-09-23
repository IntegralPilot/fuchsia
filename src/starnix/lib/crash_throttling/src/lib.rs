// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::{Inspector, Node};
use futures::FutureExt;
use starnix_logging::log_info;
use starnix_sync::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use zx::{self as zx};

/// The maximum number of crashes we allow to happen for a process within the last
/// CrashReporter.crash_loop_age_out before we consider it to be crash looping. 8 within 8 minutes
/// was chosen as a balance between "definitely a crash loop" and "still saves system resources."
pub const CRASH_LOOP_LIMIT: usize = 8;

/// While throttled, we should still occasionally file a report with a higher "weight" that can
/// represent the rest of the crashes.
const REPORT_EVERY_X_WHILE_THROTTLED: u32 = 100;

/// Decides whether to throttle crashes for a given process based on how frequently they are
/// occurring. Records information about throttled crashes for diagnostics.
pub struct CrashThrottler {
    /// Diagnostics information. A mapping from process name -> number of crashes for that process
    /// that weren't uploaded because of process throttling.
    throttled_core_dumps: Arc<Mutex<HashMap<String, i64>>>,

    /// Tracks when crashes occurred for each process name.
    crashes_per_process: Arc<Mutex<HashMap<String, CrashInfo>>>,

    /// The period before a crash is no longer considered for detecting crash loops.
    pub crash_loop_age_out: zx::MonotonicDuration,

    /// Whether excessive crash reports should be throttled.
    enable_throttling: bool,
}

pub struct PendingCrashReport {
    /// The current task's argv.
    pub argv: Vec<String>,

    /// The crashed process name.
    pub argv0: String,

    /// How many crashes this report represents. For example, a value of 10 would indicate that
    /// this report will represent 9 other throttled crashes for this process.
    pub weight: u32,
}

impl CrashThrottler {
    pub fn new(
        inspect_node: &Node,
        crash_loop_age_out: zx::MonotonicDuration,
        enable_throttling: bool,
    ) -> Self {
        let throttler = Self {
            throttled_core_dumps: Arc::new(Mutex::new(Default::default())),
            crashes_per_process: Arc::new(Mutex::new(Default::default())),
            crash_loop_age_out,
            enable_throttling,
        };

        throttler.record_throttling_in_inspect(inspect_node);
        throttler
    }

    /// Locally records that a crash for `process_name` occurred at `runtime` and returns a guard
    /// for an in-flight report if few enough overall are in-flight, as well as the weight that
    /// should be assigned to the crash report.
    ///
    /// Note: runtime is the total time the device has been on according to the monotonic clock, not
    /// the amount of time the process was running.
    pub fn should_report(
        &self,
        argv: Vec<String>,
        argv0: String,
        runtime: zx::MonotonicInstant,
    ) -> Option<PendingCrashReport> {
        if !self.enable_throttling {
            return Some(PendingCrashReport { argv, argv0, weight: 1 });
        }

        // Locally record that the crash occurred.
        let mut crashes_per_process = self.crashes_per_process.lock();
        let crash_info = crashes_per_process.entry(argv0.clone()).or_default();
        crash_info.crash_runtimes.push_back(runtime);

        crash_info.prune_crash_runtimes(runtime, self.crash_loop_age_out);

        // Even if we're not throttled, we still need to have a weight of 1 so incrementing this
        // here will let us later use it as the weight.
        crash_info.num_crashes_while_throttled += 1;

        // Check if this particular process has been filing too many reports.
        if crash_info.is_throttled_at(runtime, self.crash_loop_age_out)
            && (crash_info.num_crashes_while_throttled < REPORT_EVERY_X_WHILE_THROTTLED)
        {
            log_info!(
                "Process '{argv0}' is throttled due to suspected crash loop, will fold report into later crash"
            );
            *self.throttled_core_dumps.lock().entry(argv0).or_default() += 1;
            return None;
        }

        let weight = crash_info.num_crashes_while_throttled;
        crash_info.num_crashes_while_throttled = 0;

        Some(PendingCrashReport { argv, argv0, weight })
    }

    fn record_throttling_in_inspect(&self, inspect_node: &Node) {
        let throttled_core_dumps = self.throttled_core_dumps.clone();
        let crashes_per_process = self.crashes_per_process.clone();
        let crash_loop_age_out = self.crash_loop_age_out;

        inspect_node.record_lazy_child("coredumps_throttled", move || {
            let throttled_core_dumps = throttled_core_dumps.clone();
            let crashes_per_process = crashes_per_process.clone();

            async move {
                let inspector = Inspector::default();
                let mut crashes_per_process = crashes_per_process.lock();
                let runtime = zx::MonotonicInstant::get();

                for (process, count) in throttled_core_dumps.lock().iter() {
                    let Some(crash_info) = crashes_per_process.get_mut(process) else {
                        continue;
                    };

                    crash_info.prune_crash_runtimes(runtime, crash_loop_age_out);

                    let process_node = inspector.root().create_child(process);
                    process_node.record_bool(
                        "currently_throttled",
                        crash_info.is_throttled_at(runtime, crash_loop_age_out),
                    );
                    process_node.record_int("total_throttled_crashes", *count);
                    if let Some(end) = crash_info.throttling_end(crash_loop_age_out) {
                        process_node.record_int("throttling_runtime_end_millis", end.into_millis());
                    }

                    inspector.root().record(process_node);
                }
                Ok(inspector)
            }
            .boxed()
        });
    }
}

#[derive(Default)]
struct CrashInfo {
    /// How many crashes have occurred while throttled. Resets to 0 if the throttling ends or if a
    /// representative report is uploaded every REPORT_EVERY_X_WHILE_THROTTLED.
    num_crashes_while_throttled: u32,

    /// When the crashes occurred. Crashes that occurred more than CrashReporter.crash_loop_age_out
    /// ago may be removed.
    crash_runtimes: VecDeque<zx::MonotonicInstant>,
}

impl CrashInfo {
    /// Whether the process is throttled at a given instant.
    fn is_throttled_at(
        &self,
        runtime: zx::MonotonicInstant,
        crash_loop_age_out: zx::MonotonicDuration,
    ) -> bool {
        self.crash_runtimes.iter().filter(|&&x| (runtime - x) < crash_loop_age_out).count()
            > CRASH_LOOP_LIMIT
    }

    /// When a process will no longer be throttled, if it currently is throttled.
    fn throttling_end(
        &self,
        crash_loop_age_out: zx::MonotonicDuration,
    ) -> Option<zx::MonotonicDuration> {
        let throttling_end = self.crash_runtimes.iter().nth_back(CRASH_LOOP_LIMIT - 1)?;
        Some(crash_loop_age_out + zx::Duration::from_nanos(throttling_end.into_nanos()))
    }

    // Only keeps entries that are within `crash_loop_age_out`.
    fn prune_crash_runtimes(
        &mut self,
        runtime: zx::MonotonicInstant,
        crash_loop_age_out: zx::MonotonicDuration,
    ) {
        self.crash_runtimes.retain(|&x| (runtime - x) < crash_loop_age_out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRASH_LOOP_AGE_OUT: zx::MonotonicDuration = zx::Duration::from_minutes(8);

    #[test]
    fn not_throttled() {
        let throttler = CrashThrottler::new(
            &fuchsia_inspect::Node::default(),
            CRASH_LOOP_AGE_OUT,
            /*enable_throttling=*/ true,
        );

        assert!(
            throttler
                .should_report(vec![], "test-process".to_string(), zx::Instant::from_nanos(0))
                .is_some()
        );
    }

    #[test]
    fn throttled() {
        let throttler = CrashThrottler::new(
            &fuchsia_inspect::Node::default(),
            CRASH_LOOP_AGE_OUT,
            /*enable_throttling=*/ true,
        );

        for _ in 0..CRASH_LOOP_LIMIT {
            assert!(
                throttler
                    .should_report(vec![], "test-process".to_string(), zx::Instant::from_nanos(0))
                    .is_some()
            );
        }
        assert!(
            throttler
                .should_report(vec![], "test-process".to_string(), zx::Instant::from_nanos(0))
                .is_none()
        );
    }

    #[test]
    fn throttling_ages_out() {
        let throttler = CrashThrottler::new(
            &fuchsia_inspect::Node::default(),
            CRASH_LOOP_AGE_OUT,
            /*enable_throttling=*/ true,
        );

        for _ in 0..CRASH_LOOP_LIMIT {
            assert!(
                throttler
                    .should_report(vec![], "test-process".to_string(), zx::Instant::from_nanos(0))
                    .is_some()
            );
        }
        assert!(
            throttler
                .should_report(vec![], "test-process".to_string(), zx::Instant::from_nanos(0))
                .is_none()
        );
        assert!(
            throttler
                .should_report(
                    vec![],
                    "test-process".to_string(),
                    zx::Instant::from_nanos(CRASH_LOOP_AGE_OUT.into_nanos())
                )
                .is_some()
        );
    }

    #[test]
    fn reports_some_crashes_while_throttled() {
        const RUNTIME: zx::MonotonicInstant = zx::Instant::from_nanos(0);
        let throttler = CrashThrottler::new(
            &fuchsia_inspect::Node::default(),
            CRASH_LOOP_AGE_OUT,
            /*enable_throttling=*/ true,
        );

        for _ in 0..CRASH_LOOP_LIMIT {
            assert!(throttler.should_report(vec![], "test-process".to_string(), RUNTIME).is_some());
        }

        for _ in 0..REPORT_EVERY_X_WHILE_THROTTLED - 1 {
            assert!(throttler.should_report(vec![], "test-process".to_string(), RUNTIME).is_none());
        }

        assert_eq!(
            throttler.should_report(vec![], "test-process".to_string(), RUNTIME).unwrap().weight,
            REPORT_EVERY_X_WHILE_THROTTLED
        );
    }

    #[test]
    fn is_throttled_filters() {
        let mut crash_info: CrashInfo = Default::default();

        crash_info.crash_runtimes.push_back(zx::MonotonicInstant::from_nanos(0));
        for _ in 0..CRASH_LOOP_LIMIT {
            crash_info.crash_runtimes.push_back(zx::MonotonicInstant::from_nanos(50));
        }

        assert!(
            crash_info.is_throttled_at(zx::MonotonicInstant::from_nanos(0), CRASH_LOOP_AGE_OUT)
        );
        assert!(!crash_info.is_throttled_at(
            zx::MonotonicInstant::from_nanos(CRASH_LOOP_AGE_OUT.into_nanos()),
            CRASH_LOOP_AGE_OUT
        ));
    }

    #[test]
    fn throttling_ends() {
        let age_out = zx::Duration::from_millis(200);
        let throttler = CrashThrottler::new(
            &fuchsia_inspect::Node::default(),
            age_out,
            /*enable_throttling=*/ true,
        );

        let mut time = zx::Instant::from_nanos(0);

        for _ in 0..CRASH_LOOP_LIMIT {
            assert!(throttler.should_report(vec![], "test-process".to_string(), time).is_some());
        }

        assert!(throttler.should_report(vec![], "test-process".to_string(), time).is_none());

        time += age_out + zx::Duration::from_millis(50);

        assert!(throttler.should_report(vec![], "test-process".to_string(), time).is_some());
    }
}
