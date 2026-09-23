// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_bluetooth::{ChannelMode, ChannelParameters};
use fidl_fuchsia_bluetooth_bredr::*;
use fuchsia_bluetooth::profile::avrcp::*;
use fuchsia_bluetooth::types::Uuid;
use log::info;
use profile_client::ProfileClient;

/// Set of features supported when we are the CT role.
/// See AVRCP v1.6.2 Section 8 Table 8.1.
pub(crate) const CONTROLLER_SUPPORTED_FEATURES: AvrcpControllerFeatures =
    AvrcpControllerFeatures::from_bits_truncate(
        AvrcpControllerFeatures::CATEGORY1.bits()
            | AvrcpControllerFeatures::CATEGORY2.bits()
            | AvrcpControllerFeatures::SUPPORTSBROWSING.bits(),
    );

/// Set of features supported when we are the TG role.
/// See AVRCP v1.6.2 Section 8 Table 8.2.
pub(crate) const TARGET_SUPPORTED_FEATURES: AvrcpTargetFeatures =
    AvrcpTargetFeatures::from_bits_truncate(
        AvrcpTargetFeatures::CATEGORY1.bits()
            | AvrcpTargetFeatures::CATEGORY2.bits()
            | AvrcpTargetFeatures::SUPPORTSBROWSING.bits(),
    );

/// The common service definition for AVRCP Target and Controller.
/// AVRCP 1.6, Section 8.
fn build_common_service_definition() -> ServiceDefinition {
    ServiceDefinition {
        protocol_descriptor_list: Some(vec![
            ProtocolDescriptor {
                protocol: Some(ProtocolIdentifier::L2Cap),
                params: Some(vec![DataElement::Uint16(PSM_AVCTP as u16)]),
                ..Default::default()
            },
            ProtocolDescriptor {
                protocol: Some(ProtocolIdentifier::Avctp),
                // Indicate AVCTP v1.4.
                params: Some(vec![DataElement::Uint16(0x0104)]),
                ..Default::default()
            },
        ]),
        profile_descriptors: Some(vec![ProfileDescriptor {
            profile_id: Some(ServiceClassProfileIdentifier::AvRemoteControl),
            major_version: Some(1),
            minor_version: Some(6),
            ..Default::default()
        }]),
        ..Default::default()
    }
}

/// Make the SDP definition for the AVRCP Controller service.
/// AVRCP 1.6, Section 8, Table 8.1.
fn make_controller_service_definition() -> ServiceDefinition {
    let mut service = build_common_service_definition();

    let service_class_uuids: Vec<fidl_fuchsia_bluetooth::Uuid> =
        vec![Uuid::new16(AV_REMOTE_CLASS).into(), Uuid::new16(AV_REMOTE_CONTROLLER_CLASS).into()];
    service.service_class_uuids = Some(service_class_uuids);

    service.additional_attributes = Some(vec![Attribute {
        id: Some(SDP_SUPPORTED_FEATURES), // SDP Attribute "SUPPORTED FEATURES"
        element: Some(DataElement::Uint16(CONTROLLER_SUPPORTED_FEATURES.bits())),
        ..Default::default()
    }]);

    service.additional_protocol_descriptor_lists = Some(vec![vec![
        ProtocolDescriptor {
            protocol: Some(ProtocolIdentifier::L2Cap),
            params: Some(vec![DataElement::Uint16(PSM_AVCTP_BROWSE as u16)]),
            ..Default::default()
        },
        ProtocolDescriptor {
            protocol: Some(ProtocolIdentifier::Avctp),
            // Indicates AVCTP v1.4.
            params: Some(vec![DataElement::Uint16(0x0104)]),
            ..Default::default()
        },
    ]]);

    service
}

/// Make the SDP definition for the AVRCP Target service.
/// AVRCP 1.6, Section 8, Table 8.2.
fn make_target_service_definition() -> ServiceDefinition {
    let mut service = build_common_service_definition();

    let service_class_uuids: Vec<fidl_fuchsia_bluetooth::Uuid> =
        vec![Uuid::new16(AV_REMOTE_TARGET_CLASS).into()];
    service.service_class_uuids = Some(service_class_uuids);

    service.additional_attributes = Some(vec![Attribute {
        id: Some(SDP_SUPPORTED_FEATURES), // SDP Attribute "SUPPORTED FEATURES"
        element: Some(DataElement::Uint16(TARGET_SUPPORTED_FEATURES.bits())),
        ..Default::default()
    }]);

    service.additional_protocol_descriptor_lists = Some(vec![vec![
        ProtocolDescriptor {
            protocol: Some(ProtocolIdentifier::L2Cap),
            params: Some(vec![DataElement::Uint16(PSM_AVCTP_BROWSE as u16)]),
            ..Default::default()
        },
        ProtocolDescriptor {
            protocol: Some(ProtocolIdentifier::Avctp),
            // Indicates AVCTP v1.4.
            params: Some(vec![DataElement::Uint16(0x0104)]),
            ..Default::default()
        },
    ]]);

    service
}

pub fn connect_and_advertise() -> Result<(ProfileProxy, ProfileClient), Error> {
    let profile_svc = fuchsia_component::client::connect_to_protocol::<ProfileMarker>()
        .context("Failed to connect to Bluetooth profile service")?;

    let search_attributes = vec![
        ATTR_SERVICE_CLASS_ID_LIST,
        ATTR_PROTOCOL_DESCRIPTOR_LIST,
        ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST,
        ATTR_ADDITIONAL_PROTOCOL_DESCRIPTOR_LIST,
        SDP_SUPPORTED_FEATURES,
    ];

    let service_defs = vec![make_controller_service_definition(), make_target_service_definition()];
    let channel_parameters = ChannelParameters {
        channel_mode: Some(ChannelMode::EnhancedRetransmission),
        ..Default::default()
    };
    let mut profile_client =
        ProfileClient::advertise(profile_svc.clone(), service_defs, channel_parameters)?;

    profile_client
        .add_search(ServiceClassProfileIdentifier::AvRemoteControl, Some(search_attributes))?;

    info!("Registered service search & advertisement");

    Ok((profile_svc, profile_client))
}
