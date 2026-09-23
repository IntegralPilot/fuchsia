// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use bt_obex::ObexError;
use fidl_fuchsia_bluetooth_map as fidl_bt_map;
use objects::ObexObjectError;
use packet_encoding::{codable_as_bitmask, decodable_enum};
use std::fmt;
use std::str::FromStr;

pub mod packets;

use thiserror::Error;

// Tag IDs are listed in MAP v1.4.2 Section 6.3.1.
pub const NOTIFICATION_STATUS_TAG_ID: u8 = 0x0E;
pub const MAP_SUPPORTED_FEATURES_TAG_ID: u8 = 0x29;

/// Errors that occur during the use of the MAP library.
#[non_exhaustive]
#[derive(Error, Debug)]
pub enum Error {
    #[error("Obex error: {:?}", .0)]
    Obex(#[from] ObexError),

    #[error("Invalid message type")]
    InvalidMessageType,

    #[error("Service record item is missing or invalid: {:?}", .0)]
    InvalidSdp(ServiceRecordItem),

    #[error("Service is not GOEP interoperable")]
    NotGoepInteroperable,

    #[error("Invalid parameters")]
    InvalidParameters,

    #[error("MAS instance does not exist: (id {:?})", .0)]
    MasInstanceDoesNotExist(u8),

    #[error("Feature is not supported by the remote peer")]
    NotSupported,

    #[error("Invalid Message Notification Service state")]
    InvalidMnsState,

    #[error("Not connected to any Message Access Service instances")]
    MasUnavailable,

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl Error {
    pub fn other(error_msg: impl std::fmt::Debug) -> Self {
        Error::Other(anyhow::format_err!("{error_msg:?}"))
    }
}

impl From<&Error> for fidl_bt_map::Error {
    fn from(value: &Error) -> Self {
        match value {
            &Error::Obex(_) => fidl_bt_map::Error::Unavailable,
            &Error::InvalidMessageType => fidl_bt_map::Error::Unknown,
            &Error::InvalidSdp(_) => fidl_bt_map::Error::Unavailable,
            &Error::NotGoepInteroperable => fidl_bt_map::Error::Unavailable,
            &Error::InvalidParameters => fidl_bt_map::Error::BadRequest,
            &Error::MasInstanceDoesNotExist(_) => fidl_bt_map::Error::NotFound,
            &Error::NotSupported => fidl_bt_map::Error::NotSupported,
            &Error::InvalidMnsState => fidl_bt_map::Error::BadRequest,
            &Error::MasUnavailable => fidl_bt_map::Error::Unavailable,
            &Error::Other(_) => fidl_bt_map::Error::Unknown,
        }
    }
}

impl From<Error> for fidl_bt_map::Error {
    fn from(value: Error) -> Self {
        (&value).into()
    }
}

/// Service record item expected from MAP related SDP.
#[derive(Debug)]
pub enum ServiceRecordItem {
    MasServiceClassId,
    ObexProtocolDescriptor,
    MapProfileDescriptor,
    MasInstanceId,
    SupportedMessageTypes,
    MapSupportedFeatures,
    ServiceName,
}

bitflags! {
    /// See MAP v1.4.2 section 7.1 SDP Interoperability Requirements.
    /// According to MAP v1.4.2 section 6.3.1, the features are
    /// represented in big-endian byte ordering.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct MapSupportedFeatures: u32 {
        const NOTIFICATION_REGISTRATION                     = 0x00000001;
        const NOTIFICATION                                  = 0x00000002;
        const BROWSING                                      = 0x00000004;
        const UPLOADING                                     = 0x00000008;
        const DELETE                                        = 0x00000010;
        const INSTANCE_INFORMATION                          = 0x00000020;
        const EXTENDED_EVENT_REPORT_1_1                     = 0x00000040;
        const EVENT_REPORT_VERSION_1_2                      = 0x00000080;
        const MESSAGE_FORMAT_VERSION_1_1                    = 0x00000100;
        const MESSAGES_LISTING_FORMAT_VERSION_1_1           = 0x00000200;
        const PERSISTENT_MESSAGE_HANDLES                    = 0x00000400;
        const DATABASE_IDENTIFIER                           = 0x00000800;
        const FOLDER_VERSION_COUNTER                        = 0x00001000;
        const CONVERSATION_VERSION_COUNTER                  = 0x00002000;
        const PARTICIPANT_PRESENCE_CHANGE_NOTIFICATION      = 0x00004000;
        const PARTICIPANT_CHAT_STATE_CHANGE_NOTIFICATION    = 0x00008000;
        const PBAP_CONTACT_CROSS_REFERENCE                  = 0x00010000;
        const NOTIFICATION_FILTERING                        = 0x00020000;
        const UTC_OFFSET_TIMESTAMP_FORMAT                   = 0x00040000;
        // Below feature bits are only available for Message Access
        // Services and not for Message Notification Services.
        const MAPSUPPORTEDFEATURES_IN_CONNECT_REQUEST       = 0x00080000;
        const CONVERSATION_LISTING                          = 0x00100000;
        const OWNER_STATUS                                  = 0x00200000;
        const MESSAGE_FORWARDING                            = 0x00400000;

        const SUPPORTS_NOTIFICATION = Self::NOTIFICATION.bits() | Self::NOTIFICATION_REGISTRATION.bits();
    }
}

decodable_enum! {
    pub enum MessageType<u8, Error, InvalidMessageType> {
        Email = 0x1,
        SmsGsm = 0x2,
        SmsCdma = 0x4,
        Mms = 0x8,
        Im = 0x10,
        // Bits 5-7 are RFU.
    }
}

impl MessageType {
    const MESSAGE_TYPE_EMAIL: &'static str = "EMAIL";
    const MESSAGE_TYPE_SMS_GSM: &'static str = "SMS_GSM";
    const MESSAGE_TYPE_SMS_CDMA: &'static str = "SMS_CDMA";
    const MESSAGE_TYPE_MMS: &'static str = "MMS";
    const MESSAGE_TYPE_IM: &'static str = "IM";
}

impl fmt::Display for MessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let val = match self {
            Self::Email => Self::MESSAGE_TYPE_EMAIL,
            Self::SmsGsm => Self::MESSAGE_TYPE_SMS_GSM,
            Self::SmsCdma => Self::MESSAGE_TYPE_SMS_CDMA,
            Self::Mms => Self::MESSAGE_TYPE_MMS,
            Self::Im => Self::MESSAGE_TYPE_IM,
        };
        write!(f, "{}", val)
    }
}

