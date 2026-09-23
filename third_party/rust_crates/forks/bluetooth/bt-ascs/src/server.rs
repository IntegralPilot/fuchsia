// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::core::CodecId;
use bt_common::generic_audio::metadata_ltv::Metadata;
use bt_common::{PeerId, Uuid};
use bt_gatt::server::{LocalService, Server, ServiceDefinition, ServiceId};
use bt_gatt::server::{ReadResponder, WriteResponder};
use bt_gatt::types::{
    AttributePermissions, CharacteristicProperty, GattError, Handle, SecurityLevels,
};
use bt_gatt::Characteristic;

use futures::channel::oneshot::{self, Canceled};
use futures::stream::{FusedStream, FuturesUnordered};
use futures::task::{Poll, Waker};
use futures::{Future, Stream, StreamExt};
use pin_project::pin_project;
use std::collections::{HashMap, VecDeque};

use crate::types::*;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("Server Only Operation")]
    ServerOnlyOperation,
    #[error("Service is already published")]
    AlreadyPublished,
    #[error("Issue publishing service: {0}")]
    PublishError(#[from] bt_gatt::types::Error),
    #[error("Unknown Peer: {0}")]
    UnknownPeer(bt_common::PeerId),
}

#[pin_project(project = LocalServiceProj)]
enum LocalServiceState<T: bt_gatt::ServerTypes> {
    NotPublished {
        waker: Option<Waker>,
    },
    Preparing {
        #[pin]
        fut: T::LocalServiceFut,
    },
    Published {
        service: T::LocalService,
        #[pin]
        events: T::ServiceEventStream,
    },
    Terminated,
}

impl<T: bt_gatt::ServerTypes> Default for LocalServiceState<T> {
    fn default() -> Self {
        Self::NotPublished { waker: None }
    }
}

impl<T: bt_gatt::ServerTypes> LocalServiceState<T> {
    fn service(&self) -> Option<&T::LocalService> {
        let Self::Published { service, .. } = self else {
            return None;
        };
        Some(service)
    }

    fn notify(&self, characteristic: &Handle, data: &[u8], peers: &[PeerId]) -> bool {
        let Some(service) = self.service() else {
            return false;
        };
        service.notify(characteristic, data, peers);
        true
    }
}

impl<T: bt_gatt::ServerTypes> Stream for LocalServiceState<T> {
    type Item = Result<bt_gatt::server::ServiceEvent<T>, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        // SAFETY:
        //  - Wakers are Unpin
        //  - We re-pin the structurally pinned futures in Preparing and Published
        //    (service is untouched)
        //  - Terminated is empty
        loop {
            match self.as_mut().project() {
                LocalServiceProj::Terminated => return Poll::Ready(None),
                LocalServiceProj::NotPublished { .. } => {
                    self.as_mut()
                        .set(LocalServiceState::NotPublished { waker: Some(cx.waker().clone()) });
                    return Poll::Pending;
                }
                LocalServiceProj::Preparing { fut } => {
                    let service_result = futures::ready!(fut.poll(cx));
                    let Ok(service) = service_result else {
                        self.as_mut().set(LocalServiceState::Terminated);
                        return Poll::Ready(Some(Err(Error::from(ServerError::PublishError(
                            service_result.err().unwrap(),
                        )))));
                    };
                    let events = service.publish();
                    self.as_mut().set(LocalServiceState::Published { service, events });
                    continue;
                }
                LocalServiceProj::Published { service: _, events } => {
                    let item = futures::ready!(events.poll_next(cx));
                    let Some(gatt_result) = item else {
                        self.as_mut().set(LocalServiceState::Terminated);
                        return Poll::Ready(Some(Err(Error::from(ServerError::PublishError(
                            "GATT server terminated".into(),
                        )))));
                    };
                    let Ok(event) = gatt_result else {
                        self.as_mut().set(LocalServiceState::Terminated);
                        return Poll::Ready(Some(Err(Error::from(ServerError::PublishError(
                            gatt_result.err().unwrap(),
                        )))));
                    };
                    return Poll::Ready(Some(Ok(event)));
                }
            }
        }
    }
}

