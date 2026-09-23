// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_sys as fsys;
#[cfg(target_os = "fuchsia")]
use fuchsia_inspect::{self as inspect, Property};
use std::fmt;

use crate::error::Error;
#[cfg(target_os = "fuchsia")]
use crate::inspect::{DebugExt, InspectData, Inspectable, IsInspectable, ToProperty};
use crate::types::{Address, HostId, addresses_to_custom_string};

/// `HostInfo` contains informational parameters and state for a bt-host device.
#[derive(Clone, Debug, PartialEq)]
pub struct HostInfo {
    /// Uniquely identifies a host on the current system.
    pub id: HostId,

    /// The Bluetooth technologies that are supported by this adapter.
    pub technology: fsys::TechnologyType,

    /// The known Classic and LE addresses associated with this Host.
    /// This is guaranteed to be nonempty. The Public Address is always first.
    pub addresses: Vec<Address>,

    /// Indicates whether or not this is the active host. The system has one active host which
    /// handles all Bluetooth procedures.
    pub active: bool,

    /// The local name of this host. This is the name that is visible to other devices when this
    /// host is in the discoverable mode. Not present if the local device name is unknown.
    pub local_name: Option<String>,

    /// Whether or not the local adapter is currently discoverable over BR/EDR and
    /// LE physical channels.
    pub discoverable: bool,

    /// Whether or not device discovery is currently being performed.
    pub discovering: bool,
}

impl TryFrom<&fsys::HostInfo> for HostInfo {
    type Error = Error;
    fn try_from(src: &fsys::HostInfo) -> Result<HostInfo, Self::Error> {
        let addresses =
            src.addresses.as_ref().ok_or_else(|| Error::missing("HostInfo.addresses"))?;
        if addresses.is_empty() {
            return Err(Error::conversion("HostInfo.addresses must be nonempty"));
        }
        let addresses = addresses.iter().map(Into::into).collect();
        Ok(HostInfo {
            id: HostId::from(src.id.ok_or_else(|| Error::missing("HostInfo.id"))?),
            technology: src.technology.ok_or_else(|| Error::missing("HostInfo.technology"))?,
            addresses,
            active: src.active.unwrap_or_default(),
            local_name: src.local_name.clone(),
            discoverable: src.discoverable.unwrap_or_default(),
            discovering: src.discovering.unwrap_or_default(),
        })
    }
}

impl TryFrom<fsys::HostInfo> for HostInfo {
    type Error = Error;
    fn try_from(src: fsys::HostInfo) -> Result<HostInfo, Self::Error> {
        HostInfo::try_from(&src)
    }
}

impl From<&HostInfo> for fsys::HostInfo {
    fn from(src: &HostInfo) -> fsys::HostInfo {
        fsys::HostInfo {
            id: Some(src.id.into()),
            technology: Some(src.technology),
            active: Some(src.active),
            local_name: src.local_name.clone(),
            discoverable: Some(src.discoverable),
            discovering: Some(src.discovering),
            addresses: Some(src.addresses.iter().map(Into::into).collect()),
            ..Default::default()
        }
    }
}

impl From<HostInfo> for fsys::HostInfo {
    fn from(src: HostInfo) -> fsys::HostInfo {
        fsys::HostInfo::from(&src)
    }
}

impl fmt::Display for HostInfo {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(fmt, "HostInfo:")?;
        writeln!(fmt, "\tidentifier:\t{}", self.id)?;
        writeln!(fmt, "\taddresses:\t{}", addresses_to_custom_string(&self.addresses, "\n\t\t\t"))?;
        writeln!(fmt, "\tactive:\t{}", self.active)?;
        writeln!(fmt, "\ttechnology:\t{:?}", self.technology)?;
        writeln!(fmt, "\tlocal name:\t{}", self.local_name.as_deref().unwrap_or("(unknown)"))?;
        writeln!(fmt, "\tdiscoverable:\t{}", self.discoverable)?;
        writeln!(fmt, "\tdiscovering:\t{}", self.discovering)
    }
}

#[cfg(target_os = "fuchsia")]
impl Inspectable<HostInfo> {
    pub fn update(&mut self, info: HostInfo) {
        self.inspect.update(&info);
        self.inner = info;
    }
}

#[cfg(target_os = "fuchsia")]
pub struct HostInfoInspect {
    _inspect: inspect::Node,
    identifier: inspect::UintProperty,
    technology: inspect::StringProperty,
    active: inspect::UintProperty,
    discoverable: inspect::UintProperty,
    discovering: inspect::UintProperty,
}

#[cfg(target_os = "fuchsia")]
impl HostInfoInspect {
    fn update(&mut self, info: &HostInfo) {
        self.identifier.set(info.id.0);
        self.technology.set(&info.technology.debug());
        self.active.set(info.active.to_property());
        self.discoverable.set(info.discoverable.to_property());
        self.discovering.set(info.discovering.to_property());
    }
}

#[cfg(target_os = "fuchsia")]
impl IsInspectable for HostInfo {
    type I = HostInfoInspect;
}

#[cfg(target_os = "fuchsia")]
impl InspectData<HostInfo> for HostInfoInspect {
    fn new(info: &HostInfo, inspect: inspect::Node) -> HostInfoInspect {
        HostInfoInspect {
            identifier: inspect.create_uint("identifier", info.id.0),
            technology: inspect.create_string("technology", info.technology.debug()),
            active: inspect.create_uint("active", info.active.to_property()),
            discoverable: inspect.create_uint("discoverable", info.discoverable.to_property()),
            discovering: inspect.create_uint("discovering", info.discovering.to_property()),
            _inspect: inspect,
        }
    }
}

