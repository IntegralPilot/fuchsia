// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use addr::TargetAddr;

use crate::{FastbootConnectionState, FastbootTargetState, TargetHandle, TargetState};

#[derive(Debug, Hash, Copy, Clone, PartialEq, Eq)]
pub enum FastbootInterface {
    Usb,
    Udp,
    Tcp,
}

/// Represents a target description, e.g. as produced in events within the daemon
#[derive(Debug, Default, Hash, Clone, PartialEq, Eq)]
pub struct Description {
    pub nodename: Option<String>,
    pub addresses: Vec<TargetAddr>,
    pub serial: Option<String>,
    pub ssh_port: Option<u16>,
    pub fastboot_interface: Option<FastbootInterface>,
}

impl From<&TargetHandle> for Description {
    fn from(value: &TargetHandle) -> Self {
        let (addresses, serial) = match &value.state {
            TargetState::Product { addrs, serial } => (addrs.clone(), serial.clone()),
            TargetState::Fastboot(FastbootTargetState { serial_number, connection_state }) => {
                let addresses = match connection_state {
                    FastbootConnectionState::Usb => Vec::<TargetAddr>::new(),
                    FastbootConnectionState::Tcp(addresses)
                    | FastbootConnectionState::Udp(addresses) => {
                        addresses.iter().map(Into::into).collect()
                    }
                };
                (addresses, Some(serial_number.clone()))
            }
            _ => (vec![], None),
        };
        Self { nodename: value.node_name.clone(), addresses, serial, ..Default::default() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FastbootConnectionState, FastbootTargetState, TargetState};
    use addr::{TargetAddr, TargetIpAddr};
    use pretty_assertions::assert_eq;
    use std::net::{Ipv4Addr, SocketAddr};

    #[fuchsia::test]
    fn test_from_target_handle_for_description_product_state() {
        let addr = TargetAddr::from(SocketAddr::from(([192, 168, 1, 1], 22)));
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Product {
                addrs: vec![addr.clone()],
                serial: Some("123".to_string()),
            },
            manual: false,
        };

        let desc = Description::from(&handle);

        assert_eq!(desc.nodename, Some("test-node".to_string()));
        assert_eq!(desc.addresses, vec![addr]);
        assert_eq!(desc.serial, Some("123".to_string()));
    }

    #[fuchsia::test]
    fn test_from_target_handle_for_description_fastboot_usb() {
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: "fb-123".to_string(),
                connection_state: FastbootConnectionState::Usb,
            }),
            manual: false,
        };

        let desc = Description::from(&handle);

        assert_eq!(desc.nodename, Some("test-node".to_string()));
        assert!(desc.addresses.is_empty());
        assert_eq!(desc.serial, Some("fb-123".to_string()));
    }

    #[fuchsia::test]
    fn test_from_target_handle_for_description_fastboot_tcp() {
        let addr = TargetIpAddr::new(Ipv4Addr::new(192, 168, 1, 2).into(), 0, 5554);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Fastboot(FastbootTargetState {
                serial_number: "fb-456".to_string(),
                connection_state: FastbootConnectionState::Tcp(vec![addr.clone()]),
            }),
            manual: false,
        };

        let desc = Description::from(&handle);

        assert_eq!(desc.nodename, Some("test-node".to_string()));
        assert_eq!(desc.addresses, vec![TargetAddr::from(addr)]);
        assert_eq!(desc.serial, Some("fb-456".to_string()));
    }

    #[fuchsia::test]
    fn test_from_target_handle_for_description_unknown_state() {
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };

        let desc = Description::from(&handle);

        assert_eq!(desc.nodename, Some("test-node".to_string()));
        assert!(desc.addresses.is_empty());
        assert_eq!(desc.serial, None);
    }
}
