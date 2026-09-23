// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::signals::SignalInfo;
use crate::task::CurrentTask;
use crash_throttling::{CrashThrottler, PendingCrashReport};
use fidl_fuchsia_feedback::{
    Annotation, CrashReport, CrashReporterProxy, MAX_ANNOTATION_VALUE_LENGTH,
    MAX_CRASH_SIGNATURE_LENGTH, NativeCrashReport, SpecificCrashReport,
};
use fuchsia_inspect::Node;
use starnix_logging::{
    CATEGORY_STARNIX, CoreDumpInfo, CoreDumpList, log_error, log_info, log_warn,
};

pub struct CrashReporter {
    /// Diagnostics information about crashed tasks.
    core_dumps: CoreDumpList,

    /// Throttles crash reports to avoid spamming the system.
    throttler: CrashThrottler,

    /// Connection to the feedback stack for reporting crashes.
    proxy: Option<CrashReporterProxy>,
}

impl CrashReporter {
    pub fn new(
        inspect_node: &Node,
        proxy: Option<CrashReporterProxy>,
        crash_loop_age_out: zx::MonotonicDuration,
        enable_throttling: bool,
    ) -> Self {
        Self {
            core_dumps: CoreDumpList::new(inspect_node.create_child("coredumps")),
            throttler: CrashThrottler::new(inspect_node, crash_loop_age_out, enable_throttling),
            proxy,
        }
    }

    /// Returns a PendingCrashReport if the crash report should be reported. Otherwise, returns
    /// None.
    pub fn begin_crash_report(&self, current_task: &CurrentTask) -> Option<PendingCrashReport> {
        let argv = current_task
            .read_argv(MAX_ANNOTATION_VALUE_LENGTH as usize)
            .unwrap_or_else(|_| vec!["<unknown>".into()])
            .into_iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>();
        let argv0 = argv.get(0).map(AsRef::as_ref).unwrap_or_else(|| "<unknown>");

        // Get the filename.
        let argv0 = argv0.rsplit_once("/").unwrap_or(("", &argv0)).1.to_string();

        self.throttler.should_report(argv, argv0, zx::MonotonicInstant::get())
    }

