// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::create_proxy;
use fidl::prelude::*;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_power_system as fsystem;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_async as fasync;
use fuchsia_component::server as fserver;
use fuchsia_component_test::LocalComponentHandles;
use futures::channel::mpsc;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use log::info;

pub fn new_mocks_provider(
    is_power_framework_available: bool,
) -> (
    impl Fn(LocalComponentHandles) -> BoxFuture<'static, Result<(), anyhow::Error>>
    + Sync
    + Send
    + 'static,
    mpsc::UnboundedReceiver<Signal>,
) {
    let (send_signals, recv_signals) = mpsc::unbounded();

    let mock = move |handles: LocalComponentHandles| {
        run_mocks(send_signals.clone(), handles, is_power_framework_available).boxed()
    };

    (mock, recv_signals)
}

async fn run_mocks(
    send_signals: mpsc::UnboundedSender<Signal>,
    handles: LocalComponentHandles,
    is_power_framework_available: bool,
) -> Result<(), Error> {
    let mut fs = fserver::ServiceFs::new();

    if is_power_framework_available {
        let send_sag_signals = send_signals.clone();
        fs.dir("svc").add_fidl_service(move |stream| {
            fasync::Task::spawn(run_activity_governor(send_sag_signals.clone(), stream)).detach();
        });
    }

    let send_sys2_signals = send_signals.clone();
    fs.dir("svc").add_fidl_service(move |stream| {
        fasync::Task::spawn(run_sys2_system_controller(send_sys2_signals.clone(), stream)).detach();
    });

    // The black_hole directory points to a channel we will never answer, so that capabilities
    // provided from this directory will behave similarly to as if they were from an unlaunched
    // component.
    let (proxy, _server_end) = create_proxy::<fio::DirectoryMarker>();
    fs.add_remote("black_hole", proxy);

    fs.serve_connection(handles.outgoing_dir)?;
    fs.collect::<()>().await;

    Ok(())
}

#[derive(Debug)]
pub enum LeaseState {
    Acquired,
    Dropped,
}

#[derive(Debug)]
pub enum Signal {
    Sys2Shutdown(
        // The responder is held here to keep the current request and request stream alive, but is
        // not actively read, hence the 'dead_code' attribute.
        #[allow(dead_code)] fsys::SystemControllerShutdownResponder,
    ),
    ShutdownControlLease(LeaseState),
}

async fn run_activity_governor(
    send_signals: mpsc::UnboundedSender<Signal>,
    mut stream: fsystem::ActivityGovernorRequestStream,
) {
    info!("new connection to {}", fsystem::ActivityGovernorMarker::DEBUG_NAME);

    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fsystem::ActivityGovernorRequest::AcquireWakeLease { name: _, responder } => {
                let (client_token, server_token) = fsystem::LeaseToken::create();
                let send_signals2 = send_signals.clone();
                fasync::Task::spawn(async move {
                    fasync::OnSignals::new(&server_token, zx::Signals::EVENTPAIR_PEER_CLOSED)
                        .await
                        .unwrap();
                    send_signals2
                        .unbounded_send(Signal::ShutdownControlLease(LeaseState::Dropped))
                        .expect("receiver dropped");
                })
                .detach();
                send_signals
                    .unbounded_send(Signal::ShutdownControlLease(LeaseState::Acquired))
                    .expect("receiver acquired");
                responder.send(Ok(client_token)).unwrap();
            }
            _ => unreachable!("Unexpected request to ActivityGovernor"),
        }
    }
}

async fn run_sys2_system_controller(
    send_signals: mpsc::UnboundedSender<Signal>,
    mut stream: fsys::SystemControllerRequestStream,
) {
    info!("new connection to {}", fsys::SystemControllerMarker::DEBUG_NAME);
    async move {
        match stream.try_next().await? {
            Some(fsys::SystemControllerRequest::Shutdown { responder }) => {
                info!("Shutdown called");
                // Send the responder out with the signal.
                // The responder keeps the current request and the request stream alive.
                send_signals.unbounded_send(Signal::Sys2Shutdown(responder))?;
            }
            _ => (),
        }
        Ok(())
    }
    .unwrap_or_else(|e: anyhow::Error| {
        panic!("couldn't run {}: {:?}", fsys::SystemControllerMarker::PROTOCOL_NAME, e);
    })
    .await
}
