// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_feedback as ffeedback;
use fidl_fuchsia_mem as fmem;
use fuchsia_async as fasync;
use fuchsia_async::TimeoutExt;
use fuchsia_inspect::Inspector;
use fuchsia_runtime as fruntime;
use futures::future::BoxFuture;
use starnix_c_file_buffer::CFileBuffer;
use starnix_core::task::ThreadLockupInfo;
use starnix_logging::{log_debug, log_error, log_info, log_warn};
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;
use zx::{self, Task};

// We use `inspector_print_debug_info` directly instead of
// `backtrace_request_thread` because `backtrace_request_thread` relies on the
// exception mechanism (crashsvc). If we use exceptions, the exception is
// attributed to the main Starnix kernel process (which detects the lockup),
// not the process containing the locked-up thread. This would prevent
// crashsvc from accessing the correct thread state and stack. By holding the
// thread handle directly, we can inspect it regardless of its process.
// SAFETY: This declares external C symbols from the Zircon inspector library.
unsafe extern "C" {
    fn inspector_print_debug_info(
        out: *mut std::ffi::c_void,
        process: zx::sys::zx_handle_t,
        thread: zx::sys::zx_handle_t,
    );
}

async fn dump_thread_backtrace<'a>(
    thread: &zx::Thread,
    file_buffer: &'a mut CFileBuffer,
    timeout: zx::MonotonicDuration,
) -> Result<&'a str, anyhow::Error> {
    let _suspend_token = thread.suspend()?;

    // Wait for suspended signal asynchronously.
    fasync::OnSignals::new(thread, zx::Signals::THREAD_SUSPENDED)
        .on_timeout(timeout, || Err(zx::Status::TIMED_OUT))
        .await?;

    // Reset the buffer to overwrite previous contents.
    file_buffer.reset().map_err(|e| anyhow::anyhow!("Failed to reset CFileBuffer: {}", e))?;

    // SAFETY: Calling FFI is safe when passing valid handles.
    unsafe {
        let process_self = fruntime::process_self().raw_handle();
        let file_ptr = file_buffer.file();
        inspector_print_debug_info(
            file_ptr.as_raw() as *mut std::ffi::c_void,
            process_self,
            thread.raw_handle(),
        );
    }

    let data = file_buffer.data();
    let backtrace_str = str::from_utf8(data)?;
    Ok(backtrace_str)
}

const LOCKUP_DETECTOR_INTERVAL_MINUTES: i64 = 2;
const RCU_STALL_THRESHOLD_SAMPLES: u32 = 4;
const THREAD_DUMP_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);

struct ActiveRcuRead {
    consecutive_polls_active: u32,
    first_seen: zx::MonotonicInstant,
    last_counter_index: u8,
}

struct RcuStall {
    thread: zx::Thread,
    koid: zx::Koid,
    first_seen: zx::MonotonicInstant,
}

#[derive(Default)]
struct Lockups {
    long_running: Vec<ThreadLockupInfo>,
    current_koids: BTreeSet<zx::Koid>,
    newly_locked: Vec<ThreadLockupInfo>,
}

#[derive(Default)]
struct LockupDetectorContext {
    event_id: Option<String>,
    reported_lockup_koids: BTreeSet<zx::Koid>,
    reported_rcu_koids: BTreeSet<zx::Koid>,
    active_rcu_reads: HashMap<zx::Koid, ActiveRcuRead>,
}

fn truncate_annotation_value(mut value: String) -> String {
    let max_annotation_len = ffeedback::MAX_ANNOTATION_VALUE_LENGTH as usize;
    if value.len() > max_annotation_len {
        let mut limit = max_annotation_len.saturating_sub(3);
        while !value.is_char_boundary(limit) {
            limit -= 1;
        }
        value.truncate(limit);
        value.push_str("...");
    }
    value
}

fn format_thread_names(thread_names: BTreeSet<String>) -> String {
    let names_str = thread_names.into_iter().collect::<Vec<_>>().join(", ");
    truncate_annotation_value(names_str)
}

