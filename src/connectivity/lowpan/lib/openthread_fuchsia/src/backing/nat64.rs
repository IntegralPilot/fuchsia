// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::Platform;
use crate::ot::{InfraInterface, Ip4Address, Ip6Address, Ip6Prefix};
use anyhow::{Error, Result};
use fuchsia_sync::Mutex;
use futures::future::BoxFuture;
use openthread_sys::*;
use std::task::{Context, Poll};

pub(crate) struct Nat64Instance {
    nat64_prefix_req_sender: Mutex<
        fmpsc::UnboundedSender<Mutex<Option<BoxFuture<'static, (ot::NetifIndex, Ip6Prefix)>>>>,
    >,
}

const K_VALID_NAT64_PREFIX_LENGTHS: [u8; 6] = [96, 64, 56, 48, 40, 32];
const K_WELL_KNOWN_IPV4_ONLY_ADDRESS1: Ip4Address = Ip4Address::new(192, 0, 0, 170);
const K_WELL_KNOWN_IPV4_ONLY_ADDRESS2: Ip4Address = Ip4Address::new(192, 0, 0, 171);

pub(crate) struct Nat64PlatformInstance {
    nat64_prefix_req_receiver:
        fmpsc::UnboundedReceiver<Mutex<Option<BoxFuture<'static, (ot::NetifIndex, Ip6Prefix)>>>>,
    nat64_pending_fut: Mutex<Option<BoxFuture<'static, (ot::NetifIndex, Ip6Prefix)>>>,
}

impl Nat64Instance {
    pub fn new(
        nat64_prefix_req_sender: fmpsc::UnboundedSender<
            Mutex<Option<BoxFuture<'static, (ot::NetifIndex, Ip6Prefix)>>>,
        >,
    ) -> Nat64Instance {
        Nat64Instance { nat64_prefix_req_sender: Mutex::new(nat64_prefix_req_sender) }
    }
}

impl Nat64PlatformInstance {
    pub fn new(
        nat64_prefix_req_receiver: fmpsc::UnboundedReceiver<
            Mutex<Option<BoxFuture<'static, (ot::NetifIndex, Ip6Prefix)>>>,
        >,
    ) -> Nat64PlatformInstance {
        Nat64PlatformInstance { nat64_prefix_req_receiver, nat64_pending_fut: Mutex::new(None) }
    }
}

// The prefix length must be 32, 40, 48, 56, 64, 96. IPv4 bytes are added
// after the prefix, skipping over the bits 64 to 71 (byte at index `8`)
// which must be set to zero. The suffix is set to zero (per RFC 6052).
//
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |PL| 0-------------32--40--48--56--64--72--80--88--96--104---------|
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |32|     prefix    |v4(32)         | u | suffix                    |
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |40|     prefix        |v4(24)     | u |(8)| suffix                |
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |48|     prefix            |v4(16) | u | (16)  | suffix            |
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |56|     prefix                |(8)| u |  v4(24)   | suffix        |
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |64|     prefix                    | u |   v4(32)      | suffix    |
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
//    |96|     prefix                                    |    v4(32)     |
//    +--+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+---+
fn extract_ipv4_addr_from_ipv6(
    ip6_addr: std::net::Ipv6Addr,
    prefix_length: u8,
) -> std::net::Ipv4Addr {
    // Check the prefix_length is valid. If not, don't extract
    if !K_VALID_NAT64_PREFIX_LENGTHS.contains(&prefix_length) {
        return std::net::Ipv4Addr::new(0, 0, 0, 0);
    }

    let ip6_vec = ip6_addr.octets();

    // get the first idx
    let mut ip6_idx = (prefix_length / 8) as usize;
    let mut ip4_idx: usize = 0;

    let mut res: [u8; 4] = [0; 4];

    while ip4_idx < 4 {
        if ip6_idx == 8 {
            ip6_idx += 1;
            continue;
        }
        res[ip4_idx] = ip6_vec[ip6_idx];
        ip4_idx += 1;
        ip6_idx += 1;
    }

    std::net::Ipv4Addr::from(res)
}

