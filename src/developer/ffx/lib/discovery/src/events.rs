// Copyright 2021 The Fuchsia Authors. All rights 1eserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::{Error, Result};
use addr::{TargetAddr, TargetIpAddr};
use manual_targets::watcher::{ManualTargetEvent, ManualTargetState};
use netext::IsLocalAddr;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt::{self, Display};
use usb_fastboot_discovery::FastbootEvent;
// TODO(colnnelson): Long term it would be nice to have this be pulled into the mDNS library
// so that it can speak our language. Or even have the mdns library not export FIDL structs
// but rather some other well-defined type
use fidl_fuchsia_developer_ffx as ffx;

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub enum FastbootConnectionState {
    Usb,
    Tcp(Vec<TargetIpAddr>),
    Udp(Vec<TargetIpAddr>),
}

impl Display for FastbootConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let res = match self {
            Self::Usb => format!("Usb"),
            Self::Tcp(addr) => format!("Tcp({:?})", addr),
            Self::Udp(addr) => format!("Udp({:?})", addr),
        };
        write!(f, "{}", res)
    }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub struct FastbootTargetState {
    pub serial_number: String,
    pub connection_state: FastbootConnectionState,
}

impl Display for FastbootTargetState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.serial_number, self.connection_state)
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub enum TargetState {
    Unknown,
    Product { addrs: Vec<TargetAddr>, serial: Option<String> },
    Fastboot(FastbootTargetState),
    Zedboot,
}

impl Display for TargetState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let res = match self {
            TargetState::Unknown => "Unknown".to_string(),
            TargetState::Product { addrs: addr, serial } => {
                format!(
                    "Product(addrs: [{}] serial: {:?})",
                    addr.iter().map(|a| format!("{}", a)).collect::<Vec<_>>().join(", "),
                    serial.as_ref().map_or("", |s| s.as_str())
                )
            }
            TargetState::Fastboot(state) => format!("Fastboot({})", state),
            TargetState::Zedboot => "Zedboot".to_string(),
        };
        write!(f, "{}", res)
    }
}

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq, Hash, Clone, Serialize, Deserialize)]
pub struct TargetHandle {
    pub node_name: Option<String>,
    pub state: TargetState,
    pub manual: bool,
}

impl Display for TargetHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = self.node_name.as_ref().map_or("", |n| n.as_str());
        write!(
            f,
            "node: {:?} in state: {}{}",
            name,
            self.state,
            if self.manual { "(manual)" } else { "" }
        )
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub enum TargetEvent {
    /// Indicates a Target has been discovered.
    Added(TargetHandle),
    /// Indicates a Target has been lost.
    Removed(TargetHandle),
}

impl TargetEvent {
    /// Returns the inner [TargetHandle] reference with the lifetime of this object.
    pub fn target_handle(&self) -> &TargetHandle {
        match self {
            Self::Added(h) | Self::Removed(h) => h,
        }
    }

    pub(crate) fn from_usb_event(
        event: usb_driver_api::DeviceEvent,
        node_name: Option<String>,
    ) -> TargetEvent {
        match event {
            usb_driver_api::DeviceEvent::Added { cid, serial } => {
                TargetEvent::Added(TargetHandle {
                    node_name,
                    state: TargetState::Product { addrs: vec![TargetAddr::UsbCtx(cid)], serial },
                    manual: false,
                })
            }
            usb_driver_api::DeviceEvent::Removed { cid } => TargetEvent::Removed(TargetHandle {
                node_name,
                state: TargetState::Product { addrs: vec![TargetAddr::UsbCtx(cid)], serial: None },
                manual: false,
            }),
        }
    }
}

impl TryFrom<ffx::MdnsEventType> for TargetEvent {
    type Error = Error;

    fn try_from(e: ffx::MdnsEventType) -> Result<Self> {
        match e {
            ffx::MdnsEventType::TargetFound(info)
            | ffx::MdnsEventType::TargetRediscovered(info) => {
                Ok(TargetEvent::Added(TargetHandle::try_from(info)?))
            }
            ffx::MdnsEventType::TargetExpired(info) => {
                Ok(TargetEvent::Removed(TargetHandle::try_from(info)?))
            }
            ffx::MdnsEventType::SocketBound(_) => Err(Error::SocketBoundUnsupported),
        }
    }
}

impl TryFrom<emulator_instance::EmulatorTargetAction> for TargetEvent {
    type Error = Error;

