// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;

use anyhow::Error;
use fuchsia_component::client::connect_to_protocol;
use openthread::ot::{Ip4Cidr, Nat64State};
use std::cell::{Cell, RefCell};

const NAT64_IP4CIDR_DEFAULT_SUBNET: [u8; 4] = [240, 16, 0, 0];
const NAT64_IP4CIDR_DEFAULT_PREFIX_LEN: u8 = 24;

#[derive(Debug)]
pub struct Nat64 {
    pub lowpan_nat_ctrl: RefCell<Option<fidl_fuchsia_net_masquerade::ControlProxy>>,
    pub active_cidr_addr: Cell<Option<Ip4Cidr>>,
}

pub fn get_first_usable_addr_in_subnet(subnet: [u8; 4], prefix_len: u8) -> [u8; 4] {
    // Stop the lowpan-ot-driver for earier debugging
    assert!(prefix_len <= 32, "IP subnet prefix is too long");

    if prefix_len == 32 {
        return subnet;
    }

    let mask: u32 = !((1u64 << (32 - prefix_len)) - 1) as u32;

    // Mask out the subnet bits and then add one to represent the first usable address
    ((u32::from_be_bytes(subnet) & mask) + 1).to_be_bytes()
}

fn is_same_ip4_cidr(addr1: Option<Ip4Cidr>, addr2: Option<Ip4Cidr>) -> bool {
    if addr1.is_none() || addr2.is_none() {
        return false;
    }
    addr1.unwrap() == addr2.unwrap()
}

impl<OT, NI, BI> OtDriver<OT, NI, BI>
where
    OT: Send + ot::InstanceInterface,
    NI: NetworkInterface,
    BI: BackboneInterface,
{
    pub fn on_nat64_translator_state_changed(&self) {
        let driver_state = self.driver_state.lock();

        if let Some(wlan_client_nicid) = self.backbone_if.get_nicid() {
            if let Err(e) = driver_state.nat64.on_nat64_translator_state_changed(
                &driver_state.ot_instance,
                &self.net_if,
                wlan_client_nicid.into(),
            ) {
                warn!("failed to process NAT64 changes: {:?}, disabling NAT64 in OpenThread", e);
                driver_state.ot_instance.nat64_set_enabled(false);
            }
        }
    }
}

impl Nat64 {
    pub fn new() -> Self {
        Nat64 { lowpan_nat_ctrl: RefCell::new(None), active_cidr_addr: Cell::new(None) }
    }

    pub fn on_nat64_translator_state_changed<OT, NI>(
        &self,
        ot_instance: &OT,
        net_if: &NI,
        wlan_client_nicid: u64,
    ) -> Result<(), Error>
    where
        OT: Send + ot::InstanceInterface,
        NI: NetworkInterface,
    {
        let translator_cidr;
        if let Ok(cidr) = ot_instance.nat64_get_cidr() {
            translator_cidr = Some(cidr);
        } else {
            // Skip if NAT64 translator has not been configured with a CIDR
            return Ok(());
        }

        let active_cidr = self.active_cidr_addr.get();
        if !is_same_ip4_cidr(translator_cidr, active_cidr) {
            // Someone sets a new CIDR for NAT64
            if let Some(addr) = active_cidr {
                self.delete_route(net_if, addr.get_address_bytes(), addr.get_length());
            }
            self.active_cidr_addr.set(translator_cidr);
            info!("NAT64: CIDR updated to {:?}", self.active_cidr_addr.get());
        }

        let active_cidr = self.active_cidr_addr.get();
        if ot_instance.nat64_get_translator_state() == Nat64State::Active {
            if let Some(addr) = active_cidr {
                self.add_route(net_if, addr.get_address_bytes(), addr.get_length());
                self.enable_masquerade(
                    wlan_client_nicid,
                    addr.get_address_bytes(),
                    addr.get_length(),
                )?;
                info!("NAT64: adding route for NAT64");
            } else {
                panic!("NAT64: no active CIDR recorded in lowpan");
            }
        } else {
            if let Some(addr) = active_cidr {
                self.delete_route(net_if, addr.get_address_bytes(), addr.get_length());
                self.disable_masquerade();
            }
            info!("NAT64: deleting route for NAT64");
        }

        Ok(())
    }

