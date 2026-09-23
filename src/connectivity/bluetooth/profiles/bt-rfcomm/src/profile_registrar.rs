// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use async_utils::stream::{StreamItem, StreamWithEpitaph, Tagged, WithEpitaph, WithTag};
use bt_rfcomm::ServerChannel;
use core::pin::Pin;
use core::task::{Context, Poll};
use fidl::endpoints::{RequestStream, create_request_stream};
use fidl_fuchsia_bluetooth::{self as fidl_bt, ErrorCode};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_async as fasync;
use fuchsia_bluetooth::profile::{
    ChannelParameters, ProtocolDescriptor, Psm, ServiceDefinition, l2cap_connect_parameters,
    psm_from_protocol,
};
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect_derive::Inspect;
use futures::channel::mpsc;
use futures::select;
use futures::stream::{FusedStream, Stream, StreamExt};
use log::{info, trace, warn};
use std::collections::HashSet;
use std::ops::RangeFrom;

use crate::fidl_service::Service;
use crate::profile::*;
use crate::rfcomm::RfcommServer;
use crate::types::{ServiceGroup, ServiceGroupHandle, Services};

/// Unique identifier assigned to the currently active managed advertisement ID.
type ManagedAdvertisementId = u64;

/// A stream associated with a FIDL client's `bredr.Advertise` request. Produces an event any time
/// the client's `ConnectionReceiver` stream produces an event.
/// On stream termination, returns the unique `ServiceGroupHandle` associated with this FIDL client.
type ManagedAdvertisementEventStream = StreamWithEpitaph<
    Tagged<ServiceGroupHandle, bredr::ConnectionReceiverEventStream>,
    ServiceGroupHandle,
>;

/// An advertisement made with the upstream `Profile` service that is managed by this server.
struct ManagedAdvertisement {
    /// Unique identifier describing this advertisement.
    id: ManagedAdvertisementId,
    /// The control handle associated with the `ConnectionReceiver` stream for this advertisement.
    /// Used to send an `OnRevoke` event to cancel this advertisement.
    connection_receiver_handle: bredr::ConnectionReceiverControlHandle,
    /// The connection relay that processes incoming `bredr.ConnectionReceiver.Connect` requests
    /// from the upstream `Profile` server. These represent incoming L2CAP connections made by a
    /// remote peer.
    connection_relay:
        StreamWithEpitaph<bredr::ConnectionReceiverRequestStream, ManagedAdvertisementId>,
}

/// The current advertisement status of the `ProfileRegistrar`.
#[derive(Default)]
enum AdvertiseStatus {
    /// We are not currently advertising.
    #[default]
    NotAdvertising,
    /// Currently advertising with a managed set of services. Incoming L2CAP/RFCOMM connections
    /// will be forwarded to the appropriate FIDL client of the `bt-rfcomm` server.
    /// Use the `AdvertiseStatus::Stream` implementation to receive connections.
    Advertising(ManagedAdvertisement),
}

impl Stream for AdvertiseStatus {
    type Item = StreamItem<
        <bredr::ConnectionReceiverRequestStream as Stream>::Item,
        ManagedAdvertisementId,
    >;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = Pin::into_inner(self);
        match this {
            Self::NotAdvertising => Poll::Ready(None),
            Self::Advertising(ManagedAdvertisement { connection_relay, .. }) => {
                connection_relay.poll_next_unpin(cx)
            }
        }
    }
}

impl FusedStream for AdvertiseStatus {
    fn is_terminated(&self) -> bool {
        match &self {
            Self::NotAdvertising => true,
            Self::Advertising(ManagedAdvertisement { connection_relay, .. }) => {
                connection_relay.is_terminated()
            }
        }
    }
}

impl AdvertiseStatus {
    fn id(&self) -> Option<ManagedAdvertisementId> {
        match &self {
            Self::NotAdvertising => None,
            Self::Advertising(managed) => Some(managed.id),
        }
    }
}

fn get_parameters_from_advertise_request(
    payload: bredr::ProfileAdvertiseRequest,
) -> Result<(bredr::ConnectionReceiverProxy, ChannelParameters), Error> {
    let receiver = payload.receiver.unwrap().into_proxy();
    let parameters = ChannelParameters::try_from(&payload.parameters.unwrap_or_default())?;
    Ok((receiver, parameters))
}

fn advertise_response(services: Vec<bredr::ServiceDefinition>) -> bredr::ProfileAdvertiseResponse {
    bredr::ProfileAdvertiseResponse { services: Some(services), ..Default::default() }
}

/// The ProfileRegistrar handles requests over the `fidl.fuchsia.bluetooth.bredr.Profile` service.
/// Clients can advertise, search, and connect to services.
/// The ProfileRegistrar can be thought of as a relay for clients using Profile. Clients not
/// requesting RFCOMM services will be relayed directly to the upstream server.
///
/// The ProfileRegistrar manages a single RFCOMM advertisement group. If a new client
/// requests to advertise services, the server will unregister the active advertisement, group
/// together all the services, and re-register.
/// The ProfileRegistrar also manages the `RfcommServer` - which is responsible for handling
/// connections over the RFCOMM PSM.
#[derive(Inspect)]
pub struct ProfileRegistrar {
    /// A connection to the upstream provider of the `bredr.Profile` service. Typically bt-host.
    profile_upstream: bredr::ProfileProxy,
    /// The next available unique identifier for a managed advertisement. Does not necessarily mean
    /// we are advertising.
    next_id: RangeFrom<ManagedAdvertisementId>,
    /// The current advertisement status of the registrar. Manages the advertisement and holds the
    /// connection relay stream which receives incoming L2CAP/RFCOMM connections and forwards to
    /// the appropriate FIDL client.
    active_registration: AdvertiseStatus,
    /// The set of service definitions that are being advertised to the upstream `bredr.Profile`
    /// server. This is a collection of services from various FIDL clients of this registrar.
    registered_services: Services,
    /// The server that handles allocating RFCOMM server channels, incoming/outgoing connections,
    /// and multiplexing client channels over the RFCOMM L2CAP PSM.
    #[inspect(forward)]
    rfcomm_server: RfcommServer,
}

impl ProfileRegistrar {
    pub fn new(profile_upstream: bredr::ProfileProxy) -> Self {
        Self {
            profile_upstream,
            next_id: RangeFrom { start: 0 },
            active_registration: AdvertiseStatus::default(),
            registered_services: Services::new(),
            rfcomm_server: RfcommServer::new(),
        }
    }

    /// Creates and returns a Task representing the running server. The returned task will process
    /// `Profile` and `RfcommTest` requests from the `incoming_services` stream.
    pub fn start(self, incoming_services: mpsc::Receiver<Service>) -> fasync::Task<()> {
        let handler_fut = self.handle_fidl_requests(incoming_services);
        fasync::Task::spawn(handler_fut)
    }

    fn get_next_id(&mut self) -> ManagedAdvertisementId {
        self.next_id.next().unwrap()
    }

    /// Returns true if the requested `new_psms` do not overlap with the currently registered PSMs.
    fn is_disjoint_psms(&self, new_psms: &HashSet<Psm>) -> bool {
        self.registered_services.psms().is_disjoint(new_psms)
    }

    /// Unregisters all the active services advertised by this server.
    /// This should be called when the upstream server drops the single service advertisement that
    /// this server manages.
    async fn unregister_all_services(&mut self) {
        self.registered_services = Services::new();
        self.rfcomm_server.free_all_server_channels().await;
        let _ = self.refresh_advertisement().await;
    }

