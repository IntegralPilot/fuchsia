// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bredr::PSM_RFCOMM;
use bt_map::{Error as MapError, *};
use bt_obex::profile::{
    goep_l2cap_psm_attribute, parse_obex_search_result, GOEP_L2CAP_PSM_ATTRIBUTE,
};
use bt_obex::server::TransportType;
use fuchsia_bluetooth::profile::*;
use fuchsia_bluetooth::types::Uuid;
use log::trace;
use profile_client::ProfileClient;
use std::collections::HashSet;
use {fidl_fuchsia_bluetooth as fidl_bt, fidl_fuchsia_bluetooth_bredr as bredr};

/// Arbitrary PSM value based on the dynamic range described in
/// Bluetooth Core spec v5.3 Vol 3 Part A Section 4.2.
/// This PSM value will be used for MNS connection.
/// All PSM values shall have the least significant bit of the most
/// significant octet equal to 0 and the least significant bit of all
/// other octets equal to 1.
const MNS_PSM: u16 = 0x1033;

const PROFILE_MAJOR_VERSION: u8 = 1;
const PROFILE_MINOR_VERSION: u8 = 4;

const MNS_SERVICE_NAME: &str = "MAP MNS-Fuchsia";

/// SDP Attribute ID for MapSupportedFeatures.
/// https://www.bluetooth.com/specifications/assigned-numbers/service-discovery
const ATTR_MAS_INSTANCE_ID: u16 = 0x0315;
const ATTR_SUPPORTED_MESSAGE_TYPES: u16 = 0x0316;
const ATTR_MAP_SUPPORTED_FEATURES: u16 = 0x0317;

/// Human readable attribute for the service name.
/// According to Core v5.3 vol. 3 part B section 5.1.8,
/// human readable attributes IDs are in the range 0x0100 to 0x01FF
/// and from Assigned Numbers section 5.2, the ID offset for ServiceName is 0x0000.
pub const ATTR_SERVICE_NAME: u16 = 0x100;

const FALLBACK_MAS_SERVICE_NAME: &str = "MAP MAS instance";

/// Represents a single Message Access Server (MAS) instance at a MSE peer.
/// A MSE peer may have one or more MAS instances.
/// MCE can access each MAS Instance by a dedicated OBEX connection.
/// See MAP v1.4.2 section 7.1.1 for deetails.
#[derive(Debug, PartialEq)]
pub struct MasConfig {
    instance_id: u8, // ID that identifies this MAS instance.
    name: String,
    supported_message_types: HashSet<MessageType>,
    connection_params: bredr::ConnectParameters,
    features: MapSupportedFeatures,
}

impl MasConfig {
    pub fn new(
        instance_id: u8,
        name: impl ToString,
        supported_message_types: Vec<MessageType>,
        connection_params: bredr::ConnectParameters,
        features: MapSupportedFeatures,
    ) -> Self {
        Self {
            instance_id,
            name: name.to_string(),
            supported_message_types: HashSet::from_iter(supported_message_types.into_iter()),
            connection_params,
            features,
        }
    }

    pub fn instance_id(&self) -> u8 {
        self.instance_id
    }

    pub fn features(&self) -> MapSupportedFeatures {
        self.features
    }

    pub fn supported_message_types(&self) -> &HashSet<MessageType> {
        &self.supported_message_types
    }

    pub fn connection_params(&self) -> &bredr::ConnectParameters {
        &self.connection_params
    }

    /// Cross checks the profile search result with the service definition for
    /// Message Access Service as listed in MAP v1.4.2 section 7.1.1.
    pub fn from_search_result(
        protocol: Vec<bredr::ProtocolDescriptor>,
        attributes: Vec<bredr::Attribute>,
    ) -> Result<Self, MapError> {
        // Ensure MAS service is advertised.
        let service_ids = find_service_classes(&attributes);
        if service_ids
            .iter()
            .find(|id| {
                id.number
                    == bredr::ServiceClassProfileIdentifier::MessageAccessServer.into_primitive()
            })
            .is_none()
        {
            return Err(MapError::InvalidSdp(ServiceRecordItem::MasServiceClassId));
        }

        // Ensure MAP profile is advertised.
        let profile_desc = find_profile_descriptors(&attributes)
            .map_err(|_| MapError::InvalidSdp(ServiceRecordItem::MapProfileDescriptor))?;
        if profile_desc
            .iter()
            .find(|desc| {
                desc.profile_id == Some(bredr::ServiceClassProfileIdentifier::MessageAccessProfile)
            })
            .is_none()
        {
            return Err(MapError::InvalidSdp(ServiceRecordItem::MapProfileDescriptor));
        }

        let protocol = protocol
            .iter()
            .map(|p| ProtocolDescriptor::try_from(p))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MapError::InvalidParameters)?;

