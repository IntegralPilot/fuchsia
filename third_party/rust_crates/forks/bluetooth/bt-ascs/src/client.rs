// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::stream::{BoxStream, FuturesUnordered, StreamExt};
use std::collections::{HashMap, HashSet};

use bt_common::packet_encoding::Decodable;
use bt_common::Uuid;
use bt_gatt::client::{CharacteristicNotification, PeerService, ServiceCharacteristic};
use bt_gatt::types::Handle;

use crate::types::*;

/// 16-bit UUID value for the characteristics offered by the Audio
/// Stream Control Service.
pub const ASE_CONTROL_POINT_UUID: Uuid = Uuid::from_u16(0x2BC6);
pub const SINK_ASE_UUID: Uuid = Uuid::from_u16(0x2BC4);
pub const SOURCE_ASE_UUID: Uuid = Uuid::from_u16(0x2BC5);

#[derive(Debug, thiserror::Error, PartialEq, Clone)]
pub enum ClientError {
    #[error("Remote server is missing control point characteristic")]
    MissingControlPointCharacteristic,
    #[error("Remote server should only have 1 control point characteristic")]
    ExtraControlPointCharacteristic,
    #[error("Remote server doesn't have any audio stream endpoints")]
    MissingAudioStreamEndpoints,
    #[error("Notification stream closed before receiving all responses")]
    NotificationStreamClosed,
    #[error(
        "ASE {ase_id:?} transitioned to unexpected state: expected {expected:?}, got {actual:?}"
    )]
    UnexpectedStateTransition { ase_id: AseId, expected: AseState, actual: AseState },
    #[error("Unknown ASE ID: {0:?}")]
    UnknownAseId(AseId),
    #[error("ASE {ase_id:?} is in invalid starting state {actual:?} for opcode {opcode:?}")]
    InvalidStartState { ase_id: AseId, opcode: AseControlPointOpcode, actual: AseState },
    #[error("Remote server rejected operation: {0:?}")]
    RejectedOperation(Vec<ResponseCode>),
    #[error("ASE {ase_id:?} has invalid direction {direction:?} for opcode {opcode:?}")]
    InvalidDirection { ase_id: AseId, opcode: AseControlPointOpcode, direction: AudioDirection },
}

/// Represents a single source/sink ASE state characteristic.
pub struct ClientEndpoint {
    pub endpoint: AudioStreamEndpoint,
    pub notification_stream:
        BoxStream<'static, Result<CharacteristicNotification, bt_gatt::types::Error>>,
}

impl ClientEndpoint {
    pub async fn next_update(&mut self) -> Result<(AseId, AudioStreamEndpoint), Error> {
        let notification = self
            .notification_stream
            .next()
            .await
            .ok_or(Error::Client(ClientError::NotificationStreamClosed))??;
        let new_endpoint = AudioStreamEndpoint::from_char_value(
            self.endpoint.handle,
            self.endpoint.direction,
            &notification.value[..],
        )
        .map_err(|e| {
            bt_gatt::types::Error::Conversion(format!(
                "Failed to decode AudioStreamEndpoint: {:?}",
                e
            ))
        })?;
        Ok((self.endpoint.ase_id, new_endpoint))
    }
}

/// Represents a client request to configure QoS parameters on an ASE.
#[derive(Debug, Clone, PartialEq)]
pub enum QosConfigurationRequest {
    /// Completely custom QoS configuration parameters.
    Custom(QosConfiguration),

    /// Automatically derive constraint parameters (framing, PHYs,
    /// retransmissions, latency, presentation delay) from the server's
    /// published preferred parameters in the local cache, only requiring
    /// the host-allocated dynamic properties.
    Preferred {
        ase_id: AseId,
        cig_id: CigId,
        cis_id: CisId,
        sdu_interval: SduInterval,
        max_sdu: MaxSdu,
    },
}

impl QosConfigurationRequest {
    pub fn try_into_config<T: bt_gatt::GattTypes>(
        self,
        client: &AudioStreamControlServiceClient<T>,
    ) -> Result<QosConfiguration, Error> {
        match self {
            Self::Custom(c) => Ok(c),
            Self::Preferred { ase_id, cig_id, cis_id, sdu_interval, max_sdu } => {
                let endpoint = &client
                    .endpoints
                    .lookup_by_ase_id(ase_id)
                    .ok_or(Error::Client(ClientError::UnknownAseId(ase_id)))?
                    .endpoint;

                let params = endpoint.qos_parameters().ok_or(Error::Client(
                    ClientError::InvalidStartState {
                        ase_id,
                        opcode: AseControlPointOpcode::ConfigQos,
                        actual: endpoint.state,
                    },
                ))?;

                let presentation_delay = params.presentation_delay_range.min().clone();

                Ok(QosConfiguration {
                    ase_id,
                    cig_id,
                    cis_id,
                    sdu_interval,
                    framing: params.framing,
                    phy: params.preferred_phys.clone(),
                    max_sdu,
                    retransmission_number: params.preferred_retransmission_number,
                    max_transport_latency: params.max_transport_latency,
                    presentation_delay,
                })
            }
        }
    }
}

// See ASCS v1.0.1 4.2 for details.
pub struct AseControlPoint {
    pub handle: Handle,
    pub notification_stream:
        BoxStream<'static, Result<CharacteristicNotification, bt_gatt::types::Error>>,
}

/// Creates Audio Stream Control Service (ASCS) client instance
pub struct AudioStreamControlServiceClient<T: bt_gatt::GattTypes> {
    pub gatt_client: T::PeerService,
    pub control_point: AseControlPoint,
    pub endpoints: DiscoveredEndpoints,
}

pub struct DiscoveredEndpoints {
    pub sink: HashMap<Handle, ClientEndpoint>,
    pub source: HashMap<Handle, ClientEndpoint>,
}

impl DiscoveredEndpoints {
    /// Returns the ASE IDs of all discovered sink Audio Stream Endpoints.
    pub fn sink_ases(&self) -> Vec<AseId> {
        self.sink.values().map(|e| e.endpoint.ase_id).collect()
    }