    fn try_from(e: emulator_instance::EmulatorTargetAction) -> Result<Self> {
        match e {
            emulator_instance::EmulatorTargetAction::Add(info) => {
                Ok(TargetEvent::Added(TargetHandle::try_from(info)?))
            }
            emulator_instance::EmulatorTargetAction::Remove(info) => {
                Ok(TargetEvent::Removed(TargetHandle::try_from(info)?))
            }
        }
    }
}

impl TryFrom<ffx::TargetInfo> for TargetHandle {
    type Error = Error;

    fn try_from(info: ffx::TargetInfo) -> Result<Self> {
        let addresses = info.addresses.unwrap_or_default();
        // Get the TargetAddrs
        let mut addrs: Vec<_> =
            addresses.into_iter().filter_map(|x| TargetIpAddr::try_from(x).ok()).collect();
        // Sorting them this way put ipv6 above ipv4
        addrs.sort_by(|a, b| b.cmp(a));

        fn assert_non_empty_addrs(addrs: &Vec<TargetIpAddr>) -> Result<()> {
            if addrs.is_empty() {
                return Err(Error::TargetHasNoAddresses);
            }
            Ok(())
        }

        // Let the target state first dictate what state the device is in. If there is an RCS
        // connection, this supercedes anything else and should set the device into a `PRODUCT`
        // state. Other states appear to be a bit more iffy, so just use the fields in `TargetInfo`
        // to settle on the state afterward.
        //
        // It appears it's possible to be in product mode and also have a fastboot interface set at
        // the same time.
        let state = match (info.target_state, info.fastboot_interface) {
            (Some(ffx::TargetState::Product), _) | (_, None) => {
                assert_non_empty_addrs(&addrs)?;
                TargetState::Product {
                    addrs: addrs.into_iter().map(Into::into).collect(),
                    serial: info.serial_number,
                }
            }
            (_, Some(iface)) => {
                let serial_number = info.serial_number.unwrap_or_else(|| "".to_string());
                let connection_state = match iface {
                    ffx::FastbootInterface::Usb => FastbootConnectionState::Usb,
                    ffx::FastbootInterface::Udp => {
                        assert_non_empty_addrs(&addrs)?;
                        FastbootConnectionState::Udp(addrs)
                    }
                    ffx::FastbootInterface::Tcp => {
                        assert_non_empty_addrs(&addrs)?;
                        FastbootConnectionState::Tcp(addrs)
                    }
                };
                TargetState::Fastboot(FastbootTargetState { serial_number, connection_state })
            }
        };

        let manual = info.is_manual.unwrap_or(false);
        Ok(TargetHandle { node_name: info.nodename, state, manual })
    }
}

impl From<FastbootEvent> for TargetEvent {
    fn from(fastboot_event: FastbootEvent) -> Self {
        match fastboot_event {
            FastbootEvent::Discovered(serial) => {
                let handle = TargetHandle {
                    node_name: Some("".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: serial,
                        connection_state: FastbootConnectionState::Usb,
                    }),
                    manual: false,
                };
                TargetEvent::Added(handle)
            }
            FastbootEvent::Lost(serial) => {
                let handle = TargetHandle {
                    node_name: Some("".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: serial,
                        connection_state: FastbootConnectionState::Usb,
                    }),
                    manual: false,
                };
                TargetEvent::Removed(handle)
            }
        }
    }
}

impl From<ManualTargetEvent> for TargetEvent {
    fn from(manual_target_event: ManualTargetEvent) -> Self {
        match manual_target_event {
            ManualTargetEvent::Discovered(manual_target, manual_state) => {
                let state = match manual_state {
                    ManualTargetState::Disconnected => TargetState::Unknown,
                    ManualTargetState::Product => TargetState::Product {
                        addrs: vec![manual_target.addr().into()],
                        serial: None,
                    },
                    ManualTargetState::Fastboot => TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state: FastbootConnectionState::Tcp(vec![
                            manual_target.addr().into(),
                        ]),
                    }),
                };

                let handle = TargetHandle {
                    node_name: Some(manual_target.addr().to_string()),
                    state,
                    manual: true,
                };
                TargetEvent::Added(handle)
            }
            ManualTargetEvent::Lost(manual_target) => {
                let handle = TargetHandle {
                    node_name: Some(manual_target.addr().to_string()),
                    state: TargetState::Unknown,
                    manual: true,
                };
                TargetEvent::Removed(handle)
            }
        }
    }
}

