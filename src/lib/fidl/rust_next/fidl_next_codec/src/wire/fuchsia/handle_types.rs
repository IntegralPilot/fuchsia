// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::mem::MaybeUninit;

use crate::fuchsia::{HandleDecoder, HandleEncoder};
use crate::{
    Constrained, Decode, DecodeError, Encode, EncodeError, EncodeOption, FromWire, FromWireOption,
    IntoNatural, Slot, ValidationError, Wire, munge, wire,
};

use zx::sys::zx_handle_t;

macro_rules! define_wire_handle_types {
    ($(
        $wire:ident($wire_optional:ident):
            $natural:ident $(<$($generics:ident $(: $bound:path)?),+>)?
    ),* $(,)?) => { $(
        #[doc = concat!("A Zircon ", stringify!($natural), ".")]
        #[derive(Debug)]
        #[repr(transparent)]
        pub struct $wire {
            handle: wire::fuchsia::Handle,
        }

        // TODO: validate handle rights.
        impl Constrained for $wire {
            type Constraint = ();

            fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
                Ok(())
            }
        }

        // SAFETY: `$wire` is a `#[repr(transparent)]` wrapper around `wire::fuchsia::Handle`,
        // which is `Wire`.
        unsafe impl Wire for $wire {
            type Narrowed<'de> = Self;

            #[inline]
            fn zero_padding(out: &mut MaybeUninit<Self>) {
                munge!(let Self { handle } = out);
                wire::fuchsia::Handle::zero_padding(handle);
            }
        }

        impl $wire {
            #[doc = concat!("Encodes a ", stringify!($natural), " as present in an output.")]
            pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
                munge!(let Self { handle } = out);
                wire::fuchsia::Handle::set_encoded_present(handle);
            }

            /// Returns whether the underlying `zx_handle_t` is invalid.
            pub fn is_invalid(&self) -> bool {
                self.handle.is_invalid()
            }

            /// Returns the underlying [`zx_handle_t`].
            #[inline]
            pub fn as_raw_handle(&self) -> zx_handle_t {
                self.handle.as_raw_handle()
            }
        }

        // SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded
        // `$wire` because it delegates to `Handle::decode` which guarantees the slot is valid.
        unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for $wire {
            fn decode(
                mut slot: Slot<'_, Self>,
                decoder: &mut D,
                constraint: Self::Constraint,
            ) -> Result<(), DecodeError> {
                munge!(let Self { handle } = slot.as_mut());
                wire::fuchsia::Handle::decode(handle, decoder, constraint)
            }
        }

        #[doc = concat!("An optional Zircon ", stringify!($natural), ".")]
        #[derive(Debug)]
        #[repr(transparent)]
        pub struct $wire_optional {
            handle: wire::fuchsia::OptionalHandle,
        }

        // TODO: validate handle rights.
        impl Constrained for $wire_optional {
            type Constraint = ();

            fn validate(_: Slot<'_, Self>, _: Self::Constraint) -> Result<(), ValidationError> {
                Ok(())
            }
        }

        // SAFETY: `$wire_optional` is a `#[repr(transparent)]` wrapper around
        // `wire::fuchsia::OptionalHandle`, which is `Wire`.
        unsafe impl Wire for $wire_optional {
            type Narrowed<'de> = Self;

            #[inline]
            fn zero_padding(out: &mut MaybeUninit<Self>) {
                munge!(let Self { handle } = out);
                wire::fuchsia::OptionalHandle::zero_padding(handle);
            }
        }

        impl $wire_optional {
            #[doc = concat!("Encodes a ", stringify!($natural), " as present in an output.")]
            pub fn set_encoded_present(out: &mut MaybeUninit<Self>) {
                munge!(let Self { handle } = out);
                wire::fuchsia::OptionalHandle::set_encoded_present(handle);
            }

            #[doc = concat!("Encodes a ", stringify!($natural), " as absent in an output.")]
            pub fn set_encoded_absent(out: &mut MaybeUninit<Self>) {
                munge!(let Self { handle } = out);
                wire::fuchsia::OptionalHandle::set_encoded_absent(handle);
            }

            #[doc = concat!("Returns whether a ", stringify!($natural), " is present.")]
            pub fn is_some(&self) -> bool {
                !self.handle.is_some()
            }

            #[doc = concat!("Returns whether a ", stringify!($natural), " is absent.")]
            pub fn is_none(&self) -> bool {
                self.handle.is_none()
            }

            /// Returns the underlying [`zx_handle_t`], if any.
            #[inline]
            pub fn as_raw_handle(&self) -> Option<zx_handle_t> {
                self.handle.as_raw_handle()
            }
        }

        // SAFETY: If `decode` returns `Ok`, `slot` is guaranteed to contain a valid decoded
        // `$wire_optional` because it delegates to `OptionalHandle::decode` which guarantees the
        // slot is valid.
        unsafe impl<D: HandleDecoder + ?Sized> Decode<D> for $wire_optional {
            fn decode(
                mut slot: Slot<'_, Self>,
                decoder: &mut D, constraint: Self::Constraint,
            ) -> Result<(), DecodeError> {
                munge!(let Self { handle } = slot.as_mut());
                wire::fuchsia::OptionalHandle::decode(handle, decoder, constraint)
            }
        }

        // SAFETY: `$wire` is `#[repr(transparent)]` over `Handle`. `encode` delegates to
        // `zx::NullableHandle`'s `Encode` implementation, which fully initializes the underlying
        // `Handle`, thus initializing `$wire`.
        unsafe impl<
            E: HandleEncoder + ?Sized,
            $($($generics $(: $bound)?,)+)?
        > Encode<$wire, E> for zx::$natural $(<$($generics,)+>)? {
            fn encode(
                self,
                encoder: &mut E,
                out: &mut MaybeUninit<$wire>,
                constraint:  <$wire as Constrained>::Constraint,
            ) -> Result<(), EncodeError> {
                munge!(let $wire { handle } = out);
                zx::NullableHandle::from(self).encode(encoder, handle, constraint)
            }
        }

        impl $(<$($generics $(: $bound)?,)+>)? FromWire<$wire>
            for zx::$natural $(<$($generics,)+>)?
        {
            fn from_wire(wire: $wire) -> Self {
                zx::NullableHandle::from_wire(wire.handle).into()
            }
        }

        impl IntoNatural for $wire {
            type Natural = zx::$natural;
        }

        // SAFETY: `$wire_optional` is `#[repr(transparent)]` over `OptionalHandle`.
        // `encode_option` delegates to `zx::NullableHandle`'s `EncodeOption` implementation (via
        // `Option`'s `Encode`), which fully initializes the underlying `OptionalHandle`, thus
        // initializing `$wire_optional`.
        unsafe impl<
            E: HandleEncoder + ?Sized,
            $($($generics $(: $bound)?,)+)?
        > EncodeOption<$wire_optional, E> for zx::$natural $(<$($generics,)+>)? {
            fn encode_option(
                this: Option<Self>,
                encoder: &mut E,
                out: &mut MaybeUninit<$wire_optional>,
                constraint: (),
            ) -> Result<(), EncodeError> {
                munge!(let $wire_optional { handle } = out);
                Encode::encode(this.map(zx::NullableHandle::from), encoder, handle, constraint)
            }
        }

        impl $(<$($generics $(: $bound)?,)+>)? FromWireOption<$wire_optional>
            for zx::$natural $(<$($generics,)+>)?
        {
            fn from_wire_option(wire: $wire_optional) -> Option<Self> {
                zx::NullableHandle::from_wire_option(wire.handle).map(zx::$natural::from)
            }
        }

        impl IntoNatural for $wire_optional {
            type Natural = Option<zx::$natural>;
        }
    )* };
}