    /// Returns the ASE IDs of all discovered source Audio Stream Endpoints.
    pub fn source_ases(&self) -> Vec<AseId> {
        self.source.values().map(|e| e.endpoint.ase_id).collect()
    }

    pub(crate) fn lookup_by_ase_id(&self, ase_id: AseId) -> Option<&ClientEndpoint> {
        if let Some(e) = self.sink.values().find(|e| e.endpoint.ase_id == ase_id) {
            return Some(e);
        }
        if let Some(e) = self.source.values().find(|e| e.endpoint.ase_id == ase_id) {
            return Some(e);
        }
        None
    }

    pub(crate) fn lookup_by_ase_id_mut(&mut self, ase_id: AseId) -> Option<&mut ClientEndpoint> {
        if let Some(e) = self.sink.values_mut().find(|e| e.endpoint.ase_id == ase_id) {
            return Some(e);
        }
        if let Some(e) = self.source.values_mut().find(|e| e.endpoint.ase_id == ase_id) {
            return Some(e);
        }
        None
    }
}

/// The outcome of an ASE control point operation, partitioning successfully
/// transitioned endpoints and failed response codes.
#[derive(Debug, Clone, PartialEq)]
pub struct AseControlOperationOutcome {
    accepted: HashMap<AseId, AudioStreamEndpoint>,
    rejected: HashMap<AseId, ResponseCode>,
}

impl AseControlOperationOutcome {
    pub(crate) fn new(
        accepted: HashMap<AseId, AudioStreamEndpoint>,
        rejected: HashMap<AseId, ResponseCode>,
    ) -> Self {
        Self { accepted, rejected }
    }

    /// Returns the map of successfully transitioned [`AudioStreamEndpoint`]s,
    /// keyed by their [`AseId`].
    pub fn accepted(&self) -> &HashMap<AseId, AudioStreamEndpoint> {
        &self.accepted
    }

    /// Returns the map of rejected [`ResponseCode`]s, keyed by their [`AseId`].
    pub fn rejected(&self) -> &HashMap<AseId, ResponseCode> {
        &self.rejected
    }
}

impl<T: bt_gatt::GattTypes> AudioStreamControlServiceClient<T> {
    pub async fn create(gatt_client: T::PeerService) -> Result<Self, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        let control_point = Self::discover_control_point(&gatt_client).await?;
        let endpoints = Self::discover_all_endpoints(&gatt_client).await?;