// Get prefix from the IPv6 address
fn get_ipv6_prefix(
    ip_addr: std::net::Ipv6Addr,
    prefix_length: usize,
) -> Option<std::net::Ipv6Addr> {
    if prefix_length > 128 {
        return None;
    }

    let mut res = [0u16; 8];
    let ip_seg = ip_addr.segments();
    let (byte_count, reminder) = (prefix_length / 16, prefix_length % 16);
    let mut res_idx = 0;
    while res_idx < byte_count {
        res[res_idx] = ip_seg[res_idx];
        res_idx += 1;
    }
    if reminder > 0 && reminder < 16 {
        let mask = ((1 << reminder) - 1) << (16 - reminder);
        res[res_idx] = ip_seg[res_idx] & mask;
    }
    Some(std::net::Ipv6Addr::from(res))
}

// Check "ipv4only.arpa" via Adjacent Infrastructure Link.
// Equivalent to posix implementation in "InfraNetif::DiscoverNat64PrefixDone()"
fn process_ail_dns_lookup_result(
    ip_vec: Vec<fidl_fuchsia_net::IpAddress>,
) -> Result<Ip6Prefix, Error> {
    let mut prefix: Option<Ip6Prefix> = None;
    for ip in ip_vec {
        if let fidl_fuchsia_net::IpAddress::Ipv6(ip_addr) = ip {
            let ip6_address: Ip6Address = ip_addr.addr.into();
            for length in K_VALID_NAT64_PREFIX_LENGTHS {
                let ip4_address = extract_ipv4_addr_from_ipv6(ip6_address, length);
                if ip4_address.eq(&K_WELL_KNOWN_IPV4_ONLY_ADDRESS1)
                    || ip4_address.eq(&K_WELL_KNOWN_IPV4_ONLY_ADDRESS2)
                {
                    let mut found_duplicate = false;
                    for dup_length in K_VALID_NAT64_PREFIX_LENGTHS {
                        if dup_length == length {
                            continue;
                        }
                        let ip4_address_dup = extract_ipv4_addr_from_ipv6(ip6_address, dup_length);

                        if ip4_address_dup.eq(&ip4_address) {
                            found_duplicate = true;
                            break;
                        }
                    }
                    if !found_duplicate {
                        if let Some(ip6_prefix_addr) = get_ipv6_prefix(ip6_address, length.into()) {
                            prefix = Some(Ip6Prefix::new(ip6_prefix_addr, length));
                            break;
                        }
                    }
                }
                if prefix.is_some() {
                    break;
                }
            }
        }
    }
    match prefix {
        Some(p) => Ok(p),
        None => Err(Error::msg("NAT64 AIL result lookup is empty")),
    }
}

impl PlatformBacking {
    fn on_nat64_prefix_request(&self, infra_if_idx: ot::NetifIndex) {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn otPlatInfraIfDiscoverNat64Prefix(infra_if_idx: u32) -> otError {
            PlatformBacking::on_nat64_prefix_request(
                // SAFETY: Must only be called from OpenThread thread
                unsafe { PlatformBacking::as_ref() },
                infra_if_idx,
            );
            ot::Error::None.into()
        }

        let fut = async move {
            // async dns lookup
            let name_lookup_proxy_res = fuchsia_component::client::connect_to_protocol::<
                fidl_fuchsia_net_name::LookupMarker,
            >();

            if let Err(e) = name_lookup_proxy_res {
                warn!("failed to connect to fidl_fuchsia_net_name::LookupMarker: {:?}", e);
                return (infra_if_idx, Ip6Prefix::new(Ip6Address::new(0, 0, 0, 0, 0, 0, 0, 0), 0));
            }

            let lookup_result;
            match name_lookup_proxy_res
                .unwrap()
                .lookup_ip(
                    "ipv4only.arpa",
                    &fidl_fuchsia_net_name::LookupIpOptions {
                        ipv6_lookup: Some(true),
                        ..Default::default()
                    },
                )
                .await
            {
                Ok(res) => {
                    lookup_result = res;
                }
                Err(e) => {
                    warn!("failed to do dns lookup_ip: {:?}", e);
                    return (
                        infra_if_idx,
                        Ip6Prefix::new(Ip6Address::new(0, 0, 0, 0, 0, 0, 0, 0), 0),
                    );
                }
            };

            let prefix;
            match lookup_result {
                Ok(fidl_fuchsia_net_name::LookupResult { addresses: Some(ip_vec), .. }) => {
                    info!("processed dns response, result: {:?}", ip_vec);
                    match process_ail_dns_lookup_result(ip_vec) {
                        Ok(prefix_output) => {
                            prefix = prefix_output;
                        }
                        Err(_) => {
                            warn!("malformed DNS response, dropping the prefix");
                            prefix = Ip6Prefix::new(Ip6Address::new(0, 0, 0, 0, 0, 0, 0, 0), 0);
                        }
                    }
                }
                Ok(fidl_fuchsia_net_name::LookupResult { addresses: None, .. }) => {
                    info!("failed to process dns lookup result: empty result");
                    prefix = Ip6Prefix::new(Ip6Address::new(0, 0, 0, 0, 0, 0, 0, 0), 0);
                }
                Err(e) => {
                    warn!("failed to process dns lookup result: {:?}", e);
                    prefix = Ip6Prefix::new(Ip6Address::new(0, 0, 0, 0, 0, 0, 0, 0), 0);
                }
            }

            (infra_if_idx, prefix)
        };

        self.nat64
            .nat64_prefix_req_sender
            .lock()
            .unbounded_send(Mutex::new(Some(fut.boxed())))
            .expect("on_net64_prefix_request");
    }
}

