// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg_attr(not(any(test, feature = "testing")), no_std)]

pub mod arm64;
mod mmio;
pub mod riscv64;
pub mod traits;
pub mod x86;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

use core::marker::PhantomData;

pub use mmio::{Mmio, MmioBank, MmioPtr, Offset};

mod private {
    pub trait Sealed {}
}

/// Base marker for register access permissions.
///
/// This trait is sealed and cannot be implemented outside of this crate,
/// ensuring that permissions cannot be forged.
pub trait Accessible: private::Sealed {}

/// A tag for register read access.
#[derive(Clone, Copy, Debug)]
pub enum Read {}

/// A tag for disallowed register read access.
#[derive(Clone, Copy, Debug)]
pub enum NoRead {}

/// A tag for safe register write access: this is when writes to the register
/// are statically known to be sound (e.g., in the cases of MMIO that lights up
/// an LED or a performance counter system register).
#[derive(Clone, Copy, Debug)]
pub enum SafeWrite {}

/// A tag for unsafe register write access: this is when writes to the register
/// may contextually violate the memory model (e.g., system register that
/// toggles the MMU or in modeling a page table entry as a register). In such
/// cases, the user must attest to any given write being sound.
#[derive(Clone, Copy, Debug)]
pub enum UnsafeWrite {}

/// A tag for disallowed register write access.
#[derive(Clone, Copy, Debug)]
pub enum NoWrite {}

impl private::Sealed for (Read, NoWrite) {}
impl private::Sealed for (Read, SafeWrite) {}
impl private::Sealed for (Read, UnsafeWrite) {}
impl private::Sealed for (NoRead, SafeWrite) {}
impl private::Sealed for (NoRead, UnsafeWrite) {}

impl Accessible for (Read, NoWrite) {}
impl Accessible for (Read, SafeWrite) {}
impl Accessible for (Read, UnsafeWrite) {}
impl Accessible for (NoRead, SafeWrite) {}
impl Accessible for (NoRead, UnsafeWrite) {}

/// Marker for readable register access.
pub trait Readable: Accessible {}
impl Readable for (Read, NoWrite) {}
impl Readable for (Read, SafeWrite) {}
impl Readable for (Read, UnsafeWrite) {}

/// Marker for writable register access.
pub trait Writable: Accessible {}
impl Writable for (Read, SafeWrite) {}
impl Writable for (Read, UnsafeWrite) {}
impl Writable for (NoRead, SafeWrite) {}
impl Writable for (NoRead, UnsafeWrite) {}

// Aliases for brevity.

/// A tag for read-only register access.
pub type Ro = (Read, NoWrite);

/// A tag for read + safe-write register access.
pub type RwSafe = (Read, SafeWrite);

/// A tag for read + unsafe-write register access.
pub type RwUnsafe = (Read, UnsafeWrite);

/// A tag for safe-write-only register access.
pub type WoSafe = (NoRead, SafeWrite);

/// A tag for unsafe-write-only register access.
pub type WoUnsafe = (NoRead, UnsafeWrite);

/// Expresses how one set of access permissions can imply a narrower set. For
/// example, read-writable should imply readable.
pub trait AccessRestrictsTo<Access: Accessible>: Accessible {}

// Any set of permissions restricts to itself.
impl<Access: Accessible> AccessRestrictsTo<Access> for Access {}

impl AccessRestrictsTo<(Read, NoWrite)> for (Read, SafeWrite) {}
impl AccessRestrictsTo<(Read, NoWrite)> for (Read, UnsafeWrite) {}

impl AccessRestrictsTo<(Read, UnsafeWrite)> for (Read, SafeWrite) {}

impl AccessRestrictsTo<(NoRead, SafeWrite)> for (Read, SafeWrite) {}
impl AccessRestrictsTo<(NoRead, UnsafeWrite)> for (Read, UnsafeWrite) {}

impl AccessRestrictsTo<(NoRead, UnsafeWrite)> for (NoRead, SafeWrite) {}
impl AccessRestrictsTo<(NoRead, UnsafeWrite)> for (Read, SafeWrite) {}