impl<T: bt_gatt::ServerTypes> FusedStream for LocalServiceState<T> {
    fn is_terminated(&self) -> bool {
        match self {
            Self::Terminated => true,
            _ => false,
        }
    }
}

impl<T: bt_gatt::ServerTypes> LocalServiceState<T> {
    fn is_not_published(&self) -> bool {
        matches!(self, LocalServiceState::NotPublished { .. })
    }
}

#[derive(Debug, Clone)]
struct AseControlOperationAction {
    peer_id: PeerId,
    opcode: Option<AseControlPointOpcode>,
    response_codes: Vec<ResponseCode>,
    new_endpoints: Vec<AudioStreamEndpoint>,
}

impl AseControlOperationAction {
    fn notify_control_point_value(&self) -> Option<Vec<u8>> {
        if self.opcode.is_none() || self.response_codes.is_empty() {
            return None;
        }
        use bt_common::packet_encoding::Encodable;

        let opcode = self.opcode.unwrap().into();
        let notification = if let ResponseCode::InvalidLength { .. }
        | ResponseCode::UnsupportedOpcode { .. } = self.response_codes[0]
        {
            ControlPointNotification::new_error(opcode, self.response_codes[0].to_code())
        } else {
            let mut cp_notification = ControlPointNotification::new(opcode);
            for response in &self.response_codes {
                cp_notification.add_response(response.clone());
            }
            cp_notification
        };
        let mut buf = vec![0; notification.encoded_len()];
        notification.encode(&mut buf).unwrap();
        Some(buf)
    }
}

struct AseControlOperationFut {
    peer_id: PeerId,
    /// The opcode of the operation that this future is for, if there is one.
    opcode: Option<AseControlPointOpcode>,
    /// The current set of response codes gathered for this operation.
    /// Empty if there are no responses to send.
    current_response_codes: Vec<ResponseCode>,
    /// New Endpoint States after the operation is complete
    endpoints: Vec<AudioStreamEndpoint>,
    /// Queue of responses we are still waiting on from the operation.
    waiting: FuturesUnordered<
        futures::channel::oneshot::Receiver<Result<AudioStreamEndpoint, ResponseCode>>,
    >,
}

impl From<&AseControlOperationFut> for AseControlOperationAction {
    fn from(value: &AseControlOperationFut) -> Self {
        Self {
            peer_id: value.peer_id,
            opcode: value.opcode,
            response_codes: value.current_response_codes.clone(),
            new_endpoints: value.endpoints.clone(),
        }
    }
}

impl Future for AseControlOperationFut {
    type Output = AseControlOperationAction;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        loop {
            if self.waiting.is_terminated() {
                let this = std::pin::Pin::into_inner(self);
                return Poll::Ready((&*this).into());
            }
            let update = futures::ready!(self.waiting.poll_next_unpin(cx));
            match update {
                None => continue,
                Some(Err(Canceled)) => {
                    log::warn!("Detected dropped responder!");
                    // TODO: maybe figure out how to determine which AseId got canceled here
                    self.current_response_codes
                        .push(ResponseCode::UnspecifiedError { ase_id: AseId(0x00) });
                    // Bail on the rest of them.
                    self.waiting.clear();
                }
                Some(Ok(Ok(endpoint))) => {
                    self.current_response_codes
                        .push(ResponseCode::Success { ase_id: endpoint.ase_id });
                    self.endpoints.push(endpoint);
                }
                Some(Ok(Err(response_code))) => self.current_response_codes.push(response_code),
            }
        }
    }
}

#[pin_project]
pub struct AudioStreamControlServiceServer<T: bt_gatt::ServerTypes> {
    service_def: ServiceDefinition,
    #[pin]
    local_service: LocalServiceState<T>,
    default_client_endpoints: ClientEndpoints,
    client_endpoints: HashMap<PeerId, ClientEndpoints>,
    outgoing_events: VecDeque<ServiceEvent>,
    // TODO: maybe these should be FuturesOrdered if the operations should be FIFOed
    // Not pinned as AseControlOperationFut is Unpin.
    responses: futures::stream::FuturesUnordered<AseControlOperationFut>,
}