define_wire_handle_types! {
    Process(OptionalProcess): Process,
    Thread(OptionalThread): Thread,
    Vmo(OptionalVmo): Vmo,
    Channel(OptionalChannel): Channel,
    Event(OptionalEvent): Event,
    Port(OptionalPort): Port,
    Interrupt(OptionalInterrupt): Interrupt<K: zx::InterruptKind, T: zx::Timeline>,
    // PciDevice(OptionalPciDevice): PciDevice,
    DebugLog(OptionalDebugLog): DebugLog,
    Socket(OptionalSocket): Socket,
    Resource(OptionalResource): Resource,
    EventPair(OptionalEventPair): EventPair,
    Job(OptionalJob): Job,
    Vmar(OptionalVmar): Vmar,
    Fifo(OptionalFifo): Fifo<R, W>,
    Guest(OptionalGuest): Guest,
    Vcpu(OptionalVcpu): Vcpu,
    Timer(OptionalTimer): Timer<T: zx::Timeline>,
    Iommu(OptionalIommu): Iommu,
    Bti(OptionalBti): Bti,
    Profile(OptionalProfile): Profile,
    Pmt(OptionalPmt): Pmt,
    // SuspendToken(OptionalSuspendToken): SuspendToken,
    Pager(OptionalPager): Pager,
    Exception(OptionalException): Exception,
    Clock(OptionalClock): Clock<Reference: zx::Timeline, Output: zx::Timeline>,
    Stream(OptionalStream): Stream,
    // Msi(OptionalMsi): Msi,
    Iob(OptionalIob): Iob,
    Counter(OptionalCounter): Counter,
}