/// A convenience supertrait for the traits expected of layout types over a
/// specific base type. This is not intended to be implemented explicitly, only
/// through its blanket implementation.
pub trait LayoutOver<Base>: Copy + From<Base> + Into<Base> {}

impl<Layout, Base> LayoutOver<Base> for Layout where Layout: Copy + From<Base> + Into<Base> {}

/// Represents an abstracted means of register access.
pub trait IoHandle {
    /// The underlying base type of the register (assumed integral in
    /// practice).
    type Base: Copy;
}

/// Represents an abstracted means of register reads.
pub trait ReadHandle: IoHandle {
    /// Performs the read.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the implementation-specific
    /// preconditions for a sound read are met. For example, in the case of a
    /// pointer, that the instance is valid, properly aligned, and pointing to
    /// initialized memory.
    unsafe fn read_raw(&self) -> Self::Base;
}

/// Represents an abstracted means of register writes.
pub trait WriteHandle: IoHandle {
    /// Performs the write of the provided value.
    ///
    /// # Safety
    ///
    /// The caller must guarantee...
    ///
    /// * that the value-agnostic, implementation-specific preconditions for a
    ///   sound write are met;
    ///
    /// * and further that the particular value will not cause undefined
    ///   behaviour when written (in an implementation-specific way).
    ///
    /// For example, in the case of a pointer to an MMIO address, the
    /// conditions would be that the pointer instance is valid and aligned, and
    /// that the particular value to write does not misconfigure hardware in a
    /// way that leads to undefined behavior (such as configuring DMA to
    /// overwrite arbitrary memory).
    unsafe fn write_raw(&self, value: Self::Base);
}

/// Represents an abstracted means of atomic read-modify-write register I/O.
//
// TODO(https://github.com/rust-lang/rust/issues/132980): Consider generic
// versions of the bit-set and bit-clear methods when we can consider const
// generics of layout values.
pub trait AtomicIoHandle: ReadHandle + WriteHandle {
    /// Atomically swaps the contents of the register with the provided value,
    /// returning the original contents.
    ///
    /// # Safety
    ///
    /// The caller must guarantee...
    ///
    /// * that the value-agnostic, implementation-specific preconditions for a
    ///   sound write are met;
    ///
    /// * and further that the particular value will not cause undefined
    ///   behaviour when written (in an implementation-specific way).
    unsafe fn atomic_swap_raw(&self, value: Self::Base) -> Self::Base;

    /// Atomically sets the provided bits on the contents of the register,
    /// returning the original contents.
    ///
    /// # Safety
    ///
    /// The caller must guarantee...
    ///
    /// * that the value-agnostic, implementation-specific preconditions for a
    ///   sound write are met;
    ///
    /// * and further that the particular value will not cause undefined
    ///   behaviour when written (in an implementation-specific way).
    unsafe fn atomic_set_bits_raw(&self, bits: Self::Base) -> Self::Base;

    /// Atomically clears the provided bits on the contents of the register,
    /// returning the original contents.
    ///
    /// # Safety
    ///
    /// The caller must guarantee...
    ///
    /// * that the value-agnostic, implementation-specific preconditions for a
    ///   sound write are met;
    ///
    /// * and further that the particular value will not cause undefined
    ///   behaviour when written (in an implementation-specific way).
    unsafe fn atomic_clear_bits_raw(&self, bits: Self::Base) -> Self::Base;
}

/// `Register` represents a structured register layout, and its access
/// interface and permissions. This is the core abstraction of the crate.
///
/// If `Access` expresses unsafe-writability, then all write methods are marked
/// as unsafe.
#[derive(Clone, Copy, Debug)]
pub struct Register<Layout, Access, Io>
where
    Io: IoHandle,
    Layout: LayoutOver<<Io as IoHandle>::Base>,
    Access: Accessible,
{
    io: Io,
    _marker: PhantomData<(Layout, Access)>,
}