fn build_annotations(lockups: &Lockups, rcu_stalls: &Vec<RcuStall>) -> Vec<ffeedback::Annotation> {
    let koids_str =
        format!("{:?}", lockups.current_koids.iter().map(|k| k.raw_koid()).collect::<Vec<_>>());
    let rcu_koids_str =
        format!("{:?}", rcu_stalls.iter().map(|k| k.koid.raw_koid()).collect::<Vec<_>>());

    let mut thread_names = BTreeSet::new();
    for registered in &lockups.long_running {
        let name = if let Ok(name) = registered.thread.get_name() {
            format!("{}({})", name, registered.koid.raw_koid())
        } else {
            format!("koid-{}", registered.koid.raw_koid())
        };
        thread_names.insert(name);
    }
    let mut rcu_thread_names = BTreeSet::new();
    for stall in rcu_stalls {
        let name = if let Ok(name) = stall.thread.get_name() {
            format!("{}({})", name, stall.koid.raw_koid())
        } else {
            format!("koid-{}", stall.koid.raw_koid())
        };
        rcu_thread_names.insert(name);
    }

    vec![
        ffeedback::Annotation {
            key: "starnix.lockup_thread_koids".to_string(),
            value: truncate_annotation_value(koids_str),
        },
        ffeedback::Annotation {
            key: "starnix.lockup_thread_names".to_string(),
            value: format_thread_names(thread_names),
        },
        ffeedback::Annotation {
            key: "starnix.rcu_lockup_thread_koids".to_string(),
            value: truncate_annotation_value(rcu_koids_str),
        },
        ffeedback::Annotation {
            key: "starnix.rcu_lockup_thread_names".to_string(),
            value: format_thread_names(rcu_thread_names),
        },
    ]
}

