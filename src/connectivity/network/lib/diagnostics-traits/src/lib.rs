// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Target-agnostic abstractions for Inspect so that code that builds for both
//! host and Fuchsia targets can implement Inspect support.

#![no_std]
extern crate alloc;

#[cfg(target_os = "fuchsia")]
mod fuchsia;

#[cfg(target_os = "fuchsia")]
pub use fuchsia::*;

use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use core::convert::Infallible as Never;
use core::fmt::{Debug, Display};

use net_types::ip::IpAddress;

/// A trait abstracting a state inspector.
///
/// This trait follows roughly the same shape as the API provided by the
/// fuchsia_inspect crate, but we abstract it out so not to take the dependency.
///
/// Given we have the trait, we can fill it in with some helpful default
/// implementations for common types that are exposed as well, like IP addresses
/// and such.
pub trait Inspector: Sized {
    /// The type given to record contained children.
    type ChildInspector<'a>: Inspector;

    /// Records a nested inspector with `name` calling `f` with the nested child
    /// to be filled in.
    ///
    /// This is used to group and contextualize data.
    fn record_child<F: FnOnce(&mut Self::ChildInspector<'_>)>(&mut self, name: &str, f: F);

    /// Records a child without a name.
    ///
    /// The `Inpector` is expected to keep track of the number of unnamed
    /// children and allocate names appropriately from that.
    fn record_unnamed_child<F: FnOnce(&mut Self::ChildInspector<'_>)>(&mut self, f: F);

    /// Records a child whose name is the display implementation of `T`.
    fn record_display_child<T: Display, F: FnOnce(&mut Self::ChildInspector<'_>)>(
        &mut self,
        name: T,
        f: F,
    ) {
        self.record_child(&format!("{name}"), f)
    }

    /// Records a child whose name is the Debug implementation of `T`.
    fn record_debug_child<T: Debug, F: FnOnce(&mut Self::ChildInspector<'_>)>(
        &mut self,
        name: T,
        f: F,
    ) {
        self.record_child(&format!("{name:?}"), f)
    }

    /// Records anything that can be represented by a usize.
    fn record_usize<T: Into<usize>>(&mut self, name: &str, value: T);

    /// Records anything that can be represented by a u64.
    fn record_uint<T: Into<u64>>(&mut self, name: &str, value: T);

    /// Records anything that can be represented by a i64.
    fn record_int<T: Into<i64>>(&mut self, name: &str, value: T);

    /// Records anything that can be represented by a f64.
    fn record_double<T: Into<f64>>(&mut self, name: &str, value: T);

    /// Records a str value.
    fn record_str(&mut self, name: &str, value: &str);

    /// Records an owned string.
    fn record_string(&mut self, name: &str, value: String);

    /// Records a boolean.
    fn record_bool(&mut self, name: &str, value: bool);

    /// Records a `value` that implements `Display` as its display string.
    fn record_display<T: Display>(&mut self, name: &str, value: T) {
        self.record_string(name, format!("{value}"))
    }

    /// Records a `value` that implements `Debug` as its debug string.
    fn record_debug<T: Debug>(&mut self, name: &str, value: T) {
        self.record_string(name, format!("{value:?}"))
    }

    /// Records an IP address.
    fn record_ip_addr<A: IpAddress>(&mut self, name: &str, value: A) {
        self.record_display(name, value)
    }

    /// Records an implementor of [`InspectableValue`].
    fn record_inspectable_value<V: InspectableValue>(&mut self, name: &str, value: &V) {
        value.record(name, self)
    }

    /// Records an implementor of [`InspectableInstant`].
    fn record_instant<V: InspectableInstant>(&mut self, name: InstantPropertyName, value: &V) {
        value.record(name, self)
    }

    /// Records an implementor of [`Inspectable`] under `name`.
    fn record_inspectable<V: Inspectable>(&mut self, name: &str, value: &V) {
        self.record_child(name, |inspector| {
            inspector.delegate_inspectable(value);
        });
    }

    /// Delegates more fields to be added by an [`Inspectable`] implementation.
    fn delegate_inspectable<V: Inspectable>(&mut self, value: &V) {
        value.record(self)
    }
}

/// A trait that allows a type to record its fields to an `inspector`.
///
/// This trait is used for types that are exposed to [`Inspector`]s many times
/// so recording them can be deduplicated.
pub trait Inspectable {
    /// Records this value into `inspector`.
    fn record<I: Inspector>(&self, inspector: &mut I);
}

impl Inspectable for () {
    fn record<I: Inspector>(&self, _inspector: &mut I) {}
}

/// A trait that marks a type as inspectable.
///
/// This trait is used for types that are exposed to [`Inspector`]s many times
/// so recording them can be deduplicated.
///
/// This type differs from [`Inspectable`] in that it receives a `name`
/// parameter. This is typically used for types that record a single entry.
pub trait InspectableValue {
    /// Records this value into `inspector`.
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I);
}

impl InspectableValue for Never {
    fn record<I: Inspector>(&self, _name: &str, _inspector: &mut I) {
        match *self {}
    }
}

impl<V: InspectableValue> InspectableValue for Arc<V> {
    fn record<I: Inspector>(&self, name: &str, inspector: &mut I) {
        self.as_ref().record(name, inspector)
    }
}

/// An extension to `Inspector` that allows transforming and recording device
/// identifiers.
///
/// How to record device IDs is delegated to bindings via this trait, so we
/// don't need to propagate `InspectableValue` implementations everywhere in
/// core unnecessarily.
pub trait InspectorDeviceExt<D> {
    /// Records an entry named `name` with value `device`.
    fn record_device<I: Inspector>(inspector: &mut I, name: &str, device: &D);

