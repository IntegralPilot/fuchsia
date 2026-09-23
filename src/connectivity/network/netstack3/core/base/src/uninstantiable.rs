// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A collection of uninstantiable types.
//!
//! These uninstantiable types can be used to satisfy trait bounds in
//! uninstantiable situations. Different parts of core provide implementations
//! of their local traits on these types, so they can be used in uninstantiable
//! contexts.

use core::convert::Infallible as Never;
use core::marker::PhantomData;

use explicit::UnreachableExt as _;

use crate::{
    BidirectionalConverter, CoreTimerContext, CoreTxMetadataContext, CounterContext, Device,
    DeviceIdContext, ResourceCounterContext, TimerBindingsTypes, TxMetadataBindingsTypes,
};

/// An uninstantiable type.
#[derive(Clone, Copy)]
pub struct Uninstantiable(Never);

impl AsRef<Never> for Uninstantiable {
    fn as_ref(&self) -> &Never {
        &self.0
    }
}

impl<I, O> BidirectionalConverter<I, O> for Uninstantiable {
    fn convert_back(&self, _: O) -> I {
        self.uninstantiable_unreachable()
    }
    fn convert(&self, _: I) -> O {
        self.uninstantiable_unreachable()
    }
}

/// An uninstantiable type that wraps an instantiable type, `A`.
///
/// This type can be used to more easily implement traits where `A` already
/// implements the trait.
// TODO(https://github.com/rust-lang/rust/issues/118212): Simplify the trait
// implementations once Rust supports function delegation. Those impls are
// spread among the core crates.
pub struct UninstantiableWrapper<A>(Never, PhantomData<A>);

impl<A> AsRef<Never> for UninstantiableWrapper<A> {
    fn as_ref(&self) -> &Never {
        let Self(never, _marker) = self;
        &never
    }
}

impl<T, BT, C> CoreTimerContext<T, BT> for UninstantiableWrapper<C>
where
    BT: TimerBindingsTypes,
    C: CoreTimerContext<T, BT>,
{
    fn convert_timer(dispatch_id: T) -> BT::DispatchId {
        C::convert_timer(dispatch_id)
    }
}

impl<P, C> CounterContext<C> for UninstantiableWrapper<P> {
    fn counters(&self) -> &C {
        self.uninstantiable_unreachable()
    }
}

impl<P, R, C> ResourceCounterContext<R, C> for UninstantiableWrapper<P> {
    fn per_resource_counters(&self, _resource: &R) -> &C {
        self.uninstantiable_unreachable()
    }
}

impl<T, BT, C> CoreTxMetadataContext<T, BT> for UninstantiableWrapper<C>
where
    BT: TxMetadataBindingsTypes,
{
    fn convert_tx_meta(&self, _tx_meta: T) -> BT::TxMetadata {
        self.uninstantiable_unreachable()
    }
}

impl<D: Device, C: DeviceIdContext<D>> DeviceIdContext<D> for UninstantiableWrapper<C> {
    type DeviceId = C::DeviceId;
    type WeakDeviceId = C::WeakDeviceId;
}

impl<A: Iterator> Iterator for UninstantiableWrapper<A> {
    type Item = A::Item;

    fn next(&mut self) -> Option<Self::Item> {
        self.uninstantiable_unreachable()
    }
}