        let attributes = attributes
            .iter()
            .map(|a| Attribute::try_from(a))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| MapError::InvalidParameters)?;

        // Ensure either L2CAP or RFCOMM connection is supported.
        let connection = parse_obex_search_result(&protocol, &attributes)
            .ok_or(MapError::NotGoepInteroperable)?;

        // Get information about this MAS instance.
        let id = attributes
            .iter()
            .find_map(|a| {
                if a.id != ATTR_MAS_INSTANCE_ID {
                    return None;
                }
                let DataElement::Uint8(raw_val) = a.element else {
                    return None;
                };
                Some(raw_val)
            })
            .ok_or(MapError::InvalidSdp(ServiceRecordItem::MasInstanceId))?;

        let name = attributes
            .iter()
            .find_map(|a| {
                // TODO(https://fxbug.dev/328074442): once getting languaged-based
                // attributes is supported, get the attributes through
                // that instead.
                if a.id != ATTR_SERVICE_NAME {
                    return None;
                }
                match &a.element {
                    DataElement::Str(bytes) => String::from_utf8(bytes.to_vec())
                        .ok()
                        .or_else(|| Some(FALLBACK_MAS_SERVICE_NAME.to_string())),
                    _ => Some(FALLBACK_MAS_SERVICE_NAME.to_string()),
                }
            })
            .ok_or(MapError::InvalidSdp(ServiceRecordItem::ServiceName))?;

        // Get supported times
        let supported_message_types = attributes
            .iter()
            .find_map(|a| {
                if a.id != ATTR_SUPPORTED_MESSAGE_TYPES {
                    return None;
                }
                let DataElement::Uint8(raw_val) = a.element else {
                    return None;
                };
                let supported: HashSet<MessageType> =
                    MessageType::from_bits(raw_val).filter_map(|r| r.ok()).collect();
                Some(supported)
            })
            .ok_or(MapError::InvalidSdp(ServiceRecordItem::SupportedMessageTypes))?;

        // We intersect the features supported by Sapphire and the features supported by
        // the peer device.
        let features = attributes
            .iter()
            .find_map(|a| {
                if a.id != ATTR_MAP_SUPPORTED_FEATURES {
                    return None;
                }
                let DataElement::Uint32(raw_val) = a.element else {
                    return None;
                };
                Some(MapSupportedFeatures::from_bits_truncate(raw_val))
            })
            .ok_or(MapError::InvalidSdp(ServiceRecordItem::MapSupportedFeatures))?;

        let config = MasConfig {
            instance_id: id,
            name,
            supported_message_types,
            connection_params: connection,
            features,
        };
        Ok(config)
    }
}

/// Returns the transport type from the list of protocol descriptors.
pub(crate) fn transport_type_from_protocol(
    descriptors: &Vec<ProtocolDescriptor>,
) -> Result<TransportType, MapError> {
    match psm_from_protocol(&descriptors) {
        Some(psm) if u16::from(psm) == PSM_RFCOMM => Ok(TransportType::Rfcomm),
        Some(_) => Ok(TransportType::L2cap),
        None => Err(MapError::other(format!(
            "can't determine TransportType from protocols {descriptors:?}"
        ))),
    }
}

/// Features supported by the MNS server side implementation of the MCE
/// role.
pub(crate) fn mns_supported_features() -> MapSupportedFeatures {
    MapSupportedFeatures::NOTIFICATION_REGISTRATION
        | MapSupportedFeatures::NOTIFICATION
        | MapSupportedFeatures::EXTENDED_EVENT_REPORT_1_1
        | MapSupportedFeatures::MESSAGES_LISTING_FORMAT_VERSION_1_1
}

