// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod interface_aware_logger;
pub mod link_properties_state;

use fidl_fuchsia_net_interfaces_ext::PortClass;

// TODO(https://fxbug.dev/432299715): Share this definition with netcfg.
//
// The classification of the interface. This is not necessarily the same
// as the PortClass of the interface.
#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
pub enum InterfaceType {
    Ethernet,
    WlanClient,
    WlanAp,
    Blackhole,
    Bluetooth,
    Virtual,
}

impl std::fmt::Display for InterfaceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = format!("{:?}", self);
        write!(f, "{}", name.to_lowercase())
    }
}

// TODO(https://fxbug.dev/432301507): Read this from shared configuration.
// TODO(https://fxbug.dev/432298588): Add Id and Name as alternate groupings.
//
// The specifier for how the time series are initialized. For example, if Type
// is specified with Ethernet and WlanClient, then there will be a separate
// time series for Ethernet and WlanClient updates, further broken down by
// v4 and v6 protocols.
pub enum InterfaceTimeSeriesGrouping {
    Type(Vec<InterfaceType>),
}

// TODO(https://fxbug.dev/432298588): Add Id and Name as alternate groupings.
//
// The identifier for the interface. Used to determine which time series should
// have updates applied.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum InterfaceIdentifier {
    // An interface is expected to only have a single type.
    Type(InterfaceType),
}

impl std::fmt::Display for InterfaceIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Type(ty) => format!("TYPE_{ty}"),
        };
        write!(f, "{}", name)
    }
}

pub fn identifiers_from_port_class(port_class: PortClass) -> Vec<InterfaceIdentifier> {
    match port_class {
        PortClass::Ethernet => vec![InterfaceType::Ethernet],
        PortClass::WlanClient => vec![InterfaceType::WlanClient],
        PortClass::WlanAp => vec![InterfaceType::WlanAp],
        PortClass::Blackhole => vec![InterfaceType::Blackhole],
        PortClass::Loopback => vec![InterfaceType::Bluetooth, InterfaceType::Virtual],
        PortClass::Virtual | PortClass::Ppp | PortClass::Bridge | PortClass::Lowpan => {
            vec![InterfaceType::Virtual]
        }
    }
    .into_iter()
    .map(|port_class| InterfaceIdentifier::Type(port_class))
    .collect()
}
