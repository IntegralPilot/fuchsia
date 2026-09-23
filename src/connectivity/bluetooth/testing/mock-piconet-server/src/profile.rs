// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use bt_rfcomm::profile::{is_rfcomm_protocol, server_channel_from_protocol};
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::profile::*;
use std::collections::HashSet;

use crate::types::ServiceRecord;

/// Builds the L2Cap Protocol Descriptor from the provided `psm`.
pub fn build_l2cap_descriptor(psm: Psm) -> Vec<bredr::ProtocolDescriptor> {
    vec![bredr::ProtocolDescriptor {
        protocol: Some(bredr::ProtocolIdentifier::L2Cap),
        params: Some(vec![bredr::DataElement::Uint16(psm.into())]),
        ..Default::default()
    }]
}

fn parse_service_definition(
    def: &bredr::ServiceDefinition,
) -> Result<ServiceRecord, anyhow::Error> {
    let definition: ServiceDefinition = def.try_into()?;
    // Parse the service class UUIDs into ServiceClassProfileIdentifiers.
    let svc_ids = {
        let uuids_vec = definition
            .service_class_uuids
            .iter()
            .map(|uuid| bredr::ServiceClassProfileIdentifier::try_from(uuid.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        HashSet::from_iter(uuids_vec)
    };
    if svc_ids.is_empty() {
        return Err(format_err!("There must be at least one service class UUID"));
    };

    // The primary protocol may be empty and not specify a PSM. However, in the case of RFCOMM, it
    // must be fully populated.
    let primary_protocol = definition.protocol_descriptor_list.clone();
    if is_rfcomm_protocol(&primary_protocol) {
        let _channel_number = server_channel_from_protocol(&primary_protocol)
            .ok_or_else(|| format_err!("Invalid RFCOMM descriptor"))?;
    }

    // Convert (potential) additional PSMs into local Psm type.
    let additional_psms = definition.additional_psms();

    Ok(ServiceRecord::new(
        svc_ids,
        primary_protocol,
        additional_psms,
        definition.profile_descriptors,
        definition.additional_attributes,
    ))
}

pub fn parse_service_definitions(
    svc_defs: Vec<bredr::ServiceDefinition>,
) -> Result<Vec<ServiceRecord>, anyhow::Error> {
    let mut parsed_svc_defs = vec![];

    for def in &svc_defs {
        let parsed_def = parse_service_definition(def)?;
        parsed_svc_defs.push(parsed_def);
    }

    Ok(parsed_svc_defs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bt_rfcomm::ServerChannel;
    use fuchsia_bluetooth::types::Uuid;

    const SDP_SUPPORTED_FEATURES: u16 = 0x0311;

    /// Builds the smallest service definition that is valid - only the Service Class IDs.
    /// Returns the definition and the expected parsed ServiceRecord.
    fn minimal_service_definition() -> (bredr::ServiceDefinition, ServiceRecord) {
        let def = bredr::ServiceDefinition {
            service_class_uuids: Some(vec![
                Uuid::new16(bredr::ServiceClassProfileIdentifier::Headset.into_primitive()).into(),
                Uuid::new16(bredr::ServiceClassProfileIdentifier::Handsfree.into_primitive())
                    .into(),
            ]),
            ..Default::default()
        };

        let ids = vec![
            bredr::ServiceClassProfileIdentifier::Headset,
            bredr::ServiceClassProfileIdentifier::Handsfree,
        ]
        .into_iter()
        .collect();
        let record = ServiceRecord::new(ids, vec![], HashSet::new(), vec![], vec![]);

        (def, record)
    }

    /// Builds an A2DP Sink Service Definition and the expected parsed ServiceRecord.
    /// Uses the provided `psm` for the service.
    pub(crate) fn a2dp_service_definition(psm: Psm) -> (bredr::ServiceDefinition, ServiceRecord) {
        let prof_descs = vec![bredr::ProfileDescriptor {
            profile_id: Some(bredr::ServiceClassProfileIdentifier::AdvancedAudioDistribution),
            major_version: Some(1),
            minor_version: Some(2),
            ..Default::default()
        }];
        let protocol_descriptor_list = vec![
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![bredr::DataElement::Uint16(psm.into())]),
                ..Default::default()
            },
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Avdtp),
                params: Some(vec![bredr::DataElement::Uint16(0x0103)]), // Indicate v1.3
                ..Default::default()
            },
        ];
        let def = bredr::ServiceDefinition {
            service_class_uuids: Some(vec![Uuid::new16(0x110B).into()]), // Audio Sink UUID
            protocol_descriptor_list: Some(protocol_descriptor_list.clone()),
            profile_descriptors: Some(prof_descs.clone()),
            additional_attributes: Some(vec![]),
            ..Default::default()
        };

        let a2dp_ids = vec![bredr::ServiceClassProfileIdentifier::AudioSink].into_iter().collect();
        let primary_protocol = protocol_descriptor_list
            .iter()
            .map(|p| ProtocolDescriptor::try_from(p).unwrap())
            .collect();
        let record =
            ServiceRecord::new(a2dp_ids, primary_protocol, HashSet::new(), prof_descs, vec![]);

        (def, record)
    }

    /// Builds an example AVRCP Service Definition and the expected parsed ServiceRecord.
    /// This is done in the same function to maintain data consistency, such that if the
    /// parsing implementation changes, only this builder will need to be updated.
    fn avrcp_service_definition() -> (bredr::ServiceDefinition, ServiceRecord) {
        use bredr::ServiceClassProfileIdentifier::{AvRemoteControl, AvRemoteControlController};
        let prof_descs = vec![bredr::ProfileDescriptor {
            profile_id: Some(bredr::ServiceClassProfileIdentifier::AvRemoteControl),
            major_version: Some(1),
            minor_version: Some(6),
            ..Default::default()
        }];
        let avrcp_attribute = Attribute {
            id: SDP_SUPPORTED_FEATURES, // SDP Attribute "SUPPORTED FEATURES"
            element: DataElement::Uint16(1),
        };

        let avrcp_ids = vec![
            bredr::ServiceClassProfileIdentifier::AvRemoteControl,
            bredr::ServiceClassProfileIdentifier::AvRemoteControlController,
        ]
        .into_iter()
        .collect();
        let additional_psms = vec![Psm::AVCTP_BROWSE].into_iter().collect();
        let protocol_descriptor_list = vec![
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![bredr::DataElement::Uint16(bredr::PSM_AVCTP as u16)]),
                ..Default::default()
            },
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Avctp),
                params: Some(vec![bredr::DataElement::Uint16(0x0103)]), // Indicate v1.3
                ..Default::default()
            },
        ];
        let def = bredr::ServiceDefinition {
            service_class_uuids: Some(vec![
                Uuid::new16(AvRemoteControl.into_primitive()).into(),
                Uuid::new16(AvRemoteControlController.into_primitive()).into(),
            ]),
            protocol_descriptor_list: Some(protocol_descriptor_list.clone()),
            additional_protocol_descriptor_lists: Some(vec![vec![
                bredr::ProtocolDescriptor {
                    protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                    params: Some(vec![bredr::DataElement::Uint16(bredr::PSM_AVCTP_BROWSE as u16)]),
                    ..Default::default()
                },
                bredr::ProtocolDescriptor {
                    protocol: Some(bredr::ProtocolIdentifier::Avctp),
                    params: Some(vec![bredr::DataElement::Uint16(0x0103)]),
                    ..Default::default()
                },
            ]]),
            profile_descriptors: Some(prof_descs.clone()),
            additional_attributes: Some(vec![(&avrcp_attribute).into()]),
            ..Default::default()
        };
        let primary_protocol = protocol_descriptor_list
            .iter()
            .map(|p| ProtocolDescriptor::try_from(p).unwrap())
            .collect();
        let record = ServiceRecord::new(
            avrcp_ids,
            primary_protocol,
            additional_psms,
            prof_descs,
            vec![avrcp_attribute],
        );

        (def, record)
    }

    #[test]
    fn parse_l2cap_service_definitions_success() {
        // Empty is ok.
        let empty = vec![];
        let parsed = parse_service_definitions(empty);
        assert_eq!(Ok(vec![]), parsed.map_err(|e| format!("{:?}", e)));

        // Bare minimum case. Only the ServiceClassProfileIdentifiers are provided in the service.
        let (id_only_def, expected_id_only_record) = minimal_service_definition();
        let parsed = parse_service_definitions(vec![id_only_def]);
        assert_eq!(Ok(vec![expected_id_only_record]), parsed.map_err(|e| format!("{:?}", e)));

        // Normal case, multiple services.
        let (a2dp_def, expected_a2dp_record) = a2dp_service_definition(Psm::new(25));
        let (avrcp_def, expected_avrcp_record) = avrcp_service_definition();
        let service_defs = vec![a2dp_def, avrcp_def];
        let parsed = parse_service_definitions(service_defs);
        assert_eq!(
            Ok(vec![expected_a2dp_record, expected_avrcp_record]),
            parsed.map_err(|e| format!("{:?}", e))
        );
    }

    /// Builds an example RFCOMM-requesting SPP service and returns the expected parsed
    /// ServiceRecord.
    pub(crate) fn rfcomm_service_definition(
        rfcomm_channel: ServerChannel,
    ) -> (bredr::ServiceDefinition, ServiceRecord) {
        let prof_descs = vec![bredr::ProfileDescriptor {
            profile_id: Some(bredr::ServiceClassProfileIdentifier::SerialPort),
            major_version: Some(1),
            minor_version: Some(2),
            ..Default::default()
        }];
        let protocol_descriptor_list = vec![
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![]), // For RFCOMM services), the PSM is omitted.
                ..Default::default()
            },
            bredr::ProtocolDescriptor {
                protocol: Some(bredr::ProtocolIdentifier::Rfcomm),
                params: Some(vec![bredr::DataElement::Uint8(rfcomm_channel.into())]),
                ..Default::default()
            },
        ];
        let def = bredr::ServiceDefinition {
            service_class_uuids: Some(vec![Uuid::new16(
                bredr::ServiceClassProfileIdentifier::SerialPort.into_primitive(),
            )
            .into()]),
            protocol_descriptor_list: Some(protocol_descriptor_list.clone()),
            profile_descriptors: Some(prof_descs.clone()),
            ..Default::default()
        };
        let spp_ids = vec![bredr::ServiceClassProfileIdentifier::SerialPort].into_iter().collect();
        let primary_protocol = protocol_descriptor_list
            .iter()
            .map(|p| ProtocolDescriptor::try_from(p).unwrap())
            .collect();
        let record =
            ServiceRecord::new(spp_ids, primary_protocol, HashSet::new(), prof_descs, vec![]);
        (def, record)
    }

    #[test]
    fn parse_rfcomm_service_definitions_success() {
        let (spp_def, expected_record) =
            rfcomm_service_definition(ServerChannel::try_from(3).expect("valid"));
        let parsed = parse_service_definitions(vec![spp_def]);
        assert_eq!(Ok(vec![expected_record]), parsed.map_err(|e| format!("{:?}", e)));
    }
}