        Ok(Self { gatt_client, control_point, endpoints })
    }

    async fn discover_control_point(gatt_client: &T::PeerService) -> Result<AseControlPoint, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        let cp_chars = ServiceCharacteristic::<T>::find(gatt_client, ASE_CONTROL_POINT_UUID)
            .await
            .map_err(Error::Gatt)?;
        if cp_chars.is_empty() {
            return Err(ClientError::MissingControlPointCharacteristic.into());
        }
        if cp_chars.len() > 1 {
            return Err(ClientError::ExtraControlPointCharacteristic.into());
        }
        let cp_handle = *cp_chars[0].handle();
        let cp_stream = gatt_client.subscribe(&cp_handle);
        Ok(AseControlPoint { handle: cp_handle, notification_stream: cp_stream.boxed() })
    }

    async fn discover_all_endpoints(
        gatt_client: &T::PeerService,
    ) -> Result<DiscoveredEndpoints, Error>
    where
        <T as bt_gatt::GattTypes>::NotificationStream: std::marker::Send,
    {
        let sink_chars = ServiceCharacteristic::<T>::find(gatt_client, SINK_ASE_UUID)
            .await
            .map_err(Error::Gatt)?;

        let source_chars = ServiceCharacteristic::<T>::find(gatt_client, SOURCE_ASE_UUID)
            .await
            .map_err(Error::Gatt)?;

        if sink_chars.is_empty() && source_chars.is_empty() {
            return Err(ClientError::MissingAudioStreamEndpoints.into());
        }

        let mut sink_endpoints = HashMap::new();
        for c in sink_chars {
            let handle = *c.handle();
            let endpoint =
                Self::read_and_create_endpoint(gatt_client, handle, AudioDirection::Sink).await?;
            let notification_stream = gatt_client.subscribe(&handle).boxed();
            sink_endpoints.insert(handle, ClientEndpoint { endpoint, notification_stream });
        }

        let mut source_endpoints = HashMap::new();
        for c in source_chars {
            let handle = *c.handle();
            let endpoint =
                Self::read_and_create_endpoint(gatt_client, handle, AudioDirection::Source).await?;
            let notification_stream = gatt_client.subscribe(&handle).boxed();
            source_endpoints.insert(handle, ClientEndpoint { endpoint, notification_stream });
        }

        Ok(DiscoveredEndpoints { sink: sink_endpoints, source: source_endpoints })
    }

    /// Reads the current value of an ASE characteristic from the remote server
    /// and decodes it into an [`AudioStreamEndpoint`].
    async fn read_and_create_endpoint(
        gatt_client: &T::PeerService,
        handle: Handle,
        direction: AudioDirection,
    ) -> Result<AudioStreamEndpoint, Error> {
        let mut buf = vec![0; 255];
        let (read_bytes, _truncated) =
            gatt_client.read_characteristic(&handle, 0, &mut buf[..]).await.map_err(Error::Gatt)?;
        let endpoint = AudioStreamEndpoint::from_char_value(handle, direction, &buf[0..read_bytes])
            .map_err(|e| {
                bt_gatt::types::Error::Conversion(format!(
                    "Failed to decode AudioStreamEndpoint: {:?}",
                    e
                ))
            })?;
        Ok(endpoint)
    }

    /// Validates that the target ASE IDs are known to this client and that
    /// the requested `opcode` is allowed in their current state according
    /// to the ASCS state machine.
    ///
    /// Returns `Ok(())` if validation passes, or a `ClientError` (such as
    /// `UnknownAseId` or `InvalidStartState`) if any target ASE fails
    /// validation.
    fn validate_arguments<'a>(
        &mut self,
        opcode: AseControlPointOpcode,
        pending_ases: impl IntoIterator<Item = &'a AseId>,
    ) -> Result<(), Error> {
        for &ase_id in pending_ases {
            let endpoint = &self
                .endpoints
                .lookup_by_ase_id(ase_id)
                .ok_or(Error::Client(ClientError::UnknownAseId(ase_id)))?
                .endpoint;

            if !opcode.allowed_in_state(&endpoint.state) {
                return Err(Error::Client(ClientError::InvalidStartState {
                    ase_id,
                    opcode,
                    actual: endpoint.state,
                }));
            }
        }
        Ok(())
    }

    /// Encodes an ASE control operation and writes it to the ASE Control Point
    /// characteristic.
    async fn write_operation(&mut self, operation: &AseControlOperation) -> Result<(), Error> {
        use bt_common::packet_encoding::Encodable;

        let mut buf = vec![0; operation.encoded_len()];
        operation
            .encode(&mut buf[..])
            .map_err(|e| Error::Internal(format!("Failed to encode operation: {:?}", e)))?;

        self.gatt_client
            .write_characteristic(
                &self.control_point.handle,
                bt_gatt::types::WriteMode::WithoutResponse,
                0,
                &buf[..],
            )
            .await
            .map_err(Error::Gatt)
    }

    async fn collect_operation_responses(
        &mut self,
        opcode: u8,
        mut remaining_ases: HashSet<AseId>,
    ) -> Result<HashMap<AseId, ResponseCode>, Error> {
        let mut responses = HashMap::new();

        while !remaining_ases.is_empty() {
            let notification = self
                .control_point
                .notification_stream
                .next()
                .await
                .ok_or(Error::Client(ClientError::NotificationStreamClosed))?;
            let notification = notification.map_err(Error::from)?;

            let (cp_notification, _) = ControlPointNotification::decode(&notification.value[..]);
            let cp_notification = cp_notification.map_err(bt_gatt::types::Error::Encoding)?;
            if cp_notification.opcode != opcode {
                continue;
            }

            if cp_notification.number_of_ases == 0xFF {
                return Err(Error::Client(ClientError::RejectedOperation(
                    cp_notification.responses,
                )));
            }

            for r in cp_notification.responses {
                if remaining_ases.remove(&r.ase_id()) {
                    responses.insert(r.ase_id(), r);
                }
            }
        }

        Ok(responses)
    }

    async fn perform_and_verify_operation(
        &mut self,
        operation: AseControlOperation,
        pending_ases: HashSet<AseId>,
    ) -> Result<AseControlOperationOutcome, Error> {
        self.write_operation(&operation).await?;

        let opcode_u8 = u8::try_from(&operation)?;
        let responses = self.collect_operation_responses(opcode_u8, pending_ases).await?;

        let mut successful_endpoints = HashMap::new();
        let opcode = AseControlPointOpcode::try_from(&operation).map_err(|e| {
            Error::Internal(format!("Failed to resolve opcode from operation: {:?}", e))
        })?;

        let (successful, failed_responses): (
            HashMap<AseId, ResponseCode>,
            HashMap<AseId, ResponseCode>,
        ) = responses.into_iter().partition(|(_, r)| r.is_success());

        let successful_ases: HashSet<AseId> = successful.keys().cloned().collect();

        if successful_ases.is_empty() {
            return Ok(AseControlOperationOutcome::new(HashMap::new(), failed_responses));
        }

        let updated_endpoints = {
            let mut notification_futures = FuturesUnordered::new();
            for endpoint in
                self.endpoints.sink.values_mut().chain(self.endpoints.source.values_mut())
            {
                if !successful_ases.contains(&endpoint.endpoint.ase_id) {
                    continue;
                }
                notification_futures.push(endpoint.next_update());
            }

            let mut updated_endpoints = Vec::with_capacity(successful_ases.len());
            while let Some(res) = notification_futures.next().await {
                updated_endpoints.push(res?);
            }
            updated_endpoints
        };

        for (ase_id, endpoint) in &updated_endpoints {
            if let Some(client_endpoint) = self.endpoints.lookup_by_ase_id_mut(*ase_id) {
                client_endpoint.endpoint = endpoint.clone();
            }
        }

        for (ase_id, endpoint) in updated_endpoints {
            if !opcode.allows_transition_to(endpoint.direction, &endpoint.state) {
                let expected = opcode.operation_target_states(endpoint.direction)[0];
                return Err(Error::Client(ClientError::UnexpectedStateTransition {
                    ase_id: endpoint.ase_id,
                    expected,
                    actual: endpoint.state,
                }));
            }
            successful_endpoints.insert(ase_id, endpoint);
        }

        Ok(AseControlOperationOutcome::new(successful_endpoints, failed_responses))
    }

    /// Performs the Configure Codec operation on one or more ASEs.
    ///
    /// # Arguments
    /// * `codec_configurations` - A vector of `CodecConfiguration` structs
    ///   containing the parameters for each ASE to be configured.
    ///
    /// # Returns
    /// On success, returns an [`AseControlOperationOutcome`] containing the
    /// results of the operation.
    pub async fn configure_codec(
        &mut self,
        codec_configurations: Vec<CodecConfiguration>,
    ) -> Result<AseControlOperationOutcome, Error> {
        let pending_ases: HashSet<AseId> = codec_configurations.iter().map(|c| c.ase_id).collect();
        self.validate_arguments(AseControlPointOpcode::ConfigCodec, &pending_ases)?;

        let op = AseControlOperation::ConfigCodec { codec_configurations, responses: vec![] };

        self.perform_and_verify_operation(op, pending_ases).await
    }

    /// Performs the Configure QoS operation on one or more ASEs.
    ///
    /// # Arguments
    /// * `requests` - A vector of `QosConfigurationRequest` enums specifying
    ///   either custom or server-preferred configurations for each ASE.
    ///
    /// # Returns
    /// On success, returns an [`AseControlOperationOutcome`] containing the
    /// results of the operation.
    pub async fn configure_qos(
        &mut self,
        requests: Vec<QosConfigurationRequest>,
    ) -> Result<AseControlOperationOutcome, Error> {
        let qos_configurations: Vec<QosConfiguration> = requests
            .into_iter()
            .map(|req| req.try_into_config(self))
            .collect::<Result<Vec<_>, Error>>()?;

        let pending_ases: HashSet<AseId> = qos_configurations.iter().map(|q| q.ase_id).collect();
        self.validate_arguments(AseControlPointOpcode::ConfigQos, &pending_ases)?;

        // TODO(b/431814103): Consider ATT MTU limits when configuring multiple ASEs.
        // If the encoded request exceeds the current MTU, we may need to break it into
        // multiple Control Point writes.
        let op = AseControlOperation::ConfigQos { qos_configurations, responses: vec![] };

        self.perform_and_verify_operation(op, pending_ases).await
    }

    /// Performs the Enable operation on one or more ASEs.
    ///
    /// # Arguments
    /// * `ases_with_metadata` - A vector of `AseIdWithMetadata` structs
    ///   specifying the target ASE IDs and their codec metadata.
    ///
    /// # Returns
    /// On success, returns an [`AseControlOperationOutcome`] containing the
    /// results of the operation.
    pub async fn enable(
        &mut self,
        ases_with_metadata: Vec<AseIdWithMetadata>,
    ) -> Result<AseControlOperationOutcome, Error> {
        let pending_ases: HashSet<AseId> = ases_with_metadata.iter().map(|a| a.ase_id).collect();
        self.validate_arguments(AseControlPointOpcode::Enable, &pending_ases)?;

        let op = AseControlOperation::Enable { ases_with_metadata, responses: vec![] };

        self.perform_and_verify_operation(op, pending_ases).await
    }

    /// Performs the Receiver Start Ready operation on one or more Source ASEs.
    ///
    /// # Arguments
    /// * `ases` - A vector of `AseId`s to start.
    ///
    /// # Returns
    /// On success, returns an [`AseControlOperationOutcome`] containing the
    /// results of the operation.
    pub async fn receiver_start_ready(
        &mut self,
        ases: Vec<AseId>,
    ) -> Result<AseControlOperationOutcome, Error> {
        let pending_ases: HashSet<AseId> = ases.iter().cloned().collect();
        self.validate_arguments(AseControlPointOpcode::ReceiverStartReady, &pending_ases)?;

        for ase_id in &pending_ases {
            let endpoint = self
                .endpoints
                .lookup_by_ase_id(*ase_id)
                .expect("ASE ID verified by validate_arguments");
            if endpoint.endpoint.direction != AudioDirection::Source {
                return Err(Error::Client(ClientError::InvalidDirection {
                    ase_id: *ase_id,
                    opcode: AseControlPointOpcode::ReceiverStartReady,
                    direction: endpoint.endpoint.direction,
                }));
            }
        }

        let op = AseControlOperation::ReceiverStartReady { ases };

        self.perform_and_verify_operation(op, pending_ases).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bt_common::core::CodecId;
    use bt_gatt::test_utils::{FakePeerService, FakeTypes};
    use bt_gatt::types::{AttributePermissions, CharacteristicProperty};
    use bt_gatt::Characteristic;

    const CONTROL_POINT_HANDLE: Handle = Handle(1);
    const SINK_ASE_HANDLE: Handle = Handle(2);
    const SOURCE_ASE_HANDLE: Handle = Handle(3);

    fn setup_fake_service() -> FakePeerService {
        let mut service = FakePeerService::new();
        // Add Control Point
        service.add_characteristic(
            Characteristic {
                handle: CONTROL_POINT_HANDLE,
                uuid: ASE_CONTROL_POINT_UUID,
                properties: CharacteristicProperty::Write
                    | CharacteristicProperty::WriteWithoutResponse
                    | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        // Add Sink ASE
        service.add_characteristic(
            Characteristic {
                handle: SINK_ASE_HANDLE,
                uuid: SINK_ASE_UUID,
                properties: CharacteristicProperty::Read | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![0x01, 0x00], // ASE ID 1, State Idle
        );
        // Add Source ASE
        service.add_characteristic(
            Characteristic {
                handle: SOURCE_ASE_HANDLE,
                uuid: SOURCE_ASE_UUID,
                properties: CharacteristicProperty::Read | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![0x02, 0x00], // ASE ID 2, State Idle
        );
        service
    }

    fn run_to_completion<F: std::future::Future>(fut: F) -> F::Output {
        let mut fut = std::pin::pin!(fut);
        let mut cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(res) => res,
            std::task::Poll::Pending => panic!("Future did not complete synchronously"),
        }
    }

    #[test]
    fn client_creation_success() {
        let service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let client = run_to_completion(client_fut).expect("client creation should succeed");

        assert_eq!(client.control_point.handle, CONTROL_POINT_HANDLE);
        assert_eq!(client.endpoints.sink.len(), 1);
        assert_eq!(client.endpoints.source.len(), 1);

        let sink = &client.endpoints.sink[&SINK_ASE_HANDLE].endpoint;
        assert_eq!(sink.ase_id, AseId(1));
        assert_eq!(sink.state, AseState::Idle);
        assert_eq!(sink.direction, AudioDirection::Sink);

        let source = &client.endpoints.source[&SOURCE_ASE_HANDLE].endpoint;
        assert_eq!(source.ase_id, AseId(2));
        assert_eq!(source.state, AseState::Idle);
        assert_eq!(source.direction, AudioDirection::Source);

        assert_eq!(client.endpoints.sink_ases(), vec![AseId(1)]);
        assert_eq!(client.endpoints.source_ases(), vec![AseId(2)]);
    }

    #[test]
    fn client_creation_missing_control_point() {
        let mut service = FakePeerService::new();
        // Only add Sink (no Control Point)
        service.add_characteristic(
            Characteristic {
                handle: SINK_ASE_HANDLE,
                uuid: SINK_ASE_UUID,
                properties: CharacteristicProperty::Read | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![0x01, 0x00],
        );

        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service);
        let err = match run_to_completion(client_fut) {
            Err(e) => e,
            Ok(_) => panic!("expected error, got Ok"),
        };
        assert!(matches!(err, Error::Client(ClientError::MissingControlPointCharacteristic)));
    }

    #[test]
    fn client_creation_missing_endpoints() {
        let mut service = FakePeerService::new();
        // Only add Control Point (no endpoints)
        service.add_characteristic(
            Characteristic {
                handle: CONTROL_POINT_HANDLE,
                uuid: ASE_CONTROL_POINT_UUID,
                properties: CharacteristicProperty::Write | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service);
        let err = match run_to_completion(client_fut) {
            Err(e) => e,
            Ok(_) => panic!("expected error, got Ok"),
        };
        assert!(matches!(err, Error::Client(ClientError::MissingAudioStreamEndpoints)));
    }

    #[test]
    fn client_creation_extra_control_point() {
        let mut service = FakePeerService::new();
        // Add first Control Point
        service.add_characteristic(
            Characteristic {
                handle: CONTROL_POINT_HANDLE,
                uuid: ASE_CONTROL_POINT_UUID,
                properties: CharacteristicProperty::Write | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );
        // Add second Control Point
        service.add_characteristic(
            Characteristic {
                handle: Handle(4),
                uuid: ASE_CONTROL_POINT_UUID,
                properties: CharacteristicProperty::Write | CharacteristicProperty::Notify,
                permissions: AttributePermissions::default(),
                descriptors: vec![],
            },
            vec![],
        );

        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service);
        let err = match run_to_completion(client_fut) {
            Err(e) => e,
            Ok(_) => panic!("expected error, got Ok"),
        };
        assert!(matches!(err, Error::Client(ClientError::ExtraControlPointCharacteristic)));
    }

    #[test]
    fn configure_codec() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        let config1 = CodecConfiguration {
            ase_id: AseId(1),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };
        let config2 = CodecConfiguration {
            ase_id: AseId(2),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x01, // Opcode: Config Codec
                0x02, // Num ASEs
                0x01, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 1 Config
                0x02, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 2 Config
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x01, // Opcode
                    0x02, // Num ASEs
                    0x01, 0x00, 0x00, // ASE 1 Response: Success
                    0x02, 0x03, 0x02, // ASE 2 Response: Failed (Invalid ASE ID)
                ],
                maybe_truncated: false,
            }),
        );

        #[rustfmt::skip]
        service.notify(
            &SINK_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SINK_ASE_HANDLE,
                value: vec![
                    0x01, // ASE ID: 1
                    0x01, // ASE State: Codec Configured
                    0x00, 0x01, 0x02, 0x0A, 0x00, 0x10, 0x27, 0x00, 0x40, 0x9C, 0x00,
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
                ],
                maybe_truncated: false,
            }),
        );

        let configure_fut = client.configure_codec(vec![config1, config2]);
        let outcome = run_to_completion(configure_fut).expect("configure codec should succeed");

        assert_eq!(
            client.endpoints.sink[&SINK_ASE_HANDLE].endpoint.state,
            AseState::CodecConfigured
        );
        assert_eq!(outcome.accepted().len(), 1);
        let endpoint = &outcome.accepted()[&AseId(1)];
        let qos = endpoint.qos_parameters().unwrap();
        assert_eq!(qos.framing, Framing::Unframed);
        assert_eq!(qos.preferred_phys, vec![Phy::Le1MPhy]);
        assert_eq!(qos.preferred_retransmission_number, 2);

        assert_eq!(outcome.rejected().len(), 1);
        assert_eq!(outcome.rejected()[&AseId(2)], ResponseCode::InvalidAseId { value: 2 });
    }

    #[test]
    fn configure_codec_fail_unknown_ase_id() {
        let service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        let config = CodecConfiguration {
            ase_id: AseId(99), // Non-existent ASE ID
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };

        let configure_fut = client.configure_codec(vec![config]);
        let err = run_to_completion(configure_fut).expect_err("should fail due to unknown ASE ID");
        assert!(matches!(err, Error::Client(ClientError::UnknownAseId(AseId(99)))));
    }

    #[test]
    fn configure_codec_global_error() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        let config = CodecConfiguration {
            ase_id: AseId(1),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x01, // Opcode: Config Codec
                0x01, // Num ASEs
                0x01, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 1 Config
            ],
        );

        // Let's mimic server side throwing a global invalid length error.
        // (in reality this wouldn't happen because our configure codec operation was
        // encoded correctly).
        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x01, // Opcode
                    0xFF, // Num ASEs: Global Error
                    0x00, 0x02, 0x00, // ASE 0, Invalid Length, None
                ],
                maybe_truncated: false,
            }),
        );

        let configure_fut = client.configure_codec(vec![config]);
        let err = run_to_completion(configure_fut).expect_err("should fail with global error");

        assert!(matches!(
            err,
            Error::Client(ClientError::RejectedOperation(ref responses)) if matches!(
                responses.as_slice(),
                [ResponseCode::InvalidLength { .. }]
            )
        ));
    }

    #[test]
    fn configure_qos_success() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Pre-condition both Sink and Source endpoints to CodecConfigured with valid
        // QoS constraints
        let sink_value = vec![
            0x01, // ASE ID: 1
            0x01, // ASE State: Codec Configured
            0x00, 0x01, 0x02, 0x0A, 0x00, 0x10, 0x27, 0x00, 0x40, 0x9C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        client.endpoints.sink.get_mut(&SINK_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SINK_ASE_HANDLE,
                AudioDirection::Sink,
                &sink_value,
            )
            .unwrap();

        let source_value = vec![
            0x02, // ASE ID: 2
            0x01, // ASE State: Codec Configured
            0x00, 0x01, 0x02, 0x0A, 0x00, 0x10, 0x27, 0x00, 0x40, 0x9C, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];
        client.endpoints.source.get_mut(&SOURCE_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SOURCE_ASE_HANDLE,
                AudioDirection::Source,
                &source_value,
            )
            .unwrap();

        // Request 1 (Preferred): Resolves constraints for AseId(1) from the cache
        let req1 = QosConfigurationRequest::Preferred {
            ase_id: AseId(1),
            cig_id: CigId::try_from(1).unwrap(),
            cis_id: CisId::try_from(1).unwrap(),
            sdu_interval: SduInterval::try_from(std::time::Duration::from_micros(10000)).unwrap(),
            max_sdu: MaxSdu::try_from(100).unwrap(),
        };

        // Request 2 (Custom): Custom configuration for AseId(2)
        let req2 = QosConfigurationRequest::Custom(QosConfiguration {
            ase_id: AseId(2),
            cig_id: CigId::try_from(1).unwrap(),
            cis_id: CisId::try_from(2).unwrap(),
            sdu_interval: SduInterval::try_from(std::time::Duration::from_micros(10000)).unwrap(),
            framing: Framing::Unframed,
            phy: vec![Phy::Le1MPhy],
            max_sdu: MaxSdu::try_from(100).unwrap(),
            retransmission_number: 2,
            max_transport_latency: MaxTransportLatency::try_from(std::time::Duration::from_millis(
                10,
            ))
            .unwrap(),
            presentation_delay: PresentationDelay { microseconds: 40000 },
        });

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x02, // Opcode: Config QoS
                0x02, // Num ASEs: 2
                0x01, // ASE ID: 1
                0x01, 0x01, 0x10, 0x27, 0x00, 0x00, 0x01, 0x64, 0x00, 0x02, 0x0A, 0x00, 0x10, 0x27, 0x00, // ASE 1 Preferred QoS
                0x02, // ASE ID: 2
                0x01, 0x02, 0x10, 0x27, 0x00, 0x00, 0x01, 0x64, 0x00, 0x02, 0x0A, 0x00, 0x40, 0x9C, 0x00, // ASE 2 Custom QoS
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x02, // Opcode: Config QoS
                    0x02, // Num ASEs: 2
                    0x01, 0x00, 0x00, // ASE ID: 1, Success
                    0x02, 0x00, 0x00, // ASE ID: 2, Success
                ],
                maybe_truncated: false,
            }),
        );

        #[rustfmt::skip]
        service.notify(
            &SINK_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SINK_ASE_HANDLE,
                value: vec![
                    0x01, // ASE ID: 1
                    0x02, // ASE State: QoS Configured
                    0x01, 0x01, 0x10, 0x27, 0x00, 0x00, 0x01, 0x64, 0x00, 0x02, 0x0A, 0x00, 0x40, 0x9C, 0x00,
                ],
                maybe_truncated: false,
            }),
        );

        #[rustfmt::skip]
        service.notify(
            &SOURCE_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SOURCE_ASE_HANDLE,
                value: vec![
                    0x02, // ASE ID: 2
                    0x02, // ASE State: QoS Configured
                    0x01, 0x02, 0x10, 0x27, 0x00, 0x00, 0x01, 0x64, 0x00, 0x02, 0x0A, 0x00, 0x40, 0x9C, 0x00,
                ],
                maybe_truncated: false,
            }),
        );

        let configure_qos_fut = client.configure_qos(vec![req1, req2]);
        let outcome = run_to_completion(configure_qos_fut).expect("configure QoS should succeed");

        assert_eq!(outcome.rejected().len(), 0);
        assert_eq!(client.endpoints.sink[&SINK_ASE_HANDLE].endpoint.state, AseState::QosConfigured);
        assert_eq!(
            client.endpoints.source[&SOURCE_ASE_HANDLE].endpoint.state,
            AseState::QosConfigured
        );
    }

    #[test]
    fn configure_qos_fail_invalid_start_state() {
        let service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Transition the sink ASE to Streaming state to test client-side validation.
        let streaming_value = vec![
            0x01, // ASE ID: 1
            0x04, // ASE State: Streaming
            0x01, 0x01, 0x00,
        ];
        client.endpoints.sink.get_mut(&SINK_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SINK_ASE_HANDLE,
                AudioDirection::Sink,
                &streaming_value,
            )
            .unwrap();

        let qos_config = QosConfiguration {
            ase_id: AseId(1),
            cig_id: CigId::try_from(1).unwrap(),
            cis_id: CisId::try_from(1).unwrap(),
            sdu_interval: SduInterval::try_from(std::time::Duration::from_micros(10000)).unwrap(),
            framing: Framing::Unframed,
            phy: vec![Phy::Le1MPhy],
            max_sdu: MaxSdu::try_from(100).unwrap(),
            retransmission_number: 2,
            max_transport_latency: MaxTransportLatency::try_from(std::time::Duration::from_millis(
                10,
            ))
            .unwrap(),
            presentation_delay: PresentationDelay { microseconds: 40000 },
        };

        let configure_qos_fut =
            client.configure_qos(vec![QosConfigurationRequest::Custom(qos_config)]);
        let err =
            run_to_completion(configure_qos_fut).expect_err("should fail client-side validation");

        assert!(matches!(
            err,
            Error::Client(ClientError::InvalidStartState {
                ase_id: AseId(1),
                opcode: AseControlPointOpcode::ConfigQos,
                actual: AseState::Streaming,
            })
        ));
    }

    #[test]
    fn configure_codec_fail_malformed_control_point_notification() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        let config = CodecConfiguration {
            ase_id: AseId(1),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x01, // Opcode: Config Codec
                0x01, // Num ASEs
                0x01, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 1 Config
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![0x01], // Only 1 byte, malformed (< 2 bytes)
                maybe_truncated: false,
            }),
        );

        let configure_fut = client.configure_codec(vec![config]);
        let err = run_to_completion(configure_fut).expect_err("should fail with decoding error");

        assert!(matches!(
            err,
            Error::Gatt(bt_gatt::types::Error::Encoding(
                bt_common::packet_encoding::Error::UnexpectedDataLength
            ))
        ));
    }

    #[test]
    fn configure_codec_fail_malformed_ase_notification() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        let config = CodecConfiguration {
            ase_id: AseId(1),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x01, // Opcode: Config Codec
                0x01, // Num ASEs
                0x01, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 1 Config
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x01, // Opcode: Config Codec
                    0x01, // Num ASEs: 1
                    0x01, 0x00, 0x00, // ASE 1 Response: Success
                ],
                maybe_truncated: false,
            }),
        );

        #[rustfmt::skip]
        service.notify(
            &SINK_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SINK_ASE_HANDLE,
                value: vec![0x01], // Only 1 byte, malformed (< 2 bytes)
                maybe_truncated: false,
            }),
        );

        let configure_fut = client.configure_codec(vec![config]);
        let err = run_to_completion(configure_fut).expect_err("should fail with conversion error");

        assert!(matches!(
            err,
            Error::Gatt(bt_gatt::types::Error::Conversion(ref s)) if s.contains("Failed to decode AudioStreamEndpoint")
        ));
    }

    #[test]
    fn configure_codec_fail_unexpected_state_updates_local_cache() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        let config1 = CodecConfiguration {
            ase_id: AseId(1),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };
        let config2 = CodecConfiguration {
            ase_id: AseId(2),
            target_latency: TargetLatency::TargetLowLatency,
            target_phy: TargetPhy::Le1MPhy,
            codec_id: CodecId::Assigned(bt_common::core::CodingFormat::Lc3),
            codec_specific_configuration: vec![],
        };

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x01, // Opcode: Config Codec
                0x02, // Num ASEs
                0x01, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 1 Config
                0x02, 0x01, 0x01, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, // ASE 2 Config
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x01, // Opcode: Config Codec
                    0x02, // Num ASEs: 2
                    0x01, 0x00, 0x00, // ASE 1 Response: Success
                    0x02, 0x00, 0x00, // ASE 2 Response: Success
                ],
                maybe_truncated: false,
            }),
        );

        // Notify ASE 1 with Codec Configured (valid!)
        #[rustfmt::skip]
        service.notify(
            &SINK_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SINK_ASE_HANDLE,
                value: vec![
                    0x01, // ASE ID: 1
                    0x01, // ASE State: Codec Configured
                    0x00, 0x01, 0x02, 0x0A, 0x00, 0x10, 0x27, 0x00, 0x40, 0x9C, 0x00,
                    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00,
                ],
                maybe_truncated: false,
            }),
        );

        // Notify ASE 2 with Streaming (invalid for Config Codec!)
        #[rustfmt::skip]
        service.notify(
            &SOURCE_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SOURCE_ASE_HANDLE,
                value: vec![
                    0x02, // ASE ID: 2
                    0x04, // ASE State: Streaming (unexpected!)
                    0x01, // CIG ID: 1
                    0x02, // CIS ID: 2
                    0x00, // Metadata Length: 0
                ],
                maybe_truncated: false,
            }),
        );

        let configure_fut = client.configure_codec(vec![config1, config2]);
        let err = run_to_completion(configure_fut)
            .expect_err("should fail with unexpected state transition");

        assert!(matches!(
            err,
            Error::Client(ClientError::UnexpectedStateTransition {
                ase_id: AseId(2),
                expected: AseState::CodecConfigured,
                actual: AseState::Streaming,
            })
        ));

        // Both endpoints in our local cache MUST be updated with the latest notified
        // states!
        assert_eq!(
            client.endpoints.sink[&SINK_ASE_HANDLE].endpoint.state,
            AseState::CodecConfigured
        );
        assert_eq!(client.endpoints.source[&SOURCE_ASE_HANDLE].endpoint.state, AseState::Streaming);
    }

    #[test]
    fn enable_success() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Pre-condition SINK_ASE_HANDLE to QosConfigured state
        let sink_value = vec![
            0x01, // ASE ID: 1
            0x02, // ASE State: QoS Configured
            0x01, 0x01, 0x10, 0x27, 0x00, 0x00, 0x01, 0x64, 0x00, 0x02, 0x0A, 0x00, 0x40, 0x9C,
            0x00,
        ];
        client.endpoints.sink.get_mut(&SINK_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SINK_ASE_HANDLE,
                AudioDirection::Sink,
                &sink_value,
            )
            .unwrap();

        let enable_req = AseIdWithMetadata { ase_id: AseId(1), metadata: vec![] };

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x03, // Opcode: Enable
                0x01, // Num ASEs
                0x01, // ASE ID: 1
                0x00, // Metadata Length: 0
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x03, // Opcode: Enable
                    0x01, // Num ASEs
                    0x01, 0x00, 0x00, // ASE ID: 1, Success
                ],
                maybe_truncated: false,
            }),
        );

        #[rustfmt::skip]
        service.notify(
            &SINK_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SINK_ASE_HANDLE,
                value: vec![
                    0x01, // ASE ID: 1
                    0x03, // ASE State: Enabling
                    0x01, // CIG ID: 1
                    0x01, // CIS ID: 1
                    0x00, // Metadata Length: 0
                ],
                maybe_truncated: false,
            }),
        );

        let enable_fut = client.enable(vec![enable_req]);
        let outcome = run_to_completion(enable_fut).expect("enable should succeed");

        assert_eq!(outcome.rejected().len(), 0);
        assert_eq!(client.endpoints.sink[&SINK_ASE_HANDLE].endpoint.state, AseState::Enabling);
    }

    #[test]
    fn enable_fail_invalid_start_state() {
        let service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Transition the sink ASE to Streaming state to test client-side validation.
        let streaming_value = vec![
            0x01, // ASE ID: 1
            0x04, // ASE State: Streaming
            0x01, // CIG ID: 1
            0x01, // CIS ID: 1
            0x00, // Metadata Length: 0
        ];
        client.endpoints.sink.get_mut(&SINK_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SINK_ASE_HANDLE,
                AudioDirection::Sink,
                &streaming_value,
            )
            .unwrap();

        let enable_req = AseIdWithMetadata { ase_id: AseId(1), metadata: vec![] };

        let enable_fut = client.enable(vec![enable_req]);
        let err = run_to_completion(enable_fut).expect_err("should fail client-side validation");

        assert!(matches!(
            err,
            Error::Client(ClientError::InvalidStartState {
                ase_id: AseId(1),
                opcode: AseControlPointOpcode::Enable,
                actual: AseState::Streaming,
            })
        ));
    }

    #[test]
    fn receiver_start_ready_success() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Pre-condition SOURCE_ASE_HANDLE to Enabling state
        let source_value = vec![
            0x02, // ASE ID: 2
            0x03, // ASE State: Enabling
            0x01, 0x01, 0x00,
        ];
        client.endpoints.source.get_mut(&SOURCE_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SOURCE_ASE_HANDLE,
                AudioDirection::Source,
                &source_value,
            )
            .unwrap();

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x04, // Opcode: Receiver Start Ready
                0x01, // Num ASEs
                0x02, // ASE ID: 2
            ],
        );

        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x04, // Opcode: Receiver Start Ready
                    0x01, // Num ASEs
                    0x02, 0x00, 0x00, // ASE ID: 2, Success
                ],
                maybe_truncated: false,
            }),
        );

        #[rustfmt::skip]
        service.notify(
            &SOURCE_ASE_HANDLE,
            Ok(CharacteristicNotification {
                handle: SOURCE_ASE_HANDLE,
                value: vec![
                    0x02, // ASE ID: 2
                    0x04, // ASE State: Streaming
                    0x01, // CIG ID: 1
                    0x01, // CIS ID: 1
                    0x00, // Metadata Length: 0
                ],
                maybe_truncated: false,
            }),
        );

        let start_fut = client.receiver_start_ready(vec![AseId(2)]);
        let outcome = run_to_completion(start_fut).expect("receiver start ready should succeed");

        assert_eq!(outcome.rejected().len(), 0);
        assert_eq!(client.endpoints.source[&SOURCE_ASE_HANDLE].endpoint.state, AseState::Streaming);
    }

    #[test]
    fn receiver_start_ready_fail_invalid_start_state() {
        let service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // ASE 2 (Source) is in Idle state, which is invalid for ReceiverStartReady
        let start_fut = client.receiver_start_ready(vec![AseId(2)]);
        let err = run_to_completion(start_fut).expect_err("should fail client-side validation");

        assert!(matches!(
            err,
            Error::Client(ClientError::InvalidStartState {
                ase_id: AseId(2),
                opcode: AseControlPointOpcode::ReceiverStartReady,
                actual: AseState::Idle,
            })
        ));
    }

    #[test]
    fn receiver_start_ready_fail_invalid_direction() {
        let service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Pre-condition SINK_ASE_HANDLE (ASE 1) to Enabling state
        let sink_value = vec![
            0x01, // ASE ID: 1
            0x03, // ASE State: Enabling
            0x01, 0x01, 0x00,
        ];
        client.endpoints.sink.get_mut(&SINK_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SINK_ASE_HANDLE,
                AudioDirection::Sink,
                &sink_value,
            )
            .unwrap();

        let start_fut = client.receiver_start_ready(vec![AseId(1)]);
        let err = run_to_completion(start_fut).expect_err("should fail client-side validation");

        assert!(matches!(
            err,
            Error::Client(ClientError::InvalidDirection {
                ase_id: AseId(1),
                opcode: AseControlPointOpcode::ReceiverStartReady,
                direction: AudioDirection::Sink,
            })
        ));
    }

    #[test]
    fn receiver_start_ready_rejected() {
        let mut service = setup_fake_service();
        let client_fut = AudioStreamControlServiceClient::<FakeTypes>::create(service.clone());
        let mut client = run_to_completion(client_fut).expect("client creation should succeed");

        // Pre-condition SOURCE_ASE_HANDLE to Enabling state
        let source_value = vec![
            0x02, // ASE ID: 2
            0x03, // ASE State: Enabling
            0x01, 0x01, 0x00,
        ];
        client.endpoints.source.get_mut(&SOURCE_ASE_HANDLE).unwrap().endpoint =
            AudioStreamEndpoint::from_char_value(
                SOURCE_ASE_HANDLE,
                AudioDirection::Source,
                &source_value,
            )
            .unwrap();

        #[rustfmt::skip]
        service.expect_characteristic_value(
            &CONTROL_POINT_HANDLE,
            vec![
                0x04, // Opcode: Receiver Start Ready
                0x01, // Num ASEs
                0x02, // ASE ID: 2
            ],
        );

        // Notify rejection from server due to Insufficient Resources (Response Code
        // 0x0D)
        #[rustfmt::skip]
        service.notify(
            &CONTROL_POINT_HANDLE,
            Ok(CharacteristicNotification {
                handle: CONTROL_POINT_HANDLE,
                value: vec![
                    0x04, // Opcode: Receiver Start Ready
                    0x01, // Num ASEs
                    0x02, 0x0D, 0x00, // ASE ID: 2, Insufficient Resources
                ],
                maybe_truncated: false,
            }),
        );

        let start_fut = client.receiver_start_ready(vec![AseId(2)]);
        let outcome = run_to_completion(start_fut).expect("receiver start ready should succeed");

        assert_eq!(outcome.rejected().len(), 1);
        assert_eq!(
            outcome.rejected()[&AseId(2)],
            ResponseCode::InsufficientResources { ase_id: AseId(2) }
        );
        // The state should remain Enabling
        assert_eq!(client.endpoints.source[&SOURCE_ASE_HANDLE].endpoint.state, AseState::Enabling);
    }
}