    /// Callers should first check whether the crash should be reported via begin_crash_report.
    pub fn handle_core_dump(
        &self,
        current_task: &CurrentTask,
        signal_info: &SignalInfo,
        pending_crash_report: PendingCrashReport,
    ) {
        fuchsia_trace::instant!(CATEGORY_STARNIX, "RecordCoreDump", fuchsia_trace::Scope::Process);

        let argv = pending_crash_report.argv;
        let argv0 = pending_crash_report.argv0;
        let process_koid = current_task
            .thread_group()
            .process
            .koid()
            .expect("handles for processes with crashing threads are still valid");
        let thread_koid = current_task
            .running_state()
            .thread
            .get()
            .expect("handles for crashing threads are still valid")
            .koid;
        let linux_pid = current_task.pid.id as i64;
        let thread_name = current_task.command().to_string();

        // TODO(https://fxbug.dev/356912301) use boot time
        let uptime = zx::MonotonicInstant::get() - current_task.thread_group().start_time;

        let dump_info = CoreDumpInfo {
            process_koid,
            thread_koid,
            linux_pid,
            uptime: uptime.into_nanos(),
            argv: argv.clone(),
            thread_name: thread_name.clone(),
            signal: signal_info.signal.to_string(),
        };
        self.core_dumps.record_core_dump(dump_info);

        let mut argv_joined = argv.join(" ");
        truncate_with_ellipsis(&mut argv_joined, MAX_ANNOTATION_VALUE_LENGTH as usize);

        let mut env_joined = current_task
            .read_env(MAX_ANNOTATION_VALUE_LENGTH as usize)
            .unwrap_or_else(|_| vec![])
            .into_iter()
            .map(|a| a.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        truncate_with_ellipsis(&mut env_joined, MAX_ANNOTATION_VALUE_LENGTH as usize);

        let signal_str = signal_info.signal.to_string();

        // Truncate program name to fit in crash signature with a space and signal string added.
        let max_signature_prefix_len = MAX_CRASH_SIGNATURE_LENGTH as usize - (signal_str.len() + 1);
        let mut crash_signature = argv0.clone();
        truncate_with_ellipsis(&mut crash_signature, max_signature_prefix_len);
        crash_signature.push(' ');
        crash_signature.push_str(&signal_str);

        let crash_report = CrashReport {
            crash_signature: Some(crash_signature),
            program_name: Some(argv0.clone()),
            program_uptime: Some(uptime.into_nanos()),
            specific_report: Some(SpecificCrashReport::Native(NativeCrashReport {
                process_koid: Some(process_koid.raw_koid()),
                process_name: Some(argv0),
                thread_koid: Some(thread_koid.raw_koid()),
                thread_name: Some(thread_name),
                ..Default::default()
            })),
            annotations: Some(vec![
                // Note that this pid will be different from the Zircon process koid that's visible
                // to the rest of Fuchsia. We want to include both so that this can be correlated
                // against debugging artifacts produced by Android code.
                Annotation { key: "linux.pid".to_string(), value: linux_pid.to_string() },
                Annotation { key: "linux.argv".to_string(), value: argv_joined },
                Annotation { key: "linux.env".to_string(), value: env_joined },
                Annotation { key: "linux.signal".to_string(), value: signal_str },
            ]),
            is_fatal: Some(true),
            weight: Some(pending_crash_report.weight),
            ..Default::default()
        };

        if let Some(reporter) = &self.proxy {
            let reporter = reporter.clone();
            // Do the actual report in the background since they can take a while to file.
            current_task.kernel().kthreads.spawn_future(
                move || async move {
                    match reporter.file_report(crash_report).await {
                        Ok(Ok(_)) => (),
                        Ok(Err(filing_error)) => {
                            log_error!(filing_error:?; "Couldn't file crash report.");
                        }
                        Err(fidl_error) => log_warn!(
                            fidl_error:?;
                            "Couldn't file crash report due to error on underlying channel."
                        ),
                    };
                },
                "crash-filing",
            );
        } else {
            log_info!(crash_report:?; "no crash reporter available for crash");
        }
    }
}

fn truncate_with_ellipsis(s: &mut String, max_len: usize) {
    if s.len() <= max_len {
        return;
    }

    // 3 bytes for ellipsis.
    let max_content_len = max_len - 3;

    // String::truncate panics if the new max length is in the middle of a character, so we need to
    // find an appropriate byte boundary.
    let mut new_len = 0;
    let mut iter = s.char_indices();
    while let Some((offset, _)) = iter.next() {
        if offset > max_content_len {
            break;
        }
        new_len = offset;
    }

    s.truncate(new_len);
    s.push_str("...");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_noop_on_max_length_string() {
        let mut s = String::from("1234567890");
        let before = s.clone();
        truncate_with_ellipsis(&mut s, 10);
        assert_eq!(s, before);
    }

    #[test]
    fn truncate_adds_ellipsis() {
        let mut s = String::from("1234567890");
        truncate_with_ellipsis(&mut s, 9);
        assert_eq!(s.len(), 9);
        assert_eq!(s, "123456...", "truncate must add ellipsis and still fit under max len");
    }

    #[test]
    fn truncate_is_sensible_in_middle_of_multibyte_chars() {
        let mut s = String::from("æææææææææ");
        // æ is 2 bytes, so any odd byte length should be in the middle of a character. Truncate
        // adds 3 bytes for the ellipsis so we actually need an even max length to hit the middle
        // of a character.
        truncate_with_ellipsis(&mut s, 8);
        assert_eq!(s.len(), 7, "may end up shorter than provided max length w/ multi-byte chars");
        assert_eq!(s, "ææ...", "truncate must remove whole characters and add ellipsis");
    }
}
