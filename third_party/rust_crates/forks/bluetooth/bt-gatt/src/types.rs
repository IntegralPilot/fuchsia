// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::{PeerId, Uuid};
use thiserror::Error;

/// Errors that can be returned from GATT procedures. These errors are sent from
/// the peer. These are defined to match the Bluetooth Core Spec (v5.4, Vol 3,
/// Part F, Sec 3.4.1.1)
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum GattError {
    InvalidHandle = 1,
    ReadNotPermitted = 2,
    WriteNotPermitted = 3,
    InvalidPdu = 4,
    InsufficientAuthentication = 5,
    RequestNotSupported = 6,
    InvalidOffset = 7,
    InsufficientAuthorization = 8,
    InsufficientEncryptionKeySize = 12,
    InvalidAttributeValueLength = 13,
    UnlikelyError = 14,
    InsufficientEncryption = 15,
    InsufficientResources = 17,
    ValueNotAllowed = 19,
    ApplicationError80 = 128,
    ApplicationError81 = 129,
    ApplicationError82 = 130,
    ApplicationError83 = 131,
    ApplicationError84 = 132,
    ApplicationError85 = 133,
    ApplicationError86 = 134,
    ApplicationError87 = 135,
    ApplicationError88 = 136,
    ApplicationError89 = 137,
    ApplicationError8A = 138,
    ApplicationError8B = 139,
    ApplicationError8C = 140,
    ApplicationError8D = 141,
    ApplicationError8E = 142,
    ApplicationError8F = 143,
    ApplicationError90 = 144,
    ApplicationError91 = 145,
    ApplicationError92 = 146,
    ApplicationError93 = 147,
    ApplicationError94 = 148,
    ApplicationError95 = 149,
    ApplicationError96 = 150,
    ApplicationError97 = 151,
    ApplicationError98 = 152,
    ApplicationError99 = 153,
    ApplicationError9A = 154,
    ApplicationError9B = 155,
    ApplicationError9C = 156,
    ApplicationError9D = 157,
    ApplicationError9E = 158,
    ApplicationError9F = 159,
    WriteRequestRejected = 252,
    CccDescriptorImproperlyConfigured = 253,
    ProcedureAlreadyInProgress = 254,
    OutOfRange = 255,
    InvalidParameters = 257,
    TooManyResults = 258,
}

impl std::fmt::Display for GattError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl std::error::Error for GattError {}

impl TryFrom<u32> for GattError {
    type Error = Error;

