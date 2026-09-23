// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "1024"]

use anyhow::{Context, Error, format_err};
use fidl_map::MessagingClientRequestStream;
use fuchsia_bluetooth::profile::ProtocolDescriptor;
use fuchsia_bluetooth::types::{Channel, PeerId};
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::health::Reporter;
use futures::channel::mpsc::{self, UnboundedReceiver};
use futures::stream::FuturesUnordered;
use futures::{StreamExt, future, pin_mut};
use log::{error, info, warn};
use profile_client::{ProfileClient, ProfileEvent};
use {fidl_fuchsia_bluetooth_bredr as bredr, fidl_fuchsia_bluetooth_map as fidl_map};

mod message_access_service;
mod message_notification_service;
mod messaging_client;
mod profile;

use messaging_client::{AccessorTasks, MessagingClient};
use profile::MasConfig;

/// The maximum number of FIDL service client connections that will be serviced concurrently.
const MAX_CONCURRENT_CONNECTIONS: usize = 10;

/// All FIDL services that are exposed by this component's ServiceFs.
enum Service {
    MessagingClient(MessagingClientRequestStream),
}

// Remote MSE peer can assume two roles. One as a client of the Message Notification Service and
// the other as the server of the Message Access Service.
enum Peer {
    MnsClient(PeerId, Vec<ProtocolDescriptor>, Channel),
    MasServer(PeerId, MasConfig),
}

impl TryFrom<ProfileEvent> for Peer {
    type Error = anyhow::Error;

    fn try_from(value: ProfileEvent) -> Result<Peer, Error> {
        match value {
            ProfileEvent::PeerConnected { id, protocol, channel } => {
                let protocol = protocol
                    .iter()
                    .map(|p| ProtocolDescriptor::try_from(p))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format_err!("{e:?}"))?;

                return Ok(Peer::MnsClient(id, protocol, channel));
            }
            ProfileEvent::SearchResult { id, protocol, attributes } => {
                let Some(protocol) = protocol else {
                    return Err(format_err!("Received peer ({id:}) with no protocol"));
                };

                match MasConfig::from_search_result(protocol, attributes) {
                    Ok(mas_config) => return Ok(Peer::MasServer(id, mas_config)),
                    Err(e) => return Err(e.into()),
                };
            }
        }
    }
}

async fn run_messaging_client(
    mut profile_client: ProfileClient,
    mut messaging_client: MessagingClient,
    mut fidl_stream_receiver: UnboundedReceiver<MessagingClientRequestStream>,
) {
    // Tracks all the running Accessor and NotificationRelayer services.
    let mut accessor_service_futs = FuturesUnordered::new();
    let mut mns_signal_process_futs = FuturesUnordered::new();

    loop {
        futures::select! {
            request = profile_client.select_next_some() => {
                if let Err(e) = request {
                    warn!(e:%; "Failed to get profile client event");
                    break;
                }
                let maybe_peer: Result<Peer, _> = request.unwrap().try_into();
                match maybe_peer {
                    Err(e) => warn!(e:?; "Failed to process profile event"),
                    Ok(Peer::MnsClient(id, protocol, channel)) => {
                        info!(peer_id:% = id; "Received connection from MNS client");
                        if let Err(e) = messaging_client.new_mns_connection(id, protocol, channel).await {
                            warn!(peer_id:% = id, e:%; "Failed to establish MNS connection");
                        }
                    }
                    Ok(Peer::MasServer(id, mas_config)) => {
                        if let Err(e) = messaging_client.connect_new_mas(id, mas_config).await {
                            warn!(peer_id:% = id, e:%; "Could not connect to MAS");
                            continue;
                        }
                        info!(peer_id:% = id; "Connected to new MAS");
                    }
                }
            }
            stream = fidl_stream_receiver.select_next_some() => {
                if let Err(e) = messaging_client.set_fidl_stream(stream) {
                    warn!(e:?; "Failed to set client connection");
                }
            }
            tasks = messaging_client.select_next_some() => {
                info!("Running a new Accessor FIDL server");
                let AccessorTasks{fidl_task, mns_signal_process_task} = tasks;
                accessor_service_futs.push(fidl_task);
                mns_signal_process_futs.push(mns_signal_process_task);
            }
            peer_id = accessor_service_futs.select_next_some() => {
                info!(peer_id:%; "Accessor FIDL server terminated");
            }
            _ = mns_signal_process_futs.select_next_some() => {},
            complete => break,
        }
    }
}

#[fuchsia::main(logging_tags = ["bt-map-mce"])]
async fn main() -> Result<(), Error> {
    // Connect to Profile service.
    let profile_proxy = fuchsia_component::client::connect_to_protocol::<bredr::ProfileMarker>()
        .context("Failed to connect to Bluetooth Profile service")?;

    // First advertise ourselves as MCE and look for MSE peers.
    let profile_client = profile::connect_and_advertise(profile_proxy.clone())
        .context("Unable to connect to BrEdr Profile Service")?;

    let (fidl_stream_sender, fidl_stream_receiver) = mpsc::unbounded();

    // Run the messaging client.
    let messaging_client = MessagingClient::new(profile_proxy);
    let mce_fut = run_messaging_client(profile_client, messaging_client, fidl_stream_receiver);
    pin_mut!(mce_fut);

    // Run the fidl service to accept incoming fidl requests.
    let mut fs = ServiceFs::new();
    let _ = fs.dir("svc").add_fidl_service(Service::MessagingClient);
    let _ = fs.take_and_serve_directory_handle().context("Failed to serve ServiceFs directory")?;

    fuchsia_inspect::component::health().set_ok();
    let fidl_fut = fs.for_each_concurrent(
        MAX_CONCURRENT_CONNECTIONS,
        |Service::MessagingClient(stream)| async {
            let _ = fidl_stream_sender
                .unbounded_send(stream)
                .inspect_err(|e| warn!(e:%; "failed to send MessagingClientRequestStream"));
        },
    );
    pin_mut!(fidl_fut);

    info!("MAP Messaging Client Equipment running.");

    match future::select(mce_fut, fidl_fut).await {
        future::Either::Left((_, _)) => {
            error!("MessagingClient server loop finished unexpectedly");
        }
        future::Either::Right((result, _)) => {
            error!("Service FS unexpectedly finished: {:?}. Exiting", result);
        }
    }
    Ok(())
}
