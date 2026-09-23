// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth as fidl_bt;
use fidl_fuchsia_bluetooth_bredr::{
    self as fidl_bredr, ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST, ATTR_SERVICE_CLASS_ID_LIST,
    ProfileDescriptor,
};
use fidl_table_validation::ValidFidlTable;
#[cfg(target_os = "fuchsia")]
use fuchsia_inspect as inspect;
#[cfg(target_os = "fuchsia")]
use fuchsia_inspect_derive::{AttachError, Inspect, Unit};
use std::cmp::min;
use std::collections::HashSet;

use crate::assigned_numbers::AssignedNumber;
use crate::assigned_numbers::constants::SERVICE_CLASS_UUIDS;
use crate::error::Error;
use crate::types::Uuid;

/// BR/EDR types for the AVRCP profile.
pub mod avrcp;

/// The Protocol and Service Multiplexer (PSM) for L2cap connections.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Psm(u16);

impl Psm {
    /// PSMs commonly used in the codebase.
    pub const RFCOMM: Self = Self(fidl_bredr::PSM_RFCOMM);
    pub const HID_CONTROL: Self = Self(fidl_bredr::PSM_HID_CONTROL);
    pub const HID_INTERRUPT: Self = Self(fidl_bredr::PSM_HID_INTERRUPT);
    pub const AVDTP: Self = Self(fidl_bredr::PSM_AVDTP);
    pub const AVCTP: Self = Self(fidl_bredr::PSM_AVCTP);
    pub const AVCTP_BROWSE: Self = Self(fidl_bredr::PSM_AVCTP_BROWSE);
    pub const DYNAMIC: Self = Self(fidl_bredr::PSM_DYNAMIC);

    pub fn new(value: u16) -> Self {
        Self(value)
    }
}

impl From<Psm> for u16 {
    fn from(src: Psm) -> u16 {
        src.0
    }
}

/// Try to interpret a DataElement as a ProfileDesciptor.
/// Returns None if the DataElement is not in the correct format to represent a ProfileDescriptor.
pub fn elem_to_profile_descriptor(elem: &fidl_bredr::DataElement) -> Option<ProfileDescriptor> {
    if let fidl_bredr::DataElement::Sequence(seq) = elem {
        if seq.len() != 2 {
            return None;
        }

        let profile_id = match **seq[0].as_ref()? {
            fidl_bredr::DataElement::Uuid(uuid) => {
                let uuid: Uuid = uuid.into();
                uuid.try_into().ok()?
            }
            _ => return None,
        };

        let [major_version, minor_version] = match **seq[1].as_ref()? {
            fidl_bredr::DataElement::Uint16(val) => val.to_be_bytes(),
            _ => return None,
        };

        return Some(ProfileDescriptor {
            profile_id: Some(profile_id),
            major_version: Some(major_version),
            minor_version: Some(minor_version),
            ..Default::default()
        });
    }
    None
}

/// Find an element representing the Bluetooth Profile Descriptor List in `attributes`, and
/// convert the elements in the list into ProfileDescriptors.
/// Returns an Error if no matching element was found, or if any element of the list couldn't be converted
/// into a ProfileDescriptor.
pub fn find_profile_descriptors(
    attributes: &[fidl_bredr::Attribute],
) -> Result<Vec<ProfileDescriptor>, Error> {
    let attr = attributes
        .iter()
        .find(|a| a.id == Some(ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST))
        .ok_or_else(|| Error::profile("missing profile descriptor"))?;

    let Some(fidl_bredr::DataElement::Sequence(profiles)) = &attr.element else {
        return Err(Error::profile("attribute element is invalidly formatted"));
    };
    let mut result = Vec::new();
    for elem in profiles {
        let elem = elem.as_ref().ok_or_else(|| Error::profile("null DataElement in sequence"))?;
        result.push(
            elem_to_profile_descriptor(&*elem)
                .ok_or_else(|| Error::profile("couldn't convert to a ProfileDescriptor"))?,
        );
    }
    if result.is_empty() { Err(Error::profile("no profile descriptor found")) } else { Ok(result) }
}

pub fn profile_descriptor_to_assigned(profile_desc: &ProfileDescriptor) -> Option<AssignedNumber> {
    let Some(id) = profile_desc.profile_id else {
        return None;
    };
    SERVICE_CLASS_UUIDS.iter().find(|scn| id.into_primitive() == scn.number).cloned()
}

/// Returns the PSM from the provided `protocol`. Returns None if the protocol
/// is not L2CAP or does not contain a PSM.
pub fn psm_from_protocol(protocol: &Vec<ProtocolDescriptor>) -> Option<Psm> {
    for descriptor in protocol {
        if descriptor.protocol == fidl_bredr::ProtocolIdentifier::L2Cap {
            if descriptor.params.len() != 1 {
                return None;
            }

            if let DataElement::Uint16(psm) = descriptor.params[0] {
                return Some(Psm::new(psm));
            }
            return None;
        }
    }
    None
}

/// Returns the RFCOMM or L2CAP channel number from the provided `protocol`.
/// Returns an Error if the protocol is not a valid L2CAP or RFCOMM service.
pub fn channel_number_from_protocol(
    protocol: &Vec<fidl_bredr::ProtocolDescriptor>,
) -> Result<i32, Error> {
    for prot in protocol {
        let Some(params) = prot.params.as_ref() else {
            return Err(Error::profile("Invalid service definition"));
        };
        if prot.protocol == Some(fidl_bredr::ProtocolIdentifier::L2Cap) {
            // Get the L2CAP PSM from the protocol descriptor. May be empty if the service
            // requests RFCOMM. If so, we can skip to the next entry in the protocol list.
            let [fidl_bredr::DataElement::Uint16(l2cap_port)] = params[..] else {
                continue;
            };
            return Ok(l2cap_port.into());
        }

        if prot.protocol == Some(fidl_bredr::ProtocolIdentifier::Rfcomm) {
            // If RFCOMM is specified, then the RFCOMM port must be populated.
            let [fidl_bredr::DataElement::Uint8(rfcomm_port)] = params[..] else {
                return Err(Error::profile("Invalid RFCOMM service definition"));
            };
            return Ok(rfcomm_port.into());
        }
    }
    Err(Error::profile("ProtocolDescriptor missing channel number"))
}

/// Returns the L2CAP or RFCOMM channel number from the BR/EDR connection parameters.
/// Returns Error if the provided `parameters` are invalid.
pub fn channel_number_from_parameters(
    parameters: &fidl_bredr::ConnectParameters,
) -> Result<i32, Error> {
    match parameters {
        fidl_bredr::ConnectParameters::Rfcomm(fidl_bredr::RfcommParameters {
            channel: Some(port),
            ..
        }) => Ok((*port).into()),
        fidl_bredr::ConnectParameters::L2cap(fidl_bredr::L2capParameters {
            psm: Some(psm),
            ..
        }) => Ok((*psm).into()),
        _ => Err(Error::profile(format!("Invalid parameters: {parameters:?}"))),
    }
}

/// Search for Service Class UUIDs of well known services from a list of attributes (such as returned via Service Search)
pub fn find_service_classes(
    attributes: &[fidl_fuchsia_bluetooth_bredr::Attribute],
) -> Vec<AssignedNumber> {
    let uuids = find_all_service_classes(attributes);
    SERVICE_CLASS_UUIDS
        .iter()
        .filter(|scn| uuids.contains(&Uuid::new16(scn.number)))
        .cloned()
        .collect()
}

/// Search for all Service Class UUID from a list of attributes (such as returned via Service Search)
pub fn find_all_service_classes(
    attributes: &[fidl_fuchsia_bluetooth_bredr::Attribute],
) -> Vec<Uuid> {
    let Some(attr) = attributes.iter().find(|a| a.id == Some(ATTR_SERVICE_CLASS_ID_LIST)) else {
        return vec![];
    };
    let Some(fidl_fuchsia_bluetooth_bredr::DataElement::Sequence(elems)) = &attr.element else {
        return vec![];
    };
    elems
        .iter()
        .filter_map(|e| {
            e.as_ref().and_then(|e| match **e {
                fidl_fuchsia_bluetooth_bredr::DataElement::Uuid(uuid) => Some(uuid.into()),
                _ => None,
            })
        })
        .collect()
}

/// Given two SecurityRequirements, combines both into requirements as strict as either.
/// A stricter SecurityRequirements is defined as:
///   1) Authentication required is stricter than not.
///   2) Secure Connections required is stricter than not.
pub fn combine_security_requirements(
    reqs: &SecurityRequirements,
    other: &SecurityRequirements,
) -> SecurityRequirements {
    let authentication_required =
        match (reqs.authentication_required, other.authentication_required) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(x), None) | (None, Some(x)) => Some(x),
            _ => None,
        };
    let secure_connections_required =
        match (reqs.secure_connections_required, other.secure_connections_required) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(x), None) | (None, Some(x)) => Some(x),
            _ => None,
        };
    SecurityRequirements { authentication_required, secure_connections_required }
}

