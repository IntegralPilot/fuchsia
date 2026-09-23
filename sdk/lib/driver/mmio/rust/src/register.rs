// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Register abstractions for MMIO.
//!
//! This module provides the [`Register`] trait which simplifies interacting with
//! bit-level registers at a fixed offset within an MMIO region.

use crate::{Mmio, MmioExt, MmioOperand};

use std::marker::PhantomData;

/// A proxy struct for interacting with a specific `Register` over a specific `Mmio`.
///
/// This provides a more ergonomic API than calling methods directly on the `MmioExt`
/// trait.
pub struct RegisterProxy<'a, M: Mmio + ?Sized, R: Register> {
    mmio: &'a M,
    _phantom: PhantomData<R>,
}

impl<'a, M: Mmio + ?Sized, R: Register> RegisterProxy<'a, M, R> {
    /// Creates a new proxy.
    pub fn new(mmio: &'a M) -> Self {
        Self { mmio, _phantom: PhantomData }
    }
}

impl<'a, M: Mmio + ?Sized, R: ReadableRegister> RegisterProxy<'a, M, R> {
    /// Reads the register from MMIO.
    pub fn read(&self) -> R {
        R::read(self.mmio)
    }
}

/// A mutable proxy struct for interacting with a specific `Register` over a specific `Mmio`.
pub struct RegisterProxyMut<'a, M: Mmio + ?Sized, R: Register> {
    mmio: &'a mut M,
    _phantom: PhantomData<R>,
}

impl<'a, M: Mmio + ?Sized, R: Register> RegisterProxyMut<'a, M, R> {
    /// Creates a new proxy.
    pub fn new(mmio: &'a mut M) -> Self {
        Self { mmio, _phantom: PhantomData }
    }
}

impl<'a, M: Mmio + ?Sized, R: ReadableRegister> RegisterProxyMut<'a, M, R> {
    /// Reads the register from MMIO.
    pub fn read(&self) -> R {
        R::read(self.mmio)
    }
}

impl<'a, M: Mmio + ?Sized, R: WritableRegister> RegisterProxyMut<'a, M, R> {
    /// Writes the register to MMIO.
    pub fn write(&mut self, val: R) {
        val.write(self.mmio)
    }
}

impl<'a, M: Mmio + ?Sized, R: ReadableRegister + WritableRegister> RegisterProxyMut<'a, M, R> {
    /// Reads, modifies with the closure, and writes the register back to MMIO.
    pub fn update<F: FnOnce(&mut R)>(&mut self, f: F) {
        let mut reg = self.read();
        f(&mut reg);
        self.write(reg);
    }
}

/// A proxy struct for interacting with a specific `IndexedRegister` over a specific `Mmio`.
pub struct IndexedRegisterProxy<'a, M: Mmio + ?Sized, R: IndexedRegister> {
    mmio: &'a M,
    _phantom: PhantomData<R>,
}

impl<'a, M: Mmio + ?Sized, R: IndexedRegister> IndexedRegisterProxy<'a, M, R> {
    /// Creates a new proxy.
    pub fn new(mmio: &'a M) -> Self {
        Self { mmio, _phantom: PhantomData }
    }
}

impl<'a, M: Mmio + ?Sized, R: ReadableIndexedRegister> IndexedRegisterProxy<'a, M, R> {
    /// Reads the register from MMIO at the specified index.
    pub fn read(&self, index: usize) -> R {
        R::read_index(self.mmio, index)
    }
}

/// A mutable proxy struct for interacting with a specific `IndexedRegister` over a specific `Mmio`.
pub struct IndexedRegisterProxyMut<'a, M: Mmio + ?Sized, R: IndexedRegister> {
    mmio: &'a mut M,
    _phantom: PhantomData<R>,
}

impl<'a, M: Mmio + ?Sized, R: IndexedRegister> IndexedRegisterProxyMut<'a, M, R> {
    /// Creates a new proxy.
    pub fn new(mmio: &'a mut M) -> Self {
        Self { mmio, _phantom: PhantomData }
    }
}

impl<'a, M: Mmio + ?Sized, R: ReadableIndexedRegister> IndexedRegisterProxyMut<'a, M, R> {
    /// Reads the register from MMIO at the specified index.
    pub fn read(&self, index: usize) -> R {
        R::read_index(self.mmio, index)
    }
}

