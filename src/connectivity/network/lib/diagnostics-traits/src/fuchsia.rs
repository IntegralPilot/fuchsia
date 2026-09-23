// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::format;
use alloc::string::String;
use core::fmt::Display;
use core::marker::PhantomData;

use fuchsia_async as fasync;
use fuchsia_inspect::Node;
use log::warn;

use crate::{
    InspectableInstant, Inspector, InspectorDeviceExt, InspectorRouteTableExt, InstantPropertyName,
};

/// Provides an abstract interface for extracting inspect device identifier.
pub trait InspectorDeviceIdProvider<DeviceId> {
    /// Extracts the device identifier from the provided opaque type.
    fn device_id(id: &DeviceId) -> u64;
}

/// Provides an abstract interface for extracting inspect route table identifier.
pub trait InspectorRouteTableIdProvider<R> {
    /// Extracts the route table identifier from the provided opaque type.
    fn route_table_id(id: &R) -> u32;
}

/// Provides a Fuchsia implementation of `Inspector`.
pub struct FuchsiaInspector<'a, D> {
    node: &'a Node,
    unnamed_count: usize,
    _marker: PhantomData<D>,
}

impl<'a, D> FuchsiaInspector<'a, D> {
    /// Create a new `FuchsiaInspector` rooted at `node`.
    pub fn new(node: &'a Node) -> Self {
        Self { node, unnamed_count: 0, _marker: Default::default() }
    }
}

impl<'a, D> Inspector for FuchsiaInspector<'a, D> {
    type ChildInspector<'l> = FuchsiaInspector<'l, D>;

    fn record_child<F: FnOnce(&mut Self::ChildInspector<'_>)>(&mut self, name: &str, f: F) {
        self.node.record_child(name, |node| f(&mut FuchsiaInspector::new(node)))
    }

    fn record_unnamed_child<F: FnOnce(&mut Self::ChildInspector<'_>)>(&mut self, f: F) {
        let Self { node: _, unnamed_count, _marker: _ } = self;
        let id = core::mem::replace(unnamed_count, *unnamed_count + 1);
        self.record_child(&format!("{id}"), f)
    }

    fn record_usize<T: Into<usize>>(&mut self, name: &str, value: T) {
        let value: u64 = value.into().try_into().unwrap_or_else(|e| {
            warn!("failed to inspect usize value that does not fit in a u64: {e:?}");
            u64::MAX
        });
        self.node.record_uint(name, value)
    }

    fn record_uint<T: Into<u64>>(&mut self, name: &str, value: T) {
        self.node.record_uint(name, value.into())
    }

    fn record_int<T: Into<i64>>(&mut self, name: &str, value: T) {
        self.node.record_int(name, value.into())
    }

    fn record_double<T: Into<f64>>(&mut self, name: &str, value: T) {
        self.node.record_double(name, value.into())
    }

    fn record_str(&mut self, name: &str, value: &str) {
        self.node.record_string(name, value)
    }

    fn record_string(&mut self, name: &str, value: String) {
        self.node.record_string(name, value)
    }

    fn record_bool(&mut self, name: &str, value: bool) {
        self.node.record_bool(name, value)
    }
}

impl<'a, D, P: InspectorDeviceIdProvider<D>> InspectorDeviceExt<D> for FuchsiaInspector<'a, P> {
    fn record_device<I: Inspector>(inspector: &mut I, name: &str, device: &D) {
        inspector.record_uint(name, P::device_id(device))
    }

    fn device_identifier_as_address_zone(id: D) -> impl Display {
        P::device_id(&id)
    }
}

impl<'a, R, P: InspectorRouteTableIdProvider<R>> InspectorRouteTableExt<R>
    for FuchsiaInspector<'a, P>
{
    fn record_route_table<I: Inspector>(inspector: &mut I, name: &str, table: &R) {
        inspector.record_uint(name, P::route_table_id(table))
    }

    fn display_route_table(table: &R) -> impl Display {
        P::route_table_id(table)
    }
}

impl InspectableInstant for fasync::MonotonicInstant {
    fn record<I: Inspector>(&self, name: InstantPropertyName, inspector: &mut I) {
        inspector.record_int(name.into(), self.into_nanos());
    }
}