/// Example Bluetooth host for testing.
pub fn example_host(id: HostId, active: bool, discoverable: bool) -> fsys::HostInfo {
    fsys::HostInfo {
        id: Some(id.into()),
        technology: Some(fsys::TechnologyType::LowEnergy),
        active: Some(active),
        local_name: Some("fuchsia123".to_string()),
        discoverable: Some(discoverable),
        discovering: Some(true),
        addresses: Some(vec![Address::Public([1, 2, 3, 4, 5, 6]).into()]),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use diagnostics_assertions::assert_data_tree;
    use fidl_fuchsia_bluetooth as fbt;
    use fuchsia_inspect as inspect;

    #[test]
    fn from_fidl_id_not_present() {
        let info = HostInfo::try_from(fsys::HostInfo::default());
        assert!(info.is_err());
    }

    #[test]
    fn from_fidl_technology_not_present() {
        let info = fsys::HostInfo { id: Some(fbt::HostId { value: 1 }), ..Default::default() };
        let info = HostInfo::try_from(info);
        assert!(info.is_err());
    }

    #[test]
    fn from_fidl_addresses_not_present() {
        let info = fsys::HostInfo {
            id: Some(fbt::HostId { value: 1 }),
            technology: Some(fsys::TechnologyType::LowEnergy),
            ..Default::default()
        };
        let info = HostInfo::try_from(info);
        assert!(info.is_err());
    }

    #[test]
    fn from_fidl_addresses_is_empty() {
        let info = fsys::HostInfo {
            id: Some(fbt::HostId { value: 1 }),
            technology: Some(fsys::TechnologyType::LowEnergy),
            addresses: Some(vec![]),
            ..Default::default()
        };
        let info = HostInfo::try_from(info);
        assert!(info.is_err());
    }

    #[test]
    fn from_fidl_optional_fields_not_present() {
        let info = fsys::HostInfo {
            id: Some(fbt::HostId { value: 1 }),
            technology: Some(fsys::TechnologyType::LowEnergy),
            addresses: Some(vec![fbt::Address {
                type_: fbt::AddressType::Public,
                bytes: [1, 2, 3, 4, 5, 6],
            }]),
            ..Default::default()
        };
        let expected = HostInfo {
            id: HostId(1),
            technology: fsys::TechnologyType::LowEnergy,
            addresses: vec![Address::Public([1, 2, 3, 4, 5, 6])],
            active: false,
            local_name: None,
            discoverable: false,
            discovering: false,
        };

        let info = HostInfo::try_from(info).expect("expected successful conversion");
        assert_eq!(expected, info);
    }

    #[test]
    fn from_fidl_optional_fields_present() {
        let info = fsys::HostInfo {
            id: Some(fbt::HostId { value: 1 }),
            technology: Some(fsys::TechnologyType::LowEnergy),
            active: Some(true),
            local_name: Some("name".to_string()),
            discoverable: Some(false),
            discovering: Some(true),
            addresses: Some(vec![fbt::Address {
                type_: fbt::AddressType::Public,
                bytes: [1, 2, 3, 4, 5, 6],
            }]),
            ..Default::default()
        };
        let expected = HostInfo {
            id: HostId(1),
            technology: fsys::TechnologyType::LowEnergy,
            addresses: vec![Address::Public([1, 2, 3, 4, 5, 6])],
            active: true,
            local_name: Some("name".to_string()),
            discoverable: false,
            discovering: true,
        };

        let info = HostInfo::try_from(info).expect("expected successful conversion");
        assert_eq!(expected, info);
    }

    #[test]
    fn to_fidl() {
        let info = HostInfo {
            id: HostId(1),
            technology: fsys::TechnologyType::LowEnergy,
            addresses: vec![Address::Public([1, 2, 3, 4, 5, 6])],
            active: false,
            local_name: Some("name".to_string()),
            discoverable: false,
            discovering: false,
        };
        let expected = fsys::HostInfo {
            id: Some(fbt::HostId { value: 1 }),
            technology: Some(fsys::TechnologyType::LowEnergy),
            active: Some(false),
            local_name: Some("name".to_string()),
            discoverable: Some(false),
            discovering: Some(false),
            addresses: Some(vec![fbt::Address {
                type_: fbt::AddressType::Public,
                bytes: [1, 2, 3, 4, 5, 6],
            }]),
            ..Default::default()
        };

        assert_eq!(expected, info.into());
    }

    #[fuchsia::test]
    async fn inspect() {
        let inspector = inspect::Inspector::default();
        let node = inspector.root().create_child("info");
        let info = HostInfo {
            id: HostId(1),
            technology: fsys::TechnologyType::LowEnergy,
            addresses: vec![Address::Public([1, 2, 3, 4, 5, 6])],
            active: false,
            local_name: Some("name".to_string()),
            discoverable: false,
            discovering: true,
        };
        let mut info = Inspectable::new(info, node);
        assert_data_tree!(inspector, root: {
            info: contains {
                identifier: 1u64,
                technology: "LowEnergy",
                active: 0u64,
                discoverable: 0u64,
                discovering: 1u64
            }
        });

        info.update(HostInfo {
            id: HostId(1),
            technology: fsys::TechnologyType::LowEnergy,
            addresses: vec![Address::Public([1, 2, 3, 4, 5, 6])],
            active: true,
            local_name: Some("foo".to_string()),
            discoverable: true,
            discovering: true,
        });
        assert_data_tree!(inspector, root: {
            info: contains {
                identifier: 1u64,
                technology: "LowEnergy",
                active: 1u64,
                discoverable: 1u64,
                discovering: 1u64
            }
        });
    }
}