const CONTROL_POINT_HANDLE: Handle = Handle(1);
const BASE_ENDPOINT_HANDLE: Handle = Handle(2);

pub const ASCS_UUID: Uuid = Uuid::from_u16(0x184E);

// As only one ASCS service is allowed on a host, define an arbitrary ServiceId
// that every AudioStreamControlServiceServer will attempt to use so publishing
// multiple will fail.
pub(crate) const ASCS_SERVICE_ID: ServiceId = ServiceId::new(1123901);

impl<T: bt_gatt::ServerTypes> AudioStreamControlServiceServer<T> {
    pub fn new(source_count: u8, sink_count: u8) -> Self {
        let default_client_endpoints = ClientEndpoints::new(source_count, sink_count);
        let mut chars: Vec<Characteristic> = (&default_client_endpoints).into();
        chars.push(Self::build_control_point());
        let mut service_def = ServiceDefinition::new(
            ASCS_SERVICE_ID,
            ASCS_UUID,
            bt_gatt::types::ServiceKind::Primary,
        );
        for c in chars {
            service_def.add_characteristic(c).unwrap();
        }
        Self {
            service_def,
            local_service: Default::default(),
            default_client_endpoints,
            client_endpoints: Default::default(),
            outgoing_events: VecDeque::new(),
            responses: FuturesUnordered::new(),
        }
    }

    fn build_control_point() -> Characteristic {
        let properties = CharacteristicProperty::Write
            | CharacteristicProperty::WriteWithoutResponse
            | CharacteristicProperty::Notify;
        let permissions =
            AttributePermissions::with_levels(&properties, &SecurityLevels::encryption_required());
        Characteristic {
            handle: CONTROL_POINT_HANDLE,
            uuid: Uuid::from_u16(0x2BC6),
            properties,
            permissions,
            descriptors: Vec::new(),
        }
    }

    pub fn publish(&mut self, server: &T::Server) -> Result<(), Error> {
        if !self.local_service.is_not_published() {
            return Err(Error::from(ServerError::AlreadyPublished));
        }
        let LocalServiceState::NotPublished { waker } = std::mem::replace(
            &mut self.local_service,
            LocalServiceState::Preparing { fut: server.prepare(self.service_def.clone()) },
        ) else {
            unreachable!();
        };
        waker.map(Waker::wake);
        Ok(())
    }

    pub fn cis_established(
        &mut self,
        peer_id: PeerId,
        ase_id: AseId,
        cis: (CigId, CisId),
    ) -> Result<(), Error> {
        let endpoints = self
            .client_endpoints
            .get_mut(&peer_id)
            .ok_or(Error::from(ServerError::UnknownPeer(peer_id)))?;
        endpoints.established_cis(ase_id, cis);
        for operation in endpoints.autonomous_operations() {
            self.queue_operation_unpin(peer_id, operation);
        }
        Ok(())
    }

    pub fn cis_released(
        &mut self,
        peer_id: PeerId,
        ase_id: AseId,
        cis: (CigId, CisId),
    ) -> Result<(), Error> {
        let endpoints = self
            .client_endpoints
            .get_mut(&peer_id)
            .ok_or(Error::from(ServerError::UnknownPeer(peer_id)))?;
        endpoints.released_cis(ase_id, cis);
        for operation in endpoints.autonomous_operations() {
            self.queue_operation_unpin(peer_id, operation);
        }
        Ok(())
    }

    fn queue_operation_unpin(&mut self, peer_id: PeerId, op: AseControlOperation) {
        let (events, fut) =
            op.apply(peer_id, self.client_endpoints.get(&peer_id).unwrap().endpoints.clone());
        self.outgoing_events.extend(events);
        self.responses.push(fut);
    }

    fn queue_operation(self: std::pin::Pin<&mut Self>, peer_id: PeerId, op: AseControlOperation) {
        let this = self.project();
        let (events, fut) =
            op.apply(peer_id, this.client_endpoints.get(&peer_id).unwrap().endpoints.clone());
        this.outgoing_events.extend(events);
        this.responses.push(fut);
    }

