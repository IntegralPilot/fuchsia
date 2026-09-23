// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Shareable core context fakes.

use alloc::vec::Vec;
use core::marker::PhantomData;

use derivative::Derivative;
use packet::BufferMut;

use crate::testutil::{FakeFrameCtx, FakeTxMetadata, WithFakeFrameContext};
use crate::{
    ContextProvider, CoreTxMetadataContext, CounterContext, NetworkSerializer,
    ResourceCounterContext, SendFrameError, SendableFrameMeta, TxMetadataBindingsTypes,
};

/// A test helper used to provide an implementation of a core context.
#[derive(Derivative)]
#[derivative(Default(bound = "S: Default"))]
pub struct FakeCoreCtx<S, Meta, DeviceId> {
    /// Generic state kept in this fake context.
    pub state: S,
    /// A fake frame context for outgoing frames.
    pub frames: FakeFrameCtx<Meta>,
    _devices_marker: PhantomData<DeviceId>,
}

impl<S, Meta, DeviceId> ContextProvider for FakeCoreCtx<S, Meta, DeviceId> {
    type Context = Self;

    fn context(&mut self) -> &mut Self::Context {
        self
    }
}

impl<C, S, Meta, DeviceId> CounterContext<C> for FakeCoreCtx<S, Meta, DeviceId>
where
    S: CounterContext<C>,
{
    fn counters(&self) -> &C {
        CounterContext::<C>::counters(&self.state)
    }
}

impl<C, S, R, Meta, DeviceId> ResourceCounterContext<R, C> for FakeCoreCtx<S, Meta, DeviceId>
where
    S: ResourceCounterContext<R, C>,
{
    fn per_resource_counters<'a>(&'a self, resource: &'a R) -> &'a C {
        ResourceCounterContext::<R, C>::per_resource_counters(&self.state, resource)
    }
}

impl<BC, S, Meta, DeviceId> SendableFrameMeta<FakeCoreCtx<S, Meta, DeviceId>, BC> for Meta {
    fn send_meta<SS>(
        self,
        core_ctx: &mut FakeCoreCtx<S, Meta, DeviceId>,
        bindings_ctx: &mut BC,
        frame: SS,
    ) -> Result<(), SendFrameError<SS>>
    where
        SS: NetworkSerializer,
        SS::Buffer: BufferMut,
    {
        self.send_meta(&mut core_ctx.frames, bindings_ctx, frame)
    }
}

impl<S, Meta, DeviceId> FakeCoreCtx<S, Meta, DeviceId> {
    /// Constructs a `FakeCoreCtx` with the given state and default
    /// `FakeTimerCtx`, and `FakeFrameCtx`.
    pub fn with_state(state: S) -> Self {
        FakeCoreCtx { state, frames: FakeFrameCtx::default(), _devices_marker: PhantomData }
    }

    /// Get the list of frames sent so far.
    pub fn frames(&self) -> &[(Meta, Vec<u8>)] {
        self.frames.frames()
    }

    /// Take the list of frames sent so far.
    pub fn take_frames(&mut self) -> Vec<(Meta, Vec<u8>)> {
        self.frames.take_frames()
    }

    /// Consumes the `FakeCoreCtx` and returns the inner state.
    pub fn into_state(self) -> S {
        self.state
    }
}

impl<S, Meta, DeviceId> WithFakeFrameContext<Meta> for FakeCoreCtx<S, Meta, DeviceId> {
    fn with_fake_frame_ctx_mut<O, F: FnOnce(&mut FakeFrameCtx<Meta>) -> O>(&mut self, f: F) -> O {
        f(&mut self.frames)
    }
}

impl<T, S, Meta, DeviceId, BT> CoreTxMetadataContext<T, BT> for FakeCoreCtx<S, Meta, DeviceId>
where
    BT: TxMetadataBindingsTypes<TxMetadata = FakeTxMetadata>,
{
    fn convert_tx_meta(&self, _tx_meta: T) -> FakeTxMetadata {
        FakeTxMetadata
    }
}

impl<S, Meta, DeviceId> AsRef<S> for FakeCoreCtx<S, Meta, DeviceId> {
    fn as_ref(&self) -> &S {
        &self.state
    }
}
