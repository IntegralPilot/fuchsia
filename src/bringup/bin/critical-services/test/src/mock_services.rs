// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

use anyhow::Error;
use fidl::prelude::*;
use fidl_fuchsia_hardware_hidbus as fhidbus;
use fidl_fuchsia_hardware_input as finput;
use fidl_fuchsia_hardware_power_statecontrol as statecontrol;
use fidl_fuchsia_test_pwrbtn as test_pwrbtn;
use fuchsia_async as fasync;
use fuchsia_component::server as fserver;
use futures::channel::mpsc;
use futures::{StreamExt, TryFutureExt, TryStreamExt};
use log::{info, warn};

fn event_handle_rights() -> zx::Rights {
    zx::Rights::BASIC | zx::Rights::SIGNAL
}

#[fuchsia::main(logging_tags = ["critical-services-mock-services"])]
async fn main() -> Result<(), Error> {
    info!("started");
    let (send_test_result, recv_test_result) = mpsc::channel(10);
    let mut recv_test_result = Some(recv_test_result);

    let event = zx::Event::create();
    let event_for_test_protocol = event.duplicate_handle(event_handle_rights())?;

    let mut fs = fserver::ServiceFs::new();
    fs.dir("svc").add_fidl_service(move |mut stream: test_pwrbtn::TestsRequestStream| {
        let recv_test_result = recv_test_result.take();
        fasync::Task::spawn(
            async move {
                info!("new connection to {}", test_pwrbtn::TestsMarker::DEBUG_NAME);
                match stream.try_next().await? {
                    Some(test_pwrbtn::TestsRequest::Run { responder }) => {
                        if let Some(mut recv_test_result) = recv_test_result {
                            let _ = recv_test_result.next().await;
                            responder.send()?;
                        } else {
                            panic!("Get already called");
                        }
                    }
                    _ => (),
                }
                Ok(())
            }
            .unwrap_or_else(|e: anyhow::Error| {
                panic!("couldn't run fuchsia.test.pwrbtn.Tests: {:?}", e);
            }),
        )
        .detach();
    });
    fs.dir("svc").add_fidl_service(move |mut stream: statecontrol::AdminRequestStream| {
        let mut send_test_result = send_test_result.clone();
        let event_for_test_protocol = event_for_test_protocol
            .duplicate_handle(event_handle_rights())
            .expect("failed to clone event");
        fasync::Task::spawn(
            async move {
                info!("new connection to {}", statecontrol::AdminMarker::DEBUG_NAME);
                match stream.try_next().await? {
                    Some(statecontrol::AdminRequest::Shutdown { responder, options: _ }) => {
                        // If we respond to critical-services it will go back to check the signals
                        // on the event we gave it, see that ZX_USER_SIGNAL_0 is still set, and
                        // call this again, and thus be stuck in a loop until the test is torn
                        // down. This isn't useful, and generates quite a bit of log noise at
                        // the end of the test. To avoid this, we need to clear the signal on
                        // the event.
                        event_for_test_protocol.signal(zx::Signals::USER_0, zx::Signals::NONE)?;

                        // Failing to send the response is fine, the critical-services code doesn't
                        // wait for a reply to this call and therefore it might have closed the
                        // channel by the time we try to send the reply.
                        let _ = responder.send(Ok(()));

                        send_test_result.try_send(()).expect("failed to send test completion");
                    }

                    Some(other) => {
                        panic!("only expecting calls to Shutdown, but got: {:?}", other);
                    }
                    None => {
                        // the connection closed
                    }
                }
                Ok(())
            }
            .unwrap_or_else(|e: anyhow::Error| {
                panic!("couldn't run fuchsia.power.hardware.statecontrol.Admin: {:?}", e);
            }),
        )
        .detach();
    });
    // A pseudo_directory must be used here because a ServiceFs does not support portions of
    // fuchsia.io required by `fdio_watch_directory`, which critical-services uses on this directory.
    let input_dir = vfs::pseudo_directory! {
        "mock_input_device" => vfs::service::endpoint(move |_, channel| {
            let event = event.duplicate_handle(event_handle_rights()).expect("failed to clone event");
            fasync::Task::spawn(
                async move {
                    info!("new connection to the mock input device");
                    let mut stream = finput::ControllerRequestStream::from_channel(channel);

                    while let Some(req) = stream.try_next().await? {
                        match req {
                            finput::ControllerRequest::OpenSession { control_handle: _, session } => {
                                let event = event.duplicate_handle(event_handle_rights()).expect("failed to clone event");
                                fasync::Task::spawn(
                                    async move {
                                        info!("new session to the mock input device");
                                        let mut stream = session.into_stream();

                                        while let Some(req) = stream.try_next().await? {
                                            match req {
                                                finput::DeviceRequest::GetReportDesc { responder } => {
                                                    info!("sending report desc");
                                                    // The following sequence of numbers is a HID report containing a
                                                    // power button press. This is used to convince critical-services that
                                                    // the power button has been pressed, and that it should begin a
                                                    // system power off.
                                                    responder.send(&[
                                                        0x05, 0x01, // Usage Page (Generic Desktop Ctrls)
                                                        0x09, 0x80, // Usage (Sys Control)
                                                        0xA1, 0x01, // Collection (Application)
                                                        0x09, 0x81, //   Usage (Sys Power Down)
                                                        0x15, 0x00, //   Logical Minimum (0)
                                                        0x25, 0x01, //   Logical Maximum (1)
                                                        0x75, 0x08, //   Report Size (8)
                                                        0x95, 0x01, //   Report Count (1)
                                                        0x81,
                                                        0x02, //   Input (Data,Var,Abs,No Wrap,Linear,Preferred State,No Null Position)
                                                        0xC0, // End Collection
                                                    ]).expect("failed sending report desc");
                                                }
                                                finput::DeviceRequest::GetReportsEvent { responder } => {
                                                    info!("sending reports event and signalling the event");
                                                    let event_dup = event.duplicate_handle(event_handle_rights())?;
                                                    responder.send(Ok(event_dup))
                                                        .expect("failed sending reports event");
                                                    event.signal(zx::Signals::NONE, zx::Signals::USER_0)?;
                                                }
                                                finput::DeviceRequest::ReadReport { responder } => {
                                                    info!("sending report");
                                                    let msg = &[1]; // 1 means "power off", 0 would mean "don't power off"
                                                    responder.send(Ok(fhidbus::Report {buf: Some(msg.to_vec()),
                                                         timestamp: Some(zx::MonotonicInstant::get().into_nanos()),
                                                         ..Default::default()})
                                                    ).unwrap_or_else(|e| {
                                                        warn!("failed sending response to ReadReport: {:?}", e);
                                                    });
                                                }
                                                _ => panic!("unexpected call to fuchsia.hardware.input.Device"),
                                            }
                                        }
                                        Ok(())
                                    }
                                    .unwrap_or_else(|e: anyhow::Error| {
                                        panic!("couldn't run fuchsia.hardware.input.Device: {:?}", e);
                                    })
                                ).detach()
                            }
                        }
                    }
                    Ok(())
                }
                .unwrap_or_else(|e: anyhow::Error| {
                    panic!("couldn't run fuchsia.hardware.input.Device: {:?}", e);
                })
            ).detach()
        }),
    };
    fs.add_remote(
        "input",
        vfs::directory::serve_read_only(input_dir, vfs::execution_scope::ExecutionScope::new()),
    );
    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;

    Ok(())
}
