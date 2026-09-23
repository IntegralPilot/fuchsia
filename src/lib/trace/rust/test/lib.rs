// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use fuchsia_trace as trace;
use fuchsia_trace::TraceFutureExt;
use fuchsia_trace_observer as trace_observer;
use std::future::{Future, poll_fn};
use std::sync::{Barrier, Condvar, Mutex};
use std::task::Poll;

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_enabled() -> bool {
    return trace::is_enabled();
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_category_disabled_cstr() -> bool {
    return trace::category_enabled(c"-disabled");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_category_disabled_str() -> bool {
    return trace::category_enabled("-disabled");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_category_enabled_cstr() -> bool {
    return trace::category_enabled(c"+enabled");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_category_enabled_str() -> bool {
    return trace::category_enabled("+enabled");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_counter_macro_cstr() {
    trace::counter!(c"+enabled", c"name", 42, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_counter_macro_str() {
    trace::counter!("+enabled", "name", 42, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_counter_macro_str_and_string() {
    trace::counter!("+enabled", "name".to_string(), 42, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_instant_macro_cstr() {
    trace::instant!(c"+enabled", c"name", trace::Scope::Process, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_instant_macro_str() {
    trace::instant!("+enabled", "name", trace::Scope::Process, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_instant_macro_str_and_string() {
    trace::instant!("+enabled", "name".to_string(), trace::Scope::Process, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_macro_cstr() {
    trace::duration!(c"+enabled", c"name", "x" => 5, "y" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_macro_str() {
    trace::duration!("+enabled", "name", "x" => 5, "y" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_macro_str_and_string() {
    trace::duration!("+enabled", "name".to_string(), "x" => 5, "y" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_macro_with_scope() {
    // N.B. The ordering here is intentional. The duration! macro emits a trace
    // event when the scoped object is dropped. From an output perspective,
    // that means we are looking to see that the instant event occurs first.
    trace::duration!(c"+enabled", c"name", "x" => 5, "y" => 10);
    trace::instant!(c"+enabled", c"name", trace::Scope::Process, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_begin_end_macros_cstr() {
    trace::duration_begin!(c"+enabled", c"name", "x" => 5);
    trace::instant!(c"+enabled", c"name", trace::Scope::Process, "arg" => 10);
    trace::duration_end!(c"+enabled", c"name", "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_begin_end_macros_str() {
    trace::duration_begin!("+enabled", "name", "x" => 5);
    trace::instant!("+enabled", "name", trace::Scope::Process, "arg" => 10);
    trace::duration_end!("+enabled", "name", "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_duration_begin_end_macros_str_and_string() {
    let name = "name".to_string();
    trace::duration_begin!("+enabled", &name, "x" => 5);
    trace::instant!("+enabled", &name, trace::Scope::Process, "arg" => 10);
    trace::duration_end!("+enabled", &name, "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_vthread_duration_begin_end_macros() {
    trace::vthread_duration_begin!(c"+enabled", c"name", "vthread_name", 12u64, "x" => 5);
    trace::instant!(c"+enabled", c"name", trace::Scope::Process, "arg" => 10);
    trace::vthread_duration_end!(c"+enabled", c"name", "vthread_name", 12u64, "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_blob_macro_cstr() {
    trace::blob!(c"+enabled", c"name", "blob contents".as_bytes().to_vec().as_slice(), "x" => 5);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_blob_macro_str() {
    trace::blob!("+enabled", "name", "blob contents".as_bytes().to_vec().as_slice(), "x" => 5);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_blob_macro_str_and_string() {
    trace::blob!("+enabled", "name".to_string(), "blob contents".as_bytes().to_vec().as_slice(), "x" => 5);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_flow_begin_step_end_macros_cstr() {
    trace::flow_begin!(c"+enabled", c"name", 123.into(), "x" => 5);
    trace::flow_step!(c"+enabled", c"step", 123.into(), "z" => 42);
    trace::flow_end!(c"+enabled", c"name", 123.into(), "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_flow_begin_step_end_macros_str() {
    trace::flow_begin!("+enabled", "name", 123.into(), "x" => 5);
    trace::flow_step!("+enabled", "step", 123.into(), "z" => 42);
    trace::flow_end!("+enabled", "name", 123.into(), "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_flow_begin_step_end_macros_str_and_string() {
    let name = "name".to_string();
    trace::flow_begin!("+enabled", &name, 123.into(), "x" => 5);
    trace::flow_step!("+enabled", "step".to_string(), 123.into(), "z" => 42);
    trace::flow_end!("+enabled", &name, 123.into(), "y" => "foo");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_arglimit() {
    trace::duration!(c"+enabled", c"name",
        "1" => 1,
        "2" => 2,
        "3" => 3,
        "4" => 4,
        "5" => 5,
        "6" => 6,
        "7" => 7,
        "8" => 8,
        "9" => 9,
        "10" => 10,
        "11" => 11,
        "12" => 12,
        "13" => 13,
        "14" => 14,
        "15" => 15
    );
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_async_event_with_scope() {
    // N.B. The ordering here is intentional. The async_enter! macro emits a trace event when the
    // scoped object is instantiated and when it is dropped. From an output perspective, that means
    // we are looking to see that the instant event occurs sandwiched between the two.
    let _guard = trace::async_enter!(1.into(), c"+enabled", c"name", "x" => 5, "y" => 10);
    trace::instant!(c"+enabled", c"name", trace::Scope::Process, "arg" => 10);
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_alert_cstr() {
    trace::alert!(c"+enabled", c"alert_name");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_alert_str() {
    trace::alert!("+enabled", "alert_name");
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_alert_str_and_string() {
    let name = "alert_name".to_string();
    trace::alert!("+enabled", &name);
}

fn run_future(fut: impl Future<Output = ()>) {
    fasync::TestExecutor::new().run_singlethreaded(fut);
}

fn multi_poll_future(mut ready_after: u64) -> impl Future<Output = ()> {
    poll_fn(move |cx| {
        if ready_after > 0 {
            ready_after -= 1;
            cx.waker().clone().wake();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_future_enabled_cstr() {
    // The future is immediately ready. There will be a single duration event around the poll call.
    // No flow events will be generated.
    run_future(multi_poll_future(0).trace(trace::trace_future_args!(
        c"+enabled",
        c"name",
        3.into()
    )));

    // The future will return pending once followed by ready. There will be 2 duration events. The
    // 1st duration will contain a flow begin, and the 2nd duration will contain a flow end.
    run_future(multi_poll_future(1).trace(trace::trace_future_args!(
        c"+enabled",
        c"name",
        4.into()
    )));

    // The future will return pending twice followed by ready. There will be 3 duration events. The
    // 1st duration will contain a flow begin, the 2nd duration will contain a flow step, and the
    // 3rd duration will contain a flow end.
    run_future(multi_poll_future(2).trace(trace::trace_future_args!(
        c"+enabled",
        c"name",
        5.into()
    )));
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_future_enabled_str() {
    // The future is immediately ready. There will be a single duration event around the poll call.
    // No flow events will be generated.
    run_future(multi_poll_future(0).trace(trace::trace_future_args!("+enabled", "name", 3.into())));

    // The future will return pending once followed by ready. There will be 2 duration events. The
    // 1st duration will contain a flow begin, and the 2nd duration will contain a flow end.
    run_future(multi_poll_future(1).trace(trace::trace_future_args!("+enabled", "name", 4.into())));

    // The future will return pending twice followed by ready. There will be 3 duration events. The
    // 1st duration will contain a flow begin, the 2nd duration will contain a flow step, and the
    // 3rd duration will contain a flow end.
    run_future(multi_poll_future(2).trace(trace::trace_future_args!("+enabled", "name", 5.into())));
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_future_enabled_str_and_string() {
    let name = "name".to_string();

    // The future is immediately ready. There will be a single duration event around the poll call.
    // No flow events will be generated.
    run_future(multi_poll_future(0).trace(trace::trace_future_args!("+enabled", &name, 3.into())));

    // The future will return pending once followed by ready. There will be 2 duration events. The
    // 1st duration will contain a flow begin, and the 2nd duration will contain a flow end.
    run_future(multi_poll_future(1).trace(trace::trace_future_args!("+enabled", &name, 4.into())));

    // The future will return pending twice followed by ready. There will be 3 duration events. The
    // 1st duration will contain a flow begin, the 2nd duration will contain a flow step, and the
    // 3rd duration will contain a flow end.
    run_future(multi_poll_future(2).trace(trace::trace_future_args!("+enabled", &name, 5.into())));
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_future_enabled_with_arg() {
    run_future(multi_poll_future(1).trace(trace::trace_future_args!(
        c"+enabled",
        c"name",
        3.into(),
        "arg" => 10,
    )));
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_future_disabled() {
    run_future(multi_poll_future(1).trace(trace::trace_future_args!(
        c"-disabled",
        c"name",
        3.into(),
    )));
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_trace_future_disabled_with_arg() {
    #[allow(unreachable_code)]
    run_future(multi_poll_future(1).trace(trace::trace_future_args!(
        c"-disabled",
        c"name",
        3.into(),
        "arg" => {
            panic!("arg should not be evaluated");
            ()
        }
    )));
}

static STATE_CV: Condvar = Condvar::new();
static TRACE_STATE: Mutex<fuchsia_trace::TraceState> =
    Mutex::new(fuchsia_trace::TraceState::Stopped);
#[unsafe(no_mangle)]
pub extern "C" fn rs_check_trace_state() -> u32 {
    *TRACE_STATE.lock().unwrap() as u32
}
#[unsafe(no_mangle)]
pub extern "C" fn rs_wait_trace_state_is(expected: u32) {
    let mut state = TRACE_STATE.lock().unwrap();
    while *state as u32 != expected {
        state = STATE_CV.wait(state).unwrap();
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_setup_trace_observer() {
    static BARRIER: Barrier = Barrier::new(2);
    std::thread::spawn(|| {
        let mut executor = fasync::LocalExecutor::default();
        executor.run_singlethreaded(async move {
            let observer = trace_observer::TraceObserver::new();
            BARRIER.wait();
            while let Ok(new_state) = observer.on_state_changed().await {
                let mut state = TRACE_STATE.lock().unwrap();
                *state = new_state;
                STATE_CV.notify_all();
            }
        });
    });
    // Ensure that we've registered our trace_observer in the spawned thread before continuing on
    // and potentially starting the trace session.
    BARRIER.wait();
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_custom_track_and_async_events() {
    // 1. Existing Async Events
    let async_id = 42.into();
    trace::async_instant!(async_id, c"+enabled", c"async_instant_name", "x" => 1);
    {
        let _guard = trace::async_enter!(async_id, c"+enabled", c"async_duration_name", "y" => 2);
        trace::instant!(c"+enabled", c"instant_in_between", trace::Scope::Process, "arg" => 10);
    }

    // 2. New Custom Track Events
    let track = trace::Track::new("my_custom_track");
    trace::track_instant!(c"+enabled", track, c"track_instant_name", "a" => 10);

    // Manual begin/end
    trace::track_duration_begin!(c"+enabled", track, c"track_manual_duration", "b" => 20);
    trace::instant!(c"+enabled", c"instant_in_between_track", trace::Scope::Process, "arg" => 20);
    trace::track_duration_end!(c"+enabled", track, c"track_manual_duration", "c" => 30);

    // RAII Scoped duration
    {
        trace::track_duration!(c"+enabled", track, c"track_scoped_duration", "d" => 40);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn rs_test_custom_track_concurrent_usage() {
    let track = trace::Track::new("concurrent_custom_track");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));

    let mut handles = vec![];
    for i in 0..4 {
        let track = track.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for j in 0..50 {
                trace::track_duration!(c"+enabled", track, c"concurrent_duration", "thread" => i, "iter" => j);
                trace::track_instant!(c"+enabled", track, c"concurrent_instant", "thread" => i);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }
}