    /// Applies operations that are ready to complete (all outstanding events
    /// have been responded to)
    fn poll_operations(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) {
        let this = self.project();
        loop {
            let Poll::Ready(Some(operation)) = this.responses.poll_next_unpin(cx) else {
                return;
            };
            if let Some(notification) = operation.notify_control_point_value() {
                this.local_service.notify(
                    &CONTROL_POINT_HANDLE,
                    &notification,
                    &[operation.peer_id],
                );
            }
            if operation.new_endpoints.is_empty() {
                return;
            }
            // Apply the endpoint changes, and notify each endpoint
            let Some(current_endpoints) = this.client_endpoints.get_mut(&operation.peer_id) else {
                log::warn!(
                    "PeerId {peer_id} has disppeared while an operation was happening, ignoring..",
                    peer_id = operation.peer_id
                );
                continue;
            };
            for endpoint in operation.new_endpoints {
                this.local_service.notify(
                    &endpoint.handle,
                    endpoint.into_char_value().as_slice(),
                    &[operation.peer_id],
                );
                let _ = current_endpoints.endpoints.insert(endpoint.ase_id, endpoint);
            }
        }
    }
}

impl From<&AudioStreamEndpoint> for Characteristic {
    fn from(value: &AudioStreamEndpoint) -> Self {
        let properties = CharacteristicProperty::Read | CharacteristicProperty::Notify;
        let permissions =
            AttributePermissions::with_levels(&properties, &SecurityLevels::encryption_required());
        Characteristic {
            handle: value.handle,
            uuid: (&value.direction).into(),
            properties,
            permissions,
            descriptors: Vec::new(),
        }
    }
}

struct ClientEndpoints {
    endpoints: HashMap<AseId, AudioStreamEndpoint>,
    handles: HashMap<Handle, AseId>,
    established: HashMap<AseId, Vec<(CigId, CisId)>>,
}

impl ClientEndpoints {
    fn new(source_count: u8, sink_count: u8) -> Self {
        let dir_iter = std::iter::repeat(AudioDirection::Source)
            .take(source_count as usize)
            .chain(std::iter::repeat(AudioDirection::Sink).take(sink_count as usize));
        // AseIds shall not have an id of 0
        let (endpoints, handles) = (1..)
            .zip(dir_iter)
            .map(|(raw_ase_id, direction)| {
                let handle = Handle(BASE_ENDPOINT_HANDLE.0 + raw_ase_id as u64);
                let ase_id = AseId(raw_ase_id);
                (
                    (
                        ase_id,
                        AudioStreamEndpoint {
                            handle,
                            ase_id,
                            direction,
                            state: AseState::Idle,
                            additional: AseAdditionalParameters::None,
                        },
                    ),
                    (handle, ase_id),
                )
            })
            .unzip();
        Self { endpoints, handles, established: Default::default() }
    }

    fn clone_for_peer(&self, _peer_id: PeerId) -> Self {
        // TODO: Randomize the ASE_IDs.  Handles need to stay the same.
        Self {
            endpoints: self.endpoints.clone(),
            handles: self.handles.clone(),
            established: Default::default(),
        }
    }

    fn established_cis(&mut self, ase_id: AseId, cis: (CigId, CisId)) {
        self.established.entry(ase_id).or_default().push(cis);
    }

    fn released_cis(&mut self, ase_id: AseId, cis: (CigId, CisId)) {
        self.established.get_mut(&ase_id).map(|established| established.retain(|i| i != &cis));
    }

    fn autonomous_operations(&self) -> Vec<AseControlOperation> {
        let mut operations = Vec::new();
        for endpoint in self.endpoints.values() {
            match endpoint.state {
                AseState::Enabling
                    if self.established.get(&endpoint.ase_id).is_some_and(|e| !e.is_empty()) =>
                {
                    operations.push(AseControlOperation::ReceiverStartReady {
                        ases: vec![endpoint.ase_id],
                    });
                }
                AseState::Releasing
                    if self.established.get(&endpoint.ase_id).is_some_and(|e| e.is_empty()) =>
                {
                    operations.push(AseControlOperation::Released { ase_id: endpoint.ase_id });
                }
                _ => continue,
            }
        }
        operations
    }
}

