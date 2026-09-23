// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![deny(missing_docs)]
//! Safe Memory Mapped I/O for Fuchsia drivers.
//!
//! MMIO allows a program to interact with devices through operations on its address space. While
//! the interface is similar to regular memory access, there are some key semantic differences.
//! This crate provides safe interfaces for interacting with MMIO that are intended to be sound.
//!
//! # Memory semantics
//!
//! ## Safe Rust
//! The semantics of a safe Rust program are determined by the Rust Abstract Machine. The Rust
//! Abstract Machine notably does not provide certain guarantees that are required when interacting
//! with devices:
//!
//! - Guaranteed execution: I/O operations may be elided if the elision would have no impact on the
//!   observable behavior of the executing thread (they may also be repeated).
//! - Operation ordering: operations may be reordered if doing so does not impact the observable
//!   behavior of the executing thread.
//!
//! For memory that is under the control of the Rust Abstract Machine, the semantics of a safe Rust
//! program can depend on memory not changing value without a corresponding write.
//!
//! ## Device Memory
//! Device memory is not under the control of the Rust Abstract Machine. The guarantees around the
//! state and behavior of device memory do not provide the guarantees required for safe Rust.
//! Similarly, safe Rust cannot encode the semantics required to interact correctly with device
//! memory.
//!
//! ### Side Effects
//! Reading from or writing to device memory may have observable side effects, such as clearing a
//! status flag, advancing a buffer, or changing device state in some other way.
//!
//! ### Value Volatility
//! The value that would be returned when loading from a device memory address may change
//! completely independently of the program's control. Two subsequent reads on the same thread with
//! no interleaved write may return different values. A read of a just-written value is not
//! guaranteed to return the just-written value.
//!
//! ### Atomicity
//! A memory instruction which may be atomic when operating on regular memory is not guaranteed to
//! be atomic when interfacing with a device. For example a memory operation with a width larger
//! than the bus size supported by the device *must* be broken down into multiple operations.
//!
//! ### Elision and Spurious Accesses
//! As mentioned already, memory operations may be ellided or duplicated when executing safe Rust
//! provided they don't change the single-threaded semantics of the program. If a shared or mutable
//! reference to an object is active, memory operations may be performed to the backing memory. For
//! this reason accesses to a memory location via pointers should only be performed when there are
//! no active references that may alias the same memory.
//!
//! ### Compiler Reordering and Volatile
//! The Rust Compiler has some freedom to reorder instructions. For regular memory accesses the
//! compiler may reorder instructions where a data dependency does not prevent the reordering.
//!
//! The compiler is not allowed to reorder `volatile` operations with respect to other `volatile`
//! operations in the same thread. It is free to reorder `volatile` operations with other kinds of
//! memory accesses.
//!
//! By themselves, `volatile` operations cannot provide any sequencing guarantees with respect to
//! non-volatile operations on the same thread, or *any* kind of operation on another thread.
//!
//! # User Guide
//!
//! Users of this crate interact with device memory through the [Mmio] trait. This trait exposes
//! the low-level load and store operations to safely interact with device memory. All stores
//! performed through an [Mmio] instance are issued through a mutable reference.
//!
//! If an [Mmio] implementation also implements [MmioSplit], it is possible to split off
//! independently owned sub-regions from it. This allows concurrent mutable access to disjoint
//! MMIO regions.
//!
//! ## Operand Types
//! The [Mmio] trait provides load and store operations for the following types:
//!
//! - `u8`
//! - `u16`
//! - `u32`
//! - `u64`
//!
//! Conforming implementations of [Mmio] must perform these operations in the code order relative
//! to each other on the same thread, and in 1:1 correspondence with calls to corresponding
//! function.
//!
//! Where the operand type is larger than the bus size, an implementation should perform the
//! sub-operations in increasing address order.
//!
//! ## Dyn Compatibility and Trait Extensions
//! The [Mmio] trait is dyn compatible. Callers may want to also import the [MmioExt] trait which
//! defines some useful utilities on top of [Mmio] that would otherwise break dyn compatibility.
//! This trait is implemented automatically for any [Mmio].
//!
//! ## Alignment
//! The [Mmio] trait exposes offsets instead of addresses. Operations must be performed at a
//! suitable offset for the operand type. Valid alignment is not an intrinsic property of an offset.
//! Callers can use [Mmio::align_offset] to determine the first offset within the MMIO region
//! suitable for a given alignment.
//!
//! ## Usage Examples
//! ### VmoMapping
//! ```
//! use mmio::{Mmio, MmioExt};
//! use mmio::vmo::VmoMapping;
//! const VMO_OFFSET: usize = 0;
//! const VMO_LEN: usize = 1024;
//! # let vmo = zx::Vmo::create(VMO_LEN as u64).unwrap();
//! // Map the Vmo memory, returning an `MmioRegion`. The returned region implements `Mmio`.
//! let mut mmio = VmoMapping::map(VMO_OFFSET, VMO_LEN, vmo).unwrap();
//! // Load 4 bytes starting at offset 0.
//! let _ = mmio.load32(0);
//! // Store 2 bytes starting at offset 32.
//! let _ = mmio.store16(32, 0x1234);
//! ```
//!
//! ### Splittable VmoMapping
//! ```
//! use mmio::{Mmio, MmioExt};
//! use mmio::vmo::VmoMapping;
//! const VMO_LEN: usize = 1024;
//! # let vmo = zx::Vmo::create(VMO_LEN as u64).unwrap();
//! // Map the Vmo memory and convert it into a splittable `MmioRegion`.
//! // The returned region implements `Mmio + MmioSplit + Send`. If you only need split and not
//! // you can use `into_split` instead.
//! let mut mmio = VmoMapping::map(0, VMO_LEN, vmo).unwrap().into_split_send();
//!
//! // Split off a number of regions which have exclusive ownership of their ranges.
//! // reg1 owns the first 8 bytes, which start at offset 0 in the original mapping.
//! let mut reg1 = mmio.split_off(8);
//! // reg2 owns the next 4 bytes, which start at offset 8 in the original mapping.
//! let mut reg2 = mmio.split_off(4);
//! // reg3 owns the next 4 bytes, which start at offset 12 in the original mapping.
//! let mut reg3 = mmio.split_off(4);
//! // The original mmio owns the rest of the region, starting at offset 16 in the original
//! // mapping.
//!
//! let _ = reg1.load64(0);
//! reg1.store16(2, 0x1234);
//!
//! let _ = reg2.load32(0);
//! reg2.store8(3, 0xff);
//!
//! let _ = reg3.load32(0);
//! reg3.store16(0, 0xff00);
//! ```
//!
//! ### Alignment and Capacity
//! ```
//! use mmio::{Mmio, MmioExt};
//! use mmio::vmo::VmoMapping;
//! const VMO_LEN: usize = 1024;
//! # let vmo = zx::Vmo::create(VMO_LEN as u64).unwrap();
//! // Map the Vmo memory. The returned mapping can be converted into an `MmioRegion`.
//! let mut mmio = VmoMapping::map(0, VMO_LEN, vmo).unwrap().into_split();
//!
//! // Split off a number of regions which have exclusive ownership of their ranges.
//! // reg1 owns the first 8 bytes, which start at offset 0 in the original mapping.
//! let mut reg1 = mmio.split_off(8);
//! // reg2 owns the next 4 bytes, which start at offset 8 in the original mapping.
//! let mut reg2 = mmio.split_off(4);
//! // reg3 owns the next 4 bytes, which start at offset 12 in the original mapping.
//! let mut reg3 = mmio.split_off(4);
//! // The original mmio owns the rest of the region, starting at offset 16 in the original
//! // mapping.
//!
//! // reg1 owns the region starting at offset 0 in the Vmo, which is mapped to a page aligned
//! // boundary. It is suitably aligned for any type with an alignment <= Fuchsia's page size. It
//! // covers an 8 byte range, to can hold any `MmioOperand` type.
//! reg1.check_suitable_for::<u64>().unwrap();
//!
//! // reg2 owns the region starting at offset 8. It is aligned for any `MmioOperand` type but
//! // covers a 4 byte range.
//! reg2.check_aligned_for::<u64>().unwrap();
//! reg2.check_capacity_for::<u64>().expect_err("expect MmioError::OutOfRange");
//!
//! reg2.check_suitable_for::<u32>().unwrap();
//!
//! // reg3 owns the region starting at offset 12. It is aligned for types with an alignment <= 4
//! // and covers a 4 byte range.
//! reg3.check_aligned_for::<u64>().expect_err("expect MmioError::Unaligned");
//! reg3.check_capacity_for::<u64>().expect_err("expect MmioError::OutOfRange");
//!
//! reg3.check_suitable_for::<u32>().unwrap();
//!
//! // The original mmio object owns the region starting at offset 16 and covering the rest of the
//! // mapping.
//! mmio.check_suitable_for::<u64>().unwrap();
//! mmio.check_suitable_for::<[u64; 8]>().unwrap();
//! ```
//!
//! ## Testing
//! ### Regular Memory Implementation
//! ```
//! use mmio::memory::Memory;
//! use std::mem::MaybeUninit;
//!
//! let mut mem = MaybeUninit<[u64; 8]>::uninit();
//! // A MaybeUninit's memory can be borrowed as an Mmio region.
//! let mmio = Memory::borrow_uninit(&mut mem);
//!
//! run_test(mmio);
//!
//! // Safety: any bit pattern is valid for [u64; 8].
//! let mem = unsafe { mem.assume_init() };
//!
//! // Check that the memory is in the expected state.
//! validate_mem(mem);
//! ```
//!
//! # Implementers Guide
//! Implementers of [Mmio] and [MmioSplit] are required to uphold some guarantees. These are
//! discussed more thoroughly in the corresponding trait's documentation. These requirements are
//! intended to guarantee the semantics required to interface with devices correctly, as discussed
//! earlier.

mod arch;
mod memory;
mod mmio;
pub mod region;
mod register;
pub mod vmo;

pub use mmio::*;
#[doc(hidden)]
pub use paste;
pub use register::*;
