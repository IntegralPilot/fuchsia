// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ControlHandle;
use fidl::prelude::*;
use fidl::{Event, MethodType, Status};
use fidl_fidl_serversuite::{
    AjarTargetControlHandle, AjarTargetRequest, AjarTargetRequestStream, AnyTarget,
    ClosedTargetControlHandle, ClosedTargetRequest, ClosedTargetRequestStream,
    ClosedTargetTwoWayResultRequest, ClosedTargetTwoWayTablePayloadResponse,
    ClosedTargetTwoWayUnionPayloadRequest, ClosedTargetTwoWayUnionPayloadResponse, Empty,
    OpenTargetControlHandle, OpenTargetFlexibleTwoWayErrRequest,
    OpenTargetFlexibleTwoWayFieldsErrRequest, OpenTargetRequest, OpenTargetRequestStream,
    OpenTargetStrictTwoWayErrRequest, OpenTargetStrictTwoWayFieldsErrRequest, RunnerControlHandle,
    RunnerRequest, RunnerRequestStream, SERVER_SUITE_VERSION, StartError, TeardownReason, Test,
    UnknownMethodType,
};
use fuchsia_component::server::ServiceFs;
use futures::future::{BoxFuture, Fuse};
use futures::prelude::*;
use futures::select;
use log::info;

const DISABLED_TESTS: &[Test] = &[
    // This is for testing the test disabling functionality itself.
    Test::IgnoreDisabled,
    // TODO(https://fxbug.dev/42161447): Bindings *do* hide PEER_CLOSED from event sending.
    // But because we send events through an Option<TargetControlHandle>, and we
    // have to set it to None at the end (see the other comment about
    // https://fxbug.dev/42161447 below), we effectively re-introduce the race condition.
    Test::EventSendingDoNotReportPeerClosed,
];

// TestTag displays as the GoogleTest test name from the server suite harness.
// This helps to clarify which logs belong to which test.
#[derive(Copy, Clone)]
struct TestTag(Option<Test>);

impl std::fmt::Display for TestTag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            None => write!(f, "[ServerTest_?]"),
            Some(test) => write!(f, "[ServerTest_{}]", test.into_primitive()),
        }
    }
}

fn get_teardown_reason(error: Option<fidl::Error>) -> TeardownReason {
    let Some(error) = error else {
        // When the request stream completes without an error, it could be
        // PEER_CLOSED or a voluntary shutdown.
        return TeardownReason::PEER_CLOSED | TeardownReason::VOLUNTARY_SHUTDOWN;
    };
    match error {
        fidl::Error::InvalidBoolean
        | fidl::Error::InvalidHeader
        | fidl::Error::Invalid
        | fidl::Error::OutOfRange { .. }
        | fidl::Error::OutOfHandles
        | fidl::Error::ExtraBytes
        | fidl::Error::ExtraHandles
        | fidl::Error::NonZeroPadding { .. }
        | fidl::Error::MaxRecursionDepth
        | fidl::Error::NotNullable
        | fidl::Error::UnexpectedNullRef
        | fidl::Error::Utf8Error
        | fidl::Error::InvalidBitsValue
        | fidl::Error::InvalidEnumValue
        | fidl::Error::UnknownUnionTag
        | fidl::Error::InvalidPresenceIndicator
        | fidl::Error::InvalidInlineBitInEnvelope
        | fidl::Error::InvalidInlineMarkerInEnvelope
        | fidl::Error::InvalidNumBytesInEnvelope
        | fidl::Error::InvalidHostHandle
        | fidl::Error::IncorrectHandleSubtype { .. }
        | fidl::Error::MissingExpectedHandleRights { .. } => TeardownReason::DECODE_FAILURE,

        fidl::Error::IncompatibleMagicNumber(_) | fidl::Error::UnsupportedWireFormatVersion => {
            TeardownReason::INCOMPATIBLE_FORMAT
        }

        fidl::Error::InvalidRequestTxid
        | fidl::Error::InvalidResponseTxid
        | fidl::Error::UnknownOrdinal { .. } => TeardownReason::UNEXPECTED_MESSAGE,

        fidl::Error::ServerResponseWrite(_) => TeardownReason::WRITE_FAILURE,

        fidl::Error::UnsupportedMethod { .. }
        | fidl::Error::ClientChannelClosed { .. }
        | fidl::Error::UnexpectedSyncResponse { .. } => {
            panic!("get_teardown_reason: this error should not happen in a server: {error:?}")
        }

        _ => panic!("get_teardown_reason: missing a match arm for this error: {error:?}"),
    }
}