fn order_lockup_candidates<'a>(
    newly_locked: Vec<ThreadLockupInfo>,
    rcu_stalls: &'a [RcuStall],
) -> Vec<(zx::Unowned<'a, zx::Thread>, zx::Koid, zx::MonotonicInstant)> {
    let mut candidates: HashMap<zx::Koid, (zx::Unowned<'a, zx::Thread>, zx::MonotonicInstant)> =
        HashMap::new();

    for info in newly_locked {
        candidates
            .entry(info.koid)
            .and_modify(|(_, start_time)| *start_time = (*start_time).min(info.start_time))
            .or_insert_with(|| (info.thread, info.start_time));
    }

    for stall in rcu_stalls {
        candidates
            .entry(stall.koid)
            .and_modify(|(_, start_time)| *start_time = (*start_time).min(stall.first_seen))
            .or_insert_with(|| (stall.thread.unowned(), stall.first_seen));
    }

    let mut sorted_threads: Vec<_> = candidates
        .into_iter()
        .map(|(koid, (thread, start_time))| (thread, koid, start_time))
        .collect();
    sorted_threads.sort_by_key(|(_, _, start_time)| *start_time);
    sorted_threads
}

async fn report_lockups(
    context: &mut LockupDetectorContext,
    lockups: Lockups,
    rcu_stalls: Vec<RcuStall>,
) -> anyhow::Result<()> {
    let event_id_str = context.event_id.get_or_insert_with(|| Uuid::new_v4().to_string()).clone();
    let mut file_buffer = CFileBuffer::new(1024 * 1024)
        .map_err(|e| anyhow::anyhow!("Failed to create CFileBuffer: {}", e))?;

    let reporter =
        fuchsia_component::client::connect_to_protocol::<ffeedback::CrashReporterMarker>();
    let reporter = match reporter {
        Ok(reporter) => Some(reporter),
        Err(e) => {
            log_warn!("Failed to connect to CrashReporter: {:?}", e);
            None
        }
    };

    let annotations = build_annotations(&lockups, &rcu_stalls);
    let sorted_threads = order_lockup_candidates(lockups.newly_locked, &rcu_stalls);
    if !sorted_threads.is_empty() {
        log_info!(
            "Dumping backtraces for {} newly locked threads (out of {} total locked) in order of oldest first...",
            sorted_threads.len(),
            lockups.current_koids.len()
        );
    }
    for (thread, koid, start_time) in sorted_threads {
        let thread_name = thread
            .get_name()
            .map(|name| name.to_string())
            .unwrap_or_else(|_| "<unknown>".to_string());
        let duration = zx::MonotonicInstant::get() - start_time;

        let bt = match dump_thread_backtrace(&thread, &mut file_buffer, THREAD_DUMP_TIMEOUT).await {
            Ok(bt) => bt,
            Err(e) => {
                let thread_state = thread
                    .info()
                    .map(|info| format!("{:?}", info.state))
                    .unwrap_or_else(|_| "unknown".to_string());
                log_warn!(
                    "Failed to dump backtrace for thread {} ('{}', state: {}, running for {:.2?}): {:?}",
                    koid.raw_koid(),
                    thread_name,
                    thread_state,
                    duration,
                    e
                );
                continue;
            }
        };
        log_error!(
            "Locked thread backtrace for {} (koid: {}, running for {:.2?}):\n{}",
            thread_name,
            koid.raw_koid(),
            duration,
            bt
        );

        context.reported_lockup_koids.insert(koid);

        let Some(reporter) = &reporter else {
            continue;
        };

        let size = bt.len() as u64;
        let vmo = match zx::Vmo::create(size) {
            Ok(vmo) => vmo,
            Err(e) => {
                log_warn!("Failed to create VMO for thread {}: {:?}", koid.raw_koid(), e);
                continue;
            }
        };
        if let Err(e) = vmo.write(bt.as_bytes(), 0) {
            log_warn!("Failed to write backtrace to VMO for thread {}: {:?}", koid.raw_koid(), e);
            continue;
        }

        let report = ffeedback::CrashReport {
            program_name: Some("starnix_kernel".to_string()),
            crash_signature: Some("fuchsia-starnix_kernel-thread-lockup".to_string()),
            is_fatal: Some(false),
            specific_report: Some(ffeedback::SpecificCrashReport::TextBacktrace(
                ffeedback::TextBacktraceCrashReport {
                    fuchsia_backtrace: Some(fmem::Buffer { vmo, size }),
                    thread_name: Some(thread_name),
                    thread_koid: Some(koid.raw_koid()),
                    ..Default::default()
                },
            )),
            annotations: Some(annotations.clone()),
            event_id: Some(event_id_str.clone()),
            ..Default::default()
        };

        match reporter.file_report(report).await {
            Ok(Ok(_)) => {
                log_debug!("Filed crash report for thread lockup (thread {}).", koid.raw_koid());
            }
            Ok(Err(e)) => {
                log_warn!("Failed to file crash report for thread {}: {:?}", koid.raw_koid(), e);
            }
            Err(e) => {
                log_warn!("FIDL error calling file_report for thread {}: {:?}", koid.raw_koid(), e);
            }
        }
    }

    Ok(())
}

fn check_lockups(context: &mut LockupDetectorContext) -> Lockups {
    let long_running = starnix_core::task::ThreadLockupDetector::get_long_running_threads(
        zx::MonotonicDuration::from_minutes(LOCKUP_DETECTOR_INTERVAL_MINUTES),
    );

    let current_koids: BTreeSet<zx::Koid> = long_running.iter().map(|r| r.koid).collect();

    // Clean up threads that are no longer locked up.
    context.reported_lockup_koids.retain(|koid| current_koids.contains(koid));

    if long_running.is_empty() {
        return Lockups::default();
    }

    // Identify newly locked threads.
    let newly_locked: Vec<_> = long_running
        .iter()
        .filter(|r| !context.reported_lockup_koids.contains(&r.koid))
        .cloned()
        .collect();

    if newly_locked.is_empty() {
        return Lockups::default();
    }

    // LINT.IfChange(starnix_lockup_detector)
    log_error!(
        "Detected threads locked up for more than {} minutes: {:?}",
        LOCKUP_DETECTOR_INTERVAL_MINUTES,
        current_koids
    );
    // LINT.ThenChange(//tools/testing/tefmocheck/string_in_log_check.go:starnix_lockup_detector)
    Lockups { long_running, current_koids, newly_locked }
}

fn check_rcu_stalls(context: &mut LockupDetectorContext) -> Vec<RcuStall> {
    let now = zx::MonotonicInstant::get();
    let mut active_koids = std::collections::HashSet::new();
    let mut stalled_threads = vec![];

    starnix_core::task::ThreadLockupDetector::active_rcu_read_locks(
        |thread, koid, counter_index| {
            active_koids.insert(koid);
            let stall_info = context.active_rcu_reads.get(&koid);
            let (count, first_seen) = match stall_info {
                Some(info) => {
                    if info.last_counter_index != counter_index {
                        // Counter index changed, progress was made.
                        (1, now)
                    } else {
                        (info.consecutive_polls_active + 1, info.first_seen)
                    }
                }
                None => (1, now),
            };

            context.active_rcu_reads.insert(
                koid,
                ActiveRcuRead {
                    consecutive_polls_active: count,
                    first_seen,
                    last_counter_index: counter_index,
                },
            );

            if count >= RCU_STALL_THRESHOLD_SAMPLES
                && !context.reported_rcu_koids.contains(&koid)
                && let Ok(thread_dup) = thread.duplicate_handle(zx::Rights::SAME_RIGHTS)
            {
                context.reported_rcu_koids.insert(koid);
                stalled_threads.push(RcuStall { thread: thread_dup, koid, first_seen });
            }
        },
    );

    context.active_rcu_reads.retain(|koid, _| active_koids.contains(koid));
    context.reported_rcu_koids.retain(|koid| active_koids.contains(koid));

    for stall in &stalled_threads {
        log_warn!(
            "RCU Stall detected: Thread {} has held RCU read lock for {}ms.",
            stall.koid.raw_koid(),
            (zx::MonotonicInstant::get() - stall.first_seen).into_millis(),
        );
    }

    stalled_threads
}

pub fn start_thread_lockup_detector() -> fasync::Task<()> {
    fasync::Task::spawn(async {
        let mut context = LockupDetectorContext::default();
        loop {
            fasync::Timer::new(zx::MonotonicInstant::after(zx::MonotonicDuration::from_minutes(
                LOCKUP_DETECTOR_INTERVAL_MINUTES,
            )))
            .await;

            let _waiting_guard = starnix_core::task::ThreadLockupDetector::pause_tracking();

            let rcu_stalls = check_rcu_stalls(&mut context);
            let lockups = check_lockups(&mut context);

            if let Err(e) = report_lockups(&mut context, lockups, rcu_stalls).await {
                log_warn!("Error in thread lockup detector: {:?}", e);
            }
        }
    })
}

/// Creates a lazy inspect node that exports information about currently locked threads.
///
/// When the node is read, it queries the `ThreadLockupDetector` for all threads that have been
/// running longer than the `LOCKUP_DETECTOR_INTERVAL_MINUTES` threshold and records their KOIDs
/// and names as properties of the node.
pub fn inspect_lazy_node_callback() -> BoxFuture<'static, Result<Inspector, anyhow::Error>> {
    Box::pin(async {
        let inspector = Inspector::default();
        let long_running = starnix_core::task::ThreadLockupDetector::get_long_running_threads(
            zx::MonotonicDuration::from_minutes(LOCKUP_DETECTOR_INTERVAL_MINUTES),
        );
        for info in long_running {
            let name = info
                .thread
                .get_name()
                .map(|n| n.to_string())
                .unwrap_or_else(|_| "unknown".to_string());
            inspector.root().record_string(info.koid.raw_koid().to_string(), name);
        }
        Ok(inspector)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[fuchsia::test]
    async fn test_rcu_lockup_detector() {
        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();

        // Spawn a thread that holds an RCU read lock.
        let thread = std::thread::spawn(move || {
            let _guard = starnix_core::task::ThreadLockupDetector::track();
            let _scope = fuchsia_rcu::RcuReadScope::new();
            barrier_clone.wait(); // Synchronize with the main test thread.
            barrier_clone.wait(); // Block until test finishes.
        });

        barrier.wait(); // Wait for thread to acquire lock.

        let mut context = LockupDetectorContext::default();

        // Run check_rcu_stalls.
        // We need to run it multiple times to trigger the stall.
        // Threshold is RCU_STALL_THRESHOLD_SAMPLES = 4.

        // 1st sample
        let candidates = check_rcu_stalls(&mut context);
        assert!(candidates.is_empty());
        assert_eq!(context.active_rcu_reads.len(), 1);
        let koid = *context.active_rcu_reads.keys().next().unwrap();
        assert_eq!(context.active_rcu_reads.get(&koid).unwrap().consecutive_polls_active, 1);

        // 2nd and 3rd samples
        for i in 2..=3 {
            let candidates = check_rcu_stalls(&mut context);
            assert!(candidates.is_empty());
            assert_eq!(context.active_rcu_reads.get(&koid).unwrap().consecutive_polls_active, i);
        }

        // 4th sample (should return candidate)
        let candidates = check_rcu_stalls(&mut context);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].koid, koid);
        assert_eq!(context.active_rcu_reads.get(&koid).unwrap().consecutive_polls_active, 4);

        barrier.wait(); // Allow thread to exit.
        thread.join().unwrap();

        // Run check again, thread should be gone.
        let candidates = check_rcu_stalls(&mut context);
        assert!(candidates.is_empty());
        assert!(context.active_rcu_reads.is_empty());
    }

    #[test]
    fn test_truncate_annotation_value() {
        let max_len = ffeedback::MAX_ANNOTATION_VALUE_LENGTH as usize;

        // Under limit: unaffected.
        let short_str = "hello world".to_string();
        assert_eq!(truncate_annotation_value(short_str.clone()), short_str);

        // Exactly at limit: unaffected.
        let exact_str = "a".repeat(max_len);
        assert_eq!(truncate_annotation_value(exact_str.clone()), exact_str);

        // Over limit by 1 byte: truncated to max_len and ends with "...".
        let over_by_one = "a".repeat(max_len + 1);
        let truncated = truncate_annotation_value(over_by_one);
        assert_eq!(truncated.len(), max_len);
        assert!(truncated.ends_with("..."));

        // Large string: truncated to max_len and ends with "...".
        let large_str = "a".repeat(5000);
        let truncated = truncate_annotation_value(large_str);
        assert_eq!(truncated.len(), max_len);
        assert!(truncated.ends_with("..."));

        // Multi-byte characters: characters do not get split across UTF-8 boundaries.
        // The character '🦀' is 4 bytes.
        let crab_str = "🦀".repeat(500); // 2000 bytes
        let truncated = truncate_annotation_value(crab_str);
        assert!(truncated.len() <= max_len);
        assert!(truncated.ends_with("..."));
    }

    #[test]
    fn test_build_annotations_truncation() {
        let max_len = ffeedback::MAX_ANNOTATION_VALUE_LENGTH as usize;

        // Populate hundreds of KOIDs so that the formatted string exceeds 1024 bytes.
        let mut current_koids = BTreeSet::new();
        for i in 10000..10500 {
            current_koids.insert(zx::Koid::from_raw(i));
        }

        let lockups = Lockups { current_koids, long_running: vec![], newly_locked: vec![] };
        let rcu_stalls = vec![];

        let annotations = build_annotations(&lockups, &rcu_stalls);
        for annotation in annotations {
            assert!(
                annotation.value.len() <= max_len,
                "Annotation '{}' length {} exceeds max {}",
                annotation.key,
                annotation.value.len(),
                max_len
            );
            if annotation.key == "starnix.lockup_thread_koids" {
                assert!(annotation.value.ends_with("..."));
            }
        }
    }

    #[test]
    fn test_order_lockup_candidates() {
        let t0 = zx::MonotonicInstant::from_nanos(1000);
        let t1 = zx::MonotonicInstant::from_nanos(2000);
        let t2 = zx::MonotonicInstant::from_nanos(3000);

        // SAFETY: Handle from zx_thread_self is valid for the duration of the test.
        let thread_self =
            unsafe { zx::Unowned::<zx::Thread>::from_raw_handle(fruntime::zx_thread_self()) };

        let newly_locked = vec![
            ThreadLockupInfo {
                thread: thread_self.clone(),
                koid: zx::Koid::from_raw(2),
                start_time: t1,
            },
            ThreadLockupInfo {
                thread: thread_self.clone(),
                koid: zx::Koid::from_raw(3),
                start_time: t2,
            },
            ThreadLockupInfo {
                thread: thread_self.clone(),
                koid: zx::Koid::from_raw(1),
                start_time: t0,
            },
        ];

        let rcu_stalls = vec![];

        let sorted = order_lockup_candidates(newly_locked, &rcu_stalls);
        assert_eq!(sorted.len(), 3);
        assert_eq!(sorted[0].1, zx::Koid::from_raw(1));
        assert_eq!(sorted[0].2, t0);
        assert_eq!(sorted[1].1, zx::Koid::from_raw(2));
        assert_eq!(sorted[1].2, t1);
        assert_eq!(sorted[2].1, zx::Koid::from_raw(3));
        assert_eq!(sorted[2].2, t2);
    }
}