/// Service definition for Message Notification Service on the MCE device.
/// See MAP v.1.4.2, Section 7.1.2 for details.
fn build_mns_service_definition() -> ServiceDefinition {
    ServiceDefinition {
        service_class_uuids: vec![Uuid::new16(
            bredr::ServiceClassProfileIdentifier::MessageNotificationServer.into_primitive(),
        )
        .into()],
        protocol_descriptor_list: vec![
            ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::L2Cap, params: vec![] },
            ProtocolDescriptor {
                protocol: bredr::ProtocolIdentifier::Rfcomm,
                // Note: RFCOMM channel number is assigned by bt-rfcomm so we don't include it in
                // the parameter.
                params: vec![],
            },
            ProtocolDescriptor { protocol: bredr::ProtocolIdentifier::Obex, params: vec![] },
        ],
        information: vec![Information {
            language: "en".to_string(),
            // TODO(https://fxbug.dev/328115144): consider making this configurable
            // through structured configs.
            name: Some(MNS_SERVICE_NAME.to_string()),
            description: None,
            provider: None,
        }],
        profile_descriptors: vec![bredr::ProfileDescriptor {
            profile_id: Some(bredr::ServiceClassProfileIdentifier::MessageAccessProfile),
            major_version: Some(PROFILE_MAJOR_VERSION),
            minor_version: Some(PROFILE_MINOR_VERSION),
            ..Default::default()
        }],
        additional_attributes: vec![
            // TODO(https://fxbug.dev/352169266): request a dynamic PSM to be assigned by
            // bt-host once it's implemented.
            goep_l2cap_psm_attribute(Psm::new(MNS_PSM)),
            Attribute {
                id: ATTR_MAP_SUPPORTED_FEATURES,
                element: DataElement::Uint32(mns_supported_features().bits()),
            },
        ],
        ..Default::default()
    }
}