    /// Unregisters the group of services identified by `handle`. Re-registers any remaining
    /// services.
    /// This should be called when a profile client decides to stop advertising its services.
    async fn unregister_service(&mut self, handle: ServiceGroupHandle) -> Result<(), Error> {
        let Some(service_info) = self.registered_services.remove(handle) else {
            return Ok(());
        };

        self.rfcomm_server.free_server_channels(service_info.allocated_server_channels()).await;

        // Attempt to re-advertise - the returned services can be ignored since the registrar
        // has evicted the entry for this `handle`.
        let _advertised_services =
            self.refresh_advertisement().await.map_err(|e| format_err!("{e:?}"))?;
        Ok(())
    }

    /// Processes an incoming L2cap connection from the upstream server.
    ///
    /// Relays the connection directly to the FIDL client if the protocol is not RFCOMM.
    ///
    /// Returns an error if the `protocol` is invalidly formatted, or if the provided
    /// PSM is not represented by a client of the `ProfileRegistrar`.
    fn handle_incoming_l2cap_connection(
        &mut self,
        peer_id: PeerId,
        channel: bredr::Channel,
        protocol: Vec<bredr::ProtocolDescriptor>,
    ) -> Result<(), Error> {
        trace!(peer_id:%, protocol:?; "Incoming L2CAP connection request");
        let local = protocol
            .iter()
            .map(|p| ProtocolDescriptor::try_from(p))
            .collect::<Result<Vec<_>, _>>()?;
        match psm_from_protocol(&local).ok_or_else(|| format_err!("No PSM provided"))? {
            Psm::RFCOMM => self.rfcomm_server.new_l2cap_connection(peer_id, channel.try_into()?),
            psm => {
                match self.registered_services.iter().find(|(_, client)| client.contains_psm(psm)) {
                    Some((_, client)) => client.relay_connected(peer_id, channel, protocol),
                    None => Err(format_err!("Connection request for non-advertised PSM {psm:?}")),
                }
            }
        }
    }

    /// Validates that there is an active connection with the peer specified by `peer_id`. If not,
    /// creates and delivers the L2CAP connection to the RFCOMM server.
    async fn ensure_service_connection(&mut self, peer_id: PeerId) -> Result<(), ErrorCode> {
        if self.rfcomm_server.is_active_session(&peer_id) {
            return Ok(());
        }

        let l2cap_channel = match self
            .profile_upstream
            .connect(
                &peer_id.into(),
                &l2cap_connect_parameters(Psm::RFCOMM, fidl_bt::ChannelMode::Basic),
            )
            .await
        {
            Ok(Ok(channel)) => channel.try_into().unwrap(),
            Ok(Err(e)) => {
                return Err(e);
            }
            Err(e) => {
                warn!(peer_id:%; "Couldn't establish L2CAP connection {e:?}");
                return Err(ErrorCode::Failed);
            }
        };
        self.rfcomm_server
            .new_l2cap_connection(peer_id, l2cap_channel)
            .map_err(|_| ErrorCode::Failed)
    }

    /// Handles an outgoing L2Cap connection request made by a client of the ProfileRegistrar.
    ///
    /// Returns an error if the connection request fails.
    async fn handle_outgoing_connection(
        &mut self,
        peer_id: PeerId,
        connection: bredr::ConnectParameters,
        responder: bredr::ProfileConnectResponder,
    ) -> Result<(), Error> {
        trace!(peer_id:%; "Making outgoing connection request {connection:?}");
        // If the provided `connection` is for a non-RFCOMM PSM, simply forward the outbound
        // connection to the upstream Profile service.
        // Otherwise, route to the RFCOMM server.
        match &connection {
            bredr::ConnectParameters::L2cap { .. } => {
                let result = self
                    .profile_upstream
                    .connect(&peer_id.into(), &connection)
                    .await
                    .unwrap_or_else(|_fidl_error| Err(ErrorCode::Failed));
                let _ = responder.send(result);
            }
            bredr::ConnectParameters::Rfcomm(bredr::RfcommParameters { channel, .. }) => {
                let Some(Ok(server_channel)) = channel.map(ServerChannel::try_from) else {
                    // Missing or invalid RFCOMM channel number.
                    let _ = responder.send(Err(ErrorCode::InvalidArguments));
                    return Ok(());
                };

                // Ensure there is an L2CAP connection for the RFCOMM PSM between us and the peer.
                if let Err(e) = self.ensure_service_connection(peer_id).await {
                    let _ = responder.send(Err(e));
                    return Ok(());
                }
                // Open the specific RFCOMM channel.
                self.rfcomm_server.open_rfcomm_channel(peer_id, server_channel, responder).await?;
            }
        }
        Ok(())
    }

    /// Refreshes the current advertisement with the upstream `Profile` service.
    /// Returns the new set of services that is currently advertising. This list can be empty if
    /// there are no services to advertise or we are not advertising.
    async fn refresh_advertisement(&mut self) -> Result<Vec<ServiceDefinition>, Error> {
        let status =
            std::mem::replace(&mut self.active_registration, AdvertiseStatus::NotAdvertising);
        if let AdvertiseStatus::Advertising(ManagedAdvertisement {
            id,
            connection_receiver_handle,
            connection_relay,
        }) = status
        {
            // Revoke the current advertisement and wait for the upstream Profile server to
            // process the cancellation. The `connection_relay` will terminate after
            // the upstream has unregistered the advertisement.
            trace!(id; "Unregistering existing advertisement");
            let _ = connection_receiver_handle.send_on_revoke();
            connection_relay.map(|_| ()).collect::<()>().await;
            trace!(id; "Finished waiting for unregistration");
        }

        // We are ready to advertise. Attempt to build the advertisement parameters and set up the
        // managed advertisement.
        let Some(params) = self.registered_services.build_registration() else {
            trace!("No services to advertise");
            return Ok(vec![]);
        };
        let id = self.get_next_id();
        trace!(id:?, params:?; "Refreshing advertisement from registered services");
        let (connect_client, connect_requests) =
            create_request_stream::<bredr::ConnectionReceiverMarker>();
        // The control handle will be used to revoke the advertisement any time a refresh is
        // requested.
        let connection_receiver_handle = connect_requests.control_handle();
        let advertise_request = params.into_advertise_request(connect_client);
        let advertise_result = self
            .profile_upstream
            .advertise(advertise_request)
            .await?
            .map_err(|e| format_err!("{e:?}"))?;
        let advertised_services = ServiceDefinition::try_from_fidl(
            &advertise_result.services.expect("included in response"),
        )?;
        self.active_registration = AdvertiseStatus::Advertising(ManagedAdvertisement {
            id,
            connection_receiver_handle,
            connection_relay: connect_requests.with_epitaph(id),
        });
        info!(id:?, advertised_services:?; "Advertising via the upstream `Profile` server");
        Ok(advertised_services)
    }

