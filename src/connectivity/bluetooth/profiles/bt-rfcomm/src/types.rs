// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use bt_rfcomm::ServerChannel;
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::profile::{
    ChannelParameters, Psm, ServiceDefinition, combine_channel_parameters,
};
use fuchsia_bluetooth::types::PeerId;
use std::collections::{BTreeMap, HashSet};

use crate::profile::{psms_from_service_definitions, server_channels_from_service_definitions};

/// Every group of registered services will be assigned a ServiceGroupHandle to track
/// relevant information about the advertisement. There can be multiple `ServiceGroupHandle`s
/// per profile client. A unique handle is assigned per Profile.Advertise() call.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ServiceGroupHandle(usize);

/// Toplevel object for managing the active BR/EDR service advertisements.
///
/// Each advertisement is represented by a `ServiceGroup` and is uniquely
/// identified by a `ServiceGroupHandle`.
pub struct Services {
    /// Maps each unique ServiceGroupHandle to its corresponding ServiceGroup.
    groups: BTreeMap<ServiceGroupHandle, ServiceGroup>,
    /// Monotonic counter used to generate unique ServiceGroupHandle values.
    /// Handles are never reused, ensuring that stale events tagged with older
    /// handles cannot affect new registrations.
    next_handle_id: usize,
}