/// Given two ChannelParameters, combines both into a set of ChannelParameters
/// with the least requesting of resources.
/// This is defined as:
///   1) Basic requires fewer resources than ERTM.
///   2) A smaller SDU size is more restrictive.
pub fn combine_channel_parameters(
    params: &ChannelParameters,
    other: &ChannelParameters,
) -> ChannelParameters {
    let channel_mode = match (params.channel_mode, other.channel_mode) {
        (Some(fidl_bt::ChannelMode::Basic), _) | (_, Some(fidl_bt::ChannelMode::Basic)) => {
            Some(fidl_bt::ChannelMode::Basic)
        }
        (Some(x), None) | (None, Some(x)) => Some(x),
        _ => None,
    };
    let max_rx_sdu_size = match (params.max_rx_sdu_size, other.max_rx_sdu_size) {
        (Some(rx1), Some(rx2)) => Some(min(rx1, rx2)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        _ => None,
    };
    let security_requirements = match (&params.security_requirements, &other.security_requirements)
    {
        (Some(reqs1), Some(reqs2)) => Some(combine_security_requirements(reqs1, reqs2)),
        (Some(reqs), _) | (_, Some(reqs)) => Some(reqs.clone()),
        _ => None,
    };
    ChannelParameters { channel_mode, max_rx_sdu_size, security_requirements }
}

/// The basic building block for elements in a SDP record.
/// Corresponds directly to the FIDL `DataElement` definition - with the extra
/// properties of Clone and PartialEq.
/// See [fuchsia.bluetooth.bredr.DataElement] for more documentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataElement {
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    Uint8(u8),
    Uint16(u16),
    Uint32(u32),
    Uint64(u64),
    Str(Vec<u8>),
    Url(String),
    Uuid(fidl_bt::Uuid),
    Bool(bool),
    Sequence(Vec<Box<DataElement>>),
    Alternatives(Vec<Box<DataElement>>),
}

impl std::hash::Hash for DataElement {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        core::mem::discriminant(self).hash(state);
        match self {
            DataElement::Int8(x) => x.hash(state),
            DataElement::Int16(x) => x.hash(state),
            DataElement::Int32(x) => x.hash(state),
            DataElement::Int64(x) => x.hash(state),
            DataElement::Uint8(x) => x.hash(state),
            DataElement::Uint16(x) => x.hash(state),
            DataElement::Uint32(x) => x.hash(state),
            DataElement::Uint64(x) => x.hash(state),
            DataElement::Str(x) => x.hash(state),
            DataElement::Url(x) => x.hash(state),
            DataElement::Uuid(x) => x.value.hash(state),
            DataElement::Bool(x) => x.hash(state),
            DataElement::Sequence(x) => x.hash(state),
            DataElement::Alternatives(x) => x.hash(state),
        }
    }
}

impl TryFrom<&fidl_bredr::DataElement> for DataElement {
    type Error = Error;