    /// Handles a request to advertise a group of profile `services` that will be managed by the
    /// RFCOMM server.
    ///
    /// At least one service in `services` must request RFCOMM. The RFCOMM-requesting services are
    /// assigned ServerChannels. The services are registered together with any existing RFCOMM
    /// services. Assumes `services` contain valid `ServiceDefinition` records.
    ///
    /// Returns a stream of `ConnectionReceiverEvent`s. The event stream should be actively polled
    /// in order to detect when the client terminates the advertisement.
    async fn add_managed_advertisement(
        &mut self,
        mut services: Vec<ServiceDefinition>,
        parameters: ChannelParameters,
        receiver: bredr::ConnectionReceiverProxy,
        responder: bredr::ProfileAdvertiseResponder,
    ) -> Result<ManagedAdvertisementEventStream, Error> {
        // The requested PSMs must be disjoint from the existing set of PSMs as only one group
        // can allocate a specific PSM.
        let new_psms = psms_from_service_definitions(&services);
        trace!("Adding managed advertisement with PSMs: {new_psms:?}");
        if !self.is_disjoint_psms(&new_psms) {
            let _ = responder.send(Err(ErrorCode::Failed));
            return Err(format_err!("PSMs already allocated"));
        }

        let current_services = self.registered_services.service_definitions();

        // If the RfcommServer has enough free Server Channels, allocate and update
        // the RFCOMM-requesting services.
        let required_server_channels =
            services.iter().filter(|def| is_rfcomm_service_definition(def)).count();
        if required_server_channels > self.rfcomm_server.available_server_channels().await {
            let _ = responder.send(Err(ErrorCode::Failed));
            return Err(format_err!("Insufficient space in the RfcommServer"));
        }
        for mut service in services.iter_mut().filter(|def| is_rfcomm_service_definition(def)) {
            let server_channel = self
                .rfcomm_server
                .allocate_server_channel(receiver.clone())
                .await
                .expect("just checked");
            update_svc_def_with_server_channel(&mut service, server_channel)?;
        }

        // Save the services for the new FIDL client and refresh the advertisement.
        let mut entry = ServiceGroup::new(receiver.clone(), parameters);
        entry.set_service_defs(services);
        let next_handle = self.registered_services.insert(entry);
        let advertised_services = match self.refresh_advertisement().await {
            Ok(services) => services,
            Err(e) => {
                warn!(
                    next_handle:?;
                    "Upstream advertise failed; rolling back registration: {e:?}"
                );
                if let Err(rollback_err) = self.unregister_service(next_handle).await {
                    warn!(
                        "Failed to unregister and restore existing advertisements after rollback: {rollback_err:?}"
                    );
                }
                let _ = responder.send(Err(ErrorCode::Failed));
                return Err(e);
            }
        };
        // The returned `advertised_services` contains entries for all FIDL clients. The diff only
        // contains the services requested by this FIDL client.
        let new_advertised_services =
            service_def_difference(&current_services, &advertised_services);
        let new_advertised_services_fidl =
            ServiceDefinition::try_into_fidl(&new_advertised_services)
                .expect("valid local definitions");
        // Update the local cache with the "new" view of the FIDL client's advertised services. This
        // is a no-op if no dynamic PSMs were requested.
        self.registered_services
            .get_mut(next_handle)
            .expect("just inserted")
            .set_service_defs(new_advertised_services);
        let _ = responder.send(Ok(&advertise_response(new_advertised_services_fidl)));

        let client_event_stream =
            receiver.take_event_stream().tagged(next_handle).with_epitaph(next_handle);
        Ok(client_event_stream)
    }

    /// Forwards the `Profile.Advertise` `request` to the upstream Profile server.
    async fn forward_advertisement_upstream(
        profile_upstream: bredr::ProfileProxy,
        request: bredr::ProfileAdvertiseRequest,
        responder: bredr::ProfileAdvertiseResponder,
    ) {
        let result = profile_upstream
            .advertise(request)
            .await
            .unwrap_or(Err(fidl_fuchsia_bluetooth::ErrorCode::ProtocolError));
        let _ = responder.send(result.as_ref().map_err(|e| *e));
    }

    /// Handles a request over the `bredr.Profile` protocol.
    ///
    /// Returns Some<T> if the request requires a managed BR/EDR advertisement, None otherwise.
    async fn handle_profile_request(
        &mut self,
        request: bredr::ProfileRequest,
    ) -> Option<ManagedAdvertisementEventStream> {
        match request {
            bredr::ProfileRequest::Advertise {
                payload:
                    original_payload @ bredr::ProfileAdvertiseRequest {
                        services: Some(_),
                        receiver: Some(_),
                        ..
                    },
                responder,
            } => {
                let Ok(services_local) =
                    ServiceDefinition::try_from_fidl(original_payload.services.as_ref().unwrap())
                else {
                    let _ =
                        responder.send(Err(fidl_fuchsia_bluetooth::ErrorCode::InvalidArguments));
                    return None;
                };
                trace!("Advertise request: {services_local:?}");

                // Validate that all ServiceDefinitions request RFCOMM at most once while checking
                // whether any service in the advertisement requests RFCOMM.
                let requests_rfcomm = match service_definitions_request_rfcomm(&services_local) {
                    Ok(requests_rfcomm) => requests_rfcomm,
                    Err(e) => {
                        warn!("Advertise request contains invalid ServiceDefinition: {e}");
                        let _ = responder
                            .send(Err(fidl_fuchsia_bluetooth::ErrorCode::InvalidArguments));
                        return None;
                    }
                };

                // Non-RFCOMM requesting advertisements can be forwarded directly to the upstream
                // Profile server.
                if !requests_rfcomm {
                    Self::forward_advertisement_upstream(
                        self.profile_upstream.clone(),
                        original_payload,
                        responder,
                    )
                    .await;
                    return None;
                }

                // Otherwise, we need to locally manage the advertisement by setting up the RFCOMM
                // service.
                let result = get_parameters_from_advertise_request(original_payload);
                let Ok((receiver, parameters)) = result else {
                    warn!("Invalid parameters in `Advertise` request: {result:?}");
                    let _ =
                        responder.send(Err(fidl_fuchsia_bluetooth::ErrorCode::InvalidArguments));
                    return None;
                };

                return match self
                    .add_managed_advertisement(services_local, parameters, receiver, responder)
                    .await
                {
                    Ok(evt_stream) => Some(evt_stream),
                    Err(e) => {
                        warn!("Failed to handle `Advertise` request: {e:?}");
                        None
                    }
                };
            }
            bredr::ProfileRequest::Advertise { responder, .. } => {
                // Otherwise, the Advertise `payload` is missing a required field.
                let _ = responder.send(Err(fidl_fuchsia_bluetooth::ErrorCode::InvalidArguments));
            }
            bredr::ProfileRequest::Connect { peer_id, connection, responder, .. } => {
                let peer_id = peer_id.into();
                if let Err(e) =
                    self.handle_outgoing_connection(peer_id, connection, responder).await
                {
                    warn!(peer_id:%; "Error making outgoing connection: {e:?}");
                }
            }
            bredr::ProfileRequest::Search { payload, .. } => {
                // Searches are handled directly by the upstream Profile Server.
                let _ = self.profile_upstream.search(payload);
            }
            bredr::ProfileRequest::ConnectSco { payload, .. } => {
                let _ = self.profile_upstream.connect_sco(payload);
            }
            bredr::ProfileRequest::_UnknownMethod { .. } => {
                warn!("ProfileRequest: received unknown method");
            }
        }
        None
    }