    fn enable_masquerade(
        &self,
        wlan_client_nicid: u64,
        cidr_addr: [u8; 4],
        cidr_addr_prefix_len: u8,
    ) -> Result<(), Error> {
        let masquerade_proxy = connect_to_protocol::<fidl_fuchsia_net_masquerade::FactoryMarker>()
            .context("error connecting to masquerade factory")?;
        let config = fidl_fuchsia_net_masquerade::ControlConfig {
            // The network to be masqueraded.
            src_subnet: fidl_fuchsia_net::Subnet {
                addr: fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                    addr: cidr_addr,
                }),
                prefix_len: cidr_addr_prefix_len,
            },
            // The interface through which to masquerade.[Wlan Client]
            output_interface: wlan_client_nicid,
        };

        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fidl_fuchsia_net_masquerade::ControlMarker>();

        // Don't handle the error here. If failed, control_proxy conversion will fail.
        let _ = masquerade_proxy.create(&config, server_end).now_or_never();

        let control_proxy = client_end.into_proxy();

        control_proxy.set_enabled(true).now_or_never();

        self.lowpan_nat_ctrl.replace(Some(control_proxy));

        info!("NAT64: masquerade configured with {:?}", config);

        Ok(())
    }

    fn disable_masquerade(&self) {
        self.lowpan_nat_ctrl.replace(None);
        info!("NAT64: masquerade config disabled");
    }

    fn add_route<NI: NetworkInterface>(
        &self,
        net_if: &NI,
        cidr_addr: [u8; 4],
        cidr_addr_prefix_len: u8,
    ) {
        let addr = fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                addr: get_first_usable_addr_in_subnet(cidr_addr, cidr_addr_prefix_len),
            }),
            prefix_len: cidr_addr_prefix_len,
        };
        if let Err(err) = net_if.add_forwarding_entry(addr).ignore_already_exists() {
            warn!("Unable to add NAT64 route `{:?}` to lowpan interface: {:?}", addr, err);
        }
    }

    fn delete_route<NI: NetworkInterface>(
        &self,
        net_if: &NI,
        cidr_addr: [u8; 4],
        cidr_addr_prefix_len: u8,
    ) {
        let addr = fidl_fuchsia_net::Subnet {
            addr: fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                addr: get_first_usable_addr_in_subnet(cidr_addr, cidr_addr_prefix_len),
            }),
            prefix_len: cidr_addr_prefix_len,
        };
        if let Err(err) = net_if.remove_forwarding_entry(addr).ignore_not_found() {
            warn!("Unable to delete NAT64 route `{:?}` to lowpan interface: {:?}", addr, err);
        }
    }

    pub fn get_default_nat64_cidr() -> Ip4Cidr {
        Ip4Cidr::new(NAT64_IP4CIDR_DEFAULT_SUBNET, NAT64_IP4CIDR_DEFAULT_PREFIX_LEN)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_get_first_usable_addr_in_subnet() {
        assert_eq!(
            get_first_usable_addr_in_subnet([0xff, 0xff, 0xff, 0xff], 32),
            [0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            get_first_usable_addr_in_subnet([0xff, 0xff, 0xff, 0xff], 31),
            [0xff, 0xff, 0xff, 0xff]
        );
        assert_eq!(
            get_first_usable_addr_in_subnet([0xff, 0xff, 0xff, 0xff], 28),
            [0xff, 0xff, 0xff, 0xf1]
        );
        assert_eq!(
            get_first_usable_addr_in_subnet([0xff, 0xff, 0xff, 0xff], 24),
            [0xff, 0xff, 0xff, 0x1]
        );
        assert_eq!(
            get_first_usable_addr_in_subnet([0xff, 0xff, 0xff, 0xff], 8),
            [0xff, 0x0, 0x0, 0x1]
        );
        assert_eq!(
            get_first_usable_addr_in_subnet([0xff, 0xff, 0xff, 0xff], 0),
            [0x0, 0x0, 0x0, 0x1]
        );
        assert_eq!(
            get_first_usable_addr_in_subnet(
                NAT64_IP4CIDR_DEFAULT_SUBNET,
                NAT64_IP4CIDR_DEFAULT_PREFIX_LEN
            ),
            [240, 16, 0, 1]
        );
    }
}
