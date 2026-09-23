// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::Socket;
use fidl_fuchsia_component::{
    CreateChildArgs, ExecutionControllerEvent, RealmMarker, StartChildArgs, StoppedPayload,
};
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_tracing_controller::{
    ProvisionerMarker as TracingProvisionerMarker, SessionMarker as TracingSessionMarker,
    SessionProxy as TracingSessionProxy, StartOptions, TraceConfig,
};
use fuchsia_async::{Socket as AsyncSocket, Task};
use fuchsia_component::client::connect_to_protocol;
use futures::StreamExt;
use log::info;
use std::collections::BTreeMap;
use std::path::Path;

const COLLECTION_NAME: &str = "dynamic";
const CHILD_NAME: &str = "provider_puppet";
const CPP_PUPPET_SUBPATH: &str = "meta/provider_puppet_cpp.cm";
const RUST_PUPPET_SUBPATH: &str = "meta/provider_puppet_rust.cm";
const CPP_PUPPET_PROVIDER_NAME: &str = "provider_puppet_cpp.cm";
const RUST_PUPPET_PROVIDER_NAME: &str = "provider_puppet_rust.cm";
const TRACE_CATEGORIES: &[&str] = &["test_puppet"];

fn slash_pkg_contains(subpath: &str) -> bool {
    Path::new("/pkg").join(subpath).exists()
}