impl From<fastboot_file_discovery::FastbootEvent> for TargetEvent {
    fn from(fastboot_event: fastboot_file_discovery::FastbootEvent) -> Self {
        match fastboot_event {
            fastboot_file_discovery::FastbootEvent::Discovered(device) => {
                let address: TargetIpAddr = device.socket_addr().into();
                let connection_state = match device.mode() {
                    fastboot_file_discovery::FastbootMode::UDP => {
                        FastbootConnectionState::Udp(vec![address])
                    }
                    fastboot_file_discovery::FastbootMode::TCP => {
                        FastbootConnectionState::Tcp(vec![address])
                    }
                };

                let handle = TargetHandle {
                    node_name: None,
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state,
                    }),
                    manual: false,
                };
                TargetEvent::Added(handle)
            }
            fastboot_file_discovery::FastbootEvent::Lost(device) => {
                let address: TargetIpAddr = device.socket_addr().into();
                let connection_state = match device.mode() {
                    fastboot_file_discovery::FastbootMode::UDP => {
                        FastbootConnectionState::Udp(vec![address])
                    }
                    fastboot_file_discovery::FastbootMode::TCP => {
                        FastbootConnectionState::Tcp(vec![address])
                    }
                };
                let handle = TargetHandle {
                    node_name: Some("".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state,
                    }),
                    manual: false,
                };
                TargetEvent::Removed(handle)
            }
        }
    }
}

// For ipv6 addresses, prefer link-local to non-local
fn prefer_local(a: &TargetAddr, b: &TargetAddr) -> Ordering {
    let a_is_local = a.ip().map(|x| x.is_link_local_addr()).unwrap_or(false);
    let b_is_local = b.ip().map(|x| x.is_link_local_addr()).unwrap_or(false);
    match (a_is_local, b_is_local) {
        (true, true) | (false, false) => a.cmp(b),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    }
}

