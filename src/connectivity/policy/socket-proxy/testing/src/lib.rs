// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net::IpAddress;
use fidl_fuchsia_net_policy_socketproxy::{
    Network, NetworkDnsServers, NetworkInfo, NetworkRegistryAddResult, NetworkRegistryRemoveResult,
    NetworkRegistrySetDefaultResult, NetworkRegistryUpdateResult, StarnixNetworkInfo,
    StarnixNetworksProxy,
};
use fidl_fuchsia_posix_socket::OptionalUint32;
use std::future::Future;

fn starnix_network_info(mark: u32) -> NetworkInfo {
    NetworkInfo::Starnix(StarnixNetworkInfo {
        mark: Some(mark),
        handle: Some(0),
        ..Default::default()
    })
}

fn starnix_network(network_id: u32) -> Network {
    Network {
        network_id: Some(network_id),
        info: Some(starnix_network_info(network_id)),
        dns_servers: Some(Default::default()),
        ..Default::default()
    }
}

pub trait ToNetwork {
    fn to_network(self) -> Network;
}

impl ToNetwork for u32 {
    fn to_network(self) -> Network {
        starnix_network(self)
    }
}

impl ToNetwork for (u32, Vec<IpAddress>) {
    fn to_network(self) -> Network {
        let (v4, v6) = self.1.iter().fold((Vec::new(), Vec::new()), |(mut v4s, mut v6s), s| {
            match s {
                IpAddress::Ipv4(v4) => v4s.push(*v4),
                IpAddress::Ipv6(v6) => v6s.push(*v6),
            }
            (v4s, v6s)
        });
        let base = starnix_network(self.0);
        Network {
            dns_servers: Some(NetworkDnsServers {
                v4: Some(v4),
                v6: Some(v6),
                ..Default::default()
            }),
            ..base
        }
    }
}

impl<N: ToNetwork + Clone> ToNetwork for &N {
    fn to_network(self) -> Network {
        self.clone().to_network()
    }
}

pub trait NetworkRegistry {
    fn set_default(
        &self,
        network_id: &OptionalUint32,
    ) -> impl Future<Output = Result<NetworkRegistrySetDefaultResult, fidl::Error>>;
    fn add(
        &self,
        network: &Network,
    ) -> impl Future<Output = Result<NetworkRegistryAddResult, fidl::Error>>;
    fn update(
        &self,
        network: &Network,
    ) -> impl Future<Output = Result<NetworkRegistryUpdateResult, fidl::Error>>;
    fn remove(
        &self,
        network_id: u32,
    ) -> impl Future<Output = Result<NetworkRegistryRemoveResult, fidl::Error>>;
}

macro_rules! impl_network_registry {
    ($($ty:ty),*) => {
        $(
            impl NetworkRegistry for $ty {
                fn set_default(
                    &self,
                    network_id: &OptionalUint32,
                ) -> impl Future<Output = Result<NetworkRegistrySetDefaultResult, fidl::Error>> {
                    self.set_default(network_id)
                }

                fn add(
                    &self,
                    network: &Network,
                ) -> impl Future<Output = Result<NetworkRegistryAddResult, fidl::Error>> {
                    self.add(network)
                }

                fn update(
                    &self,
                    network: &Network,
                ) -> impl Future<Output = Result<NetworkRegistryUpdateResult, fidl::Error>> {
                    self.update(network)
                }

                fn remove(
                    &self,
                    network_id: u32,
                ) -> impl Future<Output = Result<NetworkRegistryRemoveResult, fidl::Error>> {
                    self.remove(network_id)
                }
            }
        )*
    };
    ($($ty:ty),*,) => { impl_network_registry!($($ty),*); };
}

impl_network_registry!(StarnixNetworksProxy);