#[fuchsia::main]
async fn main() {
    let (puppet_subpath, provider_name) = if slash_pkg_contains(RUST_PUPPET_SUBPATH) {
        (RUST_PUPPET_SUBPATH, RUST_PUPPET_PROVIDER_NAME)
    } else if slash_pkg_contains(CPP_PUPPET_SUBPATH) {
        (CPP_PUPPET_SUBPATH, CPP_PUPPET_PROVIDER_NAME)
    } else {
        panic!("must be packaged with a recognized puppet url");
    };

    let min_timestamp = zx::MonotonicInstant::get().into_nanos();
    let trace_session = TraceSession::start().await;
    run_puppet(format!("#{puppet_subpath}")).await;
    let (records, warnings) = trace_session.terminate().await;
    assert_eq!(warnings, [], "should not see any warnings from parsing puppets' traces");
    let max_timestamp = zx::MonotonicInstant::get().into_nanos();

    let mut per_process = BTreeMap::<fxt::ProcessKoid, PerProcessRecords>::default();
    for record in records {
        if let Some(p) = record.process() {
            let this_process = per_process.entry(p).or_default();
            if let Some(t) = record.thread() {
                this_process.per_thread.entry(t).or_default().push(record);
            } else {
                this_process.no_thread.push(record);
            }
        } else {
            panic!("hermetic trace manager should only have events w/ a process, got {record:?}");
        }
    }

    // Now that we have all the trace records from our puppet, go through them in the order of
    // the puppet's source.
    assert_eq!(per_process.len(), 1, "should capture trace records for exactly one process");
    let (puppet_process_koid, puppet_process) = per_process.into_iter().next().unwrap();

    // The test controls the test manager instance so the puppet will be the first provider, and
    // the puppet gets the default provider name.
    let expected_provider = Some(fxt::Provider { id: 1, name: provider_name.into() });

    // koids are incremented monotonically and we store our threads in a BTreeMap, so our iteration
    // order matches thread creation order.
    let mut threads = puppet_process.per_thread.into_iter();

    let (initial_thread_koid, initial_thread) = threads.next().unwrap();
    let mut initial_thread = initial_thread.into_iter();

    // instant!(c"test_puppet", c"puppet_instant", Scope::Thread);
    let instant = initial_thread.next().unwrap();
    let mut prev_timestamp = min_timestamp;
    let mut current_timestamp = record_timestamp(&instant).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        instant,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_instant".into(),
            args: vec![],
            payload: fxt::EventPayload::Instant,
        })
    );

    // counter!(c"test_puppet", c"puppet_counter", 0, "somedataseries" => 1);
    let counter = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&counter).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        counter,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_counter".into(),
            args: vec![fxt::Arg {
                name: "somedataseries".into(),
                value: fxt::ArgValue::Signed32(1)
            }],
            payload: fxt::EventPayload::Counter { id: 0 },
        })
    );

    // counter!(c"test_puppet", c"puppet_counter2", 1, "someotherdataseries" => u64::MAX - 1);
    let second_counter = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&second_counter).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        second_counter,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_counter2".into(),
            args: vec![fxt::Arg {
                name: "someotherdataseries".into(),
                value: fxt::ArgValue::Unsigned64(u64::MAX - 1),
            }],
            payload: fxt::EventPayload::Counter { id: 1 },
        })
    );

    // duration_begin!("test_puppet", "puppet_duration");
    let duration_begin = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&duration_begin).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        duration_begin,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_duration".into(),
            args: vec![],
            payload: fxt::EventPayload::DurationBegin,
        })
    );

    // duration_end!("test_puppet", "puppet_duration");
    let duration_end = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&duration_end).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        duration_end,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_duration".into(),
            args: vec![],
            payload: fxt::EventPayload::DurationEnd,
        })
    );

    // puppet_duration_raii starts here.
    let before_duration_complete_start = current_timestamp;

    // async_begin(async_id, c"test_puppet", c"puppet_async", &[]);
    let async_begin = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&async_begin).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    let expected_async_id = match &async_begin {
        fxt::TraceRecord::Event(fxt::EventRecord {
            payload: fxt::EventPayload::AsyncBegin { id },
            ..
        }) => *id,
        _ => panic!("expected AsyncBegin event, got {:?}", async_begin),
    };
    assert_eq!(
        async_begin,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_async".into(),
            args: vec![],
            payload: fxt::EventPayload::AsyncBegin { id: expected_async_id },
        })
    );

    // std::thread::spawn(move || {
    let (instant_thread_koid, instant_thread) = threads.next().unwrap();
    let mut instant_thread = instant_thread.into_iter();

    // async_instant(async_id, c"test_puppet", c"puppet_async_instant1", &[]);
    let nested_instant_begin = instant_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&nested_instant_begin).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        nested_instant_begin,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: instant_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_async_instant1".into(),
            args: vec![],
            payload: fxt::EventPayload::AsyncInstant { id: expected_async_id },
        })
    );
    assert_eq!(instant_thread.next(), None, "must have observed all events from instant thread");

    // async_end(async_id, c"test_puppet", c"puppet_async", &[]);
    let async_end = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&async_end).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        async_end,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_async".into(),
            args: vec![],
            payload: fxt::EventPayload::AsyncEnd { id: expected_async_id },
        })
    );

    // flow_begin!(c"test_puppet", c"puppet_flow", flow_id);
    let flow_begin = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&flow_begin).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    let expected_flow_id = match &flow_begin {
        fxt::TraceRecord::Event(fxt::EventRecord {
            payload: fxt::EventPayload::FlowBegin { id },
            ..
        }) => *id,
        _ => panic!("expected FlowBegin event, got {:?}", flow_begin),
    };
    assert_eq!(
        flow_begin,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_flow".into(),
            args: vec![],
            payload: fxt::EventPayload::FlowBegin { id: expected_flow_id },
        })
    );

    // std::thread::spawn(move || {
    let (flow_step_thread_koid, flow_step_thread) = threads.next().unwrap();
    let mut flow_step_thread = flow_step_thread.into_iter();
    assert_eq!(threads.next(), None, "no other threads should be seen");

    // flow_step!("test_puppet", "puppet_flow", flow_id);
    let flow_step = flow_step_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&flow_step).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        flow_step,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: flow_step_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_flow_step1".into(),
            args: vec![],
            payload: fxt::EventPayload::FlowStep { id: expected_flow_id },
        })
    );

    // duration!(c"test_puppet", c"flow_thread"); (comes later because it's raii)
    let duration_complete = flow_step_thread.next().unwrap();
    let duration_complete_start = match &duration_complete {
        fxt::TraceRecord::Event(fxt::EventRecord { timestamp, .. }) => *timestamp,
        _ => panic!(),
    };
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&duration_complete).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        duration_complete,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: duration_complete_start,
            process: puppet_process_koid,
            thread: flow_step_thread_koid,
            category: "test_puppet".into(),
            name: "flow_thread".into(),
            args: vec![],
            payload: fxt::EventPayload::DurationComplete { end_timestamp: current_timestamp },
        })
    );
    assert_eq!(flow_step_thread.next(), None, "must have observed all events from flow thread");

    // flow_end!(c"test_puppet", c"puppet_flow", flow_id);
    let flow_end = initial_thread.next().unwrap();
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&flow_end).unwrap();
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        flow_end,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: current_timestamp,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_flow".into(),
            args: vec![],
            payload: fxt::EventPayload::FlowEnd { id: expected_flow_id },
        })
    );

    // All of the remaining records are the same assertions except for their args.
    let mut expect_instant_args = |args: Vec<fxt::Arg>| {
        let next_arg_instant = initial_thread.next().unwrap();
        prev_timestamp = current_timestamp;
        current_timestamp = record_timestamp(&next_arg_instant).unwrap();
        assert!(prev_timestamp <= current_timestamp);
        assert_eq!(
            next_arg_instant,
            fxt::TraceRecord::Event(fxt::EventRecord {
                provider: expected_provider.clone(),
                timestamp: current_timestamp,
                process: puppet_process_koid,
                thread: initial_thread_koid,
                category: "test_puppet".into(),
                name: "puppet_instant_args".into(),
                args: args.to_vec(),
                payload: fxt::EventPayload::Instant,
            })
        );
    };

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeNullArg" => ());
    expect_instant_args(vec![fxt::Arg { name: "SomeNullArg".into(), value: fxt::ArgValue::Null }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeUint32" => 2145u32);
    expect_instant_args(vec![fxt::Arg {
        name: "SomeUint32".into(),
        value: fxt::ArgValue::Unsigned32(2145),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeUint64" => 423621626134123415u64);
    expect_instant_args(vec![fxt::Arg {
        name: "SomeUint64".into(),
        value: fxt::ArgValue::Unsigned64(423621626134123415),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeInt32" => -7i32);
    expect_instant_args(vec![fxt::Arg {
        name: "SomeInt32".into(),
        value: fxt::ArgValue::Signed32(-7),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeInt64" => -234516543631231i64);
    expect_instant_args(vec![fxt::Arg {
        name: "SomeInt64".into(),
        value: fxt::ArgValue::Signed64(-234516543631231),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeDouble" => std::f64::consts::PI);
    expect_instant_args(vec![fxt::Arg {
        name: "SomeDouble".into(),
        value: fxt::ArgValue::Double(std::f64::consts::PI),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeString" => "pong");
    expect_instant_args(vec![fxt::Arg {
        name: "SomeString".into(),
        value: fxt::ArgValue::String("pong".into()),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeBool" => true);
    expect_instant_args(vec![fxt::Arg {
        name: "SomeBool".into(),
        value: fxt::ArgValue::Boolean(true),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomePointer" => 4096usize as *const _);
    expect_instant_args(vec![fxt::Arg {
        name: "SomePointer".into(),
        value: fxt::ArgValue::Pointer(4096),
    }]);

    // instant!(c"test_puppet", c"puppet_instant_args", Scope::Thread, "SomeKoid" => zx::Koid::from_raw(10));
    expect_instant_args(vec![fxt::Arg {
        name: "SomeKoid".into(),
        value: fxt::ArgValue::KernelObj(10),
    }]);

    // duration!(c"test_puppet", c"puppet_duration_raii");
    let duration_complete = initial_thread.next().unwrap();
    let duration_complete_start = match &duration_complete {
        fxt::TraceRecord::Event(fxt::EventRecord { timestamp, .. }) => *timestamp,
        _ => panic!(),
    };
    prev_timestamp = current_timestamp;
    current_timestamp = record_timestamp(&duration_complete).unwrap();
    assert!(before_duration_complete_start < duration_complete_start);
    assert!(prev_timestamp <= current_timestamp);
    assert_eq!(
        duration_complete,
        fxt::TraceRecord::Event(fxt::EventRecord {
            provider: expected_provider.clone(),
            timestamp: duration_complete_start,
            process: puppet_process_koid,
            thread: initial_thread_koid,
            category: "test_puppet".into(),
            name: "puppet_duration_raii".into(),
            args: vec![],
            payload: fxt::EventPayload::DurationComplete { end_timestamp: current_timestamp },
        })
    );

    assert_eq!(initial_thread.next(), None, "must have observed all events from initial thread");
    assert!(
        current_timestamp <= max_timestamp,
        "all trace timestamps should have occurred before we stopped tracing"
    );
}

#[derive(Debug, Default)]
struct PerProcessRecords {
    no_thread: Vec<fxt::TraceRecord>,
    per_thread: BTreeMap<fxt::ThreadKoid, Vec<fxt::TraceRecord>>,
}

struct TraceSession {
    tracing_controller: Option<TracingSessionProxy>,
    collect_trace: Task<Vec<fxt::TraceRecord>>,
    drain_trace: Task<Vec<fxt::ParseWarning>>,
}

impl TraceSession {
    async fn start() -> Self {
        info!("initializing tracing...");
        let trace_provisioner = connect_to_protocol::<TracingProvisionerMarker>().unwrap();
        let (tracing_controller, server_end) =
            fidl::endpoints::create_proxy::<TracingSessionMarker>();
        let (tracing_socket, tracing_socket_write) = Socket::create_stream();
        let tracing_socket = AsyncSocket::from_socket(tracing_socket);
        trace_provisioner
            .initialize_tracing(
                server_end,
                &TraceConfig {
                    categories: Some(TRACE_CATEGORIES.iter().map(|c| c.to_string()).collect()),
                    ..Default::default()
                },
                tracing_socket_write,
            )
            .unwrap();

        let (record_stream, drain_trace) = fxt::SessionParser::new_async(tracing_socket);
        let collect_trace =
            Task::spawn(async move { record_stream.map(|r| r.unwrap()).collect::<Vec<_>>().await });

        info!("starting tracing...");
        tracing_controller
            .start_tracing(&StartOptions::default())
            .await
            .expect("starting tracing FIDL")
            .expect("start tracing");

        Self { tracing_controller: Some(tracing_controller), collect_trace, drain_trace }
    }

    async fn terminate(mut self) -> (Vec<fxt::TraceRecord>, Vec<fxt::ParseWarning>) {
        info!("terminating trace...");
        self.tracing_controller = None;

        info!("waiting for socket collection and parsing to complete...");
        (self.collect_trace.await, self.drain_trace.await)
    }
}

async fn run_puppet(url: String) {
    info!("running child component...");
    let realm = connect_to_protocol::<RealmMarker>().unwrap();

    let (component_controller, controller_server) = fidl::endpoints::create_proxy();
    realm
        .create_child(
            &fdecl::CollectionRef { name: COLLECTION_NAME.to_string() },
            &fdecl::Child {
                name: Some(CHILD_NAME.to_string()),
                url: Some(url),
                startup: Some(fdecl::StartupMode::Lazy),
                ..Default::default()
            },
            CreateChildArgs { controller: Some(controller_server), ..Default::default() },
        )
        .await
        .unwrap()
        .unwrap();

    let (execution_controller, execution_controller_server) = fidl::endpoints::create_proxy();
    component_controller
        .start(StartChildArgs::default(), execution_controller_server)
        .await
        .unwrap()
        .unwrap();

    info!("waiting for oneshot trace provider to complete on its own...");
    match execution_controller.take_event_stream().next().await {
        Some(Ok(ExecutionControllerEvent::OnStop {
            stopped_payload: StoppedPayload { status, .. },
        })) => assert_eq!(status, Some(0), "provider should have exited cleanly"),
        Some(Ok(ExecutionControllerEvent::_UnknownEvent { .. })) => {
            panic!("unknown event")
        }
        Some(Err(e)) => panic!("encountered error {e} while waiting for component to stop"),
        None => panic!("execution controller should not be closed before returning OnStop"),
    }
}

fn record_timestamp(record: &fxt::TraceRecord) -> Option<i64> {
    match record {
        // For duration complete records, the end timestamp is when it was actually written.
        fxt::TraceRecord::Event(fxt::EventRecord {
            payload: fxt::EventPayload::DurationComplete { end_timestamp },
            ..
        }) => Some(*end_timestamp),
        fxt::TraceRecord::Event(fxt::EventRecord { timestamp, .. }) => Some(*timestamp),
        fxt::TraceRecord::Scheduling(fxt::SchedulingRecord::ContextSwitch(
            fxt::ContextSwitchEvent { timestamp, .. },
        )) => Some(*timestamp),
        fxt::TraceRecord::Scheduling(fxt::SchedulingRecord::LegacyContextSwitch(
            fxt::LegacyContextSwitchEvent { timestamp, .. },
        )) => Some(*timestamp),
        fxt::TraceRecord::Scheduling(fxt::SchedulingRecord::ThreadWakeup(
            fxt::ThreadWakeupEvent { timestamp, .. },
        )) => Some(*timestamp),
        fxt::TraceRecord::Log(fxt::LogRecord { timestamp, .. }) => Some(*timestamp),
        fxt::TraceRecord::LargeBlob(fxt::LargeBlobRecord {
            metadata: Some(fxt::LargeBlobMetadata { timestamp, .. }),
            ..
        }) => Some(*timestamp),

        fxt::TraceRecord::ProviderEvent { .. }
        | fxt::TraceRecord::Blob(..)
        | fxt::TraceRecord::LargeBlob(fxt::LargeBlobRecord { metadata: None, .. })
        | fxt::TraceRecord::KernelObj(..)
        | fxt::TraceRecord::Profiler(..)
        | fxt::TraceRecord::UserspaceObj(..) => None,
    }
}
