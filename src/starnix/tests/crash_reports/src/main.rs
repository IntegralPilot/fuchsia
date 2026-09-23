// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fidl_fuchsia_feedback::{
    CrashReporterMarker, CrashReporterRequest, CrashReporterRequestStream, SpecificCrashReport,
};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, Ref, Route,
};
use futures::StreamExt;
use log::info;
use std::collections::BTreeMap;

#[fuchsia::main]
async fn main() {
    let mut events = EventStream::open().await.unwrap();
    let builder = RealmBuilder::with_params(
        RealmBuilderParams::new()
            .realm_name("crash_reporter")
            .from_relative_url("#meta/container_with_crasher.cm"),
    )
    .await
    .unwrap();

    let (reporter_send, mut reporter_requests) = futures::channel::mpsc::unbounded();
    let crash_reporter_mock = builder
        .add_local_child(
            "crash_reporter",
            move |handles| {
                let reporter_send = reporter_send.clone();
                Box::pin(async move {
                    let mut fs = ServiceFs::new();
                    fs.serve_connection(handles.outgoing_dir).unwrap();
                    fs.dir("svc").add_fidl_service(|h: CrashReporterRequestStream| Ok(h));
                    fs.forward(reporter_send).await.unwrap();
                    Ok(())
                })
            },
            ChildOptions::new(),
        )
        .await
        .unwrap();
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<CrashReporterMarker>())
                .from(&crash_reporter_mock)
                .to(Ref::child("kernel")),
        )
        .await
        .unwrap();

    info!("starting realm");
    let kernel_with_container_and_crasher = builder.build().await.unwrap();
    let realm_moniker =
        format!("realm_builder:{}", kernel_with_container_and_crasher.root.child_name());
    info!(realm_moniker:%; "started");
    let crasher_moniker = format!("{realm_moniker}/crasher");

    info!(crasher_moniker:%; "waiting for crasher to exit");
    let stopped =
        EventMatcher::ok().moniker(&crasher_moniker).wait::<Stopped>(&mut events).await.unwrap();
    let status = stopped.result().unwrap().status;
    info!(status:?; "crasher stopped");
    assert_eq!(status, ExitStatus::Crash(11));

    info!("waiting for crash report");
    let mut reporter_client = reporter_requests.next().await.unwrap();
    let report = match reporter_client.next().await.unwrap().unwrap() {
        CrashReporterRequest::FileReport { report, responder } => {
            responder.send(Ok(&Default::default())).unwrap();
            report
        }
    };

    info!("got crash report, checking fields");
    assert_eq!(report.program_name.unwrap(), "generate_linux_crash_report");
    assert!(report.program_uptime.is_some());
    assert!(report.is_fatal.unwrap());
    assert_eq!(report.crash_signature.unwrap(), "generate_linux_crash_report SIGABRT(6)");

    let native_report = match report.specific_report.unwrap() {
        SpecificCrashReport::Native(n) => n,
        other => panic!("unexpected specific crash report {other:?}"),
    };
    assert_eq!(native_report.minidump, None);
    assert_eq!(native_report.process_name.unwrap(), "generate_linux_crash_report");
    assert_eq!(native_report.thread_name.unwrap(), "generate_linux_crash_report");
    assert!(native_report.process_koid.is_some());
    assert!(native_report.thread_koid.is_some());

    let annotations = report
        .annotations
        .unwrap()
        .into_iter()
        .map(|a| (a.key, a.value))
        .collect::<BTreeMap<_, _>>();
    assert!(annotations.get("linux.pid").is_some());
    assert_eq!(
        annotations["linux.argv"],
        "data/tests/generate_linux_crash_report fake args for testing"
    );
    assert_eq!(annotations["linux.env"], "FAKE1=foo FAKE2=bar");
    assert_eq!(annotations["linux.signal"], "SIGABRT(6)");
}
