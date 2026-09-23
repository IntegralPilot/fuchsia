// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Abstractions to expose netstack3 core state to human inspection in bindings.
//!
//! This module mostly exists to abstract the dependency to Fuchsia inspect via
//! a trait, so we don't have to expose all of the internal core types to
//! bindings for it to perform the inspection.

use core::fmt::Display;

pub use diagnostics_traits::{Inspectable, InspectableValue, Inspector, InspectorDeviceExt};
use net_types::ip::IpAddress;
use net_types::{AddrAndPortFormatter, ZonedAddr};

use crate::counters::CounterRepr;

/// Extension trait for [`Inspector`] adding support for netstack3-specific
/// types.
pub trait InspectorExt: Inspector {
    /// Records a counter.
    fn record_counter<C: CounterRepr>(&mut self, name: &str, value: &C) {
        self.record_uint(name, value.get())
    }

    /// Records a `ZonedAddr` and it's port, mapping the zone into an
    /// inspectable device identifier.
    fn record_zoned_addr_with_port<I: InspectorDeviceExt<D>, A: IpAddress, D, P: Display>(
        &mut self,
        name: &str,
        addr: ZonedAddr<A, D>,
        port: P,
    ) {
        self.record_display(
            name,
            AddrAndPortFormatter::<_, _, A::Version>::new(
                addr.map_zone(|device| I::device_identifier_as_address_zone(device)),
                port,
            ),
        )
    }

    /// Records the local address of a socket.
    fn record_local_socket_addr<I: InspectorDeviceExt<D>, A: IpAddress, D, P: Display>(
        &mut self,
        addr_with_port: Option<(ZonedAddr<A, D>, P)>,
    ) {
        const NAME: &str = "LocalAddress";
        if let Some((addr, port)) = addr_with_port {
            self.record_zoned_addr_with_port::<I, _, _, _>(NAME, addr, port);
        } else {
            self.record_str(NAME, "[NOT BOUND]")
        }
    }

    /// Records the remote address of a socket.
    fn record_remote_socket_addr<I: InspectorDeviceExt<D>, A: IpAddress, D, P: Display>(
        &mut self,
        addr_with_port: Option<(ZonedAddr<A, D>, P)>,
    ) {
        const NAME: &str = "RemoteAddress";
        if let Some((addr, port)) = addr_with_port {
            self.record_zoned_addr_with_port::<I, _, _, _>(NAME, addr, port);
        } else {
            self.record_str(NAME, "[NOT CONNECTED]")
        }
    }
}

impl<T: Inspector> InspectorExt for T {}