    fn try_from(src: &fidl_bredr::DataElement) -> Result<DataElement, Error> {
        use fidl_bredr::DataElement as fDataElement;
        let element = match src {
            fDataElement::Int8(x) => DataElement::Int8(*x),
            fDataElement::Int16(x) => DataElement::Int16(*x),
            fDataElement::Int32(x) => DataElement::Int32(*x),
            fDataElement::Int64(x) => DataElement::Int64(*x),
            fDataElement::Uint8(x) => DataElement::Uint8(*x),
            fDataElement::Uint16(x) => DataElement::Uint16(*x),
            fDataElement::Uint32(x) => DataElement::Uint32(*x),
            fDataElement::Uint64(x) => DataElement::Uint64(*x),
            // TODO(https://fxbug.dev/42058871) Replace clones with moves where possible.
            fDataElement::Str(v) => DataElement::Str(v.clone()),
            fDataElement::Url(s) => DataElement::Url(s.to_string()),
            fDataElement::Uuid(uuid) => DataElement::Uuid(uuid.clone()),
            fDataElement::B(b) => DataElement::Bool(*b),
            fDataElement::Sequence(x) => {
                let mapped = x
                    .iter()
                    .filter_map(|opt| {
                        opt.as_ref().map(|t| match DataElement::try_from(&**t) {
                            Ok(elem) => Ok(Box::new(elem)),
                            Err(err) => Err(err),
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                DataElement::Sequence(mapped)
            }
            fDataElement::Alternatives(x) => {
                let mapped = x
                    .iter()
                    .filter_map(|opt| {
                        opt.as_ref().map(|t| match DataElement::try_from(&**t) {
                            Ok(elem) => Ok(Box::new(elem)),
                            Err(err) => Err(err),
                        })
                    })
                    .collect::<Result<Vec<_>, Error>>()?;
                DataElement::Alternatives(mapped)
            }
            _ => return Err(Error::conversion("Unknown DataElement type")),
        };
        Ok(element)
    }
}

impl From<&DataElement> for fidl_bredr::DataElement {
    fn from(src: &DataElement) -> fidl_bredr::DataElement {
        use fidl_bredr::DataElement as fDataElement;
        match src {
            DataElement::Int8(x) => fDataElement::Int8(*x),
            DataElement::Int16(x) => fDataElement::Int16(*x),
            DataElement::Int32(x) => fDataElement::Int32(*x),
            DataElement::Int64(x) => fDataElement::Int64(*x),
            DataElement::Uint8(x) => fDataElement::Uint8(*x),
            DataElement::Uint16(x) => fDataElement::Uint16(*x),
            DataElement::Uint32(x) => fDataElement::Uint32(*x),
            DataElement::Uint64(x) => fDataElement::Uint64(*x),
            DataElement::Str(v) => fDataElement::Str(v.clone()),
            DataElement::Url(s) => fDataElement::Url(s.to_string()),
            DataElement::Uuid(uuid) => fDataElement::Uuid(uuid.clone()),
            DataElement::Bool(b) => fDataElement::B(*b),
            DataElement::Sequence(x) => {
                let mapped =
                    x.iter().map(|t| Some(Box::new(fDataElement::from(&**t)))).collect::<Vec<_>>();
                fDataElement::Sequence(mapped)
            }
            DataElement::Alternatives(x) => {
                let mapped =
                    x.iter().map(|t| Some(Box::new(fDataElement::from(&**t)))).collect::<Vec<_>>();
                fDataElement::Alternatives(mapped)
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct DataElementConversionError {
    pub data_element: DataElement,
}

// Macro for generating impls for converting between rust types and their DataElement wrappers.
macro_rules! generate_data_element_conversion {
    ($variant: ident, $type: ty) => {
        impl TryFrom<DataElement> for $type {
            type Error = DataElementConversionError;

            fn try_from(data_element: DataElement) -> Result<$type, DataElementConversionError> {
                match data_element {
                    DataElement::$variant(x) => Ok(x),
                    _ => Err(DataElementConversionError { data_element }),
                }
            }
        }

        impl From<$type> for DataElement {
            fn from(x: $type) -> DataElement {
                DataElement::$variant(x)
            }
        }
    };
}

// Generate the impls for converting between rust types and their DataElement wrappers.
generate_data_element_conversion!(Int8, i8);
generate_data_element_conversion!(Int16, i16);
generate_data_element_conversion!(Int32, i32);
generate_data_element_conversion!(Int64, i64);
generate_data_element_conversion!(Uint8, u8);
generate_data_element_conversion!(Uint16, u16);
generate_data_element_conversion!(Uint32, u32);
generate_data_element_conversion!(Uint64, u64);
generate_data_element_conversion!(Str, Vec<u8>);
generate_data_element_conversion!(Uuid, fidl_bt::Uuid);
generate_data_element_conversion!(Url, String);
generate_data_element_conversion!(Bool, bool);

/// Information about a communications protocol.
/// Corresponds directly to the FIDL `ProtocolDescriptor` definition - with the extra
/// properties of Clone and PartialEq.
/// See [fuchsia.bluetooth.bredr.ProtocolDescriptor] for more documentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProtocolDescriptor {
    pub protocol: fidl_bredr::ProtocolIdentifier,
    pub params: Vec<DataElement>,
}

impl TryFrom<&fidl_bredr::ProtocolDescriptor> for ProtocolDescriptor {
    type Error = Error;

    fn try_from(src: &fidl_bredr::ProtocolDescriptor) -> Result<ProtocolDescriptor, Self::Error> {
        let Some(protocol) = src.protocol else {
            return Err(Error::missing("Missing ProtocolDescriptor.protocol"));
        };
        let params = src.params.as_ref().map_or(Ok(vec![]), |elems| {
            elems
                .iter()
                .map(|elem| DataElement::try_from(elem))
                .collect::<Result<Vec<DataElement>, Error>>()
        })?;
        Ok(ProtocolDescriptor { protocol, params })
    }
}

impl From<&ProtocolDescriptor> for fidl_bredr::ProtocolDescriptor {
    fn from(src: &ProtocolDescriptor) -> fidl_bredr::ProtocolDescriptor {
        let params = src.params.iter().map(|elem| fidl_bredr::DataElement::from(elem)).collect();
        fidl_bredr::ProtocolDescriptor {
            protocol: Some(src.protocol),
            params: Some(params),
            ..Default::default()
        }
    }
}

pub fn l2cap_connect_parameters(
    psm: Psm,
    mode: fidl_bt::ChannelMode,
) -> fidl_bredr::ConnectParameters {
    fidl_bredr::ConnectParameters::L2cap(fidl_bredr::L2capParameters {
        psm: Some(psm.into()),
        parameters: Some(fidl_bt::ChannelParameters {
            channel_mode: Some(mode),
            ..fidl_bt::ChannelParameters::default()
        }),
        ..fidl_bredr::L2capParameters::default()
    })
}

/// A generic attribute used for protocol information.
/// Corresponds directly to the FIDL `Attribute` definition - with the extra
/// properties of Clone and PartialEq.
/// See [fuchsia.bluetooth.bredr.Attribute] for more documentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Attribute {
    pub id: u16,
    pub element: DataElement,
}

impl TryFrom<&fidl_bredr::Attribute> for Attribute {
    type Error = Error;

    fn try_from(src: &fidl_bredr::Attribute) -> Result<Attribute, Self::Error> {
        let Some(id) = src.id else {
            return Err(Error::missing("Attribute.id"));
        };
        let Some(element) = src.element.as_ref() else {
            return Err(Error::missing("Attribute.element"));
        };
        let element = DataElement::try_from(element)?;
        Ok(Attribute { id, element })
    }
}

impl From<&Attribute> for fidl_bredr::Attribute {
    fn from(src: &Attribute) -> fidl_bredr::Attribute {
        fidl_bredr::Attribute {
            id: Some(src.id),
            element: Some(fidl_bredr::DataElement::from(&src.element)),
            ..Default::default()
        }
    }
}

/// Human-readable information about a service.
/// Corresponds directly to the FIDL `Information` definition - with the extra
/// properties of Clone and PartialEq.
/// See [fuchsia.bluetooth.bredr.Information] for more documentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Information {
    pub language: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub provider: Option<String>,
}

impl TryFrom<&fidl_bredr::Information> for Information {
    type Error = Error;

    fn try_from(src: &fidl_bredr::Information) -> Result<Information, Self::Error> {
        let language = match src.language.as_ref().map(String::as_str) {
            None | Some("") => return Err(Error::missing("bredr.Information.language")),
            Some(l) => l.to_string(),
        };

        Ok(Information {
            language,
            name: src.name.clone(),
            description: src.description.clone(),
            provider: src.provider.clone(),
        })
    }
}

impl TryFrom<&Information> for fidl_bredr::Information {
    type Error = Error;

    fn try_from(src: &Information) -> Result<fidl_bredr::Information, Self::Error> {
        if src.language.is_empty() {
            return Err(Error::missing("Information.language"));
        }

        Ok(fidl_bredr::Information {
            language: Some(src.language.clone()),
            name: src.name.clone(),
            description: src.description.clone(),
            provider: src.provider.clone(),
            ..Default::default()
        })
    }
}

/// Definition of a service that is to be advertised via Bluetooth BR/EDR.
/// Corresponds directly to the FIDL `ServiceDefinition` definition - with the extra
/// properties of Clone and PartialEq.
/// See [fuchsia.bluetooth.bredr.ServiceDefinition] for more documentation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServiceDefinition {
    pub service_class_uuids: Vec<Uuid>,
    pub protocol_descriptor_list: Vec<ProtocolDescriptor>,
    pub additional_protocol_descriptor_lists: Vec<Vec<ProtocolDescriptor>>,
    pub profile_descriptors: Vec<fidl_bredr::ProfileDescriptor>,
    pub information: Vec<Information>,
    pub additional_attributes: Vec<Attribute>,
}

impl Eq for ServiceDefinition {}

impl std::hash::Hash for ServiceDefinition {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.service_class_uuids.hash(state);
        self.protocol_descriptor_list.hash(state);
        self.additional_protocol_descriptor_lists.hash(state);
        // Hash profile_descriptors manually as FIDL table doesn't derive Hash
        for desc in &self.profile_descriptors {
            desc.profile_id.hash(state);
            desc.major_version.hash(state);
            desc.minor_version.hash(state);
        }
        self.information.hash(state);
        self.additional_attributes.hash(state);
    }
}

impl ServiceDefinition {
    pub fn try_into_fidl(this: &Vec<Self>) -> Result<Vec<fidl_bredr::ServiceDefinition>, Error> {
        this.iter().map(fidl_bredr::ServiceDefinition::try_from).collect::<Result<Vec<_>, _>>()
    }

    pub fn try_from_fidl(src: &Vec<fidl_bredr::ServiceDefinition>) -> Result<Vec<Self>, Error> {
        src.iter().map(Self::try_from).collect::<Result<Vec<_>, _>>()
    }

    /// Returns the primary PSM associated with this ServiceDefinition.
    pub fn primary_psm(&self) -> Option<Psm> {
        psm_from_protocol(&self.protocol_descriptor_list)
    }

    /// Returns the additional PSMs associated with this ServiceDefinition.
    pub fn additional_psms(&self) -> HashSet<Psm> {
        self.additional_protocol_descriptor_lists
            .iter()
            .filter_map(|protocol| psm_from_protocol(protocol))
            .collect()
    }

    /// Returns the PSM in the GOEP L2CAP Attribute, if provided.
    pub fn goep_l2cap_psm(&self) -> Option<Psm> {
        const GOEP_L2CAP_PSM_ATTRIBUTE: u16 = 0x0200;
        let Some(attribute) =
            self.additional_attributes.iter().find(|attr| attr.id == GOEP_L2CAP_PSM_ATTRIBUTE)
        else {
            return None;
        };

        let DataElement::Uint16(psm) = attribute.element else {
            return None;
        };

        Some(Psm::new(psm))
    }

    /// Returns all the PSMs associated with this ServiceDefinition.
    ///
    /// It's possible that the definition doesn't provide any PSMs, in which
    /// case the returned set will be empty.
    pub fn psm_set(&self) -> HashSet<Psm> {
        let mut psms = self.additional_psms();
        if let Some(psm) = self.primary_psm() {
            let _ = psms.insert(psm);
        }
        if let Some(psm) = self.goep_l2cap_psm() {
            let _ = psms.insert(psm);
        }

        psms
    }
}

impl TryFrom<&fidl_bredr::ServiceDefinition> for ServiceDefinition {
    type Error = Error;

    fn try_from(src: &fidl_bredr::ServiceDefinition) -> Result<ServiceDefinition, Self::Error> {
        let service_class_uuids = match &src.service_class_uuids {
            Some(uuids) if !uuids.is_empty() => uuids.iter().map(Uuid::from).collect(),
            _ => {
                return Err(Error::conversion(
                    "bredr.ServiceDefinition.service_class_uuids is empty",
                ));
            }
        };

        let protocol_descriptor_list: Vec<ProtocolDescriptor> =
            src.protocol_descriptor_list.as_ref().map_or(Ok(vec![]), |p| {
                p.iter()
                    .map(|d| ProtocolDescriptor::try_from(d))
                    .collect::<Result<Vec<ProtocolDescriptor>, Error>>()
            })?;
        let additional_protocol_descriptor_lists: Vec<Vec<ProtocolDescriptor>> =
            src.additional_protocol_descriptor_lists.as_ref().map_or(Ok(vec![]), |desc_lists| {
                desc_lists
                    .iter()
                    .map(|desc_list| {
                        desc_list.iter().map(|d| ProtocolDescriptor::try_from(d)).collect::<Result<
                            Vec<ProtocolDescriptor>,
                            Error,
                        >>(
                        )
                    })
                    .collect::<Result<Vec<Vec<ProtocolDescriptor>>, Error>>()
            })?;
        let profile_descriptors: Vec<fidl_bredr::ProfileDescriptor> =
            src.profile_descriptors.clone().unwrap_or_default();
        let information: Result<Vec<Information>, Error> = src
            .information
            .as_ref()
            .map_or(Ok(vec![]), |infos| infos.iter().map(|i| Information::try_from(i)).collect());
        let additional_attributes: Vec<Attribute> =
            src.additional_attributes.as_ref().map_or(Ok(vec![]), |attrs| {
                attrs
                    .iter()
                    .map(|a| Attribute::try_from(a))
                    .collect::<Result<Vec<Attribute>, Error>>()
            })?;

        Ok(ServiceDefinition {
            service_class_uuids,
            protocol_descriptor_list,
            additional_protocol_descriptor_lists,
            profile_descriptors,
            information: information?,
            additional_attributes,
        })
    }
}

impl TryFrom<&ServiceDefinition> for fidl_bredr::ServiceDefinition {
    type Error = Error;

    fn try_from(src: &ServiceDefinition) -> Result<fidl_bredr::ServiceDefinition, Self::Error> {
        if src.service_class_uuids.is_empty() {
            return Err(Error::conversion("ServiceDefinitions.service_class_uuids is empty"));
        }
        let service_class_uuids = src.service_class_uuids.iter().map(fidl_bt::Uuid::from).collect();

        let protocol_descriptor_list: Vec<fidl_bredr::ProtocolDescriptor> = src
            .protocol_descriptor_list
            .iter()
            .map(|d| fidl_bredr::ProtocolDescriptor::from(d))
            .collect();
        let additional_protocol_descriptor_lists: Vec<Vec<fidl_bredr::ProtocolDescriptor>> = src
            .additional_protocol_descriptor_lists
            .iter()
            .map(|desc_list| {
                desc_list.iter().map(|d| fidl_bredr::ProtocolDescriptor::from(d)).collect()
            })
            .collect();
        let profile_descriptors: Vec<fidl_bredr::ProfileDescriptor> =
            src.profile_descriptors.clone();
        let information: Result<Vec<fidl_bredr::Information>, Error> =
            src.information.iter().map(|i| fidl_bredr::Information::try_from(i)).collect();
        let additional_attributes: Vec<fidl_bredr::Attribute> =
            src.additional_attributes.iter().map(|a| fidl_bredr::Attribute::from(a)).collect();

        Ok(fidl_bredr::ServiceDefinition {
            service_class_uuids: Some(service_class_uuids),
            protocol_descriptor_list: Some(protocol_descriptor_list),
            additional_protocol_descriptor_lists: Some(additional_protocol_descriptor_lists),
            profile_descriptors: Some(profile_descriptors),
            information: Some(information?),
            additional_attributes: Some(additional_attributes),
            ..Default::default()
        })
    }
}

/// Authentication and permission requirements for an advertised service.
/// Corresponds directly to the FIDL `SecurityRequirements` definition - with the extra properties
/// of Clone and PartialEq.
/// See [fuchsia.bluetooth.bredr.SecurityRequirements] for more documentation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SecurityRequirements {
    pub authentication_required: Option<bool>,
    pub secure_connections_required: Option<bool>,
}

impl From<&fidl_bt::SecurityRequirements> for SecurityRequirements {
    fn from(src: &fidl_bt::SecurityRequirements) -> SecurityRequirements {
        SecurityRequirements {
            authentication_required: src.authentication_required,
            secure_connections_required: src.secure_connections_required,
        }
    }
}

impl From<&SecurityRequirements> for fidl_bt::SecurityRequirements {
    fn from(src: &SecurityRequirements) -> fidl_bt::SecurityRequirements {
        fidl_bt::SecurityRequirements {
            authentication_required: src.authentication_required,
            secure_connections_required: src.secure_connections_required,
            ..Default::default()
        }
    }
}

/// Minimum SDU size the service is capable of accepting.
/// See [fuchsia.bluetooth.bredr.ChannelParameters] for more documentation.
const MIN_RX_SDU_SIZE: u16 = 48;

/// Preferred L2CAP channel parameters for an advertised service.
/// Corresponds directly to the FIDL `ChannelParameters` definition - with the extra properties
/// of Clone and PartialEq.
/// The invariants of the FIDL definition are enforced - the max SDU size must be >= 48.
/// See [fuchsia.bluetooth.bredr.ChannelParameters] for more documentation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChannelParameters {
    pub channel_mode: Option<fidl_bt::ChannelMode>,
    pub max_rx_sdu_size: Option<u16>,
    pub security_requirements: Option<SecurityRequirements>,
}

impl TryFrom<&fidl_bt::ChannelParameters> for ChannelParameters {
    type Error = Error;