pub fn connect_and_advertise(profile_svc: bredr::ProfileProxy) -> Result<ProfileClient, MapError> {
    // Attributes to search for in SDP record for the Message Access Service on a MSE device.
    let search_attributes = vec![
        bredr::ATTR_SERVICE_CLASS_ID_LIST,
        bredr::ATTR_PROTOCOL_DESCRIPTOR_LIST,
        ATTR_SERVICE_NAME,
        bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST,
        GOEP_L2CAP_PSM_ATTRIBUTE,
        ATTR_MAS_INSTANCE_ID,
        ATTR_SUPPORTED_MESSAGE_TYPES,
        ATTR_MAP_SUPPORTED_FEATURES,
    ];

    let service_defs =
        vec![(&build_mns_service_definition()).try_into().map_err(MapError::other)?];
    let channel_parameters = fidl_bt::ChannelParameters {
        channel_mode: Some(fidl_bt::ChannelMode::EnhancedRetransmission),
        ..Default::default()
    };

    // MCE device advertises the MNS on it and and searches for MAS on remote peers.
    let mut profile_client =
        ProfileClient::advertise(profile_svc, service_defs, channel_parameters)
            .map_err(MapError::other)?;

    profile_client
        .add_search(
            bredr::ServiceClassProfileIdentifier::MessageAccessServer,
            Some(search_attributes),
        )
        .map_err(MapError::other)?;

    trace!("Registered service search & advertisement");
    Ok(profile_client)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns protocols with RFCOMM and OBEX protocols for testing purposes.
    fn test_protocols() -> Vec<bredr::ProtocolDescriptor> {
        vec![
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Rfcomm),
                params: Some(vec![bredr::DataElement::Uint8(8)]),
                ..Default::default()
            },
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Obex),
                params: Some(vec![]),
                ..Default::default()
            },
        ]
    }

    /// Returns attributes for testing purposes. Contains all the attributes for a MAS service record
    /// except for the GoepL2CapPsm attribute.
    fn test_attributes() -> Vec<bredr::Attribute> {
        vec![
            bredr::Attribute {
                id: Some(bredr::ATTR_SERVICE_CLASS_ID_LIST),
                element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                    bredr::DataElement::Uuid(
                        Uuid::new16(
                            bredr::ServiceClassProfileIdentifier::MessageAccessServer
                                .into_primitive(),
                        )
                        .into(),
                    ),
                ))])),
                ..Default::default()
            },
            bredr::Attribute {
                id: Some(bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
                element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                    bredr::DataElement::Sequence(vec![
                        Some(Box::new(bredr::DataElement::Uuid(
                            Uuid::new16(
                                bredr::ServiceClassProfileIdentifier::MessageAccessProfile
                                    .into_primitive(),
                            )
                            .into(),
                        ))),
                        Some(Box::new(bredr::DataElement::Uint16(0x0104))),
                    ]),
                ))])),
                ..Default::default()
            },
            bredr::Attribute {
                id: Some(ATTR_SERVICE_NAME),
                element: Some(bredr::DataElement::Str(vec![0x68, 0x69])),
                ..Default::default()
            },
            bredr::Attribute {
                id: Some(ATTR_MAS_INSTANCE_ID),
                element: Some(bredr::DataElement::Uint8(1)),
                ..Default::default()
            },
            bredr::Attribute {
                id: Some(ATTR_SUPPORTED_MESSAGE_TYPES),
                element: Some(bredr::DataElement::Uint8(0x05)), // email and sms cdma.
                ..Default::default()
            },
            bredr::Attribute {
                id: Some(ATTR_MAP_SUPPORTED_FEATURES),
                element: Some(bredr::DataElement::Uint32(0x00080007)),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn mas_config_from_search_result_rfcomm() {
        let config = MasConfig::from_search_result(test_protocols(), test_attributes())
            .expect("should succeed");

        match config.connection_params {
            bredr::ConnectParameters::L2cap(_) => panic!("should not be L2cap"),
            bredr::ConnectParameters::Rfcomm(chan) => assert_eq!(chan.channel, Some(8)),
        };

        assert!(config.features.contains(MapSupportedFeatures::NOTIFICATION_REGISTRATION));
        assert!(config.features.contains(MapSupportedFeatures::NOTIFICATION));
        assert!(config.features.contains(MapSupportedFeatures::BROWSING));
        assert!(config
            .features
            .contains(MapSupportedFeatures::MAPSUPPORTEDFEATURES_IN_CONNECT_REQUEST));

        assert_eq!(config.instance_id, 1);
        assert_eq!(config.name, "hi".to_string());

        assert_eq!(
            config.supported_message_types,
            HashSet::from([MessageType::Email, MessageType::SmsCdma])
        )
    }

    #[test]
    fn mas_config_from_search_result_l2cap() {
        let mut protocols = test_protocols();

        // Add L2CAP protocol.
        protocols.push(bredr::ProtocolDescriptor {
            protocol: Some(bredr::ProtocolIdentifier::L2Cap),
            params: Some(vec![]),
            ..Default::default()
        });

        let mut attributes = test_attributes();

        // Set service name attribute to an invalid value to test fall back name.
        for a in attributes.iter_mut() {
            if a.id == Some(ATTR_SERVICE_NAME) {
                a.element = Some(bredr::DataElement::Uint8(0x00));
            }
        }

        // Add GOEP L2CAP Psm attribute to test GOEP interoperability.
        attributes.push(bredr::Attribute {
            id: Some(GOEP_L2CAP_PSM_ATTRIBUTE),
            element: Some(bredr::DataElement::Uint16(0x1007)),
            ..Default::default()
        });

        let config = MasConfig::from_search_result(protocols, attributes).expect("should succeed");

        match config.connection_params {
            bredr::ConnectParameters::L2cap(chan) => assert_eq!(chan.psm, Some(0x1007)),
            bredr::ConnectParameters::Rfcomm(_) => panic!("should not be Rfcomm"),
        };
        assert_eq!(config.name, FALLBACK_MAS_SERVICE_NAME.to_string());
    }

    #[test]
    fn mas_config_from_search_result_rfu_message_types() {
        let protocols = test_protocols();

        let mut attributes = test_attributes();

        // Set RFU bits in supported message types attribute.
        for a in attributes.iter_mut() {
            if a.id == Some(ATTR_SUPPORTED_MESSAGE_TYPES) {
                a.element = Some(bredr::DataElement::Uint8(0b10000101)); // email and sms cdma and bit 7.
            }
        }

        let config = MasConfig::from_search_result(protocols, attributes).expect("should succeed");

        assert_eq!(
            config.supported_message_types,
            HashSet::from([MessageType::Email, MessageType::SmsCdma])
        );
    }

    #[test]
    fn mas_config_from_search_result_fail() {
        // Missing obex.
        let _ = MasConfig::from_search_result(
            vec![bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Rfcomm),
                params: Some(vec![bredr::DataElement::Uint8(8)]),
                ..Default::default()
            }],
            vec![
                bredr::Attribute {
                    id: Some(bredr::ATTR_SERVICE_CLASS_ID_LIST),
                    element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                        bredr::DataElement::Uuid(
                            Uuid::new16(
                                bredr::ServiceClassProfileIdentifier::MessageAccessServer
                                    .into_primitive(),
                            )
                            .into(),
                        ),
                    ))])),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
                    element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                        bredr::DataElement::Sequence(vec![
                            Some(Box::new(bredr::DataElement::Uuid(
                                Uuid::new16(
                                    bredr::ServiceClassProfileIdentifier::MessageAccessProfile
                                        .into_primitive(),
                                )
                                .into(),
                            ))),
                            Some(Box::new(bredr::DataElement::Uint16(0x0104))),
                        ]),
                    ))])),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_SERVICE_NAME),
                    element: Some(bredr::DataElement::Str(vec![0x68, 0x69])),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_MAS_INSTANCE_ID),
                    element: Some(bredr::DataElement::Uint8(1)),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_SUPPORTED_MESSAGE_TYPES),
                    element: Some(bredr::DataElement::Uint8(0x05)), // email and sms cdma.
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_MAP_SUPPORTED_FEATURES),
                    element: Some(bredr::DataElement::Uint32(0x00080007)),
                    ..Default::default()
                },
            ],
        )
        .expect_err("should fail");

        // Missing MAS instance ID.
        let _ = MasConfig::from_search_result(
            vec![
                bredr::ProtocolDescriptor {
                    protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                    params: Some(vec![]),
                    ..Default::default()
                },
                bredr::ProtocolDescriptor {
                    protocol: Some(bredr::ProtocolIdentifier::Rfcomm),
                    params: Some(vec![bredr::DataElement::Uint8(8)]),
                    ..Default::default()
                },
                bredr::ProtocolDescriptor {
                    protocol: Some(bredr::ProtocolIdentifier::Obex),
                    params: Some(vec![]),
                    ..Default::default()
                },
            ],
            vec![
                bredr::Attribute {
                    id: Some(bredr::ATTR_SERVICE_CLASS_ID_LIST),
                    element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                        bredr::DataElement::Uuid(
                            Uuid::new16(
                                bredr::ServiceClassProfileIdentifier::MessageAccessServer
                                    .into_primitive(),
                            )
                            .into(),
                        ),
                    ))])),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
                    element: Some(bredr::DataElement::Sequence(vec![Some(Box::new(
                        bredr::DataElement::Sequence(vec![
                            Some(Box::new(bredr::DataElement::Uuid(
                                Uuid::new16(
                                    bredr::ServiceClassProfileIdentifier::MessageAccessProfile
                                        .into_primitive(),
                                )
                                .into(),
                            ))),
                            Some(Box::new(bredr::DataElement::Uint16(0x0104))),
                        ]),
                    ))])),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(GOEP_L2CAP_PSM_ATTRIBUTE),
                    element: Some(bredr::DataElement::Uint16(0x1007)),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_SERVICE_NAME),
                    element: Some(bredr::DataElement::Uint8(0x00)),
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_SUPPORTED_MESSAGE_TYPES),
                    element: Some(bredr::DataElement::Uint8(0x05)), // email and sms cdma.
                    ..Default::default()
                },
                bredr::Attribute {
                    id: Some(ATTR_MAP_SUPPORTED_FEATURES),
                    element: Some(bredr::DataElement::Uint32(0x00080007)),
                    ..Default::default()
                },
            ],
        )
        .expect_err("should fail");
    }
}
