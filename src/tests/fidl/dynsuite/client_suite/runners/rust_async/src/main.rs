// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::Proxy;
use fidl_fidl_clientsuite::{
    AjarTargetEvent, AjarTargetEventReport, BindingsProperties, ClosedTargetEvent,
    ClosedTargetEventReport, Empty, EmptyResultClassification, EmptyResultWithErrorClassification,
    IoStyle, NonEmptyPayload, NonEmptyResultClassification, NonEmptyResultWithErrorClassification,
    OpenTargetEvent, OpenTargetEventReport, RunnerRequest, RunnerRequestStream,
    TableResultClassification, Test, UnionResultClassification, UnknownEvent, CLIENT_SUITE_VERSION,
};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use futures::prelude::*;
use futures::TryStreamExt;
use rust_util::{classify_error, method_name};

const DISABLED_TESTS: &[Test] = &[
    // No disabled tests!
];

async fn handle_runner_request(request: RunnerRequest) {
    match request {
        // =====================================================================
        //      Test management methods
        // =====================================================================
        RunnerRequest::CheckAlive { responder } => {
            responder.send().unwrap();
        }
        RunnerRequest::GetVersion { responder } => {
            responder.send(CLIENT_SUITE_VERSION).unwrap();
        }
        RunnerRequest::IsTestEnabled { test, responder } => {
            responder.send(!DISABLED_TESTS.contains(&test)).unwrap();
        }
        RunnerRequest::GetBindingsProperties { responder } => {
            responder
                .send(&BindingsProperties { io_style: Some(IoStyle::Async), ..Default::default() })
                .unwrap();
        }

        // =====================================================================
        //      Closed target methods
        // =====================================================================
        RunnerRequest::CallTwoWayNoPayload { target, responder } => {
            let client = target.into_proxy();
            match client.two_way_no_payload().await {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayStructPayload { target, responder } => {
            let client = target.into_proxy();
            match client.two_way_struct_payload().await {
                Ok(some_field) => responder
                    .send(&NonEmptyResultClassification::Success(NonEmptyPayload { some_field }))
                    .unwrap(),
                Err(err) => responder
                    .send(&NonEmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayTablePayload { target, responder } => {
            let client = target.into_proxy();
            match client.two_way_table_payload().await {
                Ok(payload) => {
                    responder.send(&TableResultClassification::Success(payload)).unwrap()
                }
                Err(err) => responder
                    .send(&TableResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayUnionPayload { target, responder } => {
            let client = target.into_proxy();
            match client.two_way_union_payload().await {
                Ok(payload) => {
                    responder.send(&UnionResultClassification::Success(payload)).unwrap()
                }
                Err(err) => responder
                    .send(&UnionResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayStructPayloadErr { target, responder } => {
            let client = target.into_proxy();
            match client.two_way_struct_payload_err().await {
                Ok(Ok(some_field)) => responder
                    .send(&NonEmptyResultWithErrorClassification::Success(NonEmptyPayload {
                        some_field,
                    }))
                    .unwrap(),
                Ok(Err(application_err)) => responder
                    .send(&NonEmptyResultWithErrorClassification::ApplicationError(application_err))
                    .unwrap(),
                Err(err) => responder
                    .send(&NonEmptyResultWithErrorClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayStructRequest { target, request, responder } => {
            let client = target.into_proxy();
            match client.two_way_struct_request(request.some_field).await {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayTableRequest { target, request, responder } => {
            let client = target.into_proxy();
            match client.two_way_table_request(&request).await {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallTwoWayUnionRequest { target, request, responder } => {
            let client = target.into_proxy();
            match client.two_way_union_request(&request).await {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallOneWayNoRequest { target, responder } => {
            let client = target.into_proxy();
            match client.one_way_no_request() {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallOneWayStructRequest { target, request, responder } => {
            let client = target.into_proxy();
            match client.one_way_struct_request(request.some_field) {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallOneWayTableRequest { target, request, responder } => {
            let client = target.into_proxy();
            match client.one_way_table_request(&request) {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallOneWayUnionRequest { target, request, responder } => {
            let client = target.into_proxy();
            match client.one_way_union_request(&request) {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }

        // =====================================================================
        //      Open target methods
        // =====================================================================
        RunnerRequest::CallStrictOneWay { target, responder } => {
            let client = target.into_proxy();
            match client.strict_one_way() {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallFlexibleOneWay { target, responder } => {
            let client = target.into_proxy();
            match client.flexible_one_way() {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallStrictTwoWay { target, responder } => {
            let client = target.into_proxy();
            match client.strict_two_way().await {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallStrictTwoWayFields { target, responder } => {
            let client = target.into_proxy();
            match client.strict_two_way_fields().await {
                Ok(some_field) => responder
                    .send(&NonEmptyResultClassification::Success(NonEmptyPayload { some_field }))
                    .unwrap(),
                Err(err) => responder
                    .send(&NonEmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallStrictTwoWayErr { target, responder } => {
            let client = target.into_proxy();
            match client.strict_two_way_err().await {
                Ok(Ok(())) => {
                    responder.send(&EmptyResultWithErrorClassification::Success(Empty)).unwrap()
                }
                Ok(Err(application_err)) => responder
                    .send(&EmptyResultWithErrorClassification::ApplicationError(application_err))
                    .unwrap(),
                Err(fidl_err) => responder
                    .send(&EmptyResultWithErrorClassification::FidlError(classify_error(fidl_err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallStrictTwoWayFieldsErr { target, responder } => {
            let client = target.into_proxy();
            match client.strict_two_way_fields_err().await {
                Ok(Ok(some_field)) => responder
                    .send(&NonEmptyResultWithErrorClassification::Success(NonEmptyPayload {
                        some_field,
                    }))
                    .unwrap(),
                Ok(Err(application_err)) => responder
                    .send(&NonEmptyResultWithErrorClassification::ApplicationError(application_err))
                    .unwrap(),
                Err(fidl_err) => responder
                    .send(&NonEmptyResultWithErrorClassification::FidlError(classify_error(
                        fidl_err,
                    )))
                    .unwrap(),
            }
        }
        RunnerRequest::CallFlexibleTwoWay { target, responder } => {
            let client = target.into_proxy();
            match client.flexible_two_way().await {
                Ok(()) => responder.send(&EmptyResultClassification::Success(Empty)).unwrap(),
                Err(err) => responder
                    .send(&EmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallFlexibleTwoWayFields { target, responder } => {
            let client = target.into_proxy();
            match client.flexible_two_way_fields().await {
                Ok(some_field) => responder
                    .send(&NonEmptyResultClassification::Success(NonEmptyPayload { some_field }))
                    .unwrap(),
                Err(err) => responder
                    .send(&NonEmptyResultClassification::FidlError(classify_error(err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallFlexibleTwoWayErr { target, responder } => {
            let client = target.into_proxy();
            match client.flexible_two_way_err().await {
                Ok(Ok(())) => {
                    responder.send(&EmptyResultWithErrorClassification::Success(Empty)).unwrap()
                }
                Ok(Err(application_err)) => responder
                    .send(&EmptyResultWithErrorClassification::ApplicationError(application_err))
                    .unwrap(),
                Err(fidl_err) => responder
                    .send(&EmptyResultWithErrorClassification::FidlError(classify_error(fidl_err)))
                    .unwrap(),
            }
        }
        RunnerRequest::CallFlexibleTwoWayFieldsErr { target, responder } => {
            let client = target.into_proxy();
            match client.flexible_two_way_fields_err().await {
                Ok(Ok(some_field)) => responder
                    .send(&NonEmptyResultWithErrorClassification::Success(NonEmptyPayload {
                        some_field,
                    }))
                    .unwrap(),
                Ok(Err(application_err)) => responder
                    .send(&NonEmptyResultWithErrorClassification::ApplicationError(application_err))
                    .unwrap(),
                Err(fidl_err) => responder
                    .send(&NonEmptyResultWithErrorClassification::FidlError(classify_error(
                        fidl_err,
                    )))
                    .unwrap(),
            }
        }

        // =====================================================================
        //      Event handling methods
        // =====================================================================
        RunnerRequest::ReceiveClosedEvents { target, reporter, responder } => {
            fasync::Task::spawn(async move {
                println!("Listening for ClosedTarget events...");
                let mut event_stream = target.into_proxy().take_event_stream();
                let reporter = reporter.into_proxy();
                while let Some(event) = event_stream.next().await {
                    match &event {
                        Ok(event) => {
                            println!("Received ClosedTarget event: {}", method_name(event));
                        }
                        Err(err) => println!("Failed reading ClosedTarget event: {}", err),
                    }
                    match event {
                        Ok(ClosedTargetEvent::OnEventNoPayload {}) => reporter
                            .report_event(&ClosedTargetEventReport::OnEventNoPayload(Empty {}))
                            .unwrap(),
                        Ok(ClosedTargetEvent::OnEventStructPayload { some_field }) => reporter
                            .report_event(&ClosedTargetEventReport::OnEventStructPayload(
                                NonEmptyPayload { some_field },
                            ))
                            .unwrap(),
                        Ok(ClosedTargetEvent::OnEventTablePayload { payload }) => reporter
                            .report_event(&ClosedTargetEventReport::OnEventTablePayload(payload))
                            .unwrap(),
                        Ok(ClosedTargetEvent::OnEventUnionPayload { payload }) => reporter
                            .report_event(&ClosedTargetEventReport::OnEventUnionPayload(payload))
                            .unwrap(),
                        Err(fidl_err) => reporter
                            .report_event(&ClosedTargetEventReport::FidlError(classify_error(
                                fidl_err,
                            )))
                            .unwrap(),
                    }
                }
                println!("Waiting for Reporter server to close channel");
                reporter.on_closed().await.unwrap();
            })
            .detach();
            responder.send().unwrap();
        }
        RunnerRequest::ReceiveAjarEvents { target, reporter, responder } => {
            fasync::Task::spawn(async move {
                println!("Listening for AjarTarget events...");
                let mut event_stream = target.into_proxy().take_event_stream();
                let reporter = reporter.into_proxy();
                while let Some(event) = event_stream.next().await {
                    match &event {
                        Ok(event) => println!("Received AjarTarget event: {}", method_name(event)),
                        Err(err) => println!("Failed reading AjarTarget event: {}", err),
                    }
                    match event {
                        Ok(AjarTargetEvent::_UnknownEvent { ordinal, .. }) => reporter
                            .report_event(&AjarTargetEventReport::UnknownEvent(UnknownEvent {
                                ordinal,
                            }))
                            .unwrap(),
                        Err(fidl_err) => reporter
                            .report_event(&AjarTargetEventReport::FidlError(classify_error(
                                fidl_err,
                            )))
                            .unwrap(),
                    }
                }
                println!("Waiting for Reporter server to close channel");
                reporter.on_closed().await.unwrap();
            })
            .detach();
            responder.send().unwrap()
        }
        RunnerRequest::ReceiveOpenEvents { target, reporter, responder } => {
            fasync::Task::spawn(async move {
                println!("Listening for OpenTarget events...");
                let mut event_stream = target.into_proxy().take_event_stream();
                let reporter = reporter.into_proxy();
                while let Some(event) = event_stream.next().await {
                    match &event {
                        Ok(event) => println!("Received OpenTarget event: {}", method_name(&event)),
                        Err(err) => println!("Failed reading OpenTarget event: {}", err),
                    }
                    match event {
                        Ok(OpenTargetEvent::StrictEvent {}) => reporter
                            .report_event(&OpenTargetEventReport::StrictEvent(Empty))
                            .unwrap(),
                        Ok(OpenTargetEvent::FlexibleEvent {}) => reporter
                            .report_event(&OpenTargetEventReport::FlexibleEvent(Empty))
                            .unwrap(),
                        Ok(OpenTargetEvent::_UnknownEvent { ordinal, .. }) => reporter
                            .report_event(&OpenTargetEventReport::UnknownEvent(UnknownEvent {
                                ordinal,
                            }))
                            .unwrap(),
                        Err(fidl_err) => reporter
                            .report_event(&OpenTargetEventReport::FidlError(classify_error(
                                fidl_err,
                            )))
                            .unwrap(),
                    }
                }
                println!("Waiting for Reporter server to close channel");
                reporter.on_closed().await.unwrap();
            })
            .detach();
            responder.send().unwrap()
        }
    }
}

enum IncomingService {
    Runner(RunnerRequestStream),
}

#[fuchsia::main]
async fn main() {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(IncomingService::Runner);
    fs.take_and_serve_directory_handle().unwrap();

    println!("Listening for incoming connections...");
    const MAX_CONCURRENT: usize = 10_000;
    fs.for_each_concurrent(MAX_CONCURRENT, |IncomingService::Runner(mut stream)| async move {
        println!("Received connection, serving requests...");
        while let Some(request) = stream.try_next().await.unwrap() {
            println!("Handling Runner request: {}", method_name(&request));
            handle_runner_request(request).await;
        }
    })
    .await;
}