impl<'a, M: Mmio + ?Sized, R: WritableIndexedRegister> IndexedRegisterProxyMut<'a, M, R> {
    /// Writes the register to MMIO at the specified index.
    pub fn write(&mut self, index: usize, val: R) {
        val.write_index(self.mmio, index)
    }
}

impl<'a, M: Mmio + ?Sized, R: ReadableIndexedRegister + WritableIndexedRegister>
    IndexedRegisterProxyMut<'a, M, R>
{
    /// Reads, modifies with the closure, and writes the register back to MMIO at the specified index.
    pub fn update<F: FnOnce(&mut R)>(&mut self, index: usize, f: F) {
        let mut reg = self.read(index);
        f(&mut reg);
        self.write(index, reg);
    }
}

/// A trait that allows a register to define its default read proxy type.
pub trait RegisterReadAccess<M: Mmio + ?Sized> {
    /// The proxy type used to read this register.
    type ReadProxy<'a>
    where
        M: 'a;

    /// Creates a new read proxy for this register using the provided MMIO region.
    fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a>;
}

/// A trait that allows a register to define its default write proxy type.
pub trait RegisterWriteAccess<M: Mmio + ?Sized> {
    /// The proxy type used to write this register.
    type WriteProxy<'a>
    where
        M: 'a;

    /// Creates a new write proxy for this register using the provided MMIO region.
    fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a>;
}

/// A trait for types representing a register at a fixed offset.
///
/// Types implementing this trait are typically wrappers around fundamental
/// types (u8, u16, u32, u64) that provide bit-level access to fields within
/// the register.
pub trait Register: Sized {
    /// The underlying integer type (e.g., u32) that holds the register bits.
    type Value: MmioOperand;

    /// The byte offset of this register within the MMIO region.
    const OFFSET: usize;

    /// Initializes the register type with a raw value, typically after reading from MMIO.
    fn from_raw(value: Self::Value) -> Self;

    /// Converts the register type back to its raw bits, typically before writing to MMIO.
    fn to_raw(&self) -> Self::Value;
}

/// A trait for registers that can be read.
pub trait ReadableRegister: Register {
    /// Loads the register's value from the MMIO region at its defined `OFFSET`.
    fn read<M: Mmio + ?Sized>(mmio: &M) -> Self {
        Self::from_raw(mmio.load::<Self::Value>(Self::OFFSET))
    }
}

/// A trait for registers that can be written.
pub trait WritableRegister: Register {
    /// Stores the register's current state into the MMIO region at its defined `OFFSET`.
    fn write<M: Mmio + ?Sized>(&self, mmio: &mut M) {
        mmio.store::<Self::Value>(Self::OFFSET, self.to_raw())
    }
}

/// A trait for types representing an array (block) of registers in MMIO.
///
/// Indexed registers are located at a base offset and repeat at a fixed stride.
/// They are typically accessed using a zero-based index.
///
/// # Examples
///
/// ```rust
/// register! {
///     #[indexed_register(offset = 0x100, stride = 4, count = 16, mode = RW)]
///     pub struct DataReg(u32) {
///         pub value, set_value: 31, 0;
///     }
/// }
/// ```
pub trait IndexedRegister: Sized {
    /// The underlying integer type (e.g., u32) that holds the register bits.
    type Value: MmioOperand;

    /// The byte offset of the first element in the register array.
    const BASE_OFFSET: usize;

    /// The byte distance between successive elements in the register array.
    const STRIDE: usize;

    /// The maximum number of valid elements in the register array.
    const COUNT: usize;

    /// Initializes the register type with a raw value, typically after reading from MMIO.
    fn from_raw(value: Self::Value) -> Self;

    /// Converts the register type back to its raw bits, typically before writing to MMIO.
    fn to_raw(&self) -> Self::Value;
}

/// A trait for indexed registers that can be read.
pub trait ReadableIndexedRegister: IndexedRegister {
    /// Loads the register value at `index` from the MMIO region.
    ///
    /// The offset is calculated as `BASE_OFFSET + (index * STRIDE)`.
    ///
    /// # Panics
    ///
    /// This method will panic if `index` is greater than or equal to `COUNT`.
    fn read_index<M: Mmio + ?Sized>(mmio: &M, index: usize) -> Self {
        assert!(index < Self::COUNT, "Register index out of bounds");
        let offset = Self::BASE_OFFSET + (index * Self::STRIDE);
        Self::from_raw(mmio.load::<Self::Value>(offset))
    }
}