impl FromStr for MessageType {
    type Err = ObexObjectError;
    fn from_str(src: &str) -> Result<Self, Self::Err> {
        match src {
            Self::MESSAGE_TYPE_EMAIL => Ok(Self::Email),
            Self::MESSAGE_TYPE_SMS_GSM => Ok(Self::SmsGsm),
            Self::MESSAGE_TYPE_SMS_CDMA => Ok(Self::SmsCdma),
            Self::MESSAGE_TYPE_MMS => Ok(Self::Mms),
            Self::MESSAGE_TYPE_IM => Ok(Self::Im),
            v => Err(ObexObjectError::invalid_data(v)),
        }
    }
}

impl From<MessageType> for fidl_bt_map::MessageType {
    fn from(value: MessageType) -> Self {
        match value {
            MessageType::Email => fidl_bt_map::MessageType::EMAIL,
            MessageType::SmsGsm => fidl_bt_map::MessageType::SMS_GSM,
            MessageType::SmsCdma => fidl_bt_map::MessageType::SMS_CDMA,
            MessageType::Mms => fidl_bt_map::MessageType::MMS,
            MessageType::Im => fidl_bt_map::MessageType::IM,
        }
    }
}

codable_as_bitmask!(MessageType, u8, Error, InvalidMessageType);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn message_type() {
        let email_and_im = 0x11;

        let types_: HashSet<MessageType> = MessageType::from_bits(email_and_im)
            .collect::<Result<HashSet<_>, _>>()
            .expect("should not fail");

        assert_eq!(2, types_.len());

        let expected = [MessageType::Email, MessageType::Im].into_iter().collect();

        assert_eq!(types_, expected);

        let all = MessageType::VARIANTS;
        let value = MessageType::to_bits(all.iter()).expect("should work");
        assert_eq!(0x1F, value);
    }

    #[test]
    fn map_supported_features() {
        const NOTIFICATION_REG: u32 = 0x1;
        assert_eq!(
            MapSupportedFeatures::from_bits_truncate(NOTIFICATION_REG),
            MapSupportedFeatures::NOTIFICATION_REGISTRATION
        );

        const NOTIFICATION_REG_AND_OWNER_STATUS: u32 = 0x200001;
        let features = MapSupportedFeatures::from_bits_truncate(NOTIFICATION_REG_AND_OWNER_STATUS);
        assert!(features.contains(MapSupportedFeatures::NOTIFICATION_REGISTRATION));
        assert!(features.contains(MapSupportedFeatures::OWNER_STATUS));
        assert_eq!(
            features,
            MapSupportedFeatures::NOTIFICATION_REGISTRATION | MapSupportedFeatures::OWNER_STATUS
        );
    }

    #[test]
    fn supports_notification() {
        // Has notification (bit 0) and registration (bit 1).
        let features = MapSupportedFeatures::from_bits_truncate(0x3);
        assert!(features.contains(MapSupportedFeatures::SUPPORTS_NOTIFICATION));

        let features = MapSupportedFeatures::NOTIFICATION;
        assert!(!features.contains(MapSupportedFeatures::SUPPORTS_NOTIFICATION));

        let features = MapSupportedFeatures::NOTIFICATION_REGISTRATION;
        assert!(!features.contains(MapSupportedFeatures::SUPPORTS_NOTIFICATION));

        let features = MapSupportedFeatures::MESSAGE_FORWARDING;
        assert!(!features.contains(MapSupportedFeatures::SUPPORTS_NOTIFICATION));
    }
}