    fn try_from(src: &fidl_bt::ChannelParameters) -> Result<ChannelParameters, Self::Error> {
        if let Some(size) = src.max_rx_packet_size {
            if size < MIN_RX_SDU_SIZE {
                return Err(Error::conversion(format!(
                    "bredr.ChannelParameters.max_rx_sdu_size is too small: {size}"
                )));
            }
        }

        Ok(ChannelParameters {
            channel_mode: src.channel_mode,
            max_rx_sdu_size: src.max_rx_packet_size,
            security_requirements: src
                .security_requirements
                .as_ref()
                .map(SecurityRequirements::from),
        })
    }
}

impl TryFrom<&ChannelParameters> for fidl_bt::ChannelParameters {
    type Error = Error;

    fn try_from(src: &ChannelParameters) -> Result<fidl_bt::ChannelParameters, Self::Error> {
        if let Some(size) = src.max_rx_sdu_size {
            if size < MIN_RX_SDU_SIZE {
                return Err(Error::conversion(format!(
                    "ChannelParameters.max_rx_sdu_size is too small: {size}"
                )));
            }
        }

        Ok(fidl_bt::ChannelParameters {
            channel_mode: src.channel_mode,
            max_rx_packet_size: src.max_rx_sdu_size,
            security_requirements: src
                .security_requirements
                .as_ref()
                .map(fidl_bt::SecurityRequirements::from),
            ..Default::default()
        })
    }
}

#[derive(Debug, Clone, ValidFidlTable, PartialEq)]
#[fidl_table_src(fidl_bredr::ScoConnectionParameters)]
pub struct ValidScoConnectionParameters {
    pub parameter_set: fidl_bredr::HfpParameterSet,
    pub air_coding_format: fidl_bt::AssignedCodingFormat,
    pub air_frame_size: u16,
    pub io_bandwidth: u32,
    pub io_coding_format: fidl_bt::AssignedCodingFormat,
    pub io_frame_size: u16,
    #[fidl_field_type(optional)]
    pub io_pcm_data_format: Option<fidl_fuchsia_hardware_audio::SampleFormat>,
    #[fidl_field_type(optional)]
    pub io_pcm_sample_payload_msb_position: Option<u8>,
    pub path: fidl_bredr::DataPath,
}

#[cfg(target_os = "fuchsia")]
impl Unit for ValidScoConnectionParameters {
    type Data = inspect::Node;
    fn inspect_create(&self, parent: &inspect::Node, name: impl AsRef<str>) -> Self::Data {
        let mut node = parent.create_child(name.as_ref());
        self.inspect_update(&mut node);
        node
    }