/// A trait for indexed registers that can be written.
pub trait WritableIndexedRegister: IndexedRegister {
    /// Stores the register's state at `index` into the MMIO region.
    ///
    /// The offset is calculated as `BASE_OFFSET + (index * STRIDE)`.
    ///
    /// # Panics
    ///
    /// This method will panic if `index` is greater than or equal to `COUNT`.
    fn write_index<M: Mmio + ?Sized>(&self, mmio: &mut M, index: usize) {
        assert!(index < Self::COUNT, "Register index out of bounds");
        let offset = Self::BASE_OFFSET + (index * Self::STRIDE);
        mmio.store::<Self::Value>(offset, self.to_raw())
    }
}

/// A macro for defining a [`Register`] or [`IndexedRegister`] and its bitfields.
///
/// This macro generates a bitfield struct that implements the [`Register`] or [`IndexedRegister`],
/// [`RegisterReadAccess`] and [`RegisterWriteAccess`] traits. The access mode (RO, WO, RW)
/// determines which of the [`ReadableRegister`]/[`ReadableIndexedRegister`] and
/// [`WritableRegister`]/[`WritableIndexedRegister`] traits are implemented.
///
/// Access modes:
/// * `RO`: Read-Only.
/// * `WO`: Write-Only.
/// * `RW`: Read-Write.
///
/// # Examples
///
/// ```rust
/// register! {
///     #[register(offset = 0x10, mode = RW)]
///     pub struct StatusReg(u32) {
///         pub enabled, set_enabled: 0, 0;
///         pub error, _: 1, 1;
///         pub value, set_value: 15, 8;
///     }
/// }
/// ```
///
/// Full-width register (no bitfields):
///
/// ```rust
/// register! {
///     #[register(offset = 0x14, mode = RW)]
///     pub struct ControlReg(u32);
/// }
/// ```
///
/// Indexed register:
///
/// ```rust
/// register! {
///     #[indexed_register(offset = 0x100, stride = 4, count = 16, mode = RO)]
///     pub struct DataReg(u32) {
///         pub value, _: 31, 0;
///     }
/// }
/// ```
///
/// Full-width indexed register:
///
/// ```rust
/// register! {
///     #[indexed_register(offset = 0x200, stride = 4, count = 8, mode = RW)]
///     pub struct ValueReg(u32);
/// }
/// ```
///
/// NOTE: If you use this macro, you must add a dependency on the bitfield crate. (At the time of
/// writing, it isn't easy to avoid this.)
#[macro_export]
macro_rules! register {
    // Multiple definitions support (with block)
    (
        #[register(offset = $offset:expr, mode = $mode:ident)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
        $($tail:tt)+
    ) => {
        $crate::register! { #[register(offset = $offset, mode = $mode)] $(#[$attr])* pub struct $name($val_type) { $($field_spec)* } }
        $crate::register! { $($tail)+ }
    };

    // Multiple definitions support (full-width)
    (
        #[register(offset = $offset:expr, mode = $mode:ident)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
        $($tail:tt)+
    ) => {
        $crate::register! { #[register(offset = $offset, mode = $mode)] $(#[$attr])* pub struct $name($val_type) ; }
        $crate::register! { $($tail)+ }
    };

    // Multiple definitions support (with block, indexed)
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = $mode:ident)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
        $($tail:tt)+
    ) => {
        $crate::register! { #[indexed_register(offset = $base_offset, stride = $stride, count = $count, mode = $mode)] $(#[$attr])* pub struct $name($val_type) { $($field_spec)* } }
        $crate::register! { $($tail)+ }
    };

    // Multiple definitions support (full-width, indexed)
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = $mode:ident)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
        $($tail:tt)+
    ) => {
        $crate::register! { #[indexed_register(offset = $base_offset, stride = $stride, count = $count, mode = $mode)] $(#[$attr])* pub struct $name($val_type) ; }
        $crate::register! { $($tail)+ }
    };

    // Read-Only with block
    (
        #[register(offset = $offset:expr, mode = RO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
            $($field_spec)*
        }

        impl $crate::Register for $name {
            type Value = $val_type;
            const OFFSET: usize = $offset;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::RegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::RegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::RegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::RegisterProxyMut::new(mmio) }
        }
    };

    // Write-Only with block
    (
        #[register(offset = $offset:expr, mode = WO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
            $($field_spec)*
        }

        impl $crate::Register for $name {
            type Value = $val_type;
            const OFFSET: usize = $offset;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::WritableRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::RegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::RegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::RegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::RegisterProxyMut::new(mmio) }
        }
    };

    // Read-Write with block
    (
        #[register(offset = $offset:expr, mode = RW)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
            $($field_spec)*
        }

        impl $crate::Register for $name {
            type Value = $val_type;
            const OFFSET: usize = $offset;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableRegister for $name {}
        impl $crate::WritableRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::RegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::RegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::RegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::RegisterProxyMut::new(mmio) }
        }
    };

    // Read-Only full-width
    (
        #[register(offset = $offset:expr, mode = RO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
        }

        impl $name {
            pub fn value(&self) -> $val_type { self.0 }
        }

        impl $crate::Register for $name {
            type Value = $val_type;
            const OFFSET: usize = $offset;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::RegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::RegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::RegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::RegisterProxyMut::new(mmio) }
        }
    };

    // Write-Only full-width
    (
        #[register(offset = $offset:expr, mode = WO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
        }

        impl $name {
            pub fn set_value(&mut self, val: $val_type) { self.0 = val; }
        }

        impl $crate::Register for $name {
            type Value = $val_type;
            const OFFSET: usize = $offset;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::WritableRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::RegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::RegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::RegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::RegisterProxyMut::new(mmio) }
        }
    };

    // Read-Write full-width
    (
        #[register(offset = $offset:expr, mode = RW)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
        }

        impl $name {
            pub fn value(&self) -> $val_type { self.0 }
            pub fn set_value(&mut self, val: $val_type) { self.0 = val; }
        }

        impl $crate::Register for $name {
            type Value = $val_type;
            const OFFSET: usize = $offset;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableRegister for $name {}
        impl $crate::WritableRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::RegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::RegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::RegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::RegisterProxyMut::new(mmio) }
        }
    };

    // Read-Only with block, indexed
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = RO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
            $($field_spec)*
        }

        impl $crate::IndexedRegister for $name {
            type Value = $val_type;
            const BASE_OFFSET: usize = $base_offset;
            const STRIDE: usize = $stride;
            const COUNT: usize = $count;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableIndexedRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::IndexedRegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::IndexedRegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::IndexedRegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::IndexedRegisterProxyMut::new(mmio) }
        }
    };

    // Write-Only with block, indexed
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = WO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
            $($field_spec)*
        }

        impl $crate::IndexedRegister for $name {
            type Value = $val_type;
            const BASE_OFFSET: usize = $base_offset;
            const STRIDE: usize = $stride;
            const COUNT: usize = $count;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::WritableIndexedRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::IndexedRegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::IndexedRegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::IndexedRegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::IndexedRegisterProxyMut::new(mmio) }
        }
    };

    // Read-Write with block, indexed
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = RW)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) { $($field_spec:tt)* }
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
            $($field_spec)*
        }

        impl $crate::IndexedRegister for $name {
            type Value = $val_type;
            const BASE_OFFSET: usize = $base_offset;
            const STRIDE: usize = $stride;
            const COUNT: usize = $count;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableIndexedRegister for $name {}
        impl $crate::WritableIndexedRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::IndexedRegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::IndexedRegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::IndexedRegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::IndexedRegisterProxyMut::new(mmio) }
        }
    };

    // Read-Only full-width, indexed
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = RO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
        }

        impl $name {
            pub fn value(&self) -> $val_type { self.0 }
        }

        impl $crate::IndexedRegister for $name {
            type Value = $val_type;
            const BASE_OFFSET: usize = $base_offset;
            const STRIDE: usize = $stride;
            const COUNT: usize = $count;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableIndexedRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::IndexedRegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::IndexedRegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::IndexedRegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::IndexedRegisterProxyMut::new(mmio) }
        }
    };

    // Write-Only full-width, indexed
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = WO)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
        }

        impl $name {
            pub fn set_value(&mut self, val: $val_type) { self.0 = val; }
        }

        impl $crate::IndexedRegister for $name {
            type Value = $val_type;
            const BASE_OFFSET: usize = $base_offset;
            const STRIDE: usize = $stride;
            const COUNT: usize = $count;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::WritableIndexedRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::IndexedRegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::IndexedRegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::IndexedRegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::IndexedRegisterProxyMut::new(mmio) }
        }
    };

    // Read-Write full-width, indexed
    (
        #[indexed_register(offset = $base_offset:expr, stride = $stride:expr, count = $count:expr, mode = RW)]
        $(#[$attr:meta])*
        pub struct $name:ident ($val_type:ty) ;
    ) => {
        ::bitfield::bitfield! {
            $(#[$attr])*
            #[derive(Copy, Clone, PartialEq, Eq, Default)]
            pub struct $name($val_type);
            impl Debug;
        }

        impl $name {
            pub fn value(&self) -> $val_type { self.0 }
            pub fn set_value(&mut self, val: $val_type) { self.0 = val; }
        }

        impl $crate::IndexedRegister for $name {
            type Value = $val_type;
            const BASE_OFFSET: usize = $base_offset;
            const STRIDE: usize = $stride;
            const COUNT: usize = $count;
            fn from_raw(value: Self::Value) -> Self { $name(value) }
            fn to_raw(&self) -> Self::Value { self.0 }
        }
        impl $crate::ReadableIndexedRegister for $name {}
        impl $crate::WritableIndexedRegister for $name {}

        impl<M: $crate::Mmio + ?Sized> $crate::RegisterReadAccess<M> for $name {
            type ReadProxy<'a> = $crate::IndexedRegisterProxy<'a, M, $name> where M: 'a;
            fn get_read_proxy<'a>(mmio: &'a M) -> Self::ReadProxy<'a> { $crate::IndexedRegisterProxy::new(mmio) }
        }
        impl<M: $crate::Mmio + ?Sized> $crate::RegisterWriteAccess<M> for $name {
            type WriteProxy<'a> = $crate::IndexedRegisterProxyMut<'a, M, $name> where M: 'a;
            fn get_write_proxy<'a>(mmio: &'a mut M) -> Self::WriteProxy<'a> { $crate::IndexedRegisterProxyMut::new(mmio) }
        }
    };
}