impl From<&ClientEndpoints> for Vec<Characteristic> {
    fn from(value: &ClientEndpoints) -> Self {
        value.endpoints.values().map(Into::into).collect()
    }
}

#[derive(Debug)]
pub struct CodecConfigureResponder {
    endpoint: AudioStreamEndpoint,
    codec_id: CodecId,
    codec_config: Vec<u8>,
    sender: futures::channel::oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
}

impl CodecConfigureResponder {
    pub fn reject(self, err: ResponseCode) {
        let _ = self.sender.send(Err(err));
    }

    pub fn accept(
        mut self,
        framing: Framing,
        preferred_phys: Vec<Phy>,
        preferred_retransmission_number: u8,
        max_transport_latency: MaxTransportLatency,
        presentation_delay_range: PresentationDelayRange,
    ) {
        self.endpoint.state = AseState::CodecConfigured;
        self.endpoint.additional = AseAdditionalParameters::CodecConfigured {
            framing,
            preferred_phys,
            preferred_retransmission_number,
            max_transport_latency,
            presentation_delay_range,
            codec_id: self.codec_id,
            codec_config: self.codec_config,
        };
        let _ = self.sender.send(Ok(self.endpoint));
    }
}

#[derive(Debug)]
pub struct QosConfigureResponder {
    endpoint: AudioStreamEndpoint,
    configuration: QosConfiguration,
    sender: futures::channel::oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
}

impl QosConfigureResponder {
    pub fn reject(self, err: ResponseCode) {
        let _ = self.sender.send(Err(err));
    }

    pub fn accept(mut self) {
        self.endpoint.state = AseState::QosConfigured;
        self.endpoint.additional = self.configuration.into();
        let _ = self.sender.send(Ok(self.endpoint));
    }
}

#[derive(Debug)]
pub struct EnableResponder {
    endpoint: AudioStreamEndpoint,
    metadata: Vec<Metadata>,
    sender: futures::channel::oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
}

impl EnableResponder {
    pub fn reject(self, err: ResponseCode) {
        let _ = self.sender.send(Err(err));
    }

    pub fn accept(mut self) {
        let AseAdditionalParameters::QosConfigured { configuration, .. } = self.endpoint.additional
        else {
            // Shouldn't happen
            let _ = self
                .sender
                .send(Err(ResponseCode::UnspecifiedError { ase_id: self.endpoint.ase_id }));
            return;
        };
        self.endpoint.state = AseState::Enabling;
        self.endpoint.additional = AseAdditionalParameters::Streaming {
            cig_id: configuration.cig_id,
            cis_id: configuration.cis_id,
            metadata: self.metadata,
            qos_configured: Some(configuration),
        };
        let _ = self.sender.send(Ok(self.endpoint));
    }
}

#[derive(Debug)]
pub struct DisableResponder {
    endpoint: AudioStreamEndpoint,
    sender: futures::channel::oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
}

impl DisableResponder {
    pub fn reject(self, err: ResponseCode) {
        let _ = self.sender.send(Err(err));
    }

    pub fn accept(mut self) {
        let AseAdditionalParameters::Streaming { qos_configured: Some(qos_configured), .. } =
            self.endpoint.additional
        else {
            unreachable!();
        };
        self.endpoint.state = AseState::QosConfigured;
        self.endpoint.additional = qos_configured.into();
        let _ = self.sender.send(Ok(self.endpoint));
    }
}

#[derive(Debug)]
pub struct UpdateMetadataResponder {
    endpoint: AudioStreamEndpoint,
    metadata: Vec<Metadata>,
    sender: futures::channel::oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
}

impl UpdateMetadataResponder {
    pub fn reject(self, err: ResponseCode) {
        let _ = self.sender.send(Err(err));
    }

    pub fn accept(mut self) {
        let AseAdditionalParameters::Streaming { metadata, .. } = &mut self.endpoint.additional
        else {
            unreachable!();
        };
        *metadata = self.metadata;
        let _ = self.sender.send(Ok(self.endpoint));
    }
}

