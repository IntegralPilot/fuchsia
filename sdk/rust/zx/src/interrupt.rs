// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon interrupts.

use crate::{
    BootTimeline, Instant, MonotonicTimeline, NullableHandle, Port, Status, Timeline, ok, sys,
};
use std::marker::PhantomData;

/// An object representing a Zircon interrupt.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Interrupt<K = RealInterruptKind, T = BootTimeline>(NullableHandle, PhantomData<(K, T)>);

impl_handle_based!(Interrupt, [K: InterruptKind, T: Timeline], [K, T]);

pub trait InterruptKind: private::Sealed {}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct VirtualInterruptKind;

impl InterruptKind for VirtualInterruptKind {}
impl private::Sealed for VirtualInterruptKind {}

#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RealInterruptKind;

impl InterruptKind for RealInterruptKind {}
impl private::Sealed for RealInterruptKind {}

pub type VirtualInterrupt = Interrupt<VirtualInterruptKind>;

impl<K: InterruptKind, T: Timeline> Interrupt<K, T> {
    /// Bind the given port with the given key.
    ///
    /// Wraps [zx_interrupt_bind](https://fuchsia.dev/reference/syscalls/interrupt_bind).
    pub fn bind_port(&self, port: &Port, key: u64) -> Result<(), Status> {
        let options = sys::ZX_INTERRUPT_BIND;
        // SAFETY: This is a basic FFI call.
        let status =
            unsafe { sys::zx_interrupt_bind(self.raw_handle(), port.raw_handle(), key, options) };
        ok(status)
    }

    /// Unbinds from a previously bound port.
    ///
    /// Wraps [zx_interrupt_bind](https://fuchsia.dev/reference/syscalls/interrupt_bind) with
    /// ZX_INTERRUPT_UNBIND set.
    pub fn unbind_port(&self, port: &Port) -> Result<(), Status> {
        let options = sys::ZX_INTERRUPT_UNBIND;
        // SAFETY: This is a basic FFI call.
        // Per the documentation, when unbinding, key is ignored.
        let status =
            unsafe { sys::zx_interrupt_bind(self.raw_handle(), port.raw_handle(), 0, options) };
        ok(status)
    }

    /// Synchronously wait for the interrupt to be triggered.
    ///
    /// Wraps [zx_interrupt_wait](https://fuchsia.dev/reference/syscalls/interrupt_wait).
    pub fn wait(&self) -> Result<Instant<T>, Status> {
        let mut timestamp = 0;
        // SAFETY: We're sure that `timestamp` has a valid address.
        let status = unsafe { sys::zx_interrupt_wait(self.raw_handle(), &mut timestamp) };
        ok(status)?;
        Ok(Instant::from_nanos(timestamp))
    }

    /// Acknowledge the interrupt.
    ///
    /// Wraps [zx_interrupt_ack](https://fuchsia.dev/reference/syscalls/interrupt_ack).
    pub fn ack(&self) -> Result<(), Status> {
        // SAFETY: This is a basic FFI call.
        let status = unsafe { sys::zx_interrupt_ack(self.raw_handle()) };
        ok(status)
    }
    /// Destroy the interrupt.
    ///
    /// Wraps [zx_interrupt_destroy](https://fuchsia.dev/reference/syscalls/interrupt_destroy).
    pub fn destroy(self) -> Result<(), Status> {
        // SAFETY: This is a basic FFI call.
        let status = unsafe { sys::zx_interrupt_destroy(self.raw_handle()) };
        ok(status)
    }
}

pub trait InterruptTimeline: Timeline {
    const CREATE_FLAGS: u32;
}

impl InterruptTimeline for MonotonicTimeline {
    const CREATE_FLAGS: u32 = sys::ZX_INTERRUPT_TIMESTAMP_MONO;
}

impl InterruptTimeline for BootTimeline {
    const CREATE_FLAGS: u32 = 0;
}

impl<T: InterruptTimeline> Interrupt<VirtualInterruptKind, T> {
    /// Create a virtual interrupt.
    ///
    /// Wraps [zx_interrupt_create](https://fuchsia.dev/reference/syscalls/interrupt_create).
    pub fn create_virtual() -> Result<Self, Status> {
        // SAFETY: We are sure that the handle has a valid address.
        let handle = unsafe {
            let mut handle = sys::ZX_HANDLE_INVALID;
            ok(sys::zx_interrupt_create(
                sys::ZX_HANDLE_INVALID,
                T::CREATE_FLAGS,
                sys::ZX_INTERRUPT_VIRTUAL,
                &mut handle,
            ))?;
            NullableHandle::from_raw(handle)
        };
        Ok(Interrupt(handle, PhantomData))
    }

    /// Triggers a virtual interrupt object.
    ///
    /// Wraps [zx_interrupt_trigger](https://fuchsia.dev/reference/syscalls/interrupt_trigger).
    pub fn trigger(&self, time: Instant<T>) -> Result<(), Status> {
        // SAFETY: this is a basic FFI call.
        let status = unsafe { sys::zx_interrupt_trigger(self.raw_handle(), 0, time.into_nanos()) };
        ok(status)
    }
}

mod private {
    pub trait Sealed {}
}

#[cfg(test)]
mod tests {
    use zx_status::Status;

    use crate::{
        BootInstant, Interrupt, MonotonicInstant, MonotonicTimeline, Port, PortOptions,
        VirtualInterrupt, VirtualInterruptKind,
    };

    #[test]
    fn bind() {
        let interrupt = VirtualInterrupt::create_virtual().unwrap();
        let port = Port::create_with_opts(PortOptions::BIND_TO_INTERRUPT);
        let key = 1;
        let result = interrupt.bind_port(&port, key);
        assert_eq!(result, Ok(()));

        // Can't bind twice.
        let result = interrupt.bind_port(&port, key);
        assert_eq!(result, Err(Status::ALREADY_BOUND));

        // ...Unless we unbind first.
        let result = interrupt.unbind_port(&port);
        assert_eq!(result, Ok(()));
        let result = interrupt.bind_port(&port, key);
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn ack() {
        let interrupt = VirtualInterrupt::create_virtual().unwrap();
        let result = interrupt.ack();
        assert_eq!(result.err(), Some(Status::BAD_STATE));
    }

    #[test]
    fn trigger() {
        let interrupt = VirtualInterrupt::create_virtual().unwrap();
        let result = interrupt.trigger(BootInstant::from_nanos(10));
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn trigger_monotimeline() {
        let interrupt =
            Interrupt::<VirtualInterruptKind, MonotonicTimeline>::create_virtual().unwrap();
        let result = interrupt.trigger(MonotonicInstant::from_nanos(10));
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn wait() {
        let interrupt = VirtualInterrupt::create_virtual().unwrap();
        let result = interrupt.trigger(BootInstant::from_nanos(10));
        assert_eq!(result, Ok(()));
        let instant = interrupt.wait().expect("wait failed");
        assert_eq!(instant.into_nanos(), 10);
    }

    #[test]
    fn wait_monotimeline() {
        let interrupt =
            Interrupt::<VirtualInterruptKind, MonotonicTimeline>::create_virtual().unwrap();
        let result = interrupt.trigger(MonotonicInstant::from_nanos(10));
        assert_eq!(result, Ok(()));
        let instant = interrupt.wait().expect("wait failed");
        assert_eq!(instant.into_nanos(), 10);
    }

    #[test]
    fn destroy() {
        let interrupt =
            Interrupt::<VirtualInterruptKind, MonotonicTimeline>::create_virtual().unwrap();
        assert_eq!(interrupt.destroy(), Ok(()));
    }
}