/// A macro for generating a block of registers over an MMIO region.
///
/// This generates a wrapper struct that contains an MMIO region, and provides
/// proxy methods to interact with the registers defined in the block.
///
/// Access modes (read/write and indexed) are automatically determined from each
/// register's definition using the [`RegisterReadAccess`] and [`RegisterWriteAccess`]
/// traits.
///
/// # Examples
///
/// ```rust
/// register_block! {
///     pub struct MyBlock<M> {
///         pub status: StatusReg,
///         pub control: ControlReg,
///         pub data: DataReg, // Can be an IndexedRegister
///     }
/// }
///
/// // Usage:
/// let block = MyBlock::new(mmio);
/// let status = block.status().read();
/// ```
#[macro_export]
macro_rules! register_block {
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident <$mmio:ident> {
            $(
                $(#[$field_attr:meta])*
                $field_vis:vis $field:ident : $reg_type:ident
            ),* $(,)?
        }
    ) => {
        $(#[$attr])*
        $vis struct $name<$mmio> {
            pub mmio: $mmio,
        }

        impl<$mmio: $crate::Mmio> $name<$mmio> {
            /// Creates a new register block wrapping the given MMIO region.
            #[allow(dead_code)]
            pub fn new(mmio: $mmio) -> Self {
                Self { mmio }
            }

            $(
                $crate::paste::paste! {
                    $(#[$field_attr])*
                    #[allow(dead_code)]
                    $field_vis fn $field(&self) -> <$reg_type as $crate::RegisterReadAccess<$mmio>>::ReadProxy<'_> {
                        <$reg_type as $crate::RegisterReadAccess<$mmio>>::get_read_proxy(&self.mmio)
                    }

                    $(#[$field_attr])*
                    #[allow(dead_code)]
                    $field_vis fn [<$field _mut>](&mut self) -> <$reg_type as $crate::RegisterWriteAccess<$mmio>>::WriteProxy<'_> {
                        <$reg_type as $crate::RegisterWriteAccess<$mmio>>::get_write_proxy(&mut self.mmio)
                    }
                }
            )*
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Memory;
    use core::mem::MaybeUninit;

    register! {
        #[register(offset = 4, mode = RW)]
        pub struct TestReg(u32) {
            pub field1, set_field1: 7, 0;
            pub field2, set_field2: 15, 8;
        }
    }

    register! {
        #[register(offset = 8, mode = RO)]
        pub struct ReadOnlyReg(u32) {
            pub field1, _: 7, 0;
        }
    }

    register! {
        #[register(offset = 12, mode = WO)]
        pub struct WriteOnlyReg(u32) {
            _, set_field1: 7, 0;
        }
    }

    register! {
        #[indexed_register(offset = 16, stride = 4, count = 2, mode = RW)]
        pub struct TestIndexedReg(u32) {
            pub field1, set_field1: 7, 0;
        }
    }

    register_block! {
        pub struct TestBlock<M> {
            pub test_reg: TestReg,
            pub ro_reg: ReadOnlyReg,
            pub wo_reg: WriteOnlyReg,
            pub indexed: TestIndexedReg,
        }
    }

    register_block! {
        /// Will not compile if the attribute below does not pass through.
        #[expect(dead_code)]
        pub struct TestBlockWithAttributtes<M> {
            pub test_reg: TestReg,

            /// Documented field.
            pub ro_reg: ReadOnlyReg,

            /// Another documented field.
            pub wo_reg: WriteOnlyReg,
        }
    }

    #[test]
    fn test_register_read_write() {
        let mut mem = MaybeUninit::<[u32; 8]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        let mut reg = TestReg::default();
        reg.set_field1(0x12);
        reg.set_field2(0x34);

        reg.write(&mut mmio);

        let reg2 = TestReg::read(&mmio);
        assert_eq!(reg, reg2);
        assert_eq!(reg2.field1(), 0x12);
        assert_eq!(reg2.field2(), 0x34);
    }

    #[test]
    fn test_update_reg() {
        let mut mem = MaybeUninit::<[u32; 8]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        mmio.update_reg::<TestReg, _>(|reg| {
            reg.set_field1(0xab);
        });

        let reg = mmio.read_reg::<TestReg>();
        assert_eq!(reg.field1(), 0xab);
        assert_eq!(reg.field2(), 0);
    }

    #[test]
    fn test_register_proxy() {
        let mut mem = MaybeUninit::<[u32; 8]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        // Test RegisterProxy (read-only)
        mmio.store32(4, 0x1234);
        let proxy = mmio.reg::<TestReg>();
        assert_eq!(proxy.read().field1(), 0x34);

        // Test RegisterProxyMut (read-write)
        {
            let mut proxy_mut = mmio.reg_mut::<TestReg>();
            proxy_mut.update(|reg| {
                reg.set_field1(0x56);
            });
            assert_eq!(proxy_mut.read().field1(), 0x56);

            proxy_mut.write(TestReg(0x78));
        }
        assert_eq!(mmio.load32(4), 0x78);
    }

    #[test]
    fn test_indexed_register() {
        let mut mem = MaybeUninit::<[u32; 12]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        let mut reg = TestIndexedReg::default();
        reg.set_field1(0x11);
        reg.write_index(&mut mmio, 0);

        reg.set_field1(0x22);
        reg.write_index(&mut mmio, 1);

        assert_eq!(mmio.load32(16), 0x11);
        assert_eq!(mmio.load32(20), 0x22);

        let reg0 = TestIndexedReg::read_index(&mmio, 0);
        assert_eq!(reg0.field1(), 0x11);

        let reg1 = TestIndexedReg::read_index(&mmio, 1);
        assert_eq!(reg1.field1(), 0x22);
    }

    #[test]
    fn test_indexed_register_proxy() {
        let mut mem = MaybeUninit::<[u32; 12]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        {
            let mut proxy = mmio.indexed_reg_mut::<TestIndexedReg>();
            proxy.write(0, TestIndexedReg(0xaa));
            proxy.update(1, |reg| reg.set_field1(0xbb));

            assert_eq!(proxy.read(0).field1(), 0xaa);
            assert_eq!(proxy.read(1).field1(), 0xbb);
        }

        assert_eq!(mmio.load32(16), 0xaa);
        assert_eq!(mmio.load32(20), 0xbb);
    }

    #[test]
    #[should_panic(expected = "Register index out of bounds")]
    fn test_indexed_register_out_of_bounds_read() {
        let mut mem = MaybeUninit::<[u32; 12]>::zeroed();
        let mmio = Memory::borrow_uninit(&mut mem);
        let _ = TestIndexedReg::read_index(&mmio, 2);
    }

    #[test]
    #[should_panic(expected = "Register index out of bounds")]
    fn test_indexed_register_out_of_bounds_write() {
        let mut mem = MaybeUninit::<[u32; 12]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);
        let reg = TestIndexedReg::default();
        reg.write_index(&mut mmio, 2);
    }

    #[test]
    fn test_register_block() {
        let mut mem = MaybeUninit::<[u32; 12]>::zeroed();
        let mmio = Memory::borrow_uninit(&mut mem);
        let mut block = TestBlock::new(mmio);

        block.test_reg_mut().write(TestReg(0x1234));
        assert_eq!(block.mmio.load32(4), 0x1234);

        block.mmio.store32(8, 0xabcd);
        assert_eq!(block.ro_reg().read().field1(), 0xcd);

        block.wo_reg_mut().write(WriteOnlyReg(0x55));
        assert_eq!(block.mmio.load32(12), 0x55);

        block.indexed_mut().write(0, TestIndexedReg(0x11));
        block.indexed_mut().write(1, TestIndexedReg(0x22));
        assert_eq!(block.mmio.load32(16), 0x11);
        assert_eq!(block.mmio.load32(20), 0x22);

        // Test immutable access
        let block = block;
        assert_eq!(block.test_reg().read().field1(), 0x34);
        assert_eq!(block.ro_reg().read().field1(), 0xcd);
        assert_eq!(block.indexed().read(0).field1(), 0x11);
    }

    register! {
        #[register(offset = 4, mode = RW)]
        /// New syntax test register
        pub struct NewSyntaxReg(u32) {
            pub field1, set_field1: 7, 0;
        }
    }

    register! {
        #[register(offset = 8, mode = RW)]
        /// Full width test register
        pub struct FullWidthReg(u32);
    }

    register! {
        #[indexed_register(offset = 12, stride = 4, count = 2, mode = RW)]
        /// New syntax indexed register
        pub struct NewSyntaxIndexedReg(u32) {
            pub field1, set_field1: 7, 0;
        }
    }

    register! {
        #[register(offset = 0x20, mode = RW)]
        pub struct MultiReg1(u32);
        #[register(offset = 0x24, mode = RO)]
        pub struct MultiReg2(u32) {
            pub field1, _: 7, 0;
        }
    }

    #[test]
    fn test_multi_register() {
        let mut mem = MaybeUninit::<[u32; 12]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        let mut reg1 = MultiReg1::default();
        reg1.set_value(0x12345678);
        reg1.write(&mut mmio);

        mmio.store32(0x24, 0xabcd);

        let reg1_read = MultiReg1::read(&mmio);
        assert_eq!(reg1_read.value(), 0x12345678);

        let reg2_read = MultiReg2::read(&mmio);
        assert_eq!(reg2_read.field1(), 0xcd);
    }

    #[test]
    fn test_new_syntax() {
        let mut mem = MaybeUninit::<[u32; 8]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        let mut reg = NewSyntaxReg::default();
        reg.set_field1(0x55);
        reg.write(&mut mmio);

        let reg2 = NewSyntaxReg::read(&mmio);
        assert_eq!(reg2.field1(), 0x55);
    }

    #[test]
    fn test_full_width() {
        let mut mem = MaybeUninit::<[u32; 8]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        let mut reg = FullWidthReg::default();
        reg.set_value(0x12345678);
        reg.write(&mut mmio);

        let reg2 = FullWidthReg::read(&mmio);
        assert_eq!(reg2.value(), 0x12345678);
    }

    #[test]
    fn test_new_syntax_indexed() {
        let mut mem = MaybeUninit::<[u32; 8]>::zeroed();
        let mut mmio = Memory::borrow_uninit(&mut mem);

        let mut reg = NewSyntaxIndexedReg::default();
        reg.set_field1(0xaa);
        reg.write_index(&mut mmio, 0);

        reg.set_field1(0xbb);
        reg.write_index(&mut mmio, 1);

        let reg0 = NewSyntaxIndexedReg::read_index(&mmio, 0);
        assert_eq!(reg0.field1(), 0xaa);

        let reg1 = NewSyntaxIndexedReg::read_index(&mmio, 1);
        assert_eq!(reg1.field1(), 0xbb);
    }
}