impl<Layout, Access, Io> Register<Layout, Access, Io>
where
    Io: IoHandle,
    Layout: LayoutOver<<Io as IoHandle>::Base>,
    Access: Accessible,
{
    /// Constructs a new register from an I/O handle.
    ///
    /// # Safety
    ///
    /// The caller must guarantee...
    ///
    /// * that the provided I/O handle meets the implementation-specific
    ///   access safety preconditions for the duration of the lifetimes of the
    ///   register instance and any copies of it (e.g., in the case of an MMIO
    ///   pointer, that the pointer is aligned and the memory it points to
    ///   remains mapped for those lifetimes);
    ///
    /// * and that `Access` correctly models the safe nature of access in this
    ///   context.
    pub const unsafe fn from_io(io: Io) -> Self {
        Self { io, _marker: PhantomData }
    }

    pub const fn io(&self) -> &Io {
        &self.io
    }

    // TODO(https://github.com/rust-lang/rust/issues/73255): Make this const
    // when ergonomically easy. Ditto for the into_*() methods below.
    pub fn into_io(self) -> Io {
        self.io
    }

    /// Reads from the register, if the backend and register permit it.
    #[inline]
    pub fn read(&self) -> Layout
    where
        Access: Readable,
        Io: ReadHandle,
    {
        // Safety: The I/O handle was attested as meeting the handle-specific
        // preconditions for reading for the duration of our lifetime.
        unsafe { self.io.read_raw().into() }
    }

    // Write subroutine consolidating the handle-specific safety justification
    // of the access.
    //
    // # Safety
    //
    // The caller must guarantee that the particular value will not cause
    // undefined behaviour when written.
    #[inline(always)]
    unsafe fn write_impl(&self, value: Layout)
    where
        Access: Writable,
        Io: WriteHandle,
    {
        // Safety: The I/O handle was attested as meeting the
        // handle-specific preconditions for writing for our lifetime; the
        // value-specific preconditions are left to the caller to justify in
        // the case of unsafe-writability. Moreover, the caller attested to the
        // safeness/unsafeness of the write access in general.
        unsafe { self.io.write_raw(value.into()) }
    }

    // Atomic swap subroutine consolidating the handle-specific safety
    // justification of the access.
    //
    // # Safety
    //
    // The caller must guarantee that the particular value will not cause
    // undefined behaviour when written.
    #[inline(always)]
    unsafe fn atomic_swap_impl(&self, value: Layout) -> Layout
    where
        Access: Readable + Writable,
        Io: AtomicIoHandle,
    {
        // Safety: The I/O handle was attested as meeting the
        // handle-specific preconditions for writing for our lifetime; the
        // value-specific preconditions are left to the caller to justify in
        // the case of unsafe-writability. Moreover, the caller attested to the
        // safeness/unsafeness of the write access in general.
        unsafe { self.io.atomic_swap_raw(value.into()).into() }
    }

    // Atomic bit-set subroutine consolidating the handle-specific safety
    // justification of the access.
    //
    // # Safety
    //
    // The caller must guarantee that the particular value will not cause
    // undefined behaviour when written.
    #[inline(always)]
    unsafe fn atomic_set_bits_impl(&self, value: Layout) -> Layout
    where
        Access: Readable + Writable,
        Io: AtomicIoHandle,
    {
        // Safety: The I/O handle was attested as meeting the
        // handle-specific preconditions for writing for our lifetime; the
        // value-specific preconditions are left to the caller to justify in
        // the case of unsafe-writability. Moreover, the caller attested to the
        // safeness/unsafeness of the write access in general.
        unsafe { self.io.atomic_set_bits_raw(value.into()).into() }
    }

    // Atomic bit-clear subroutine consolidating the handle-specific safety
    // justification of the access.
    //
    // # Safety
    //
    // The caller must guarantee that the particular value will not cause
    // undefined behaviour when written.
    #[inline(always)]
    unsafe fn atomic_clear_bits_impl(&self, value: Layout) -> Layout
    where
        Access: Readable + Writable,
        Io: AtomicIoHandle,
    {
        // Safety: The I/O handle was attested as meeting the
        // handle-specific preconditions for writing for our lifetime; the
        // value-specific preconditions are left to the caller to justify in
        // the case of unsafe-writability. Moreover, the caller attested to the
        // safeness/unsafeness of the write access in general.
        unsafe { self.io.atomic_clear_bits_raw(value.into()).into() }
    }
}