#[derive(Debug)]
pub enum ServiceEvent {
    CodecConfigure {
        configuration: CodecConfiguration,
        responder: CodecConfigureResponder,
    },
    QosConfigure {
        /// Peer configuring this stream
        peer_id: PeerId,
        /// Stream ID of the stream being configured
        /// This ID is unique to the peer
        // TODO: Consider replacing with source or sink, AseId should map to (Peer, Cig, Cis) at
        // this point
        target_configuration: QosConfiguration,
        responder: QosConfigureResponder,
    },
    Enable {
        /// Peer enabling this stream
        peer_id: PeerId,
        /// Stream ID of the stream being configured
        ase_id: AseId,
        /// Stream that this is being tied to.
        /// This CIS may already be established after QosConfigure and will
        /// match the QosConfigured value.
        cis: (CigId, CisId),
        /// Additional Metadata provided
        /// Also available using
        /// AudioStreamControlServiceServer::get_metadata(peer_id, ase_id)
        metadata: Vec<Metadata>,
        /// Responder.  Responding positively to this will start streaming if
        /// the StreamEndpoint is a sink endpoint.  For a source
        /// endpoint, an additional Start event will be generated to
        /// indicate when the client is ready to receive data.
        responder: EnableResponder,
    },
    /// Ok to start streaming.
    /// Sent only for Source Endpoints.
    /// No responder, this event has already been accepted.
    Start {
        peer_id: PeerId,
        ase_id: AseId,
        cis: (CigId, CisId),
    },
    /// Disable this stream.
    /// Stop sending data on the CisId and CigId listed, or expect the client to
    /// stop sending audio data.
    Disable {
        /// Peer disabling this stream
        peer_id: PeerId,
        /// Stream ID of this stream
        ase_id: AseId,
        /// Isochronous Stream that was previously in use.
        cis: (CigId, CisId),
        /// Responder.  Responding positively will indicate to the client that
        /// data has ceased being sent.
        responder: DisableResponder,
    },
    /// Update Metadata
    UpdateMetadata {
        peer_id: PeerId,
        ase_id: AseId,
        metadata: Vec<Metadata>,
        responder: UpdateMetadataResponder,
    },
}

impl CodecConfiguration {
    fn into_event(
        self,
        endpoint: AudioStreamEndpoint,
        sender: oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
    ) -> crate::server::ServiceEvent {
        ServiceEvent::CodecConfigure {
            configuration: self.clone(),
            responder: crate::server::CodecConfigureResponder {
                endpoint,
                codec_id: self.codec_id,
                codec_config: self.codec_specific_configuration.clone(),
                sender,
            },
        }
    }
}

impl QosConfiguration {
    fn into_event(
        self,
        peer_id: PeerId,
        endpoint: AudioStreamEndpoint,
        sender: oneshot::Sender<Result<AudioStreamEndpoint, ResponseCode>>,
    ) -> crate::server::ServiceEvent {
        ServiceEvent::QosConfigure {
            peer_id,
            target_configuration: self.clone(),
            responder: crate::server::QosConfigureResponder {
                endpoint,
                configuration: self,
                sender,
            },
        }
    }
}