    fn try_from(value: u32) -> std::result::Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::InvalidHandle),
            2 => Ok(Self::ReadNotPermitted),
            3 => Ok(Self::WriteNotPermitted),
            4 => Ok(Self::InvalidPdu),
            5 => Ok(Self::InsufficientAuthentication),
            6 => Ok(Self::RequestNotSupported),
            7 => Ok(Self::InvalidOffset),
            8 => Ok(Self::InsufficientAuthorization),
            12 => Ok(Self::InsufficientEncryptionKeySize),
            13 => Ok(Self::InvalidAttributeValueLength),
            14 => Ok(Self::UnlikelyError),
            15 => Ok(Self::InsufficientEncryption),
            17 => Ok(Self::InsufficientResources),
            19 => Ok(Self::ValueNotAllowed),
            128 => Ok(Self::ApplicationError80),
            129 => Ok(Self::ApplicationError81),
            130 => Ok(Self::ApplicationError82),
            131 => Ok(Self::ApplicationError83),
            132 => Ok(Self::ApplicationError84),
            133 => Ok(Self::ApplicationError85),
            134 => Ok(Self::ApplicationError86),
            135 => Ok(Self::ApplicationError87),
            136 => Ok(Self::ApplicationError88),
            137 => Ok(Self::ApplicationError89),
            138 => Ok(Self::ApplicationError8A),
            139 => Ok(Self::ApplicationError8B),
            140 => Ok(Self::ApplicationError8C),
            141 => Ok(Self::ApplicationError8D),
            142 => Ok(Self::ApplicationError8E),
            143 => Ok(Self::ApplicationError8F),
            144 => Ok(Self::ApplicationError90),
            145 => Ok(Self::ApplicationError91),
            146 => Ok(Self::ApplicationError92),
            147 => Ok(Self::ApplicationError93),
            148 => Ok(Self::ApplicationError94),
            149 => Ok(Self::ApplicationError95),
            150 => Ok(Self::ApplicationError96),
            151 => Ok(Self::ApplicationError97),
            152 => Ok(Self::ApplicationError98),
            153 => Ok(Self::ApplicationError99),
            154 => Ok(Self::ApplicationError9A),
            155 => Ok(Self::ApplicationError9B),
            156 => Ok(Self::ApplicationError9C),
            157 => Ok(Self::ApplicationError9D),
            158 => Ok(Self::ApplicationError9E),
            159 => Ok(Self::ApplicationError9F),
            252 => Ok(Self::WriteRequestRejected),
            253 => Ok(Self::CccDescriptorImproperlyConfigured),
            254 => Ok(Self::ProcedureAlreadyInProgress),
            255 => Ok(Self::OutOfRange),
            257 => Ok(Self::InvalidParameters),
            258 => Ok(Self::TooManyResults),
            x => Err(Error::Conversion(format!("Unknown GATT Error Value: {x}"))),
        }
    }
}

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    #[error("peer {0} was not recognized")]
    PeerNotRecognized(PeerId),
    #[error("peer {0} was disconnected")]
    PeerDisconnected(PeerId),
    #[error("conversion error: {0}")]
    Conversion(String),
    #[error("scan failed: {0}")]
    ScanFailed(String),
    #[error("another error: {0}")]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
    #[error("GATT error: {0}")]
    Gatt(#[from] GattError),
    #[error("definition error: duplicate handle(s) {0:?}")]
    DuplicateHandle(Vec<Handle>),
    #[error("service for {0:?} already published")]
    AlreadyPublished(crate::server::ServiceId),
    #[error("Encoding/decoding error: {0}")]
    Encoding(#[from] bt_common::packet_encoding::Error),
    #[error("Periodic Advertising error: {0}")]
    PeriodicAdvertising(crate::periodic_advertising::Error),
}

impl Error {
    pub fn other(e: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Other(Box::new(e))
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Other(Box::<dyn std::error::Error + Send + Sync>::from(value))
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        String::from(value).into()
    }
}

pub type Result<T> = core::result::Result<T, Error>;

/// Handles are used as local-to-your-code identifiers for Characteristics and
/// Descriptors.
///
/// Their value should be treated as opaque by clients of PeerService,
/// and are explicitly not guaranteed to be equal to a peer's attribute handles.
/// Stack implementations should provide unique handles for each Characteristic
/// and Descriptor within a PeerService for the client.  It is not necessary or
/// expected for different clients of the same service to have the same handles.
///
/// For LocalService publishing, Service Definitions are required to have no
/// duplicate Handles across their defined Characteristics and Descriptors.
/// Characteristic and ServiceDefinition will check and return
/// Error::DuplicateHandle.  Handles in ServiceDefinitions should not match the
/// attribute handles in the local GATT server.  They are local to your code for
/// use in correlating with ServiceEvents from peers.  Stacks must translate
/// the actual GATT handles to the handles provided in the ServiceDefinition
/// for these events.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Handle(pub u64);

/// Whether a service is marked as Primary or Secondary on the server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceKind {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode {
    None,
    Reliable,
    WithoutResponse,
}

/// Characteristic Properties, including Extended Properties
/// Determines how the Characteristic Value may be used.
///
/// Defined in Core Spec 5.3, Vol 3 Part G Sec 3.3.1.1 and 3.3.3.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CharacteristicProperty {
    Broadcast = 0x01,
    Read = 0x02,
    WriteWithoutResponse = 0x04,
    Write = 0x08,
    Notify = 0x10,
    Indicate = 0x20,
    AuthenticatedSignedWrites = 0x40,
    ReliableWrite = 0x100,
    WritableAuxiliaries = 0x200,
}

impl std::ops::BitOr for CharacteristicProperty {
    type Output = CharacteristicProperties;

    fn bitor(self, rhs: Self) -> Self::Output {
        CharacteristicProperties(vec![self, rhs])
    }
}

#[derive(Default, Debug, Clone)]
pub struct CharacteristicProperties(pub Vec<CharacteristicProperty>);

impl From<CharacteristicProperty> for CharacteristicProperties {
    fn from(value: CharacteristicProperty) -> Self {
        Self(vec![value])
    }
}

impl FromIterator<CharacteristicProperty> for CharacteristicProperties {
    fn from_iter<T: IntoIterator<Item = CharacteristicProperty>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::BitOr<CharacteristicProperty> for CharacteristicProperties {
    type Output = Self;

    fn bitor(mut self, rhs: CharacteristicProperty) -> Self::Output {
        self.0.push(rhs);
        self
    }
}

impl CharacteristicProperties {
    pub fn is_disjoint(&self, other: &Self) -> bool {
        for property in self.0.iter() {
            if other.0.contains(&property) {
                return false;
            }
        }
        true
    }

