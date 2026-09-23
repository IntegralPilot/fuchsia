// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ioctl::WakeupTestType;
use fuchsia_trace::Scope;
use starnix_core::perf::{TraceEvent, TraceEventQueueList};
use starnix_logging::{log_error, log_info};
use std::sync::Arc;
pub(crate) static POWER_CATEGORY: &str = "power";

pub(crate) fn trace_wakeup_test_type(
    trace_event_queues: Option<Arc<TraceEventQueueList>>,
    current_tid: i32,
    test_type: WakeupTestType,
) {
    fuchsia_trace::instant!(
        POWER_CATEGORY,
        "WakeupTest:TestStart",
        Scope::Process,
        "type" =><WakeupTestType as Into<&'static str>>::into(test_type)
    );
    // TODO(479889862): Support custom tracepoints for these events.
    if let Some(trace_event_queues) = trace_event_queues {
        if trace_event_queues.is_enabled() {
            let evt = format!("I|{current_tid}|wakeup_test_type:{:?}\n", test_type);
            let data = evt.as_bytes();
            let event = TraceEvent::new(current_tid, data.len());
            // Use the first queue, we're only writing a couple events so it should not affect the balance
            // between the other queues.
            if let Err(e) = trace_event_queues.queues[0].push_event(event, data) {
                log_error!("failed to push trace event: {e:?}");
            }
        } else {
            log_info!("Trace event queue is not enabled, skipping wakeup set timer trace event");
        }
    }
}