// This will be used to stamp out write-related methods differing only in
// safety, across the disjoint cases of safe- and unsafe-writability
macro_rules! impl_writable {
    ($write_kind:ident) => {
        impl_writable!(@impl $write_kind);
    };
    ($write_kind:ident, unsafe) => {
        impl_writable!(
            @impl $write_kind,
            unsafe,
            ///
            /// # Safety
            ///
            /// The caller must guarantee that the write does not result in
            /// undefined behaviour.
        );
    };
    (
        @impl $write_kind:ident
        $(, $unsafe:ident )?
        $(, $(#[$safety_doc:meta])* )?
    ) => {
        impl<Layout, R, Io> Register<Layout, (R, $write_kind), Io>
        where
            Io: WriteHandle,
            Layout: LayoutOver<<Io as IoHandle>::Base>,
            (R, $write_kind): Writable,
        {
            /// Writes to the register, if the backend and register permit it.
            $($(#[$safety_doc])*)?
            #[inline]
            pub $($unsafe)? fn write(&self, value: Layout) {
                // Safety: In the case of unsafe-writability, this method is
                // unsafe and the caller themselves must provide the
                // justification.
                #[allow(unused_unsafe)]
                unsafe { self.write_impl(value) }
            }

            /// Modifies the contents of the register, if the register and backend admit
            /// both reads and writes, returning whatever state the modification
            /// callback wanted to forward.
            $($(#[$safety_doc])*)?
            #[inline]
            pub $($unsafe)? fn modify<ModifyFn, Ret>(&self, cb: ModifyFn) -> Ret
            where
                (R, $write_kind): Readable,
                Io: ReadHandle,
                ModifyFn: FnOnce(&mut Layout) -> Ret,
            {
                let mut value = self.read();
                let ret = cb(&mut value);

                // Safety: In the case of unsafe-writability, this method is
                // unsafe and the caller themselves must provide the
                // justification.
                #[allow(unused_unsafe)]
                unsafe { self.write_impl(value) }
                ret
            }

            /// Atomically swaps the contents of the register with the provided
            /// value, returning the original contents.
            $($(#[$safety_doc])*)?
            #[inline]
            pub $($unsafe)? fn atomic_swap(&self, value: Layout) -> Layout
            where
                (R, $write_kind): Readable,
                Io: AtomicIoHandle,
            {
                // Safety: In the case of unsafe-writability, this method is
                // unsafe and the caller themselves must provide the
                // justification.
                #[allow(unused_unsafe)]
                unsafe { self.atomic_swap_impl(value) }
            }

            /// Atomically sets the provided bits on the contents of the
            /// register, returning the original contents.
            $($(#[$safety_doc])*)?
            #[inline]
            pub $($unsafe)? fn atomic_set_bits(&self, bits: Layout) -> Layout
            where
                (R, $write_kind): Readable,
                Io: AtomicIoHandle,
            {
                // Safety: In the case of unsafe-writability, this method is
                // unsafe and the caller themselves must provide the
                // justification.
                #[allow(unused_unsafe)]
                unsafe { self.atomic_set_bits_impl(bits) }
            }

            /// Atomically clears the provided bits on the contents of the
            /// register, returning the original contents.
            $($(#[$safety_doc])*)?
            #[inline]
            pub $($unsafe)? fn atomic_clear_bits(&self, bits: Layout) -> Layout
            where
                (R, $write_kind): Readable,
                Io: AtomicIoHandle,
            {
                // Safety: In the case of unsafe-writability, this method is
                // unsafe and the caller themselves must provide the
                // justification.
                #[allow(unused_unsafe)]
                unsafe { self.atomic_clear_bits_impl(bits) }
            }
        }
    };
}

impl_writable!(SafeWrite);
impl_writable!(UnsafeWrite, unsafe);