impl Services {
    pub fn new() -> Self {
        Self { groups: BTreeMap::new(), next_handle_id: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    pub fn get_mut(&mut self, handle: ServiceGroupHandle) -> Option<&mut ServiceGroup> {
        self.groups.get_mut(&handle)
    }

    /// Removes the service group identified by `handle`.
    ///
    /// Returns `Some(ServiceGroup)` if the group was active, or `None` if it was
    /// already removed (e.g., due to a prior revocation event).
    pub fn remove(&mut self, handle: ServiceGroupHandle) -> Option<ServiceGroup> {
        self.groups.remove(&handle)
    }

    pub fn insert(&mut self, service: ServiceGroup) -> ServiceGroupHandle {
        let handle = ServiceGroupHandle(self.next_handle_id);
        self.next_handle_id = self.next_handle_id.wrapping_add(1);
        let old_group = self.groups.insert(handle, service);
        debug_assert!(old_group.is_none(), "Duplicate ServiceGroupHandle assigned");
        handle
    }

    pub fn iter(&self) -> impl Iterator<Item = (ServiceGroupHandle, &ServiceGroup)> {
        self.groups.iter().map(|(handle, data)| (*handle, data))
    }

    /// Returns all the service definitions in the collection.
    pub fn service_definitions(&self) -> Vec<ServiceDefinition> {
        let mut services = Vec::new();
        for (_, data) in self.iter() {
            services.extend(data.service_defs().clone());
        }
        services
    }

    /// Returns currently registered PSMs.
    pub fn psms(&self) -> HashSet<Psm> {
        self.iter().map(|(_, data)| data.allocated_psms()).fold(
            HashSet::new(),
            |mut psms, current| {
                psms.extend(current);
                psms
            },
        )
    }

    /// Attempts to build a set of AdvertiseParams from the services in the group.
    /// Returns None if there are no services to be advertised.
    pub fn build_registration(&self) -> Option<AdvertiseParams> {
        if self.is_empty() {
            return None;
        }

        let mut services = Vec::new();
        let mut parameters = ChannelParameters::default();

        for (_, data) in self.iter() {
            services.extend(data.service_defs().clone());
            parameters = combine_channel_parameters(&parameters, data.channel_parameters());
        }

        Some(AdvertiseParams { services, parameters })
    }
}

/// Parameters needed to advertise a service.
#[derive(Clone, Debug, PartialEq)]
pub struct AdvertiseParams {
    pub services: Vec<ServiceDefinition>,
    pub parameters: ChannelParameters,
}

impl AdvertiseParams {
    pub fn into_advertise_request(
        self,
        receiver: ClientEnd<bredr::ConnectionReceiverMarker>,
    ) -> bredr::ProfileAdvertiseRequest {
        let fidl_services = ServiceDefinition::try_into_fidl(&self.services).unwrap();

        bredr::ProfileAdvertiseRequest {
            services: Some(fidl_services),
            parameters: Some((&self.parameters).try_into().unwrap()),
            receiver: Some(receiver),
            ..Default::default()
        }
    }
}

/// Information associated with BR/EDR service advertisement made by a `bredr.Profile` client.
#[derive(Debug)]
pub struct ServiceGroup {
    /// Connection to the FIDL client that made the `bredr.Advertise` request.
    /// Incoming L2CAP connections from a remote peer are relayed to this connection `receiver`.
    receiver: bredr::ConnectionReceiverProxy,

    /// The ChannelParameters for this service advertisement.
    channel_parameters: ChannelParameters,

    /// The service definitions that are advertised.
    service_defs: Vec<ServiceDefinition>,

    /// The allocated PSMs associated with the `service_defs`.
    allocated_psms: HashSet<Psm>,

    /// The allocated RFCOMM server channels for the `service_defs`.
    allocated_server_channels: HashSet<ServerChannel>,
}

impl ServiceGroup {
    pub fn new(
        receiver: bredr::ConnectionReceiverProxy,
        channel_parameters: ChannelParameters,
    ) -> Self {
        Self {
            receiver,
            channel_parameters,
            service_defs: vec![],
            allocated_psms: HashSet::new(),
            allocated_server_channels: HashSet::new(),
        }
    }

    pub fn service_defs(&self) -> &Vec<ServiceDefinition> {
        &self.service_defs
    }

    pub fn channel_parameters(&self) -> &ChannelParameters {
        &self.channel_parameters
    }

    pub fn allocated_psms(&self) -> &HashSet<Psm> {
        &self.allocated_psms
    }

    /// Returns the Server Channels that were allocated to this group of services.
    pub fn allocated_server_channels(&self) -> &HashSet<ServerChannel> {
        &self.allocated_server_channels
    }

    /// Returns true if the `psm` is requested by this service group.s
    pub fn contains_psm(&self, psm: Psm) -> bool {
        self.allocated_psms.contains(&psm)
    }

    /// Relays the connection parameters to the client.
    pub fn relay_connected(
        &self,
        peer_id: PeerId,
        channel: bredr::Channel,
        protocol: Vec<bredr::ProtocolDescriptor>,
    ) -> Result<(), Error> {
        self.receiver.connected(&peer_id.into(), channel, &protocol).map_err(|e| e.into())
    }

    /// Sets the ServiceDefinitions for this group.
    pub fn set_service_defs(&mut self, defs: Vec<ServiceDefinition>) {
        self.allocated_psms = psms_from_service_definitions(&defs);
        self.allocated_server_channels = server_channels_from_service_definitions(&defs);
        self.service_defs = defs;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth as fidl_bt;
    use fidl_fuchsia_bluetooth_bredr::{
        Channel, ProfileDescriptor, ProtocolIdentifier, ServiceClassProfileIdentifier,
    };
    use fuchsia_async as fasync;
    use fuchsia_bluetooth::profile::{Attribute, DataElement, ProtocolDescriptor};
    use fuchsia_bluetooth::types::Uuid;
    use futures::stream::StreamExt;
    use futures::task::Poll;

    /// Defines a Protocol requesting RFCOMM with the provided server `channel`.
    pub fn rfcomm_protocol_descriptor_list(
        channel: Option<ServerChannel>,
    ) -> Vec<ProtocolDescriptor> {
        let params = channel.map(|c| vec![DataElement::Uint8(c.into())]).unwrap_or_default();
        vec![
            ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
            ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Rfcomm, params: params },
        ]
    }

    /// Defines the SPP Service Definition, which requests RFCOMM.
    /// An optional `channel` can be provided to specify the Server Channel.
    pub fn rfcomm_service_definition(channel: Option<ServerChannel>) -> ServiceDefinition {
        ServiceDefinition {
            service_class_uuids: vec![Uuid::new16(0x1101).into()], // SPP UUID
            protocol_descriptor_list: rfcomm_protocol_descriptor_list(channel),
            profile_descriptors: vec![ProfileDescriptor {
                profile_id: Some(ServiceClassProfileIdentifier::SerialPort),
                major_version: Some(1),
                minor_version: Some(2),
                ..Default::default()
            }],
            ..ServiceDefinition::default()
        }
    }

    /// Defines a sample ServiceDefinition with the provided `psm`.
    pub fn other_service_definition(psm: Psm) -> ServiceDefinition {
        ServiceDefinition {
            service_class_uuids: vec![Uuid::new16(0x110A).into()], // A2DP
            protocol_descriptor_list: vec![
                ProtocolDescriptor {
                    protocol: ProtocolIdentifier::L2Cap,
                    params: vec![DataElement::Uint16(psm.into())],
                },
                ProtocolDescriptor {
                    protocol: ProtocolIdentifier::Avdtp,
                    params: vec![DataElement::Uint16(0x0103)], // Indicate v1.3
                },
            ],
            profile_descriptors: vec![ProfileDescriptor {
                profile_id: Some(ServiceClassProfileIdentifier::AdvancedAudioDistribution),
                major_version: Some(1),
                minor_version: Some(2),
                ..Default::default()
            }],
            ..ServiceDefinition::default()
        }
    }

    pub fn obex_service_definition(obex_l2cap_psm: Psm) -> ServiceDefinition {
        ServiceDefinition {
            service_class_uuids: vec![Uuid::new16(0x1133).into()], // MNS
            protocol_descriptor_list: vec![
                ProtocolDescriptor { protocol: ProtocolIdentifier::L2Cap, params: vec![] },
                ProtocolDescriptor { protocol: ProtocolIdentifier::Rfcomm, params: vec![] },
                ProtocolDescriptor { protocol: ProtocolIdentifier::Obex, params: vec![] },
            ],
            additional_attributes: vec![Attribute {
                id: 0x0200,
                element: DataElement::Uint16(obex_l2cap_psm.into()),
            }],
            ..ServiceDefinition::default()
        }
    }

    pub fn l2cap_protocol(psm: Psm) -> Vec<bredr::ProtocolDescriptor> {
        vec![bredr::ProtocolDescriptor {
            protocol: Some(bredr::ProtocolIdentifier::L2Cap),
            params: Some(vec![bredr::DataElement::Uint16(psm.into())]),
            ..Default::default()
        }]
    }

    fn build_service_group() -> (ServiceGroup, bredr::ConnectionReceiverRequestStream) {
        let (client, server) = create_proxy_and_stream::<bredr::ConnectionReceiverMarker>();
        let params = Default::default();

        (ServiceGroup::new(client, params), server)
    }

    #[test]
    fn test_services_collection() {
        let _exec = fasync::TestExecutor::new();
        let mut services = Services::new();

        let mut expected_defs = vec![];
        let mut expected_adv_params =
            AdvertiseParams { services: vec![], parameters: Default::default() };

        // Empty collection of Services has no associated PSMs and shouldn't build
        // into any registration data.
        assert_eq!(services.psms(), HashSet::new());
        assert_eq!(services.build_registration(), None);

        // Insert a new group.
        let (mut group1, _server1) = build_service_group();
        let defs1 = vec![rfcomm_service_definition(None)];
        group1.set_service_defs(defs1.clone());
        let _handle1 = services.insert(group1);

        expected_defs.extend(defs1.clone());
        expected_adv_params.services = expected_defs.clone();
        assert_eq!(services.build_registration(), Some(expected_adv_params.clone()));

        // Build a new ServiceGroup with RFCOMM and non-RFCOMM services and custom
        // SecurityRequirements/ChannelParameters.
        let (mut group2, _server2) = build_service_group();
        let psm = Psm::new(6);
        let sc2 = ServerChannel::try_from(1).ok();
        let defs2 = vec![other_service_definition(psm), rfcomm_service_definition(sc2)];
        group2.set_service_defs(defs2.clone());
        let new_chan_params = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::Basic),
            max_rx_sdu_size: None,
            security_requirements: None,
        };
        group2.channel_parameters = new_chan_params.clone();
        let handle2 = services.insert(group2);

        // We expect the advertisement parameters to include the stricter security
        // requirements, channel parameters, and both handle1 and handle2 ServiceDefinitions.
        let expected_psms = vec![psm].into_iter().collect();
        expected_defs.extend(defs2);
        expected_adv_params.services = expected_defs.clone();
        expected_adv_params.parameters = new_chan_params;

        assert_eq!(services.psms(), expected_psms);
        assert_eq!(services.build_registration(), Some(expected_adv_params.clone()));

        // Removing group2 should result in the new registration parameters to only
        // include group1's parameters.
        let _ = services.remove(handle2);
        expected_adv_params.services = defs1;
        expected_adv_params.parameters = ChannelParameters::default();
        assert_eq!(services.build_registration(), Some(expected_adv_params));
    }