/// Returns the FIDL method name for a request/event enum value.
// TODO(https://fxbug.dev/42078541): Provide this in FIDL bindings.
fn method_name(request_or_event: &impl std::fmt::Debug) -> String {
    let mut string = format!("{:?}", request_or_event);
    let len = string.find('{').unwrap_or(string.len());
    let len = string[..len].trim_end().len();
    string.truncate(len);
    assert!(
        string.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "failed to parse the method name from {:?}",
        request_or_event
    );
    string
}

// This is the error return type for functions that run a target server. They
// should only return an error when it is expected to occur during tests. On
// other errors (e.g. reply sending that should never fail), they should panic.
type ErrorExpectedUnderTest = fidl::Error;

async fn serve_closed_target(
    mut stream: ClosedTargetRequestStream,
    runner_ctrl: RunnerControlHandle,
    tag: TestTag,
) -> Result<(), ErrorExpectedUnderTest> {
    info!("{tag} Serving ClosedTarget...");
    while let Some(request) = stream.try_next().await? {
        info!("{tag} Handling ClosedTarget request: {}", method_name(&request));
        match request {
            ClosedTargetRequest::OneWayNoPayload { .. } => {
                runner_ctrl.send_on_received_closed_target_one_way_no_payload().unwrap();
            }
            ClosedTargetRequest::TwoWayNoPayload { responder } => {
                responder.send().unwrap();
            }
            ClosedTargetRequest::TwoWayStructPayload { v, responder } => {
                responder.send(v).unwrap();
            }
            ClosedTargetRequest::TwoWayTablePayload { payload, responder } => {
                responder
                    .send(&ClosedTargetTwoWayTablePayloadResponse {
                        v: payload.v,
                        ..Default::default()
                    })
                    .unwrap();
            }
            ClosedTargetRequest::TwoWayUnionPayload { payload, responder } => {
                let ClosedTargetTwoWayUnionPayloadRequest::V(v) = payload else {
                    panic!("{tag} unexpected union value: {payload:?}");
                };
                responder.send(&ClosedTargetTwoWayUnionPayloadResponse::V(v)).unwrap();
            }
            ClosedTargetRequest::TwoWayResult { payload, responder } => match payload {
                ClosedTargetTwoWayResultRequest::Payload(value) => {
                    responder.send(Ok(&value)).unwrap();
                }
                ClosedTargetTwoWayResultRequest::Error(value) => {
                    responder.send(Err(value)).unwrap();
                }
            },
            ClosedTargetRequest::GetHandleRights { handle, responder } => {
                let basic_info = handle.as_handle_ref().basic_info().unwrap();
                responder.send(basic_info.rights).unwrap();
            }
            ClosedTargetRequest::GetSignalableEventRights { handle, responder } => {
                let basic_info = handle.as_handle_ref().basic_info().unwrap();
                responder.send(basic_info.rights).unwrap();
            }
            ClosedTargetRequest::EchoAsTransferableSignalableEvent { handle, responder } => {
                responder.send(fidl::Event::from(handle))?;
            }
            ClosedTargetRequest::ByteVectorSize { vec, responder } => {
                responder.send(vec.len().try_into().unwrap()).unwrap();
            }
            ClosedTargetRequest::HandleVectorSize { vec, responder } => {
                responder.send(vec.len().try_into().unwrap()).unwrap();
            }
            ClosedTargetRequest::CreateNByteVector { n, responder } => {
                let bytes: Vec<u8> = vec![0; n.try_into().unwrap()];
                responder.send(&bytes)?;
            }
            ClosedTargetRequest::CreateNHandleVector { n, responder } => {
                let mut handles = Vec::new();
                for _ in 0..n {
                    handles.push(Event::create());
                }
                responder.send(handles)?;
            }
        }
    }
    Ok(())
}

async fn serve_ajar_target(
    mut stream: AjarTargetRequestStream,
    runner_ctrl: RunnerControlHandle,
    tag: TestTag,
) -> Result<(), ErrorExpectedUnderTest> {
    info!("{tag} Serving AjarTarget...");
    while let Some(request) = stream.try_next().await? {
        info!("{tag} Handling AjarTarget request: {}", method_name(&request));
        match request {
            AjarTargetRequest::_UnknownMethod { ordinal, .. } => {
                runner_ctrl
                    .send_on_received_unknown_method(ordinal, UnknownMethodType::OneWay)
                    .unwrap();
            }
        }
    }
    Ok(())
}

