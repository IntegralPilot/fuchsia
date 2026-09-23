// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Defines witness types for the Bindings route table implementation.

use core::marker::PhantomData;

use fidl_fuchsia_net_routes_ext as fnet_routes_ext;
use net_types::ip::{GenericOverIp, Ip, IpVersion, Ipv4, Ipv6};

/// Uniquely identifies a route table.
///
/// The type is a witness for the ID being unique among v4 and v6 tables.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, GenericOverIp)]
#[generic_over_ip(I, Ip)]
pub(crate) struct TableId<I: Ip>(u32, PhantomData<I>);

impl<I: Ip> TableId<I> {
    pub(crate) const fn new(id: u32) -> Option<Self> {
        // TableIds are unique in all route tables, as an implementation detail,
        // we use even numbers for v4 tables and odd numbers for v6 tables.
        if match I::VERSION {
            IpVersion::V4 => id % 2 == 0,
            IpVersion::V6 => id % 2 == 1,
        } {
            Some(Self(id, PhantomData))
        } else {
            None
        }
    }

    /// Returns the next table ID. [`None`] if overflows.
    pub fn next(&self) -> Option<Self> {
        self.0.checked_add(2).map(|x| TableId(x, PhantomData))
    }
}

impl<I: Ip> From<TableId<I>> for u32 {
    fn from(TableId(id, _marker): TableId<I>) -> u32 {
        id
    }
}

impl<I: Ip> From<TableId<I>> for fnet_routes_ext::TableId {
    fn from(TableId(id, _marker): TableId<I>) -> fnet_routes_ext::TableId {
        fnet_routes_ext::TableId::new(id)
    }
}

pub(crate) const IPV4_MAIN_TABLE_ID: TableId<Ipv4> = TableId::new(0).unwrap();
pub(crate) const IPV6_MAIN_TABLE_ID: TableId<Ipv6> = TableId::new(1).unwrap();

/// Returns the main table ID for the IP version
pub(crate) fn main_table_id<I: Ip>() -> TableId<I> {
    I::map_ip((), |()| IPV4_MAIN_TABLE_ID, |()| IPV6_MAIN_TABLE_ID)
}

/// Either a [`TableId<Ipv4>`] or [`TableId<Ipv6>`].
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, GenericOverIp)]
#[generic_over_ip()]
pub(crate) enum TableIdEither {
    V4(TableId<Ipv4>),
    V6(TableId<Ipv6>),
}

impl TableIdEither {
    pub(crate) const fn new(id: u32) -> Self {
        match (TableId::<Ipv4>::new(id), TableId::<Ipv6>::new(id)) {
            (None, None) | (Some(_), Some(_)) => unreachable!(),
            (Some(id), _) => TableIdEither::V4(id),
            (_, Some(id)) => TableIdEither::V6(id),
        }
    }
}