impl Platform {
    pub fn process_poll_nat64(&mut self, instance: &ot::Instance, cx: &mut Context<'_>) {
        while let Poll::Ready(Some(mtx)) =
            self.nat64_platform_instance.nat64_prefix_req_receiver.poll_next_unpin(cx)
        {
            self.nat64_platform_instance.nat64_pending_fut = mtx;
        }

        let mut borrowed = self.nat64_platform_instance.nat64_pending_fut.lock();
        if let Some(future) = borrowed.as_mut() {
            match future.poll_unpin(cx) {
                Poll::Ready((infra_if_idx, ip6_prefix)) => {
                    instance.plat_infra_if_discover_nat64_prefix_done(infra_if_idx, ip6_prefix);
                    *borrowed = None;
                }
                Poll::Pending => {}
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn ail_dns_lookup_process_test() {
        // empty input
        assert!(process_ail_dns_lookup_result(vec![]).is_err());
        // no ipv6 response
        assert!(
            process_ail_dns_lookup_result(vec![fidl_fuchsia_net::IpAddress::Ipv4(
                fidl_fuchsia_net::Ipv4Address { addr: [192, 0, 0, 170] }
            )])
            .is_err()
        );
        // unknown address
        assert!(
            process_ail_dns_lookup_result(vec![fidl_fuchsia_net::IpAddress::Ipv6(
                fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }
            )])
            .is_err()
        );
        // valid ipv6 address in response (32 bit prefix, found addr 1)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0xaa, 0x00, 0x01, 0x00, 0x02,
                        0x00, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(Ip6Address::new(0xfc00, 0x0001, 0, 0, 0, 0, 0, 0), 32)
        );
        // valid ipv6 address in response (32 bit prefix, found addr 2)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0xab, 0x00, 0x01, 0x00, 0x02,
                        0x00, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(Ip6Address::new(0xfc00, 0x0001, 0, 0, 0, 0, 0, 0), 32)
        );
        // valid ipv6 address in response (40 bit prefix, found addr 1)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0x00, 0xaa, 0x00, 0x02,
                        0x00, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(Ip6Address::new(0xfc00, 0x0000, 0x0100, 0, 0, 0, 0, 0), 40)
        );
        // valid ipv6 address in response (48 bit prefix, found addr 1)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0x00, 0xaa, 0x02,
                        0x00, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(Ip6Address::new(0xfc00, 0x0000, 0x0001, 0, 0, 0, 0, 0), 48)
        );
        // valid ipv6 address in response (56 bit prefix, found addr 1)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0x00, 0xaa,
                        0x00, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(Ip6Address::new(0xfc00, 0x0000, 0x0000, 0x0100, 0, 0, 0, 0), 56)
        );
        // valid ipv6 address in response (64 bit prefix, found addr 1)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0xc0, 0x00, 0x00,
                        0xaa, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(Ip6Address::new(0xfc00, 0x0000, 0x0000, 0x0001, 0, 0, 0, 0), 64)
        );
        // valid ipv6 address in response (96 bit prefix, found addr 1)
        assert_eq!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
                        0xc0, 0x00, 0x00, 0xaa
                    ]
                })
            ])
            .unwrap(),
            Ip6Prefix::new(
                Ip6Address::new(0xfc00, 0x0000, 0x0000, 0x0000, 0x0000, 0x0001, 0, 0),
                96
            )
        );
        // no valid address in response: dup (32 bit and 72 bit)
        assert!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0xaa, 0x00, 0xc0, 0x00, 0x00,
                        0xaa, 0x03, 0x00, 0x04
                    ]
                })
            ])
            .is_err()
        );
        // no valid address in response: dup (32 bit and 96 bit)
        assert!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0xaa, 0x00, 0x00, 0x00, 0x00,
                        0xc0, 0x00, 0x00, 0xaa
                    ]
                })
            ])
            .is_err()
        );
        // no valid address in response: dup (40 bit and 96 bit)
        assert!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0x00, 0xaa, 0x00, 0x00,
                        0xc0, 0x00, 0x00, 0xaa
                    ]
                })
            ])
            .is_err()
        );
        // no valid address in response: dup (56 bit and 96 bit)
        assert!(
            process_ail_dns_lookup_result(vec![
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                        0x00, 0x00, 0x00, 0x00
                    ]
                }),
                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                    addr: [
                        0xfc, 0x00, 0x00, 0x00, 0x00, 0x01, 0xc0, 0x00, 0x00, 0x00, 0xaa, 0x00,
                        0xc0, 0x00, 0x00, 0xaa
                    ]
                })
            ])
            .is_err()
        );
    }

    #[test]
    fn test_ipv4_nat64_ops() {
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xf1f2, 0xf3f4, 0xffff, 0xffff, 0xffff, 0xffff),
                32
            ),
            Ip4Address::new(0xf1, 0xf2, 0xf3, 0xf4)
        );
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xfff1, 0xf2f3, 0xfff4, 0xffff, 0xffff, 0xffff),
                40
            ),
            Ip4Address::new(0xf1, 0xf2, 0xf3, 0xf4)
        );
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xf1f2, 0xfff3, 0xf4ff, 0xffff, 0xffff),
                48
            ),
            Ip4Address::new(0xf1, 0xf2, 0xf3, 0xf4)
        );
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xfff1, 0xfff2, 0xf3f4, 0xffff, 0xffff),
                56
            ),
            Ip4Address::new(0xf1, 0xf2, 0xf3, 0xf4)
        );
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xfff1, 0xf2f3, 0xf4ff, 0xffff),
                64
            ),
            Ip4Address::new(0xf1, 0xf2, 0xf3, 0xf4)
        );
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xf1f2, 0xf3f4),
                96
            ),
            Ip4Address::new(0xf1, 0xf2, 0xf3, 0xf4)
        );

        // Invalid input
        assert_eq!(
            extract_ipv4_addr_from_ipv6(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xf1f2, 0xf3f4),
                44
            ),
            Ip4Address::new(0, 0, 0, 0)
        );
    }

    #[test]
    fn test_ipv6_nat64_ops() {
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                0
            ),
            Some(Ip6Address::new(0, 0, 0, 0, 0, 0, 0, 0))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                16
            ),
            Some(Ip6Address::new(0xffff, 0, 0, 0, 0, 0, 0, 0))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                32
            ),
            Some(Ip6Address::new(0xffff, 0xffff, 0, 0, 0, 0, 0, 0))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                63
            ),
            Some(Ip6Address::new(0xffff, 0xffff, 0xffff, 0xfffe, 0, 0, 0, 0))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                64
            ),
            Some(Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                65
            ),
            Some(Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0x8000, 0, 0, 0))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                128
            ),
            Some(Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff))
        );
        assert_eq!(
            get_ipv6_prefix(
                Ip6Address::new(0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff),
                129
            ),
            None
        );
    }
}
