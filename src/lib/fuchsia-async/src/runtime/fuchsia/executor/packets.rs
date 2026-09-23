// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::common::Executor;
use crate::epoch::Epoch;
use fuchsia_sync::{RwLock, RwLockReadGuard};
use rustc_hash::FxHashMap as HashMap;
use std::fmt;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use pin_project_lite::pin_project;

use super::common::EHandle;

#[allow(dead_code)]
struct ReceiverGuard<'a>(RwLockReadGuard<'a, Inner>);

struct RawReceiverVTable {
    receive_packet: fn(*const (), guard: ReceiverGuard<'_>, packet: zx::Packet),
}

/// A trait for handling the arrival of a packet on a `zx::Port`.
///
/// This trait should be implemented by users who wish to write their own
/// types which receive asynchronous notifications from a `zx::Port`.
/// Implementors of this trait generally contain a `futures::task::AtomicWaker` which
/// is used to wake up the task which can make progress due to the arrival of
/// the packet.
///
/// `PacketReceiver`s should be registered with a `Core` using the
/// `register_receiver` method on `Core`, `Handle`, or `Remote`.
/// Upon registration, users will receive a `ReceiverRegistration`
/// which provides `key` and `port` methods. These methods can be used to wait on
/// asynchronous signals.
///
/// Note that `PacketReceiver`s may receive false notifications intended for a
/// previous receiver, and should handle these gracefully.
pub trait PacketReceiver: Send + Sync + 'static {
    /// Receive a packet when one arrives.
    fn receive_packet(&self, packet: zx::Packet);
}

impl<T: PacketReceiver> PacketReceiver for Arc<T> {
    fn receive_packet(&self, packet: zx::Packet) {
        self.as_ref().receive_packet(packet);
    }
}

// Simple slab::Slab replacement that doesn't re-use keys
// TODO(https://fxbug.dev/42119369): figure out how to safely cancel async waits so we can re-use keys again.
pub struct PacketReceiverMap(RwLock<Inner>);

struct Inner {
    next_key: u64,
    mapping: HashMap<u64, (*const (), &'static RawReceiverVTable)>,
}

impl PacketReceiverMap {
    /// Returns a new map.
    pub fn new() -> Self {
        Self(RwLock::new(Inner { next_key: 0, mapping: HashMap::default() }))
    }

    /// Notifies the packet receiver for `key`.
    pub fn receive_packet(&self, key: u64, packet: zx::Packet) {
        let inner = self.0.read();
        if let Some(&(data, vtable)) = inner.mapping.get(&key) {
            (vtable.receive_packet)(data, ReceiverGuard(inner), packet);
        }
    }