    #[test]
    fn test_services_handle_monotonicity() {
        let _exec = fasync::TestExecutor::new();
        let mut services = Services::new();

        let (group1, _server1) = build_service_group();
        let handle1 = services.insert(group1);
        assert!(services.remove(handle1).is_some());

        // A subsequent insertion must yield a unique handle even if the previous one was freed.
        let (group2, _server2) = build_service_group();
        let handle2 = services.insert(group2);
        assert_ne!(handle1, handle2);

        assert!(services.remove(handle1).is_none());
        assert!(services.remove(handle2).is_some());
    }

    #[test]
    fn test_service_group() {
        let _exec = fasync::TestExecutor::new();

        let (mut service_group, _server) = build_service_group();

        let mut expected_server_channels = HashSet::new();
        let mut expected_psms = HashSet::new();

        assert_eq!(service_group.service_defs(), &vec![]);
        assert_eq!(service_group.allocated_server_channels(), &expected_server_channels);
        assert_eq!(service_group.allocated_psms(), &expected_psms);

        let psm = Psm::new(20);
        let other_def = other_service_definition(psm);
        service_group.set_service_defs(vec![other_def.clone()]);
        let _ = expected_psms.insert(psm);
        assert_eq!(service_group.allocated_server_channels(), &expected_server_channels);
        assert_eq!(service_group.allocated_psms(), &expected_psms);

        let sc = ServerChannel::try_from(10).unwrap();
        let rfcomm_def = rfcomm_service_definition(Some(sc));
        service_group.set_service_defs(vec![rfcomm_def, other_def]);

        let _ = expected_server_channels.insert(sc);
        assert_eq!(service_group.allocated_server_channels(), &expected_server_channels);
        assert_eq!(service_group.allocated_psms(), &expected_psms);
    }

    #[test]
    fn test_service_group_relay_connected() {
        let mut exec = fasync::TestExecutor::new();

        let (mut service_group, mut server) = build_service_group();

        let sc = ServerChannel::try_from(1).ok();
        let defs = vec![other_service_definition(Psm::new(6)), rfcomm_service_definition(sc)];
        service_group.set_service_defs(defs);

        let id = PeerId(123);
        let channel = Channel::default();
        let protocol = vec![];
        let res = service_group.relay_connected(id, channel, protocol);
        assert_matches!(res, Ok(()));

        // We expect the connected request to be relayed to the client.
        let () = match exec.run_until_stalled(&mut server.next()) {
            Poll::Ready(Some(Ok(bredr::ConnectionReceiverRequest::Connected {
                peer_id, ..
            }))) => {
                assert_eq!(peer_id, id.into());
            }
            x => panic!("Expected ready but got: {:?}", x),
        };
    }
}