impl AseControlOperation {
    fn apply(
        self,
        peer_id: PeerId,
        endpoint_map: HashMap<AseId, AudioStreamEndpoint>,
    ) -> (Vec<crate::server::ServiceEvent>, AseControlOperationFut) {
        let mut current_response_codes = Vec::new();
        let mut waiting = Vec::new();
        let mut events: Vec<crate::server::ServiceEvent> = Vec::new();
        let mut endpoints = Vec::new();
        let opcode: Option<AseControlPointOpcode> = (&self).try_into().ok();
        match self {
            Self::ConfigCodec { codec_configurations, mut responses } => {
                current_response_codes.append(&mut responses);
                for codec_configuration in codec_configurations {
                    let ase_id = codec_configuration.ase_id;
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    let (send, recv) = oneshot::channel();
                    waiting.push(recv);
                    events.push(codec_configuration.into_event(endpoint.clone(), send));
                }
            }
            Self::ConfigQos { qos_configurations, mut responses } => {
                current_response_codes.append(&mut responses);
                for configuration in qos_configurations {
                    let ase_id = configuration.ase_id;
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    let (send, recv) = oneshot::channel();
                    waiting.push(recv);
                    events.push(configuration.into_event(peer_id, endpoint.clone(), send));
                }
            }
            Self::Enable { ases_with_metadata, mut responses } => {
                current_response_codes.append(&mut responses);
                for AseIdWithMetadata { ase_id, metadata } in ases_with_metadata {
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    let (sender, recv) = oneshot::channel();
                    waiting.push(recv);
                    let cis = endpoint.get_cis().unwrap();
                    let event = ServiceEvent::Enable {
                        ase_id,
                        cis,
                        peer_id,
                        metadata: metadata.clone(),
                        responder: EnableResponder { endpoint: endpoint.clone(), metadata, sender },
                    };
                    events.push(event);
                }
            }
            Self::ReceiverStartReady { ases } => {
                for ase_id in ases {
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    if endpoint.direction != AudioDirection::Source {
                        current_response_codes.push(ResponseCode::InvalidAseDirection { ase_id });
                        continue;
                    }
                    let cis = endpoint.get_cis().unwrap();
                    // Automatically accept.
                    let mut endpoint = endpoint.clone();
                    endpoint.state = AseState::Streaming;
                    endpoints.push(endpoint);
                    current_response_codes.push(ResponseCode::Success { ase_id });
                    events.push(ServiceEvent::Start { peer_id, ase_id, cis });
                }
            }
            Self::Disable { ases } => {
                for ase_id in ases {
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    let AseAdditionalParameters::Streaming { cig_id, cis_id, .. } =
                        endpoint.additional
                    else {
                        unreachable!();
                    };
                    if endpoint.direction == AudioDirection::Source {
                        // Accept automatically, and wait for the ReceiverStopReady to send Disable
                        // event.
                        let mut endpoint = endpoint.clone();
                        endpoint.state = AseState::Disabling;
                        endpoints.push(endpoint);
                        current_response_codes.push(ResponseCode::Success { ase_id });
                        continue;
                    }
                    let (sender, recv) = oneshot::channel();
                    waiting.push(recv);
                    events.push(ServiceEvent::Disable {
                        peer_id,
                        ase_id,
                        cis: (cig_id, cis_id),
                        responder: DisableResponder { endpoint: endpoint.clone(), sender },
                    });
                }
            }
            Self::ReceiverStopReady { ases } => {
                for ase_id in ases {
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    if endpoint.direction != AudioDirection::Source {
                        current_response_codes.push(ResponseCode::InvalidAseDirection { ase_id });
                        continue;
                    }
                    let AseAdditionalParameters::Streaming { cig_id, cis_id, .. } =
                        endpoint.additional
                    else {
                        unreachable!();
                    };
                    let (sender, recv) = oneshot::channel();
                    waiting.push(recv);
                    events.push(ServiceEvent::Disable {
                        peer_id,
                        ase_id,
                        cis: (cig_id, cis_id),
                        responder: DisableResponder { endpoint: endpoint.clone(), sender },
                    });
                }
            }
            Self::Release { ases } => {
                for ase_id in ases {
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if !opcode.unwrap().allowed_in_state(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    // Automatically accept.  We will automatically perform the Released operation
                    // on the next poll.
                    let mut endpoint = endpoint.clone();
                    endpoint.state = AseState::Releasing;
                    endpoint.additional = AseAdditionalParameters::None;
                    current_response_codes.push(ResponseCode::Success { ase_id });
                    endpoints.push(endpoint);
                }
            }
            Self::Released { ase_id } => {
                // Should only happen when we detect a link loss and are told so by the client.
                // Therefore we can transition immediately.
                // If there is no endpoint by that ase_id, we do nothing.
                if let Some(mut endpoint) = endpoint_map.get(&ase_id).cloned() {
                    // TODO(b/433287917): implement caching with either preferred or cached
                    // configurations
                    endpoint.state = AseState::Idle;
                    endpoint.additional = AseAdditionalParameters::None;
                    endpoints.push(endpoint);
                } else {
                    log::warn!("Received Released for unknown endpoint: {ase_id:?}");
                }
            }
            Self::UpdateMetadata { ases_with_metadata, mut responses } => {
                current_response_codes.append(&mut responses);
                for AseIdWithMetadata { ase_id, metadata } in ases_with_metadata {
                    let Some(endpoint) = endpoint_map.get(&ase_id) else {
                        current_response_codes
                            .push(ResponseCode::InvalidAseId { value: ase_id.into() });
                        continue;
                    };
                    if ![AseState::Enabling, AseState::Streaming].contains(&endpoint.state) {
                        current_response_codes
                            .push(ResponseCode::InvalidAseStateMachineTransition { ase_id });
                        continue;
                    }
                    let (sender, recv) = oneshot::channel();
                    waiting.push(recv);
                    events.push(ServiceEvent::UpdateMetadata {
                        peer_id,
                        ase_id,
                        metadata: metadata.clone(),
                        responder: UpdateMetadataResponder {
                            endpoint: endpoint.clone(),
                            metadata,
                            sender,
                        },
                    });
                }
            }
        }
        (
            events,
            AseControlOperationFut {
                peer_id,
                opcode,
                current_response_codes,
                endpoints,
                waiting: waiting.into_iter().collect(),
            },
        )
    }
}

impl<T: bt_gatt::ServerTypes> Stream for AudioStreamControlServiceServer<T> {
    type Item = Result<ServiceEvent, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        loop {
            self.as_mut().poll_operations(cx);
            let mut this = self.as_mut().project();
            if let Some(event) = this.outgoing_events.pop_front() {
                return Poll::Ready(Some(Ok(event)));
            }
            let gatt_event = match futures::ready!(this.local_service.as_mut().poll_next(cx)) {
                None => return Poll::Ready(None),
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                Some(Ok(event)) => event,
            };
            use bt_gatt::server::ServiceEvent::*;
            let peer_id = gatt_event.peer_id();
            let peer_entry = this
                .client_endpoints
                .entry(peer_id)
                .or_insert_with(|| this.default_client_endpoints.clone_for_peer(peer_id));
            match gatt_event {
                Read { handle, offset, responder, .. } => {
                    let offset = offset as usize;
                    if handle == CONTROL_POINT_HANDLE {
                        responder.error(GattError::ReadNotPermitted);
                        continue;
                    }
                    let Some(ase_id) = peer_entry.handles.get(&handle) else {
                        responder.error(GattError::InvalidHandle);
                        continue;
                    };
                    let Some(endpoint) = peer_entry.endpoints.get(ase_id) else {
                        responder.error(GattError::UnlikelyError);
                        continue;
                    };
                    let value = endpoint.into_char_value();
                    if offset > value.len() {
                        responder.error(GattError::InvalidOffset);
                        continue;
                    }
                    responder.respond(&value[offset..]);
                    continue;
                }
                Write { peer_id, handle, offset, value, responder } => {
                    if handle != CONTROL_POINT_HANDLE {
                        responder.error(GattError::WriteNotPermitted);
                        continue;
                    }
                    if offset != 0 {
                        // Offset write isn't allowed by the service?
                        // TODO: determine if partial writes should be allowed
                        responder.error(GattError::InvalidOffset);
                        continue;
                    }
                    responder.acknowledge();
                    match AseControlOperation::try_from(value.to_owned()) {
                        Ok(op) => self.as_mut().queue_operation(peer_id, op),
                        Err(response_code) => {
                            this.local_service.notify(
                                &CONTROL_POINT_HANDLE,
                                &response_code.error_notify_value(),
                                &[peer_id],
                            );
                        }
                    };
                    continue;
                }
                ClientConfiguration { peer_id, handle, notification_type } => {
                    log::info!(
                        "ASCS Got ClientConfig for {peer_id:?}: {handle:?} {notification_type:?}"
                    );
                }
                PeerInfo { peer_id, mtu, connected, .. } => {
                    log::info!(
                        "ASCS got PeerInfo {peer_id:?}: mtu {mtu:?}, connected: {connected:?}"
                    );
                }
                _ => continue,
            }
        }
    }
}

impl<T: bt_gatt::ServerTypes> FusedStream for AudioStreamControlServiceServer<T> {
    fn is_terminated(&self) -> bool {
        self.local_service.is_terminated()
    }
}