    /// Returns the `Display` representation of the IPv6 scoped address zone
    /// associated with `D`.
    fn device_identifier_as_address_zone(device: D) -> impl Display;
}

/// An extension to `Inspector` that allows recoding route table IDs.
pub trait InspectorRouteTableExt<R> {
    /// Records an entry named `name` with value `table`.
    fn record_route_table<I: Inspector>(inspector: &mut I, name: &str, table: &R);
    /// Returns the `Display` representation of `table`.
    fn display_route_table(table: &R) -> impl Display;
}

/// A trait that marks a type as an inspectable representation of an instant in
/// time.
pub trait InspectableInstant {
    /// Records this value into `inspector`.
    fn record<I: Inspector>(&self, name: InstantPropertyName, inspector: &mut I);
}

/// A name suitable for use for recording an Instant property representing a
/// moment in time.
///
/// This type exists because Fuchsia Snapshot Viewer has special treatment of
/// property names ending in the magic string "@time".
/// [`crate::instant_property_name`] should be used to construct this type and
/// ensure that the "@time" suffix is added.
#[derive(Copy, Clone)]
pub struct InstantPropertyName {
    inner: &'static str,
}

impl InstantPropertyName {
    pub fn get(&self) -> &'static str {
        self.inner
    }
}

impl From<InstantPropertyName> for &'static str {
    fn from(InstantPropertyName { inner }: InstantPropertyName) -> Self {
        inner
    }
}

/// Implementation details that need to be `pub` in order to be used from macros
/// but should not be used otherwise.
#[doc(hidden)]
pub mod internal {
    use super::InstantPropertyName;

    /// Constructs an [`InstantPropertyName`].
    ///
    /// Use [`crate::instant_property_name`] instead.
    pub fn construct_instant_property_name_do_not_use(inner: &'static str) -> InstantPropertyName {
        InstantPropertyName { inner }
    }
}

/// Constructs an [`InstantPropertyName`] to use while recording Instants.
#[macro_export]
macro_rules! instant_property_name {
    () => {
        $crate::internal::construct_instant_property_name_do_not_use("@time")
    };
    ($lit:literal) => {
        $crate::internal::construct_instant_property_name_do_not_use(core::concat!($lit, "@time"))
    };
}