    /// Registers a raw receiver.
    fn register_raw<R>(
        &self,
        f: impl FnOnce(u64) -> (*const (), &'static RawReceiverVTable, R),
    ) -> R {
        let mut inner = self.0.write();
        let key = inner.next_key;
        inner.next_key = inner.next_key.checked_add(1).expect("ran out of keys");
        let (data, vtable, result) = f(key);
        inner.mapping.insert(key, (data, vtable));
        result
    }

    /// Registers a `PacketReceiver` with the executor and returns a registration.
    /// The `PacketReceiver` will be deregistered when the `Registration` is dropped.
    pub fn register<T: PacketReceiver>(
        &self,
        executor: Arc<Executor>,
        receiver: T,
    ) -> ReceiverRegistration<T> {
        struct Impl<T>(PhantomData<T>);

        impl<T: PacketReceiver> Impl<T> {
            const VTABLE: RawReceiverVTable = RawReceiverVTable {
                receive_packet: |data, guard, packet| {
                    // We want to drop the guard so that the receiver can be deregistered if
                    // necessary.  To make that safe, we use an epoch based garbage collector: when
                    // deregistered, the deallocation will be deferred until it's safe.
                    let _epoch_guard = Epoch::global().guard();
                    drop(guard);

                    // SAFETY: We are holding a guard on the epoch which will prevent the receiver
                    // from being dropped.
                    unsafe { &*(data as *const ReceiverRegistrationInner<T>) }
                        .receiver
                        .receive_packet(packet);
                },
            };
        }

        self.register_raw(|key| {
            let result =
                ReceiverRegistration(ManuallyDrop::new(Box::pin(ReceiverRegistrationInner {
                    executor,
                    key,
                    receiver,
                    _pinned: PhantomPinned,
                })));
            (
                result.0.as_ref().get_ref() as *const ReceiverRegistrationInner<T> as *const (),
                &Impl::<T>::VTABLE,
                result,
            )
        })
    }

    /// Registers a pinned `RawPacketReceiver` with the executor.
    ///
    /// The registration will be deregistered when dropped.
    ///
    /// NOTE: Unlike with `register`, `receive_packet` will be called whilst a lock is held, so it
    /// is not safe to register or unregister receivers within `receve_packet`.
    pub fn register_pinned<T: PacketReceiver>(
        &self,
        ehandle: EHandle,
        raw_registration: Pin<&mut RawReceiverRegistration<T>>,
    ) {
        struct Impl<T>(PhantomData<T>);

        impl<T: PacketReceiver> Impl<T> {
            const VTABLE: RawReceiverVTable = RawReceiverVTable {
                receive_packet: |data, _guard, packet| {
                    // SAFETY: This is reversing the cast we did below and we are holding a guard
                    // which will prevent the receiver from being dropped.
                    unsafe { &*(data as *const T) }.receive_packet(packet);
                },
            };
        }

        self.register_raw(|key| {
            let reg = raw_registration.project();
            *reg.registration = Some(Registration { ehandle, key });
            (reg.receiver.as_ref().get_ref() as *const T as *const (), &Impl::<T>::VTABLE, ())
        });
    }

    fn deregister(&self, key: u64) {
        self.0.write().mapping.remove(&key).unwrap_or_else(|| panic!("invalid key"));
    }

    pub fn is_empty(&self) -> bool {
        self.0.read().mapping.is_empty()
    }
}

// SAFETY: All `PacketReceiver`s are `Send`.
unsafe impl Send for PacketReceiverMap {}
// SAFETY: All `PacketReceiver`s are `Sync`.
unsafe impl Sync for PacketReceiverMap {}

#[derive(Debug)]
struct Registration {
    ehandle: EHandle,
    key: u64,
}

pin_project! {
    /// A registration of a `PacketReceiver`.
    /// When dropped, it will automatically deregister the `PacketReceiver`.
    // NOTE: purposefully does not implement `Clone`.
    #[derive(Debug)]
    #[project(!Unpin)]
    pub struct RawReceiverRegistration<T: ?Sized> {
        registration: Option<Registration>,
        #[pin]
        receiver: T,
    }

    impl<T: ?Sized> PinnedDrop for RawReceiverRegistration<T> {
        fn drop(this: Pin<&mut Self>) {
            this.unregister();
        }
    }
}

impl<T: ?Sized> RawReceiverRegistration<T> {
    /// Returns a new `ReceiverRegistration` wrapping the given `PacketReceiver`.
    pub fn new(receiver: T) -> Self
    where
        T: Sized,
    {
        Self { registration: None, receiver }
    }

    /// Returns `true` if the receiver registration has been registered with an executor.
    pub fn is_registered(&self) -> bool {
        self.registration.is_some()
    }

    /// Registers the registration with an executor.
    pub fn register(self: Pin<&mut Self>, ehandle: EHandle)
    where
        T: PacketReceiver + Sized,
    {
        if self.registration.is_none() {
            ehandle.register_pinned(self);
        }
    }