async fn serve_open_target(
    mut stream: OpenTargetRequestStream,
    runner_ctrl: RunnerControlHandle,
    tag: TestTag,
) -> Result<(), ErrorExpectedUnderTest> {
    info!("{tag} Serving OpenTarget...");
    while let Some(request) = stream.try_next().await? {
        info!("{tag} Handling OpenTarget request: {}", method_name(&request));
        match request {
            OpenTargetRequest::StrictOneWay { .. } => {
                runner_ctrl.send_on_received_open_target_strict_one_way().unwrap();
            }
            OpenTargetRequest::FlexibleOneWay { .. } => {
                runner_ctrl.send_on_received_open_target_flexible_one_way().unwrap();
            }
            OpenTargetRequest::StrictTwoWay { responder } => {
                responder.send().unwrap();
            }
            OpenTargetRequest::StrictTwoWayFields { reply_with, responder } => {
                responder.send(reply_with).unwrap();
            }
            OpenTargetRequest::StrictTwoWayErr { payload, responder } => match payload {
                OpenTargetStrictTwoWayErrRequest::ReplySuccess(Empty) => {
                    responder.send(Ok(())).unwrap();
                }
                OpenTargetStrictTwoWayErrRequest::ReplyError(reply_error) => {
                    responder.send(Err(reply_error)).unwrap();
                }
            },
            OpenTargetRequest::StrictTwoWayFieldsErr { payload, responder } => match payload {
                OpenTargetStrictTwoWayFieldsErrRequest::ReplySuccess(reply_success) => {
                    responder.send(Ok(reply_success)).unwrap();
                }
                OpenTargetStrictTwoWayFieldsErrRequest::ReplyError(reply_error) => {
                    responder.send(Err(reply_error)).unwrap();
                }
            },
            OpenTargetRequest::FlexibleTwoWay { responder } => {
                responder.send().unwrap();
            }
            OpenTargetRequest::FlexibleTwoWayFields { reply_with, responder } => {
                responder.send(reply_with).unwrap();
            }
            OpenTargetRequest::FlexibleTwoWayErr { payload, responder } => match payload {
                OpenTargetFlexibleTwoWayErrRequest::ReplySuccess(Empty) => {
                    responder.send(Ok(())).unwrap();
                }
                OpenTargetFlexibleTwoWayErrRequest::ReplyError(reply_error) => {
                    responder.send(Err(reply_error)).unwrap();
                }
            },
            OpenTargetRequest::FlexibleTwoWayFieldsErr { payload, responder } => match payload {
                OpenTargetFlexibleTwoWayFieldsErrRequest::ReplySuccess(reply_success) => {
                    responder.send(Ok(reply_success)).unwrap();
                }
                OpenTargetFlexibleTwoWayFieldsErrRequest::ReplyError(reply_error) => {
                    responder.send(Err(reply_error)).unwrap();
                }
            },
            OpenTargetRequest::_UnknownMethod { ordinal, method_type, .. } => {
                let unknown_method_type = match method_type {
                    MethodType::OneWay => UnknownMethodType::OneWay,
                    MethodType::TwoWay => UnknownMethodType::TwoWay,
                };
                runner_ctrl.send_on_received_unknown_method(ordinal, unknown_method_type).unwrap();
            }
        }
    }
    Ok(())
}

enum TargetControlHandle {
    Closed(ClosedTargetControlHandle),
    Ajar(AjarTargetControlHandle),
    Open(OpenTargetControlHandle),
}

impl TargetControlHandle {
    fn as_trait(&self) -> &dyn ControlHandle {
        match self {
            TargetControlHandle::Closed(ctrl) => ctrl,
            TargetControlHandle::Ajar(ctrl) => ctrl,
            TargetControlHandle::Open(ctrl) => ctrl,
        }
    }

    fn as_open(&self) -> &OpenTargetControlHandle {
        match self {
            TargetControlHandle::Open(ctrl) => ctrl,
            _ => panic!("target is not OpenTarget"),
        }
    }
}

struct RunnerServer {
    target_ctrl: Option<TargetControlHandle>,
    target_serve_fut: Fuse<BoxFuture<'static, Result<(), ErrorExpectedUnderTest>>>,
    tag: TestTag,
}