    pub fn contains(&self, property: CharacteristicProperty) -> bool {
        self.0.contains(&property)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SecurityLevels {
    /// Encryption is required / provided
    pub encryption: bool,
    /// Authentication is required / provided
    pub authentication: bool,
    /// Authorization is required / provided
    pub authorization: bool,
}

impl SecurityLevels {
    pub fn satisfied(&self, provided: SecurityLevels) -> bool {
        (!self.encryption || provided.encryption)
            && (!self.authentication || provided.authentication)
            && (!self.authorization || provided.encryption)
    }

    pub const fn encryption_required() -> Self {
        Self { encryption: true, authentication: false, authorization: false }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AttributePermissions {
    /// If None, then this cannot be read. Otherwise the SecurityLevels given
    /// are required to read.
    pub read: Option<SecurityLevels>,
    /// If None, then this cannot be written. Otherwise the SecurityLevels given
    /// are required to write.
    pub write: Option<SecurityLevels>,
    /// If None, then this cannot be updated. Otherwise the SecurityLevels given
    /// are required to update.
    pub update: Option<SecurityLevels>,
}

impl AttributePermissions {
    pub fn with_levels(properties: &CharacteristicProperties, levels: &SecurityLevels) -> Self {
        let notify_properties = CharacteristicProperty::Notify | CharacteristicProperty::Indicate;
        Self {
            read: properties.contains(CharacteristicProperty::Read).then_some(levels.clone()),
            write: properties.contains(CharacteristicProperty::Write).then_some(levels.clone()),
            update: (!properties.is_disjoint(&notify_properties)).then_some(levels.clone()),
        }
    }
}

/// The different types of well-known Descriptors are defined here and should be
/// automatically read during service characteristic discovery by Stack
/// Implementations and included. Other Descriptor attribute values can be read
/// using [`PeerService::read_descriptor`].
///
/// Characteristic Extended Properties are included in the
/// CharacteristicProperties of the Characteristic.
/// This is a subset of the descriptors defined in the Core Specification (v5.4,
/// Vol 3 Part G Sec 3.3.3).  Missing DescriptorTypes are handled internally
/// and are added automatically, and are omitted from descriptor APIs.
#[derive(Clone, Debug)]
pub enum DescriptorType {
    UserDescription(String),
    ServerConfiguration {
        broadcast: bool,
    },
    CharacteristicPresentation {
        format: u8,
        exponent: i8,
        unit: u16,
        name_space: u8,
        description: u16,
    },
    Other {
        uuid: Uuid,
    },
}

impl From<&DescriptorType> for Uuid {
    fn from(value: &DescriptorType) -> Self {
        // Values from https://bitbucket.org/bluetooth-SIG/public/src/main/assigned_numbers/uuids/descriptors.yaml
        match value {
            DescriptorType::UserDescription(_) => Uuid::from_u16(0x2901),
            DescriptorType::ServerConfiguration { .. } => Uuid::from_u16(0x2903),
            DescriptorType::CharacteristicPresentation { .. } => Uuid::from_u16(0x2904),
            DescriptorType::Other { uuid } => uuid.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Descriptor {
    pub handle: Handle,
    /// Permissions required needed to interact with this descriptor.  May not
    /// be accurate on clients until the descriptor is interacted with.
    pub permissions: AttributePermissions,
    pub r#type: DescriptorType,
}

/// A Characteristic on a Service. Each Characteristic has a declaration, value,
/// and zero or more descriptors.
#[derive(Clone, Debug)]
pub struct Characteristic {
    pub handle: Handle,
    pub uuid: Uuid,
    pub properties: CharacteristicProperties,
    /// Attribute Permissions that apply to the value
    pub permissions: AttributePermissions,
    pub descriptors: Vec<Descriptor>,
}

impl Characteristic {
    pub fn properties(&self) -> &CharacteristicProperties {
        &self.properties
    }

    pub fn supports_property(&self, property: &CharacteristicProperty) -> bool {
        self.properties.0.contains(property)
    }

    pub fn descriptors(&self) -> impl Iterator<Item = &Descriptor> {
        self.descriptors.iter()
    }

    pub fn handles(&self) -> impl Iterator<Item = Handle> + '_ {
        std::iter::once(self.handle).chain(self.descriptors.iter().map(|d| d.handle))
    }
}