    /// Handles incoming L2CAP/RFCOMM connect requests from the receiver of the active
    /// advertisement.
    /// Returns Ok(true) if the upstream `Profile` server terminated the advertisement, Ok(false)
    /// otherwise.
    /// Returns Error if the request couldn't be processed.
    async fn handle_connection_request(
        &mut self,
        request: StreamItem<
            Result<bredr::ConnectionReceiverRequest, fidl::Error>,
            ManagedAdvertisementId,
        >,
    ) -> Result<bool, Error> {
        use bredr::ConnectionReceiverRequest::*;
        match request {
            StreamItem::Item(Ok(Connected { peer_id, channel, protocol, .. })) => {
                self.handle_incoming_l2cap_connection(peer_id.into(), channel, protocol)?;
            }
            StreamItem::Item(Ok(_UnknownMethod { .. })) => {
                return Err(format_err!("`ConnectionReceiverRequest` unknown method"));
            }
            StreamItem::Item(Err(e)) => {
                return Err(format_err!("Error in `ConnectionReceiver` stream: {e:?}"));
            }
            StreamItem::Epitaph(id) => {
                // If the `id` matches the current advertisement, then the upstream unexpectedly
                // canceled the advertisement and we need to notify our FIDL clients. Otherwise, it
                // was an older advertisement and no action is needed (typically after we issue an
                // `OnRevoke`).
                if Some(id) == self.active_registration.id() {
                    trace!("Upstream `Profile` server unexpectedly terminated advertisement");
                    self.unregister_all_services().await;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Processes incoming service requests and events generated by clients of the
    /// `Profile` protocol.
    pub async fn handle_fidl_requests(mut self, mut incoming_services: mpsc::Receiver<Service>) {
        // `bredr.Profile` FIDL requests.
        let mut profile_requests = futures::stream::SelectAll::new();
        // `RfcommTest` FIDL requests.
        let mut test_requests = futures::stream::SelectAll::new();

        // A collection of event streams. Each stream is the `ManagedAdvertisementEventStream` tied
        // to a FIDL client's BR/EDR service advertisement request. The FIDL client can terminate
        // its advertisement by issuing a `ConnectionReceiver.OnRevoke` event.
        // Only used for service advertisements that are managed by `bt-rfcomm`.
        let mut client_event_streams = futures::stream::SelectAll::new();

        loop {
            select! {
                new_service = incoming_services.select_next_some() => {
                    match new_service {
                        Service::Profile(st) => profile_requests.push(st),
                        Service::RfcommTest(st) => test_requests.push(st),
                    }
                }
                profile_request = profile_requests.select_next_some() => {
                    let Ok(request) =  profile_request else {
                        info!("Error from Profile request: {profile_request:?}");
                        continue;
                    };
                    if let Some(evt_stream) = self.handle_profile_request(request).await {
                        client_event_streams.push(evt_stream);
                    }
                }
                connection_req = self.active_registration.select_next_some() => {
                    match self.handle_connection_request(connection_req).await {
                        Ok(true) => client_event_streams.clear(),
                        Ok(false) => {},
                        Err(e) => warn!("Error processing incoming l2cap connection request: {e:?}"),
                    }
                }
                client_event = client_event_streams.next() => {
                    // `Profile` client has terminated their service advertisement.
                    let (service_id, log_str) = match client_event {
                        Some(StreamItem::Item((id, Ok(bredr::ConnectionReceiverEvent::OnRevoke {})))) => (id, "revoked"),
                        Some(StreamItem::Epitaph(id)) => (id, "closed"),
                        _ => continue,
                    };
                    info!(service_id:?; "Client {log_str} service advertisement");
                    // Unregister the service from the ProfileRegistrar.
                    if let Err(e) = self.unregister_service(service_id).await {
                        warn!(service_id:?; "Error unregistering service: {e:?}");
                    }
                }
                request = test_requests.select_next_some() => {
                    match request {
                        Ok(req) => self.rfcomm_server.handle_test_request(req).await,
                        Err(e) => warn!("Handling `RfcommTest` request error: {e:?}"),
                    }
                }
                complete => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::types::tests::{
        l2cap_protocol, obex_service_definition, other_service_definition,
        rfcomm_service_definition,
    };

    use assert_matches::assert_matches;
    use async_test_helpers::{
        expect_stream_item, expect_stream_pending, expect_stream_terminated, run_while,
    };
    use async_utils::PollExt;
    use fidl::endpoints::{create_proxy, create_proxy_and_stream};
    use fuchsia_bluetooth::profile::DataElement;
    use futures::{Future, FutureExt, SinkExt};
    use std::pin::pin;

    /// Returns true if the provided `service` has an assigned Server Channel.
    fn service_def_has_assigned_server_channel(service: &bredr::ServiceDefinition) -> bool {
        if let Some(protocol) = &service.protocol_descriptor_list {
            for descriptor in protocol {
                if descriptor.protocol == Some(bredr::ProtocolIdentifier::Rfcomm) {
                    let Some(params) = descriptor.params.as_ref() else {
                        return false;
                    };
                    if params.is_empty() {
                        return false;
                    }
                    // The server channel should be the first element.
                    if let bredr::DataElement::Uint8(_) = params[0] {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Creates a Profile::Search request.
    fn generate_search_request(
        exec: &mut fasync::TestExecutor,
    ) -> (bredr::ProfileRequest, bredr::SearchResultsRequestStream) {
        let (c, mut s) = create_proxy_and_stream::<bredr::ProfileMarker>();
        let (results, server) = create_request_stream::<bredr::SearchResultsMarker>();

        let search_result = c.search(bredr::ProfileSearchRequest {
            service_uuid: Some(bredr::ServiceClassProfileIdentifier::AudioSink),
            results: Some(results),
            ..Default::default()
        });
        assert!(search_result.is_ok());

        match expect_stream_item(exec, &mut s) {
            Ok(req) => (req, server),
            x => panic!("Expected ProfileRequest but got: {x:?}"),
        }
    }

    /// Creates a Profile::ConnectSco request.
    fn generate_connect_sco_request(
        exec: &mut fasync::TestExecutor,
    ) -> (bredr::ProfileRequest, bredr::ScoConnectionProxy) {
        let (profile_proxy, mut profile_request_stream) =
            create_proxy_and_stream::<bredr::ProfileMarker>();
        let (connection_proxy, connection_server) = create_proxy::<bredr::ScoConnectionMarker>();

        assert!(
            profile_proxy
                .connect_sco(bredr::ProfileConnectScoRequest {
                    peer_id: Some(PeerId(1).into()),
                    initiator: Some(true),
                    params: Some(vec![bredr::ScoConnectionParameters::default()]),
                    connection: Some(connection_server),
                    ..Default::default()
                })
                .is_ok()
        );

        match expect_stream_item(exec, &mut profile_request_stream) {
            Ok(request) => (request, connection_proxy),
            x => panic!("Expected ProfileRequest but got: {:?}", x),
        }
    }

    /// Creates a Profile::Advertise request.
    /// Returns the associated request stream, and the Advertise request as a Future.
    fn make_advertise_request(
        client: &bredr::ProfileProxy,
        services: Vec<bredr::ServiceDefinition>,
    ) -> (
        bredr::ConnectionReceiverRequestStream,
        impl Future<Output = Result<Vec<bredr::ServiceDefinition>, ErrorCode>>,
    ) {
        let (connection, connection_stream) =
            create_request_stream::<bredr::ConnectionReceiverMarker>();
        let request = bredr::ProfileAdvertiseRequest {
            services: Some(services),
            receiver: Some(connection),
            ..Default::default()
        };
        let adv_fut = client
            .advertise(request)
            .map(|fidl_result| fidl_result.unwrap().map(|r| r.services.unwrap()));
        (connection_stream, adv_fut)
    }

    /// Expects a `Profile::Advertise` request in the `profile_requests` stream.
    /// Responds to the request with the unmodified set of services provided in the payload.
    #[track_caller]
    fn expect_advertise_request(
        exec: &mut fasync::TestExecutor,
        profile_requests: &mut bredr::ProfileRequestStream,
    ) -> bredr::ConnectionReceiverProxy {
        match expect_stream_item(exec, profile_requests) {
            Ok(bredr::ProfileRequest::Advertise {
                payload: bredr::ProfileAdvertiseRequest { services, receiver, .. },
                responder,
                ..
            }) => {
                let _ = responder.send(Ok(&advertise_response(services.unwrap())));
                receiver.unwrap().into_proxy()
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        }
    }

    /// Creates a Profile::Connect request for an L2cap channel.
    /// Returns the associated result future
    fn make_l2cap_connection_request(
        client: &bredr::ProfileProxy,
        peer_id: PeerId,
        psm: u16,
    ) -> impl Future<Output = Result<Result<bredr::Channel, ErrorCode>, fidl::Error>> {
        client.connect(
            &peer_id.into(),
            &l2cap_connect_parameters(Psm::new(psm), fidl_bt::ChannelMode::Basic),
        )
    }

    #[track_caller]
    fn new_client(
        exec: &mut fasync::TestExecutor,
        mut service_sender: mpsc::Sender<Service>,
        server_fut: &mut (impl Future<Output = ()> + Unpin),
    ) -> bredr::ProfileProxy {
        let (profile_client, profile_server) = create_proxy_and_stream::<bredr::ProfileMarker>();
        let send_fut = service_sender.send(Service::Profile(profile_server));
        let mut send_fut = pin!(send_fut);
        let (send_result, _server_fut) = run_while(exec, server_fut, &mut send_fut);
        assert!(send_result.is_ok());
        profile_client
    }

    /// Starts the `ProfileRegistrar` processing task.
    fn setup_handler_fut(
        server: ProfileRegistrar,
    ) -> (mpsc::Sender<Service>, impl Future<Output = ()>) {
        let (service_sender, service_receiver) = mpsc::channel(0);
        let handler_fut = server.handle_fidl_requests(service_receiver);
        (service_sender, handler_fut)
    }

    /// Creates the ProfileRegistrar with the upstream Profile service.
    fn setup_server() -> (fasync::TestExecutor, ProfileRegistrar, bredr::ProfileRequestStream) {
        let exec = fasync::TestExecutor::new();
        let (client, server) = create_proxy_and_stream::<bredr::ProfileMarker>();
        let profile_server = ProfileRegistrar::new(client);
        (exec, profile_server, server)
    }

    /// Exercises a service advertisement with an empty set of services.
    /// In practice, the upstream Host Server will reject this request but the RFCOMM
    /// server will still classify the request as non-RFCOMM only, and relay directly
    /// to the Profile Server.
    /// This test validates that the parameters are relayed directly to the Profile Server.
    #[fuchsia::test]
    fn handle_empty_advertise_request() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connects to `bt-rfcomm`.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);

        // Client decides to advertise empty services.
        let services = vec![];
        let (_connection_stream, adv_fut) = make_advertise_request(&client, services);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The advertisement request should be relayed directly upstream.
        match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise { payload, responder }) => {
                assert_eq!(payload.services, Some(vec![]));
                // Upstream responds with an empty set of services.
                let _ = responder.send(Ok(&advertise_response(vec![])));
            }
            x => panic!("Expected advertise request, got: {:?}", x),
        };

        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        // Client should be notified that the advertisement was successful.
        let result = exec.run_until_stalled(&mut adv_fut).expect("advertisement ready");
        assert_eq!(result, Ok(vec![]));
    }

    /// Exercises a service advertisement with no RFCOMM services.
    /// The ProfileRegistrar server should classify the request as non-RFCOMM only, and relay
    /// directly to the upstream Profile Server.
    #[fuchsia::test]
    fn handle_advertise_request_with_no_rfcomm() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connects to bt-rfcomm.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);

        // Client decides to advertise.
        let service =
            bredr::ServiceDefinition::try_from(&other_service_definition(Psm::new(1))).unwrap();
        let (_connection_stream, adv_fut) = make_advertise_request(&client, vec![service.clone()]);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The advertisement request should be relayed directly upstream.
        match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise { payload, responder }) => {
                let services = payload.services.unwrap();
                assert_eq!(services, vec![service]);
                let _ = responder.send(Ok(&advertise_response(services)));
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        };
        let (adv_result, _handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut);
        let advertised_services = adv_result.expect("successful advertisement");
        assert_eq!(advertised_services.len(), 1);
    }

    /// Exercises connecting l2cap channels while advertising.
    /// The ProfileRegistrar should classify the request as non-RFCOMM, relaying the Advertise
    /// request directly, and relay all incoming l2cap channel requests to the client.
    #[fuchsia::test]
    fn advertise_and_make_outgoing_l2cap_connection() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connects to bt-rfcomm.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);

        // Client decides to advertise.
        let service =
            bredr::ServiceDefinition::try_from(&other_service_definition(Psm::new(1))).unwrap();
        let (_connection_stream, adv_fut) = make_advertise_request(&client, vec![service.clone()]);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The advertisement request should be relayed directly upstream.
        let _upstream_receiver_client = expect_advertise_request(&mut exec, &mut upstream_requests);
        let (_advertised_services, mut handler_fut) =
            run_while(&mut exec, &mut handler_fut, &mut adv_fut);

        // Client requests to make an outgoing L2CAP connection. The request should be forwarded
        // directly to the upstream Profile server.
        let expected_peer_id = PeerId(1);
        let connect_fut =
            make_l2cap_connection_request(&client, expected_peer_id, bredr::PSM_AVDTP);
        let mut connect_fut = pin!(connect_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The advertisement request should be relayed directly upstream.
        let connect_responder = match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Connect { peer_id, connection, responder }) => {
                assert_eq!(peer_id, expected_peer_id.into());
                assert_eq!(
                    connection,
                    bredr::ConnectParameters::L2cap(bredr::L2capParameters {
                        psm: Some(bredr::PSM_AVDTP),
                        parameters: Some(fidl_bt::ChannelParameters {
                            channel_mode: Some(fidl_bt::ChannelMode::Basic),
                            ..fidl_bt::ChannelParameters::default()
                        }),
                        ..bredr::L2capParameters::default()
                    })
                );
                responder
            }
            x => panic!("Expected connect request, got: {:?}", x),
        };
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        exec.run_until_stalled(&mut connect_fut).expect_pending("waiting for connect response");

        // Upstream profile server responds with an error.
        connect_responder.send(Err(ErrorCode::TimedOut)).expect("upstream connect response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The client should receive the Error response to its Connect request.
        match exec.run_until_stalled(&mut connect_fut).expect("response") {
            Ok(Err(ErrorCode::TimedOut)) => {}
            x => panic!("Expected connect reply error, got {x:?}"),
        }
    }

    /// Service advertisement with only RFCOMM services. The services should be assigned
    /// Server Channels and be relayed upstream.
    #[fuchsia::test]
    fn handle_advertise_request_with_only_rfcomm() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connects to bt-rfcomm.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);

        // Client decides to advertise.
        let services =
            vec![bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap()];
        let (_connection_stream, adv_fut) = make_advertise_request(&client, services);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Upstream should receive Advertise request for a service with an assigned server channel.
        match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise { payload, responder }) => {
                let services = payload.services.unwrap();
                assert_eq!(services.len(), 1);
                assert!(service_def_has_assigned_server_channel(&services[0]));
                let _ = responder.send(Ok(&advertise_response(services)));
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        }

        let (_advertised_services, _handler_fut) =
            run_while(&mut exec, &mut handler_fut, &mut adv_fut);
    }

    /// Service advertisement with both RFCOMM and non-RFCOMM services. Only the RFCOMM
    /// services should be assigned Server Channels, and the group should be registered upstream.
    #[fuchsia::test]
    fn handle_advertise_request_with_all_services() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connects to bt-rfcomm.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);