impl RunnerServer {
    fn new() -> RunnerServer {
        RunnerServer { target_ctrl: None, target_serve_fut: Fuse::terminated(), tag: TestTag(None) }
    }

    fn target_ctrl(&self) -> &TargetControlHandle {
        self.target_ctrl.as_ref().expect(
            "the Runner is trying to access the Target either before Start() or after OnTeardown()",
        )
    }

    async fn serve(mut self, mut stream: RunnerRequestStream) {
        info!("Serving Runner...");
        loop {
            select! {
                request = stream.select_next_some() => {
                    self.handle_request(request.unwrap()).await;
                }
                result = &mut self.target_serve_fut => {
                    // TODO(https://fxbug.dev/42161447): The control handle keeps the Target channel alive.
                    // We must drop it otherwise the harness will hang waiting for PEER_CLOSED.
                    // This shouldn't be necessary: control handles should hold weak references.
                    self.target_ctrl = None;
                    info!("{} Target completed: {:?}", self.tag, &result);
                    let reason = get_teardown_reason(result.err());
                    info!("{} Sending OnTeardown event: {:?}", self.tag, reason);
                    stream.control_handle().send_on_teardown(reason).unwrap();
                }
                complete => break,
            }
        }
    }

    async fn handle_request(&mut self, request: RunnerRequest) {
        if let RunnerRequest::Start { test, .. } = request {
            self.tag = TestTag(Some(test));
        };
        info!("{} Handling Runner request: {}", self.tag, method_name(&request));
        match request {
            RunnerRequest::GetVersion { responder } => {
                responder.send(SERVER_SUITE_VERSION).unwrap();
            }
            RunnerRequest::CheckAlive { responder } => {
                responder.send().unwrap();
            }
            RunnerRequest::Start { test, any_target, responder } => {
                assert!(self.target_ctrl.is_none(), "must only call Start() once");
                if DISABLED_TESTS.contains(&test) {
                    responder.send(Err(StartError::TestDisabled)).unwrap();
                    return;
                }
                let runner_ctrl = responder.control_handle().clone();
                match any_target {
                    AnyTarget::Closed(target) => {
                        let (stream, ctrl) = target.into_stream_and_control_handle();
                        self.target_ctrl = Some(TargetControlHandle::Closed(ctrl));
                        self.target_serve_fut =
                            serve_closed_target(stream, runner_ctrl, self.tag).boxed().fuse();
                    }
                    AnyTarget::Ajar(target) => {
                        let (stream, ctrl) = target.into_stream_and_control_handle();
                        self.target_ctrl = Some(TargetControlHandle::Ajar(ctrl));
                        self.target_serve_fut =
                            serve_ajar_target(stream, runner_ctrl, self.tag).boxed().fuse();
                    }
                    AnyTarget::Open(target) => {
                        let (stream, ctrl) = target.into_stream_and_control_handle();
                        self.target_ctrl = Some(TargetControlHandle::Open(ctrl));
                        self.target_serve_fut =
                            serve_open_target(stream, runner_ctrl, self.tag).boxed().fuse();
                    }
                }
                responder.send(Ok(())).unwrap();
            }
            RunnerRequest::ShutdownWithEpitaph { epitaph_status, responder } => {
                let status = Status::ok(epitaph_status);
                self.target_ctrl().as_trait().shutdown_with_epitaph(status.into());
                responder.send().unwrap();
            }
            RunnerRequest::SendOpenTargetStrictEvent { responder } => {
                self.target_ctrl().as_open().send_strict_event().unwrap();
                responder.send().unwrap();
            }
            RunnerRequest::SendOpenTargetFlexibleEvent { responder } => {
                self.target_ctrl().as_open().send_flexible_event().unwrap();
                responder.send().unwrap();
            }
        }
    }
}

enum IncomingService {
    Runner(RunnerRequestStream),
}

#[fuchsia::main(logging_tags = ["rust"])]
async fn main() {
    let mut fs = ServiceFs::new();
    fs.dir("svc").add_fidl_service(IncomingService::Runner);
    fs.take_and_serve_directory_handle().unwrap();
    const MAX_CONCURRENT: usize = 10_000;
    info!("Rust serversuite server: ready!");
    fs.for_each_concurrent(MAX_CONCURRENT, |IncomingService::Runner(stream)| {
        RunnerServer::new().serve(stream)
    })
    .await;
}