    fn inspect_update(&self, data: &mut Self::Data) {
        data.record_string("parameter_set", &format!("{:?}", self.parameter_set));
        data.record_string("air_coding_format", &format!("{:?}", self.air_coding_format));
        data.record_uint("air_frame_size", self.air_frame_size.into());
        data.record_uint("io_bandwidth", self.io_bandwidth.into());
        data.record_string("io_coding_format", &format!("{:?}", self.io_coding_format));
        data.record_uint("io_frame_size", self.io_frame_size.into());
        if let Some(io_pcm_data_format) = &self.io_pcm_data_format {
            data.record_string("io_pcm_data_format", &format!("{:?}", io_pcm_data_format));
        }
        if let Some(io_pcm_sample_payload_msb_position) = &self.io_pcm_sample_payload_msb_position {
            data.record_uint(
                "io_pcm_sample_payload_msb_position",
                (*io_pcm_sample_payload_msb_position).into(),
            );
        }
        data.record_string("path", &format!("{:?}", self.path));
    }
}

#[cfg(target_os = "fuchsia")]
impl Inspect for &mut ValidScoConnectionParameters {
    fn iattach(self, parent: &inspect::Node, name: impl AsRef<str>) -> Result<(), AttachError> {
        // The created node is owned by the provided `parent`.
        parent.record(self.inspect_create(parent, name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    #[test]
    fn test_find_descriptors_fails_with_no_descriptors() {
        assert!(find_profile_descriptors(&[]).is_err());

        let mut attributes = vec![fidl_bredr::Attribute {
            id: Some(0x3001),
            element: Some(fidl_bredr::DataElement::Uint32(0xF00FC0DE)),
            ..Default::default()
        }];

        assert!(find_profile_descriptors(&attributes).is_err());

        // Wrong element type
        attributes.push(fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
            element: Some(fidl_bredr::DataElement::Uint32(0xABADC0DE)),
            ..Default::default()
        });

        assert!(find_profile_descriptors(&attributes).is_err());

        // Empty sequence
        attributes[1].element = Some(fidl_bredr::DataElement::Sequence(vec![]));

        assert!(find_profile_descriptors(&attributes).is_err());
    }

    #[test]
    fn test_find_descriptors_returns_descriptors() {
        let attributes = vec![fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
            element: Some(fidl_bredr::DataElement::Sequence(vec![
                Some(Box::new(fidl_bredr::DataElement::Sequence(vec![
                    Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()))),
                    Some(Box::new(fidl_bredr::DataElement::Uint16(0x0103))),
                ]))),
                Some(Box::new(fidl_bredr::DataElement::Sequence(vec![
                    Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x113A).into()))),
                    Some(Box::new(fidl_bredr::DataElement::Uint16(0x0302))),
                ]))),
            ])),
            ..Default::default()
        }];

        let result = find_profile_descriptors(&attributes);
        assert!(result.is_ok());
        let result = result.expect("result");
        assert_eq!(2, result.len());

        assert_eq!(
            fidl_bredr::ServiceClassProfileIdentifier::SerialPort,
            result[0].profile_id.unwrap()
        );
        assert_eq!(1, result[0].major_version.unwrap());
        assert_eq!(3, result[0].minor_version.unwrap());
    }

    #[test]
    fn test_find_service_classes_attribute_missing() {
        assert_eq!(find_service_classes(&[]), Vec::new());
        let attributes = vec![fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
            element: Some(fidl_bredr::DataElement::Sequence(vec![
                Some(Box::new(fidl_bredr::DataElement::Sequence(vec![
                    Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()))),
                    Some(Box::new(fidl_bredr::DataElement::Uint16(0x0103))),
                ]))),
                Some(Box::new(fidl_bredr::DataElement::Sequence(vec![
                    Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x113A).into()))),
                    Some(Box::new(fidl_bredr::DataElement::Uint16(0x0302))),
                ]))),
            ])),
            ..Default::default()
        }];
        assert_eq!(find_service_classes(&attributes), Vec::new());
    }

    #[test]
    fn test_find_service_classes_wrong_type() {
        let attributes = vec![fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_SERVICE_CLASS_ID_LIST),
            element: Some(fidl_bredr::DataElement::Uint32(0xc0defae5u32)),
            ..Default::default()
        }];
        assert_eq!(find_service_classes(&attributes), Vec::new());
    }

    #[test]
    fn test_find_service_classes_returns_known_classes() {
        let attribute = fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_SERVICE_CLASS_ID_LIST),
            element: Some(fidl_bredr::DataElement::Sequence(vec![Some(Box::new(
                fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()),
            ))])),
            ..Default::default()
        };

        let result = find_service_classes(&[attribute]);
        assert_eq!(1, result.len());
        let assigned_num = result.first().unwrap();
        assert_eq!(0x1101, assigned_num.number); // 0x1101 is the 16-bit UUID of SerialPort
        assert_eq!("SerialPort", assigned_num.name);

        let unknown_uuids = fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_SERVICE_CLASS_ID_LIST),
            element: Some(fidl_bredr::DataElement::Sequence(vec![
                Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()))),
                Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0xc0de).into()))),
            ])),
            ..Default::default()
        };

        // Discards unknown UUIDs
        let result = find_service_classes(&[unknown_uuids]);
        assert_eq!(1, result.len());
        let assigned_num = result.first().unwrap();
        assert_eq!(0x1101, assigned_num.number); // 0x1101 is the 16-bit UUID of SerialPort
        assert_eq!("SerialPort", assigned_num.name);
    }

    #[test]
    fn test_psm_from_protocol() {
        let empty = vec![];
        assert_eq!(None, psm_from_protocol(&empty));

        let no_psm = vec![ProtocolDescriptor {
            protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
            params: vec![],
        }];
        assert_eq!(None, psm_from_protocol(&no_psm));

        let psm = Psm::new(10);
        let valid_psm = vec![ProtocolDescriptor {
            protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
            params: vec![DataElement::Uint16(psm.into())],
        }];
        assert_eq!(Some(psm), psm_from_protocol(&valid_psm));

        let rfcomm = vec![
            ProtocolDescriptor {
                protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
                params: vec![], // PSM omitted for RFCOMM.
            },
            ProtocolDescriptor {
                protocol: fidl_bredr::ProtocolIdentifier::Rfcomm,
                params: vec![DataElement::Uint8(10)], // Server channel
            },
        ];
        assert_eq!(None, psm_from_protocol(&rfcomm));
    }

    #[test]
    fn test_elem_to_profile_descriptor_works() {
        let element = fidl_bredr::DataElement::Sequence(vec![
            Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()))),
            Some(Box::new(fidl_bredr::DataElement::Uint16(0x0103))),
        ]);

        let descriptor =
            elem_to_profile_descriptor(&element).expect("descriptor should be returned");

        assert_eq!(
            fidl_bredr::ServiceClassProfileIdentifier::SerialPort,
            descriptor.profile_id.unwrap()
        );
        assert_eq!(1, descriptor.major_version.unwrap());
        assert_eq!(3, descriptor.minor_version.unwrap());
    }

    #[test]
    fn test_elem_to_profile_descriptor_wrong_element_types() {
        let element = fidl_bredr::DataElement::Sequence(vec![
            Some(Box::new(fidl_bredr::DataElement::Uint16(0x1101))),
            Some(Box::new(fidl_bredr::DataElement::Uint16(0x0103))),
        ]);
        assert!(elem_to_profile_descriptor(&element).is_none());

        let element = fidl_bredr::DataElement::Sequence(vec![
            Some(Box::new(fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()))),
            Some(Box::new(fidl_bredr::DataElement::Uint32(0x0103))),
        ]);
        assert!(elem_to_profile_descriptor(&element).is_none());

        let element = fidl_bredr::DataElement::Sequence(vec![Some(Box::new(
            fidl_bredr::DataElement::Uint32(0x0103),
        ))]);
        assert!(elem_to_profile_descriptor(&element).is_none());

        let element = fidl_bredr::DataElement::Sequence(vec![None]);
        assert!(elem_to_profile_descriptor(&element).is_none());

        let element = fidl_bredr::DataElement::Uint32(0xDEADC0DE);
        assert!(elem_to_profile_descriptor(&element).is_none());
    }

    #[test]
    fn test_invalid_information_fails_gracefully() {
        let empty_language = "".to_string();

        let invalid_local = Information {
            language: empty_language.clone(),
            name: None,
            description: None,
            provider: None,
        };
        let fidl = fidl_bredr::Information::try_from(&invalid_local);
        assert!(fidl.is_err());

        // No language.
        let local = Information::try_from(&fidl_bredr::Information::default());
        assert!(local.is_err());

        let empty_lang_fidl =
            fidl_bredr::Information { language: Some(empty_language), ..Default::default() };
        let local = Information::try_from(&empty_lang_fidl);
        assert!(local.is_err());
    }

    #[test]
    fn get_psm_from_empty_service_definition() {
        let def = ServiceDefinition {
            service_class_uuids: vec![Uuid::new32(1234)],
            protocol_descriptor_list: vec![],
            additional_protocol_descriptor_lists: vec![],
            profile_descriptors: vec![],
            information: vec![],
            additional_attributes: vec![],
        };

        assert_eq!(def.primary_psm(), None);
        assert_eq!(def.additional_psms(), HashSet::new());
        assert_eq!(def.psm_set(), HashSet::new());
    }

    #[test]
    fn test_get_psm_from_service_definition() {
        let uuid = Uuid::new32(1234);
        let psm1 = Psm(10);
        let psm2 = Psm(12);
        let psm3 = Psm(4000);
        let mut def = ServiceDefinition {
            service_class_uuids: vec![uuid],
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
                params: vec![DataElement::Uint16(psm1.into())],
            }],
            additional_protocol_descriptor_lists: vec![],
            profile_descriptors: vec![],
            information: vec![],
            additional_attributes: vec![],
        };

        // Primary protocol contains one PSM.
        assert_eq!(def.primary_psm(), Some(psm1));
        assert_eq!(def.additional_psms(), HashSet::new());
        assert_eq!(def.psm_set(), HashSet::from([psm1]));

        def.additional_protocol_descriptor_lists = vec![
            vec![ProtocolDescriptor {
                protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
                params: vec![DataElement::Uint16(psm2.into())],
            }],
            vec![ProtocolDescriptor {
                protocol: fidl_bredr::ProtocolIdentifier::Avdtp,
                params: vec![DataElement::Uint16(0x0103)],
            }],
        ];

        // Additional protocol contains one PSM.
        assert_eq!(def.primary_psm(), Some(psm1));
        assert_eq!(def.additional_psms(), HashSet::from([psm2]));
        assert_eq!(def.psm_set(), HashSet::from([psm1, psm2]));

        // Additional attributes contain one PSM.
        def.additional_attributes =
            vec![Attribute { id: 0x0200, element: DataElement::Uint16(psm3.into()) }];
        assert_eq!(def.primary_psm(), Some(psm1));
        assert_eq!(def.additional_psms(), HashSet::from([psm2]));
        assert_eq!(def.goep_l2cap_psm(), Some(psm3));
        assert_eq!(def.psm_set(), HashSet::from([psm1, psm2, psm3]));
    }

    #[test]
    fn test_service_definition_conversions() {
        let uuid = fidl_bt::Uuid { value: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15] };
        let prof_descs = vec![ProfileDescriptor {
            profile_id: Some(fidl_bredr::ServiceClassProfileIdentifier::AvRemoteControl),
            major_version: Some(1),
            minor_version: Some(6),
            ..Default::default()
        }];
        let language = "en".to_string();
        let name = "foobar".to_string();
        let description = "fake".to_string();
        let provider = "random".to_string();
        let attribute_id = 0x3001;
        let attribute_value = 0xF00FC0DE;

        let local = ServiceDefinition {
            service_class_uuids: vec![uuid.into()],
            protocol_descriptor_list: vec![ProtocolDescriptor {
                protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
                params: vec![DataElement::Uint16(10)],
            }],
            additional_protocol_descriptor_lists: vec![
                vec![ProtocolDescriptor {
                    protocol: fidl_bredr::ProtocolIdentifier::L2Cap,
                    params: vec![DataElement::Uint16(12)],
                }],
                vec![ProtocolDescriptor {
                    protocol: fidl_bredr::ProtocolIdentifier::Avdtp,
                    params: vec![DataElement::Uint16(3)],
                }],
            ],
            profile_descriptors: prof_descs.clone(),
            information: vec![Information {
                language: language.clone(),
                name: Some(name.clone()),
                description: Some(description.clone()),
                provider: Some(provider.clone()),
            }],
            additional_attributes: vec![Attribute {
                id: attribute_id,
                element: DataElement::Sequence(vec![Box::new(DataElement::Uint32(
                    attribute_value,
                ))]),
            }],
        };

        let fidl = fidl_bredr::ServiceDefinition {
            service_class_uuids: Some(vec![uuid]),
            protocol_descriptor_list: Some(vec![fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![fidl_bredr::DataElement::Uint16(10)]),
                ..Default::default()
            }]),
            additional_protocol_descriptor_lists: Some(vec![
                vec![fidl_bredr::ProtocolDescriptor {
                    protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
                    params: Some(vec![fidl_bredr::DataElement::Uint16(12)]),
                    ..Default::default()
                }],
                vec![fidl_bredr::ProtocolDescriptor {
                    protocol: Some(fidl_bredr::ProtocolIdentifier::Avdtp),
                    params: Some(vec![fidl_bredr::DataElement::Uint16(3)]),
                    ..Default::default()
                }],
            ]),
            profile_descriptors: Some(prof_descs.clone()),
            information: Some(vec![fidl_bredr::Information {
                language: Some(language.clone()),
                name: Some(name.clone()),
                description: Some(description.clone()),
                provider: Some(provider.clone()),
                ..Default::default()
            }]),
            additional_attributes: Some(vec![fidl_bredr::Attribute {
                id: Some(attribute_id),
                element: Some(fidl_bredr::DataElement::Sequence(vec![Some(Box::new(
                    fidl_bredr::DataElement::Uint32(attribute_value),
                ))])),
                ..Default::default()
            }]),
            ..Default::default()
        };

        // Converting from local ServiceDefinition to the FIDL ServiceDefinition should work.
        let local_to_fidl: fidl_bredr::ServiceDefinition =
            fidl_bredr::ServiceDefinition::try_from(&local).expect("should work");
        assert_eq!(local_to_fidl, fidl);

        // Converting from FIDL ServiceDefinition to the local ServiceDefinition should work.
        let fidl_to_local: ServiceDefinition =
            ServiceDefinition::try_from(&fidl).expect("should work");
        assert_eq!(fidl_to_local, local);
    }

    #[test]
    fn test_invalid_service_definition_fails_gracefully() {
        let no_uuids_fidl = fidl_bredr::ServiceDefinition::default();
        let fidl_to_local = ServiceDefinition::try_from(&no_uuids_fidl);
        assert!(fidl_to_local.is_err());

        let empty_uuids_fidl = fidl_bredr::ServiceDefinition {
            service_class_uuids: Some(vec![]),
            ..Default::default()
        };
        let fidl_to_local = ServiceDefinition::try_from(&empty_uuids_fidl);
        assert!(fidl_to_local.is_err());
    }

    #[test]
    fn test_channel_parameters_conversions() {
        let channel_mode = Some(fidl_bt::ChannelMode::EnhancedRetransmission);
        let max_rx_sdu_size = Some(MIN_RX_SDU_SIZE);

        let local =
            ChannelParameters { channel_mode, max_rx_sdu_size, security_requirements: None };
        let fidl = fidl_bt::ChannelParameters {
            channel_mode,
            max_rx_packet_size: max_rx_sdu_size,
            ..Default::default()
        };

        let local_to_fidl =
            fidl_bt::ChannelParameters::try_from(&local).expect("conversion should work");
        assert_eq!(local_to_fidl, fidl);

        let fidl_to_local = ChannelParameters::try_from(&fidl).expect("conversion should work");
        assert_eq!(fidl_to_local, local);

        // Empty FIDL parameters is OK.
        let fidl = fidl_bt::ChannelParameters::default();
        let expected = ChannelParameters {
            channel_mode: None,
            max_rx_sdu_size: None,
            security_requirements: None,
        };

        let fidl_to_local = ChannelParameters::try_from(&fidl).expect("conversion should work");
        assert_eq!(fidl_to_local, expected);
    }

    #[test]
    fn test_invalid_channel_parameters_fails_gracefully() {
        let too_small_sdu = Some(MIN_RX_SDU_SIZE - 1);
        let local = ChannelParameters {
            channel_mode: None,
            max_rx_sdu_size: too_small_sdu,
            security_requirements: None,
        };
        let fidl =
            fidl_bt::ChannelParameters { max_rx_packet_size: too_small_sdu, ..Default::default() };

        let local_to_fidl = fidl_bt::ChannelParameters::try_from(&local);
        assert!(local_to_fidl.is_err());

        let fidl_to_local = ChannelParameters::try_from(&fidl);
        assert!(fidl_to_local.is_err());
    }

    #[test]
    fn test_security_requirements_conversions() {
        let authentication_required = Some(false);
        let secure_connections_required = Some(true);

        let local = SecurityRequirements { authentication_required, secure_connections_required };
        let fidl = fidl_bt::SecurityRequirements {
            authentication_required,
            secure_connections_required,
            ..Default::default()
        };

        let local_to_fidl = fidl_bt::SecurityRequirements::from(&local);
        assert_eq!(local_to_fidl, fidl);

        let fidl_to_local = SecurityRequirements::from(&fidl);
        assert_eq!(fidl_to_local, local);
    }

    #[test]
    fn test_combine_security_requirements() {
        let req1 = SecurityRequirements {
            authentication_required: None,
            secure_connections_required: None,
        };
        let req2 = SecurityRequirements {
            authentication_required: None,
            secure_connections_required: None,
        };
        let expected = SecurityRequirements {
            authentication_required: None,
            secure_connections_required: None,
        };
        assert_eq!(combine_security_requirements(&req1, &req2), expected);

        let req1 = SecurityRequirements {
            authentication_required: Some(true),
            secure_connections_required: None,
        };
        let req2 = SecurityRequirements {
            authentication_required: None,
            secure_connections_required: Some(true),
        };
        let expected = SecurityRequirements {
            authentication_required: Some(true),
            secure_connections_required: Some(true),
        };
        assert_eq!(combine_security_requirements(&req1, &req2), expected);

        let req1 = SecurityRequirements {
            authentication_required: Some(false),
            secure_connections_required: Some(true),
        };
        let req2 = SecurityRequirements {
            authentication_required: None,
            secure_connections_required: Some(true),
        };
        let expected = SecurityRequirements {
            authentication_required: Some(false),
            secure_connections_required: Some(true),
        };
        assert_eq!(combine_security_requirements(&req1, &req2), expected);

        let req1 = SecurityRequirements {
            authentication_required: Some(true),
            secure_connections_required: Some(false),
        };
        let req2 = SecurityRequirements {
            authentication_required: Some(false),
            secure_connections_required: Some(true),
        };
        let expected = SecurityRequirements {
            authentication_required: Some(true),
            secure_connections_required: Some(true),
        };
        assert_eq!(combine_security_requirements(&req1, &req2), expected);
    }

    #[test]
    fn test_combine_channel_parameters() {
        let p1 = ChannelParameters::default();
        let p2 = ChannelParameters::default();
        let expected = ChannelParameters::default();
        assert_eq!(combine_channel_parameters(&p1, &p2), expected);

        let p1 = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::EnhancedRetransmission),
            max_rx_sdu_size: None,
            security_requirements: None,
        };
        let p2 = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::Basic),
            max_rx_sdu_size: Some(70),
            security_requirements: None,
        };
        let expected = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::Basic),
            max_rx_sdu_size: Some(70),
            security_requirements: None,
        };
        assert_eq!(combine_channel_parameters(&p1, &p2), expected);

        let empty_seq_reqs = SecurityRequirements::default();
        let p1 = ChannelParameters {
            channel_mode: None,
            max_rx_sdu_size: Some(75),
            security_requirements: Some(empty_seq_reqs.clone()),
        };
        let p2 = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::EnhancedRetransmission),
            max_rx_sdu_size: None,
            security_requirements: None,
        };
        let expected = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::EnhancedRetransmission),
            max_rx_sdu_size: Some(75),
            security_requirements: Some(empty_seq_reqs),
        };
        assert_eq!(combine_channel_parameters(&p1, &p2), expected);

        let reqs1 = SecurityRequirements {
            authentication_required: Some(true),
            secure_connections_required: None,
        };
        let reqs2 = SecurityRequirements {
            authentication_required: Some(false),
            secure_connections_required: Some(false),
        };
        let combined_reqs = combine_security_requirements(&reqs1, &reqs2);
        let p1 = ChannelParameters {
            channel_mode: None,
            max_rx_sdu_size: Some(90),
            security_requirements: Some(reqs1),
        };
        let p2 = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::Basic),
            max_rx_sdu_size: Some(70),
            security_requirements: Some(reqs2),
        };
        let expected = ChannelParameters {
            channel_mode: Some(fidl_bt::ChannelMode::Basic),
            max_rx_sdu_size: Some(70),
            security_requirements: Some(combined_reqs),
        };
        assert_eq!(combine_channel_parameters(&p1, &p2), expected);
    }

    #[fuchsia::test]
    async fn local_sco_parameters_inspect_tree() {
        let inspect = inspect::Inspector::default();
        assert_data_tree!(inspect, root: {});

        let params = fidl_bredr::ScoConnectionParameters {
            parameter_set: Some(fidl_bredr::HfpParameterSet::D1),
            air_coding_format: Some(fidl_bt::AssignedCodingFormat::Cvsd),
            air_frame_size: Some(60),
            io_bandwidth: Some(16000),
            io_coding_format: Some(fidl_bt::AssignedCodingFormat::LinearPcm),
            io_frame_size: Some(16),
            io_pcm_data_format: Some(fidl_fuchsia_hardware_audio::SampleFormat::PcmSigned),
            io_pcm_sample_payload_msb_position: Some(1),
            path: Some(fidl_bredr::DataPath::Offload),
            ..Default::default()
        };

        let mut local: ValidScoConnectionParameters = params.try_into().expect("can convert");
        assert_data_tree!(inspect, root: {});

        let _ = local.iattach(&inspect.root(), "state").expect("can attach inspect");
        assert_data_tree!(inspect, root: {
            state: {
                parameter_set: "D1",
                air_coding_format: "Cvsd",
                air_frame_size: 60u64,
                io_bandwidth: 16000u64,
                io_coding_format: "LinearPcm",
                io_frame_size: 16u64,
                io_pcm_data_format: "PcmSigned",
                io_pcm_sample_payload_msb_position: 1u64,
                path: "Offload",
            }
        });
    }

    #[test]
    fn data_element_primitve_conversions() {
        type Result<T> = std::result::Result<T, DataElementConversionError>;

        let rust_u8 = 8u8;
        let data_element_uint8 = DataElement::Uint8(8u8);
        let data_element_uint8_into: DataElement = rust_u8.into();
        let rust_u8_ok: Result<u8> = data_element_uint8.clone().try_into();
        let rust_u8_err: Result<u16> = data_element_uint8.clone().try_into();
        assert_eq!(data_element_uint8_into, data_element_uint8);
        assert_eq!(rust_u8_ok, Ok(rust_u8));
        assert_eq!(
            rust_u8_err,
            Err(DataElementConversionError { data_element: data_element_uint8 })
        );

        let rust_i8 = 9i8;
        let data_element_int8 = DataElement::Int8(9i8);
        let data_element_int8_into: DataElement = rust_i8.into();
        let rust_i8_ok: Result<i8> = data_element_int8.clone().try_into();
        let rust_i8_err: Result<u16> = data_element_int8.clone().try_into();
        assert_eq!(data_element_int8_into, data_element_int8);
        assert_eq!(rust_i8_ok, Ok(rust_i8));
        assert_eq!(
            rust_i8_err,
            Err(DataElementConversionError { data_element: data_element_int8 })
        );

        let rust_u16 = 16u16;
        let data_element_uint16 = DataElement::Uint16(16u16);
        let data_element_uint16_into: DataElement = rust_u16.into();
        let rust_u16_ok: Result<u16> = data_element_uint16.clone().try_into();
        let rust_u16_err: Result<i16> = data_element_uint16.clone().try_into();
        assert_eq!(data_element_uint16_into, data_element_uint16);
        assert_eq!(rust_u16_ok, Ok(rust_u16));
        assert_eq!(
            rust_u16_err,
            Err(DataElementConversionError { data_element: data_element_uint16 })
        );

        let rust_i16 = 17i16;
        let data_element_int16 = DataElement::Int16(17i16);
        let data_element_int16_into: DataElement = rust_i16.into();
        let rust_i16_ok: Result<i16> = data_element_int16.clone().try_into();
        let rust_i16_err: Result<u16> = data_element_int16.clone().try_into();
        assert_eq!(data_element_int16_into, data_element_int16);
        assert_eq!(rust_i16_ok, Ok(rust_i16));
        assert_eq!(
            rust_i16_err,
            Err(DataElementConversionError { data_element: data_element_int16 })
        );

        let rust_u32 = 32u32;
        let data_element_uint32 = DataElement::Uint32(32u32);
        let data_element_uint32_into: DataElement = rust_u32.into();
        let rust_u32_ok: Result<u32> = data_element_uint32.clone().try_into();
        let rust_u32_err: Result<u16> = data_element_uint32.clone().try_into();
        assert_eq!(data_element_uint32_into, data_element_uint32);
        assert_eq!(rust_u32_ok, Ok(rust_u32));
        assert_eq!(
            rust_u32_err,
            Err(DataElementConversionError { data_element: data_element_uint32 })
        );

        let rust_i32 = 33i32;
        let data_element_int32 = DataElement::Int32(33i32);
        let data_element_int32_into: DataElement = rust_i32.into();
        let rust_i32_ok: Result<i32> = data_element_int32.clone().try_into();
        let rust_i32_err: Result<u16> = data_element_int32.clone().try_into();
        assert_eq!(data_element_int32_into, data_element_int32);
        assert_eq!(rust_i32_ok, Ok(rust_i32));
        assert_eq!(
            rust_i32_err,
            Err(DataElementConversionError { data_element: data_element_int32 })
        );

        let rust_u64 = 64u64;
        let data_element_uint64 = DataElement::Uint64(64u64);
        let data_element_uint64_into: DataElement = rust_u64.into();
        let rust_u64_ok: Result<u64> = data_element_uint64.clone().try_into();
        let rust_u64_err: Result<u16> = data_element_uint64.clone().try_into();
        assert_eq!(data_element_uint64_into, data_element_uint64);
        assert_eq!(rust_u64_ok, Ok(rust_u64));
        assert_eq!(
            rust_u64_err,
            Err(DataElementConversionError { data_element: data_element_uint64 })
        );

        let rust_i64 = 65i64;
        let data_element_int64 = DataElement::Int64(65i64);
        let data_element_int64_into: DataElement = rust_i64.into();
        let rust_i64_ok: Result<i64> = data_element_int64.clone().try_into();
        let rust_i64_err: Result<u16> = data_element_int64.clone().try_into();
        assert_eq!(data_element_int64_into, data_element_int64);
        assert_eq!(rust_i64_ok, Ok(rust_i64));
        assert_eq!(
            rust_i64_err,
            Err(DataElementConversionError { data_element: data_element_int64 })
        );

        let rust_vec = "ABC".as_bytes().to_vec();
        let data_element_str = DataElement::Str("ABC".as_bytes().to_vec());
        let data_element_str_into: DataElement = rust_vec.clone().into();
        let rust_vec_ok: Result<Vec<u8>> = data_element_str.clone().try_into();
        let rust_vec_err: Result<u16> = data_element_str.clone().try_into();
        assert_eq!(data_element_str_into, data_element_str);
        assert_eq!(rust_vec_ok, Ok(rust_vec));
        assert_eq!(
            rust_vec_err,
            Err(DataElementConversionError { data_element: data_element_str })
        );

        let rust_uuid: fidl_bt::Uuid = Uuid::new16(0x1101).into();
        let data_element_uuid = DataElement::Uuid(Uuid::new16(0x1101).into());
        let data_element_uuid_into: DataElement = rust_uuid.clone().into();
        let rust_uuid_ok: Result<fidl_bt::Uuid> = data_element_uuid.clone().try_into();
        let rust_uuid_err: Result<u16> = data_element_uuid.clone().try_into();
        assert_eq!(data_element_uuid_into, data_element_uuid);
        assert_eq!(rust_uuid_ok, Ok(rust_uuid));
        assert_eq!(
            rust_uuid_err,
            Err(DataElementConversionError { data_element: data_element_uuid })
        );

        let rust_string = String::from("ABC");
        let data_element_url = DataElement::Url(String::from("ABC"));
        let data_element_url_into: DataElement = rust_string.clone().into();
        let rust_string_ok: Result<String> = data_element_url.clone().try_into();
        let rust_string_err: Result<u16> = data_element_url.clone().try_into();
        assert_eq!(data_element_url_into, data_element_url);
        assert_eq!(rust_string_ok, Ok(rust_string));
        assert_eq!(
            rust_string_err,
            Err(DataElementConversionError { data_element: data_element_url })
        );

        let rust_bool = true;
        let data_element_bool = DataElement::Bool(true);
        let data_element_bool_into: DataElement = rust_bool.into();
        let rust_bool_ok: Result<bool> = data_element_bool.clone().try_into();
        let rust_bool_err: Result<u16> = data_element_bool.clone().try_into();
        assert_eq!(data_element_bool_into, data_element_bool);
        assert_eq!(rust_bool_ok, Ok(rust_bool));
        assert_eq!(
            rust_bool_err,
            Err(DataElementConversionError { data_element: data_element_bool })
        );
    }

    #[test]
    fn test_find_service_class_uuids() {
        // No attributes -> empty vec.
        assert_eq!(find_all_service_classes(&[]), Vec::<Uuid>::new());

        // No service class attribute -> empty vec.
        let attributes = vec![fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_BLUETOOTH_PROFILE_DESCRIPTOR_LIST),
            element: Some(fidl_bredr::DataElement::Sequence(vec![Some(Box::new(
                fidl_bredr::DataElement::Uuid(Uuid::new16(0x1101).into()),
            ))])),
            ..Default::default()
        }];
        assert_eq!(find_all_service_classes(&attributes), Vec::<Uuid>::new());

        // Wrong element type for service class attribute -> empty vec.
        let attributes = vec![fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_SERVICE_CLASS_ID_LIST),
            element: Some(fidl_bredr::DataElement::Uint32(0xc0defae5u32)),
            ..Default::default()
        }];
        assert_eq!(find_all_service_classes(&attributes), Vec::<Uuid>::new());

        // Valid attribute with a mix of elements -> returns only UUIDs.
        let uuid1 = Uuid::new16(0x1101);
        let uuid2 = Uuid::from_bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
        let attribute = fidl_bredr::Attribute {
            id: Some(fidl_bredr::ATTR_SERVICE_CLASS_ID_LIST),
            element: Some(fidl_bredr::DataElement::Sequence(vec![
                Some(Box::new(fidl_bredr::DataElement::Uuid(uuid1.into()))),
                Some(Box::new(fidl_bredr::DataElement::Uint16(5))), // Not a UUID, should be ignored.
                Some(Box::new(fidl_bredr::DataElement::Uuid(uuid2.into()))),
                None, // Empty element, should be ignored.
            ])),
            ..Default::default()
        };

        let result = find_all_service_classes(&[attribute]);
        assert_eq!(vec![uuid1, uuid2], result);
    }

    #[test]
    fn test_channel_number_from_protocol() {
        // L2CAP service with valid PSM
        let l2cap_only = vec![fidl_bredr::ProtocolDescriptor {
            protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
            params: Some(vec![fidl_bredr::DataElement::Uint16(42)]),
            ..Default::default()
        }];
        assert_eq!(channel_number_from_protocol(&l2cap_only).unwrap(), 42);

        // RFCOMM with valid server channel number
        let rfcomm = vec![
            fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![]), // No PSM for RFCOMM
                ..Default::default()
            },
            fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::Rfcomm),
                params: Some(vec![fidl_bredr::DataElement::Uint8(5)]),
                ..Default::default()
            },
        ];
        assert_eq!(channel_number_from_protocol(&rfcomm).unwrap(), 5);

        // L2CAP with invalid params (empty) is an error
        let l2cap_invalid = vec![fidl_bredr::ProtocolDescriptor {
            protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
            params: Some(vec![]),
            ..Default::default()
        }];
        assert!(channel_number_from_protocol(&l2cap_invalid).is_err());

        // RFCOMM service with missing channel number is an error
        let rfcomm_invalid = vec![
            fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![]),
                ..Default::default()
            },
            fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::Rfcomm),
                params: Some(vec![]),
                ..Default::default()
            },
        ];
        assert!(channel_number_from_protocol(&rfcomm_invalid).is_err());

        // RFCOMM invalid channel type
        let rfcomm_wrong_type = vec![
            fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::L2Cap),
                params: Some(vec![]),
                ..Default::default()
            },
            fidl_bredr::ProtocolDescriptor {
                protocol: Some(fidl_bredr::ProtocolIdentifier::Rfcomm),
                params: Some(vec![fidl_bredr::DataElement::Uint16(5)]),
                ..Default::default()
            },
        ];
        assert!(channel_number_from_protocol(&rfcomm_wrong_type).is_err());

        // Empty service is an error
        assert!(channel_number_from_protocol(&vec![]).is_err());
    }

    #[test]
    fn test_channel_number_from_parameters() {
        // RFCOMM with valid channel
        let rfcomm = fidl_bredr::ConnectParameters::Rfcomm(fidl_bredr::RfcommParameters {
            channel: Some(7),
            ..Default::default()
        });
        assert_eq!(channel_number_from_parameters(&rfcomm).unwrap(), 7);

        // L2CAP with valid PSM
        let l2cap = fidl_bredr::ConnectParameters::L2cap(fidl_bredr::L2capParameters {
            psm: Some(42),
            ..Default::default()
        });
        assert_eq!(channel_number_from_parameters(&l2cap).unwrap(), 42);

        // RFCOMM missing channel
        let rfcomm_invalid = fidl_bredr::ConnectParameters::Rfcomm(fidl_bredr::RfcommParameters {
            channel: None,
            ..Default::default()
        });
        assert!(channel_number_from_parameters(&rfcomm_invalid).is_err());

        // L2CAP missing PSM
        let l2cap_invalid = fidl_bredr::ConnectParameters::L2cap(fidl_bredr::L2capParameters {
            psm: None,
            ..Default::default()
        });
        assert!(channel_number_from_parameters(&l2cap_invalid).is_err());
    }

    fn calculate_hash<T: Hash>(t: &T) -> u64 {
        let mut s = DefaultHasher::new();
        t.hash(&mut s);
        s.finish()
    }

    #[test]
    fn test_data_element_hash() {
        // Hash of same item
        let elem1 = DataElement::Int8(5);
        let elem2 = elem1.clone();
        assert_eq!(calculate_hash(&elem1), calculate_hash(&elem2));

        // Different variant but same underlying value has different hash
        let elem3 = DataElement::Uint8(5);
        assert_ne!(calculate_hash(&elem1), calculate_hash(&elem3));

        // Same variant but different value has different hash
        let elem4 = DataElement::Int8(6);
        assert_ne!(calculate_hash(&elem1), calculate_hash(&elem4));

        // Sequence elements in different orders have different hash
        let seq1 = DataElement::Sequence(vec![
            Box::new(DataElement::Int8(1)),
            Box::new(DataElement::Int8(2)),
        ]);
        let seq2 = DataElement::Sequence(vec![
            Box::new(DataElement::Int8(2)),
            Box::new(DataElement::Int8(1)),
        ]);
        assert_ne!(calculate_hash(&seq1), calculate_hash(&seq2));

        // Sequence of sequences with same values but different grouping.
        // [[1, 2], [3]] != [[1], [2, 3]]
        let seq_seq1 = DataElement::Sequence(vec![
            Box::new(DataElement::Sequence(vec![
                Box::new(DataElement::Int8(1)),
                Box::new(DataElement::Int8(2)),
            ])),
            Box::new(DataElement::Sequence(vec![Box::new(DataElement::Int8(3))])),
        ]);
        let seq_seq2 = DataElement::Sequence(vec![
            Box::new(DataElement::Sequence(vec![Box::new(DataElement::Int8(1))])),
            Box::new(DataElement::Sequence(vec![
                Box::new(DataElement::Int8(2)),
                Box::new(DataElement::Int8(3)),
            ])),
        ]);
        assert_ne!(calculate_hash(&seq_seq1), calculate_hash(&seq_seq2));
    }

    #[test]
    fn test_service_definition_hash() {
        let def1 = ServiceDefinition {
            service_class_uuids: vec![Uuid::new16(0x1101)],
            ..Default::default()
        };
        let def2 = def1.clone();

        // Same service definition has the same hash
        assert_eq!(calculate_hash(&def1), calculate_hash(&def2));

        // Different UUID has different hash
        let def3 =
            ServiceDefinition { service_class_uuids: vec![Uuid::new16(0x1102)], ..def1.clone() };
        assert_ne!(calculate_hash(&def1), calculate_hash(&def3));

        // Different ProfileDescriptor has different hash (manual hash impl)
        let def4 = ServiceDefinition {
            profile_descriptors: vec![fidl_bredr::ProfileDescriptor {
                profile_id: Some(fidl_bredr::ServiceClassProfileIdentifier::SerialPort),
                major_version: Some(1),
                minor_version: Some(0),
                ..Default::default()
            }],
            ..def1.clone()
        };
        assert_ne!(calculate_hash(&def1), calculate_hash(&def4));

        let mut def5 = def4.clone();
        def5.profile_descriptors[0].minor_version = Some(1);
        assert_ne!(calculate_hash(&def4), calculate_hash(&def5));
    }
}