        // Client decides to advertise.
        let services = vec![
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
            bredr::ServiceDefinition::try_from(&other_service_definition(Psm::new(10))).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let n = services.len();
        let (_connection_stream, adv_fut) = make_advertise_request(&client, services);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Expect an advertise request with all n services - validate that the RFCOMM services
        // have assigned server channels.
        match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise { payload, responder }) => {
                let services = payload.services.unwrap();
                assert_eq!(services.len(), n);
                let res = services
                    .iter()
                    .filter(|&service| {
                        is_rfcomm_service_definition(&ServiceDefinition::try_from(service).unwrap())
                    })
                    .map(|service| service_def_has_assigned_server_channel(&service))
                    .fold(true, |acc, elt| acc || elt);
                assert!(res);
                let _ = responder.send(Ok(&advertise_response(services)));
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        }
        let (_advertised_services, _handler_fut) =
            run_while(&mut exec, &mut handler_fut, &mut adv_fut);
    }

    /// Tests handling two advertise requests with overlapping PSMs. The first one
    /// should succeed and be relayed upstream. The second one should fail since it
    /// is requesting already-allocated PSMs - the responder should be notified of the error.
    #[fuchsia::test]
    fn advertise_requests_same_psm() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connects to bt-rfcomm.
        let client1 = new_client(&mut exec, service_sender.clone(), &mut handler_fut);

        // Client decides to advertise.
        let psm = Psm::new(10);
        let services1 = vec![
            bredr::ServiceDefinition::try_from(&other_service_definition(psm)).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let (mut connection_stream1, adv_fut1) = make_advertise_request(&client1, services1);
        let mut adv_fut1 = pin!(adv_fut1);
        exec.run_until_stalled(&mut adv_fut1).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // First request should be received by upstream and handled successfully.
        let _adv_req1 = expect_advertise_request(&mut exec, &mut upstream_requests);
        let (_advertised_services1, mut handler_fut) =
            run_while(&mut exec, &mut handler_fut, &mut adv_fut1);

        // A different client connects to bt-rfcomm. It decides to try to advertise over same PSM.
        let client2 = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let services2 = vec![
            bredr::ServiceDefinition::try_from(&other_service_definition(psm)).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let (mut connection_stream2, adv_fut2) = make_advertise_request(&client2, services2);
        let mut adv_fut2 = pin!(adv_fut2);
        exec.run_until_stalled(&mut adv_fut2).expect_pending("waiting for advertise response");

        // The second advertise request shouldn't make it to the upstream Profile server as
        // `bt-rfcomm` should reject it.
        let (result, _handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut2);
        assert_matches!(result, Err(ErrorCode::Failed));
        // The first client should still have an active connection receiver. The second client's
        // connection receiver should be closed due to failure.
        expect_stream_pending(&mut exec, &mut upstream_requests);
        expect_stream_pending(&mut exec, &mut connection_stream1);
        expect_stream_terminated(&mut exec, &mut connection_stream2);
    }

    /// Tests that service advertisements from multiple clients are correctly registered.
    #[fuchsia::test]
    fn multiple_fidl_client_advertisements() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Client1 connects to `bt-rfcomm` and advertises two things.
        let client1 = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let psm1 = Psm::new(10);
        let services1 = vec![
            bredr::ServiceDefinition::try_from(&other_service_definition(psm1)).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let n1 = services1.len();
        let (mut connection_stream1, adv_fut1) = make_advertise_request(&client1, services1);
        let mut adv_fut1 = pin!(adv_fut1);
        exec.run_until_stalled(&mut adv_fut1).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // First advertisement request should be relayed upstream with the 2 services.
        let receiver1 = match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise {
                payload: bredr::ProfileAdvertiseRequest { services: Some(services), receiver, .. },
                responder,
                ..
            }) => {
                assert_eq!(services.len(), n1);
                let _ = responder.send(Ok(&advertise_response(services)));
                receiver.unwrap().into_proxy()
            }
            x => panic!("Expected advertise request, got: {:?}", x),
        };
        // Client #1's advertisement should be active and connection receiver open. The advertise
        // result should contain the two services in `services1`.
        let (adv_result1, mut handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut1);
        assert_eq!(adv_result1.expect("success").len(), 2);
        expect_stream_pending(&mut exec, &mut connection_stream1);

        // A different client connects to bt-rfcomm and advertises 1 RFCOMM and 2 other services.
        let client2 = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let psm2 = Psm::new(15);
        let psm3 = Psm::new(2000);
        let n2 = 3;
        let services2 = vec![
            bredr::ServiceDefinition::try_from(&other_service_definition(psm2)).unwrap(),
            bredr::ServiceDefinition::try_from(&obex_service_definition(psm3)).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let (mut connection_stream2, adv_fut2) = make_advertise_request(&client2, services2);
        let mut adv_fut2 = pin!(adv_fut2);
        exec.run_until_stalled(&mut adv_fut2).expect_pending("waiting for advertise response2");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // We expect the registrar to unregister the current advertisement by issuing a `Revoke`
        // event to the upstream Profile server.
        {
            let mut event_stream1 = receiver1.take_event_stream();
            let revoke1 = expect_stream_item(&mut exec, &mut event_stream1);
            assert_matches!(revoke1, Ok(bredr::ConnectionReceiverEvent::OnRevoke {}));
        }
        // Simulate the upstream Profile server acknowledging it by terminating the advertisement.
        drop(receiver1);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // We expect a new advertisement upstream with the combined set of services.
        let receiver2 = match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise {
                payload: bredr::ProfileAdvertiseRequest { services: Some(services), receiver, .. },
                responder,
                ..
            }) => {
                assert_eq!(services.len(), n1 + n2);
                let _ = responder.send(Ok(&advertise_response(services)));
                receiver.unwrap().into_proxy()
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        };
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Both Client #1 and Client #2 should be advertising with their connection receivers open.
        // The response to Client #2's advertise request should only contain `services2`.
        let (adv_result2, _handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut2);
        assert_eq!(adv_result2.expect("successful advertisement").len(), n2);
        expect_stream_pending(&mut exec, &mut connection_stream1);
        expect_stream_pending(&mut exec, &mut connection_stream2);
        // Can receive L2CAP connections on any of the registered PSMs.
        receiver2
            .connected(&PeerId(12).into(), bredr::Channel::default(), &l2cap_protocol(psm1))
            .expect("valid FIDL request");

        // `bt-rfcomm` should receive, parse, and pass to FIDL client #1 since it registered `psm1`.
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        let event = expect_stream_item(&mut exec, &mut connection_stream1);
        assert_matches!(event, Ok(bredr::ConnectionReceiverRequest::Connected { .. }));

        // Should be passed to FIDL client #2 since it registered `psm3`.
        receiver2
            .connected(&PeerId(12).into(), bredr::Channel::default(), &l2cap_protocol(psm3))
            .expect("valid FIDL request");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        let event = expect_stream_item(&mut exec, &mut connection_stream2);
        assert_matches!(event, Ok(bredr::ConnectionReceiverRequest::Connected { .. }));
    }

    #[fuchsia::test]
    fn fidl_client_revokes_advertisement() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // FIDL client advertises a single RFCOMM service.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let services =
            vec![bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap()];
        let (mut connection_stream, adv_fut) = make_advertise_request(&client, services);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        let receiver = expect_advertise_request(&mut exec, &mut upstream_requests);
        // Client advertisement should be active.
        let (_adv_result, mut handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut);
        expect_stream_pending(&mut exec, &mut connection_stream);
        let mut upstream_event_stream = receiver.take_event_stream();
        expect_stream_pending(&mut exec, &mut upstream_event_stream);

        // Client revokes its advertisement. The registrar should handle the disconnection and
        // unregister the active advertisement by issuing an `OnRevoke` event upstream.
        connection_stream.control_handle().send_on_revoke().expect("can send FIDL revoke event");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        let revoke = expect_stream_item(&mut exec, &mut upstream_event_stream);
        assert_matches!(revoke, Ok(bredr::ConnectionReceiverEvent::OnRevoke {}));
        // Simulate upstream processing the Revoke.
        drop(upstream_event_stream);
        drop(receiver);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        // In this case, there was only one FIDL client advertisement so no new `Advertise` request
        // should be received upstream.
        expect_stream_pending(&mut exec, &mut upstream_requests);

        drop(connection_stream);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
    }

    #[fuchsia::test]
    fn fidl_client_drops_advertisement() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // FIDL client advertises a single RFCOMM service.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let services =
            vec![bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap()];
        let (mut connection_stream, adv_fut) = make_advertise_request(&client, services);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        let receiver = expect_advertise_request(&mut exec, &mut upstream_requests);
        // Client advertisement should be active.
        let (_adv_result, mut handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut);
        expect_stream_pending(&mut exec, &mut connection_stream);
        let mut upstream_event_stream = receiver.take_event_stream();
        expect_stream_pending(&mut exec, &mut upstream_event_stream);

        // Client closes its `ConnectionReceiver` to indicate advertisement termination. Typically,
        // we expect the FIDL client to use `OnRevoke` to signal termination. The registrar should
        // still be resilient to this case.
        drop(connection_stream);
        // The registrar should handle the FIDL client disconnection and unregister the active
        // advertisement by issuing the `OnRevoke` event.
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        let revoke = expect_stream_item(&mut exec, &mut upstream_event_stream);
        assert_matches!(revoke, Ok(bredr::ConnectionReceiverEvent::OnRevoke {}));
        // Simulate upstream processing the Revoke.
        drop(upstream_event_stream);
        drop(receiver);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        // In this case, there was only one FIDL client advertisement so no new `Advertise` request
        // should be received upstream.
        expect_stream_pending(&mut exec, &mut upstream_requests);
    }

    /// Verifies that a FIDL client of `bt-rfcomm` (e.g. a profile) gets notified when its
    /// advertisement gets unexpectedly terminated by the upstream Profile server.
    #[fuchsia::test]
    fn upstream_profile_server_drops_advertisement() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // FIDL client advertises a single RFCOMM service.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let services =
            vec![bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap()];
        let (mut connection_stream, adv_fut) = make_advertise_request(&client, services);
        let mut adv_fut = pin!(adv_fut);
        exec.run_until_stalled(&mut adv_fut).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        let receiver = expect_advertise_request(&mut exec, &mut upstream_requests);
        // Client advertisement should be active.
        let (_adv_result, mut handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut);
        expect_stream_pending(&mut exec, &mut connection_stream);

        // Upstream server unexpectedly closes the advertisement. We expect the registrar to handle
        // this and notify the FIDL client that its advertisement is no longer active - no more
        // incoming connections.
        drop(receiver);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        expect_stream_terminated(&mut exec, &mut connection_stream);
    }

    #[fuchsia::test]
    fn advertise_and_connect_to_obex_dynamic_psm() {
        let (mut exec, server, mut upstream_requests) = setup_server();

        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // A new client connection to bt-rfcomm and advertises an OBEX service.
        let client = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let service1 =
            bredr::ServiceDefinition::try_from(&obex_service_definition(Psm::DYNAMIC)).unwrap();
        let (mut connection_stream1, adv_fut1) = make_advertise_request(&client, vec![service1]);
        let mut adv_fut1 = pin!(adv_fut1);
        exec.run_until_stalled(&mut adv_fut1).expect_pending("still advertising");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The advertisement request should be relayed upstream.
        let assigned_psm1 = Psm::new(0x6789);
        let receiver1 = match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise {
                payload:
                    bredr::ProfileAdvertiseRequest { services: Some(mut services), receiver, .. },
                responder,
                ..
            }) => {
                assert_eq!(services.len(), 1);
                // Assign a random dynamic PSM for the GOEP L2CAP attribute.
                services[0].additional_attributes.as_mut().unwrap()[0] = bredr::Attribute {
                    id: Some(0x0200),
                    element: Some(bredr::DataElement::Uint16(assigned_psm1.into())),
                    ..Default::default()
                };
                let _ = responder.send(Ok(&advertise_response(services)));
                receiver.unwrap().into_proxy()
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        };
        let (adv_result1, mut handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut1);
        // FIDL client should get the updated version of their definition.
        let advertised_services1 =
            ServiceDefinition::try_from_fidl(&adv_result1.expect("successful")).unwrap();
        assert_eq!(advertised_services1[0].psm_set(), HashSet::from([assigned_psm1]));

        // Trying to advertise again with the recently assigned dynamic PSM should fail immediately.
        let duplicate_service =
            bredr::ServiceDefinition::try_from(&obex_service_definition(assigned_psm1)).unwrap();
        let (_duplicate_stream, duplicate_adv_fut) =
            make_advertise_request(&client, vec![duplicate_service]);
        let mut duplicate_adv_fut = pin!(duplicate_adv_fut);
        exec.run_until_stalled(&mut duplicate_adv_fut)
            .expect_pending("waiting for advertise response");
        // The second advertise request shouldn't make it to the upstream Profile server as
        // `bt-rfcomm` should reject it.
        let (duplicate_adv_result, mut handler_fut) =
            run_while(&mut exec, &mut handler_fut, &mut duplicate_adv_fut);
        assert_matches!(duplicate_adv_result, Err(ErrorCode::Failed));

        // Trying to advertise another dynamic PSM service should be OK.
        let service2 =
            bredr::ServiceDefinition::try_from(&obex_service_definition(Psm::DYNAMIC)).unwrap();
        let (mut connection_stream2, adv_fut2) = make_advertise_request(&client, vec![service2]);
        let mut adv_fut2 = pin!(adv_fut2);
        exec.run_until_stalled(&mut adv_fut2).expect_pending("still advertising");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // We expect the registrar to unregister the current advertisement by issuing a `Revoke`
        // event to the upstream Profile server.
        {
            let mut event_stream1 = receiver1.take_event_stream();
            let revoke1 = expect_stream_item(&mut exec, &mut event_stream1);
            assert_matches!(revoke1, Ok(bredr::ConnectionReceiverEvent::OnRevoke {}));
        }
        // Simulate the upstream Profile server acknowledging it by terminating the advertisement.
        drop(receiver1);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // The refreshed advertisement request should be relayed upstream (contains both services).
        let assigned_psm2 = Psm::new(0x8080);
        let new_receiver = match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise {
                payload:
                    bredr::ProfileAdvertiseRequest { services: Some(mut services), receiver, .. },
                responder,
                ..
            }) => {
                assert_eq!(services.len(), 2);
                // Assign a different dynamic PSM for the second service's GOEP L2CAP attribute.
                services[1].additional_attributes.as_mut().unwrap()[0] = bredr::Attribute {
                    id: Some(0x0200),
                    element: Some(bredr::DataElement::Uint16(assigned_psm2.into())),
                    ..Default::default()
                };
                let _ = responder.send(Ok(&advertise_response(services)));
                receiver.unwrap().into_proxy()
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        };
        let (adv_result2, mut handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut2);
        // FIDL client should get the updated version of only their definition.
        let advertised_services2 =
            ServiceDefinition::try_from_fidl(&adv_result2.expect("successful")).unwrap();
        assert_eq!(advertised_services2.len(), 1);
        assert_eq!(advertised_services2[0].psm_set(), HashSet::from([assigned_psm2]));

        // Simulate an incoming L2CAP connection over the dynamic L2CAP PSM of the first OBEX
        // service.
        let id = PeerId(123);
        let protocol: Vec<bredr::ProtocolDescriptor> = l2cap_protocol(assigned_psm1);
        new_receiver
            .connected(&id.into(), bredr::Channel::default(), &protocol)
            .expect("valid FIDL request");
        // bt-rfcomm should receive and route the connection to the correct upstream FIDL client.
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        match expect_stream_item(&mut exec, &mut connection_stream1) {
            Ok(bredr::ConnectionReceiverRequest::Connected { peer_id, protocol, .. }) => {
                assert_eq!(peer_id, id.into());
                assert_eq!(protocol, protocol);
            }
            x => panic!("Expected incoming L2CAP request. got: {x:?}"),
        };

        // Second dynamic L2CAP PSM should be connectable as well.
        let id2 = PeerId(123);
        let protocol2 = l2cap_protocol(assigned_psm2);
        new_receiver
            .connected(&id2.into(), bredr::Channel::default(), &protocol2)
            .expect("valid FIDL request");
        // bt-rfcomm should receive and route the connection to the correct upstream FIDL client.
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        match expect_stream_item(&mut exec, &mut connection_stream2) {
            Ok(bredr::ConnectionReceiverRequest::Connected { peer_id, protocol, .. }) => {
                assert_eq!(peer_id, id.into());
                assert_eq!(protocol, protocol2);
            }
            x => panic!("Expected incoming L2CAP request. got: {x:?}"),
        };
    }

    /// Validates that client Search requests are relayed directly upstream.
    #[fuchsia::test]
    fn handle_search_request() {
        let (mut exec, mut server, mut profile_requests) = setup_server();

        let (search_request, _stream) = generate_search_request(&mut exec);
        let handle_fut = server.handle_profile_request(search_request);
        let mut handle_fut = pin!(handle_fut);
        let handle_result = exec.run_until_stalled(&mut handle_fut).expect("processed request");
        assert!(handle_result.is_none());

        // The search request should be relayed directly to the upstream Profile server.
        let result = expect_stream_item(&mut exec, &mut profile_requests);
        assert_matches!(result, Ok(bredr::ProfileRequest::Search { .. }));
    }

    /// Validates that client ConnectSco requests are relayed directly upstream.
    #[fuchsia::test]
    fn handle_connect_sco_request() {
        let (mut exec, mut server, mut profile_requests) = setup_server();

        let (connect_sco_request, _connection_proxy) = generate_connect_sco_request(&mut exec);
        let handle_fut = server.handle_profile_request(connect_sco_request);
        let mut handle_fut = pin!(handle_fut);
        let handle_result = exec.run_until_stalled(&mut handle_fut).expect("processed request");
        assert!(handle_result.is_none());

        // The connect SCO request should be relayed directly to the upstream Profile server.
        let result = expect_stream_item(&mut exec, &mut profile_requests);
        assert_matches!(result, Ok(bredr::ProfileRequest::ConnectSco { .. }));
    }

    /// Validates that a stale termination event (Epitaph) from a previously
    /// revoked client does not affect other active advertisements.
    #[fuchsia::test]
    fn stale_connection_receiver_termination_does_not_evict_active_client() {
        let (mut exec, server, mut upstream_requests) = setup_server();
        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Client A advertises a service.
        let client_a = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let svc_a =
            vec![bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap()];
        let (connection_stream_a, adv_fut_a) = make_advertise_request(&client_a, svc_a);
        let mut adv_fut_a = pin!(adv_fut_a);
        exec.run_until_stalled(&mut adv_fut_a).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        let upstream_recv1 = expect_advertise_request(&mut exec, &mut upstream_requests);
        let _ = run_while(&mut exec, &mut handler_fut, &mut adv_fut_a);

        // Client A revokes its advertisement but keeps the connection receiver open.
        connection_stream_a.control_handle().send_on_revoke().expect("send OnRevoke");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        {
            let mut es = upstream_recv1.take_event_stream();
            let r = expect_stream_item(&mut exec, &mut es);
            assert_matches!(r, Ok(bredr::ConnectionReceiverEvent::OnRevoke {}));
        }
        drop(upstream_recv1);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Client B advertises a new service.
        let client_b = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let svc_b =
            vec![bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap()];
        let (mut connection_stream_b, adv_fut_b) = make_advertise_request(&client_b, svc_b);
        let mut adv_fut_b = pin!(adv_fut_b);
        exec.run_until_stalled(&mut adv_fut_b).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");
        let upstream_recv2 = expect_advertise_request(&mut exec, &mut upstream_requests);
        let _ = run_while(&mut exec, &mut handler_fut, &mut adv_fut_b);

        // Client B's advertisement should be active.
        expect_stream_pending(&mut exec, &mut connection_stream_b);
        let mut upstream_es2 = upstream_recv2.take_event_stream();
        expect_stream_pending(&mut exec, &mut upstream_es2);

        // Client A finally closes its connection receiver, triggering a stale termination event (Epitaph).
        drop(connection_stream_a);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Verify that Client B's advertisement is NOT affected by Client A's termination.
        // If the bug is present, Client B's advertisement would be incorrectly revoked here.
        expect_stream_pending(&mut exec, &mut upstream_es2);
        expect_stream_pending(&mut exec, &mut connection_stream_b);
    }

    #[fuchsia::test]
    fn add_managed_advertisement_upstream_failure_rolls_back_registration() {
        let (mut exec, server, mut upstream_requests) = setup_server();
        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Client 1 successfully registers an advertisement.
        let client1 = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let psm1 = Psm::new(10);
        let services1 = vec![
            bredr::ServiceDefinition::try_from(&other_service_definition(psm1)).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let (mut connection_stream1, adv_fut1) = make_advertise_request(&client1, services1);
        let mut adv_fut1 = pin!(adv_fut1);
        exec.run_until_stalled(&mut adv_fut1).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        let receiver1 = expect_advertise_request(&mut exec, &mut upstream_requests);
        let (_advertised_services1, mut handler_fut) =
            run_while(&mut exec, &mut handler_fut, &mut adv_fut1);

        // Client 2 attempts to register a second service.
        let client2 = new_client(&mut exec, service_sender.clone(), &mut handler_fut);
        let psm2 = Psm::new(20);
        let services2 = vec![
            bredr::ServiceDefinition::try_from(&other_service_definition(psm2)).unwrap(),
            bredr::ServiceDefinition::try_from(&rfcomm_service_definition(None)).unwrap(),
        ];
        let (mut connection_stream2, adv_fut2) = make_advertise_request(&client2, services2);
        let mut adv_fut2 = pin!(adv_fut2);
        exec.run_until_stalled(&mut adv_fut2).expect_pending("waiting for advertise response");
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // We expect the registrar to unregister the current advertisement by issuing a `Revoke`
        // event to the upstream Profile server.
        {
            let mut event_stream1 = receiver1.take_event_stream();
            let revoke1 = expect_stream_item(&mut exec, &mut event_stream1);
            assert_matches!(revoke1, Ok(bredr::ConnectionReceiverEvent::OnRevoke {}));
        }
        drop(receiver1);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Upstream receives combined request [Service 1, Service 2], but fails.
        match expect_stream_item(&mut exec, &mut upstream_requests) {
            Ok(bredr::ProfileRequest::Advertise { responder, .. }) => {
                let _ = responder.send(Err(fidl_fuchsia_bluetooth::ErrorCode::Failed));
            }
            x => panic!("Expected advertise request, got: {x:?}"),
        }

        // Run until stalled so rollback triggers a restoration advertise request for Client 1.
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        // Upstream should receive a restoration request containing ONLY Client 1's service.
        let _restored_receiver1 = expect_advertise_request(&mut exec, &mut upstream_requests);

        // Verify that Client 2 received the failure response.
        let (adv_res2, _handler_fut) = run_while(&mut exec, &mut handler_fut, &mut adv_fut2);
        assert_matches!(adv_res2, Err(ErrorCode::Failed));

        expect_stream_pending(&mut exec, &mut connection_stream1);
        expect_stream_terminated(&mut exec, &mut connection_stream2);
    }

    #[fuchsia::test]
    fn advertise_multiple_rfcomm_descriptors_is_error() {
        let (mut exec, server, _upstream_requests) = setup_server();
        let (service_sender, handler_fut) = setup_handler_fut(server);
        let mut handler_fut = pin!(handler_fut);
        exec.run_until_stalled(&mut handler_fut).expect_pending("server active");

        let client = new_client(&mut exec, service_sender, &mut handler_fut);

        let mut def = rfcomm_service_definition(None);
        def.additional_protocol_descriptor_lists = vec![vec![ProtocolDescriptor {
            protocol: bredr::ProtocolIdentifier::Rfcomm,
            params: vec![DataElement::Uint8(0)],
        }]];
        let invalid_service = bredr::ServiceDefinition::try_from(&def).unwrap();

        let (_connection_stream, adv_fut) = make_advertise_request(&client, vec![invalid_service]);
        let mut adv_fut = pin!(adv_fut);

        let _ = exec.run_until_stalled(&mut handler_fut);
        let res = exec.run_until_stalled(&mut adv_fut);
        assert_matches!(res, Poll::Ready(Err(fidl_fuchsia_bluetooth::ErrorCode::InvalidArguments)));
    }
}