impl From<TargetHandle> for ffx::TargetInfo {
    fn from(handle: TargetHandle) -> Self {
        let (target_state, addresses, serial_number) = match handle.state {
            TargetState::Unknown => (ffx::TargetState::Unknown, None, None),
            TargetState::Product { addrs: target_addrs, serial } => {
                (ffx::TargetState::Product, Some(target_addrs), serial)
            }
            TargetState::Fastboot(fts) => {
                let addresses = match fts.connection_state {
                    FastbootConnectionState::Usb => Some(vec![]),
                    FastbootConnectionState::Tcp(addresses)
                    | FastbootConnectionState::Udp(addresses) => {
                        Some(addresses.into_iter().map(Into::into).collect())
                    }
                };
                (ffx::TargetState::Fastboot, addresses, Some(fts.serial_number))
            }
            TargetState::Zedboot => (ffx::TargetState::Zedboot, None, None),
        };
        let addresses = addresses.map(|mut addrs| {
            addrs.sort_by(|a, b| prefer_local(a, b));
            addrs.into_iter().map(|x| x.into()).collect::<Vec<ffx::TargetAddrInfo>>()
        });
        ffx::TargetInfo {
            nodename: handle.node_name,
            addresses,
            serial_number,
            rcs_state: Some(ffx::RemoteControlState::Unknown),
            target_state: Some(target_state),
            is_manual: Some(handle.manual),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use addr::TargetAddr;
    use manual_targets::watcher::ManualTarget;
    use net_declare::std_socket_addr;
    use pretty_assertions::assert_eq;
    use std::str::FromStr;

    #[test]
    fn test_from_fastbootevent_for_targetevent() -> Result<()> {
        {
            let f = FastbootEvent::Lost("1234".to_string());
            let t = TargetEvent::from(f);
            assert_eq!(
                t,
                TargetEvent::Removed(TargetHandle {
                    node_name: Some("".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "1234".to_string(),
                        connection_state: FastbootConnectionState::Usb,
                    }),
                    manual: false,
                })
            );
        }

        {
            let f = FastbootEvent::Discovered("1234".to_string());
            let t = TargetEvent::from(f);
            assert_eq!(
                t,
                TargetEvent::Added(TargetHandle {
                    node_name: Some("".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "1234".to_string(),
                        connection_state: FastbootConnectionState::Usb,
                    }),
                    manual: false,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn test_from_usb_event_for_targetevent() -> Result<()> {
        let node_name = Some("test_node".to_string());

        {
            let e =
                usb_driver_api::DeviceEvent::Added { cid: 123, serial: Some("1234".to_string()) };
            let t = TargetEvent::from_usb_event(e, node_name.clone());
            assert_eq!(
                t,
                TargetEvent::Added(TargetHandle {
                    node_name: node_name.clone(),
                    state: TargetState::Product {
                        addrs: vec![TargetAddr::UsbCtx(123)],
                        serial: Some("1234".to_string()),
                    },
                    manual: false,
                })
            );
        }

        {
            let e = usb_driver_api::DeviceEvent::Removed { cid: 123 };
            let t = TargetEvent::from_usb_event(e, node_name.clone());
            assert_eq!(
                t,
                TargetEvent::Removed(TargetHandle {
                    node_name: node_name.clone(),
                    state: TargetState::Product {
                        addrs: vec![TargetAddr::UsbCtx(123)],
                        serial: None,
                    },
                    manual: false,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn test_try_from_targetinfo_for_targethandle() -> Result<()> {
        {
            let info: ffx::TargetInfo = Default::default();
            assert!(TargetHandle::try_from(info).is_err());
        }
        {
            let info = ffx::TargetInfo { nodename: Some("foo".to_string()), ..Default::default() };
            assert!(TargetHandle::try_from(info).is_err());
        }
        {
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![]),
                ..Default::default()
            };
            assert!(TargetHandle::try_from(info).is_err());
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                ..Default::default()
            };
            assert_eq!(
                TargetHandle::try_from(info)?,
                TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product { addrs: vec![addr], serial: None },
                    manual: false,
                }
            );
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetIpAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                fastboot_interface: Some(ffx::FastbootInterface::Udp),
                ..Default::default()
            };
            assert_eq!(
                TargetHandle::try_from(info)?,
                TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state: FastbootConnectionState::Udp(vec![addr])
                    }),
                    manual: false,
                }
            );
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetIpAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                fastboot_interface: Some(ffx::FastbootInterface::Tcp),
                ..Default::default()
            };
            assert_eq!(
                TargetHandle::try_from(info)?,
                TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state: FastbootConnectionState::Tcp(vec![addr])
                    }),
                    manual: false,
                }
            );
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                fastboot_interface: Some(ffx::FastbootInterface::Usb),
                ..Default::default()
            };
            assert_eq!(
                TargetHandle::try_from(info)?,
                TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state: FastbootConnectionState::Usb
                    }),
                    manual: false,
                }
            );
        }
        Ok(())
    }

    #[test]
    fn test_product_with_fastboot_info_returns_product() {
        // This test may be for behavior that isn't intended to be supported. This is originally
        // to address (b/438248466) but this may be covering up a more specific issue.

        let socket = std_socket_addr!("127.0.0.1:8080");
        let addr = TargetIpAddr::from(socket);
        let target_addr = TargetAddr::from(socket);
        let addr_info: ffx::TargetAddrInfo = addr.into();
        let addresses = Some(vec![addr_info]);
        let nodename = Some("foo".to_string());
        let serial_number = Some("serial123".to_string());

        // Case 1: target_state is Product, fastboot_interface is None
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: addresses.clone(),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Product),
            fastboot_interface: None,
            ..Default::default()
        };
        assert_eq!(
            TargetHandle::try_from(info).unwrap(),
            TargetHandle {
                node_name: nodename.clone(),
                state: TargetState::Product {
                    addrs: vec![target_addr.clone()],
                    serial: serial_number.clone()
                },
                manual: false,
            }
        );

        // Case 2: target_state is Product, fastboot_interface is Usb
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: addresses.clone(),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Product),
            fastboot_interface: Some(ffx::FastbootInterface::Usb),
            ..Default::default()
        };
        assert_eq!(
            TargetHandle::try_from(info).unwrap(),
            TargetHandle {
                node_name: nodename.clone(),
                state: TargetState::Product {
                    addrs: vec![target_addr.clone()],
                    serial: serial_number.clone()
                },
                manual: false,
            }
        );

        // Case 3: target_state is Product, fastboot_interface is Tcp
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: addresses.clone(),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Product),
            fastboot_interface: Some(ffx::FastbootInterface::Tcp),
            ..Default::default()
        };
        assert_eq!(
            TargetHandle::try_from(info).unwrap(),
            TargetHandle {
                node_name: nodename.clone(),
                state: TargetState::Product {
                    addrs: vec![target_addr.clone()],
                    serial: serial_number.clone()
                },
                manual: false,
            }
        );

        // Case 4: target_state is Unknown, fastboot_interface is None
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: addresses.clone(),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Unknown),
            fastboot_interface: None,
            ..Default::default()
        };
        assert_eq!(
            TargetHandle::try_from(info).unwrap(),
            TargetHandle {
                node_name: nodename.clone(),
                state: TargetState::Product {
                    addrs: vec![target_addr.clone()],
                    serial: serial_number.clone()
                },
                manual: false,
            }
        );

        // Case 5: target_state is Unknown, fastboot_interface is Usb
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: addresses.clone(),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Unknown),
            fastboot_interface: Some(ffx::FastbootInterface::Usb),
            ..Default::default()
        };
        assert_eq!(
            TargetHandle::try_from(info).unwrap(),
            TargetHandle {
                node_name: nodename.clone(),
                state: TargetState::Fastboot(FastbootTargetState {
                    serial_number: serial_number.clone().unwrap(),
                    connection_state: FastbootConnectionState::Usb
                }),
                manual: false,
            }
        );

        // Case 6: target_state is Zedboot, fastboot_interface is Tcp
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: addresses.clone(),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Zedboot),
            fastboot_interface: Some(ffx::FastbootInterface::Tcp),
            ..Default::default()
        };
        assert_eq!(
            TargetHandle::try_from(info).unwrap(),
            TargetHandle {
                node_name: nodename.clone(),
                state: TargetState::Fastboot(FastbootTargetState {
                    serial_number: serial_number.clone().unwrap(),
                    connection_state: FastbootConnectionState::Tcp(vec![addr.clone()])
                }),
                manual: false,
            }
        );

        // Case 7: Product state requires addresses
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: Some(vec![]),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Product),
            fastboot_interface: None,
            ..Default::default()
        };
        assert!(TargetHandle::try_from(info).is_err());

        // Case 8: Fastboot Tcp/Udp requires addresses
        let info = ffx::TargetInfo {
            nodename: nodename.clone(),
            addresses: Some(vec![]),
            serial_number: serial_number.clone(),
            target_state: Some(ffx::TargetState::Unknown),
            fastboot_interface: Some(ffx::FastbootInterface::Tcp),
            ..Default::default()
        };
        assert!(TargetHandle::try_from(info).is_err());
    }

    #[test]
    fn test_from_mdnseventtype_for_targetevent() -> Result<()> {
        {
            //SocketBound is not supported
            let mdns_event = ffx::MdnsEventType::SocketBound(Default::default());
            assert!(TargetEvent::try_from(mdns_event).is_err());
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                ..Default::default()
            };
            let mdns_event = ffx::MdnsEventType::TargetFound(info);
            assert_eq!(
                TargetEvent::try_from(mdns_event)?,
                TargetEvent::Added(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product { addrs: vec![addr], serial: None },
                    manual: false,
                })
            );
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                serial_number: Some("12348890".to_string()),
                ..Default::default()
            };
            let mdns_event = ffx::MdnsEventType::TargetFound(info);
            assert_eq!(
                TargetEvent::try_from(mdns_event)?,
                TargetEvent::Added(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product {
                        addrs: vec![addr],
                        serial: Some("12348890".to_string())
                    },
                    manual: false,
                })
            );
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                ..Default::default()
            };
            let mdns_event = ffx::MdnsEventType::TargetRediscovered(info);
            assert_eq!(
                TargetEvent::try_from(mdns_event)?,
                TargetEvent::Added(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product { addrs: vec![addr], serial: None },
                    manual: false,
                })
            );
        }
        {
            let socket = std_socket_addr!("127.0.0.1:8080");
            let addr = TargetAddr::from(socket);
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                ..Default::default()
            };
            let mdns_event = ffx::MdnsEventType::TargetExpired(info);
            assert_eq!(
                TargetEvent::try_from(mdns_event)?,
                TargetEvent::Removed(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product { addrs: vec![addr], serial: None },
                    manual: false,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn test_from_emulatoreventtype_for_targetevent() -> Result<()> {
        let addr = TargetAddr::from_str("127.0.0.1:8080").unwrap();
        {
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                ..Default::default()
            };
            let emulator_event = emulator_instance::EmulatorTargetAction::Add(info);
            assert_eq!(
                TargetEvent::try_from(emulator_event)?,
                TargetEvent::Added(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product { addrs: vec![addr], serial: None },
                    manual: false,
                })
            );
        }
        {
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                ..Default::default()
            };
            let emulator_event = emulator_instance::EmulatorTargetAction::Remove(info);
            assert_eq!(
                TargetEvent::try_from(emulator_event)?,
                TargetEvent::Removed(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product { addrs: vec![addr], serial: None },
                    manual: false,
                })
            );
        }
        {
            let addr_info: ffx::TargetAddrInfo = addr.into();
            let info = ffx::TargetInfo {
                nodename: Some("foo".to_string()),
                addresses: Some(vec![addr_info]),
                serial_number: Some("EM-9876".to_string()),
                ..Default::default()
            };
            let emulator_event = emulator_instance::EmulatorTargetAction::Add(info);
            assert_eq!(
                TargetEvent::try_from(emulator_event)?,
                TargetEvent::Added(TargetHandle {
                    node_name: Some("foo".to_string()),
                    state: TargetState::Product {
                        addrs: vec![addr],
                        serial: Some("EM-9876".to_string()),
                    },
                    manual: false,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn test_from_manual_target_event_for_target_event() -> Result<()> {
        {
            let addr = std_socket_addr!("127.0.0.1:8080");
            let lifetime = None;
            let manual_target_event = ManualTargetEvent::Discovered(
                ManualTarget::new(addr, lifetime),
                ManualTargetState::Product,
            );
            assert_eq!(
                TargetEvent::from(manual_target_event),
                TargetEvent::Added(TargetHandle {
                    node_name: Some("127.0.0.1:8080".to_string()),
                    state: TargetState::Product { addrs: vec![addr.into()], serial: None },
                    manual: true,
                })
            );
        }
        {
            let addr = std_socket_addr!("[::1]:8032");
            let lifetime = None;
            let manual_target_event = ManualTargetEvent::Discovered(
                ManualTarget::new(addr, lifetime),
                ManualTargetState::Product,
            );
            assert_eq!(
                TargetEvent::from(manual_target_event),
                TargetEvent::Added(TargetHandle {
                    node_name: Some("[::1]:8032".to_string()),
                    state: TargetState::Product { addrs: vec![addr.into()], serial: None },
                    manual: true,
                })
            );
        }
        {
            let addr = std_socket_addr!("127.0.0.1:8080");
            let lifetime = None;
            let manual_target_event = ManualTargetEvent::Discovered(
                ManualTarget::new(addr, lifetime),
                ManualTargetState::Fastboot,
            );
            assert_eq!(
                TargetEvent::from(manual_target_event),
                TargetEvent::Added(TargetHandle {
                    node_name: Some("127.0.0.1:8080".to_string()),
                    state: TargetState::Fastboot(FastbootTargetState {
                        serial_number: "".to_string(),
                        connection_state: FastbootConnectionState::Tcp(vec![addr.into()])
                    }),
                    manual: true,
                })
            );
        }
        {
            let addr = std_socket_addr!("127.0.0.1:8080");
            let lifetime = None;
            let manual_target_event = ManualTargetEvent::Lost(ManualTarget::new(addr, lifetime));
            assert_eq!(
                TargetEvent::from(manual_target_event),
                TargetEvent::Removed(TargetHandle {
                    node_name: Some("127.0.0.1:8080".to_string()),
                    state: TargetState::Unknown,
                    manual: true,
                })
            );
        }
        Ok(())
    }

    #[fuchsia::test]
    fn test_address_sorting() {
        let non_link_local_addr: TargetAddr = "[2001:db8::1]:0".parse().unwrap();
        let link_local_addr: TargetAddr = "[fe80::1]:0".parse().unwrap();

        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Product {
                addrs: vec![non_link_local_addr.clone(), link_local_addr.clone()],
                serial: None,
            },
            manual: false,
        };

        let info: ffx::TargetInfo = handle.into();

        let addrs = info.addresses.unwrap();
        assert_eq!(addrs.len(), 2);
        let addrs: Vec<TargetAddr> = addrs.into_iter().map(|a| a.into()).collect();
        // The link-local address should come first.
        assert_eq!(addrs[0], link_local_addr);
        assert_eq!(addrs[1], non_link_local_addr);
    }
}