    /// Unregisters the registration from the executor.
    ///
    /// If the registration was registered, the executor handle and key of the
    /// previous registration are returned. Otherwise, returns `None`.
    pub fn unregister(self: Pin<&mut Self>) -> Option<(EHandle, u64)> {
        let this = self.project();
        if let Some(registration) = this.registration.take() {
            registration.ehandle.inner().receivers.deregister(registration.key);
            *this.registration = None;
            Some((registration.ehandle, registration.key))
        } else {
            None
        }
    }

    /// The key with which `Packet`s destined for this receiver should be sent on the `zx::Port`.
    ///
    /// Returns `None` if this registration has not been registered yet.
    pub fn key(&self) -> Option<u64> {
        Some(self.registration.as_ref()?.key)
    }

    /// The internal `PacketReceiver`.
    ///
    /// Returns `None` if this registration has not been registered yet.
    pub fn receiver(&self) -> &T {
        &self.receiver
    }

    /// The `zx::Port` on which packets destined for this `PacketReceiver` should be queued.
    ///
    /// Returns `None` if this registration has not been registered yet.
    pub fn port(&self) -> Option<&zx::Port> {
        Some(self.registration.as_ref()?.ehandle.port())
    }
}

/// A registration of a `PacketReceiver`.
///
/// When dropped, it will automatically deregister the `PacketReceiver`. Unlike
/// `RawReceiverRegistration`, this involves a heap allocation.
// NOTE: purposefully does not implement `Clone`.
#[derive(Debug)]
pub struct ReceiverRegistration<T: Send + 'static>(
    ManuallyDrop<Pin<Box<ReceiverRegistrationInner<T>>>>,
);

struct ReceiverRegistrationInner<T> {
    executor: Arc<Executor>,
    key: u64,
    receiver: T,
    _pinned: PhantomPinned,
}

impl<T: fmt::Debug> fmt::Debug for ReceiverRegistrationInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("ReceiverRegistrationInner")
            .field("key", &self.key)
            .field("receiver", &self.receiver)
            .finish()
    }
}

impl<T: Send + 'static> Drop for ReceiverRegistration<T> {
    fn drop(&mut self) {
        self.0.executor.receivers.deregister(self.0.key);

        // SAFETY: This is the only place where we call `ManuallyDrop::take`.
        let inner = unsafe { ManuallyDrop::take(&mut self.0) };

        // We must defer the deallocation of inner as threads might currently be referencing inner.
        Epoch::global().defer(|| drop(inner));
    }
}

impl<T: Send + 'static> ReceiverRegistration<T> {
    /// The key with which `Packet`s destined for this receiver should be sent on the `zx::Port`.
    pub fn key(&self) -> u64 {
        self.0.key
    }

    /// The internal `PacketReceiver`.
    pub fn receiver(&self) -> &T {
        &self.0.receiver
    }

    /// The `zx::Port` on which packets destined for this `PacketReceiver` should be queued.
    pub fn port(&self) -> &zx::Port {
        &self.0.executor.port
    }
}

impl<T: Send + 'static> Deref for ReceiverRegistration<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.receiver()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalExecutor;

    #[test]
    fn packet_receiver_map_does_not_reuse_keys() {
        #[derive(Debug, Clone, PartialEq)]
        struct DummyPacketReceiver {
            id: i32,
        }

        impl PacketReceiver for DummyPacketReceiver {
            fn receive_packet(&self, _: zx::Packet) {
                unimplemented!()
            }
        }

        let _executor = LocalExecutor::default();
        let reg1 = EHandle::local().register_receiver(DummyPacketReceiver { id: 1 });
        assert_eq!(reg1.key(), 0);

        let reg2 = EHandle::local().register_receiver(DummyPacketReceiver { id: 2 });
        assert_eq!(reg2.key(), 1);

        // Still doesn't reuse IDs after one is removed
        drop(reg1);

        let reg3 = EHandle::local().register_receiver(DummyPacketReceiver { id: 3 });
        assert_eq!(reg3.key(), 2);
    }
}
