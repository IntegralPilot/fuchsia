// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon fifo objects.

use crate::{NullableHandle, Status, ok, sys};
use std::mem::MaybeUninit;
use std::num::NonZeroUsize;
use zerocopy::{FromBytes, IntoBytes};

/// An object representing a Zircon fifo.
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
///
/// Encodes the element type in the type. Defaults to `()` for the entry type to allow for untyped
/// IPC. Use `Fifo::cast()` to convert an IPC-transferred fifo to one of the specific type required
/// that will support reads and writes.
#[repr(transparent)]
pub struct Fifo<R = UnspecifiedFifoElement, W = R>(
    NullableHandle,
    std::marker::PhantomData<(R, W)>,
);

impl_handle_based!(Fifo, [R, W], [R, W]);

impl<R: IntoBytes + FromBytes, W: IntoBytes + FromBytes> Fifo<R, W> {
    /// Create a pair of fifos and return their endpoints. Writing to one endpoint enqueues an
    /// element into the fifo from which the opposing endpoint reads.
    ///
    /// Wraps the
    /// [zx_fifo_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_create.md)
    /// syscall.
    pub fn create(elem_count: usize) -> Result<(Self, Fifo<W, R>), Status> {
        if std::mem::size_of::<R>() != std::mem::size_of::<W>() {
            return Err(Status::INVALID_ARGS);
        }
        let mut out0 = 0;
        let mut out1 = 0;
        let options = 0;

        // SAFETY: this is a basic FFI call, and the mutable references are valid pointers.
        let status = unsafe {
            sys::zx_fifo_create(elem_count, std::mem::size_of::<R>(), options, &mut out0, &mut out1)
        };
        ok(status)?;

        // SAFETY: if the above call succeeded, these are valid handle numbers.
        unsafe {
            Ok((
                Fifo::from(NullableHandle::from_raw(out0)),
                Fifo::from(NullableHandle::from_raw(out1)),
            ))
        }
    }

    /// Attempts to write some number of elements into the fifo. On success, returns the number of
    /// elements actually written.
    ///
    /// Wraps
    /// [zx_fifo_write](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_write.md).
    pub fn write(&self, buf: &[W]) -> Result<NonZeroUsize, Status> {
        // SAFETY: this pointer is valid for the length of the slice
        unsafe { self.write_raw(buf.as_ptr(), buf.len()) }
    }

    /// Attempts to write a single element into the fifo.
    ///
    /// Wraps
    /// [zx_fifo_write](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_write.md).
    pub fn write_one(&self, elem: &W) -> Result<(), Status> {
        // SAFETY: this pointer is valid for a single element
        unsafe { self.write_raw(elem, 1).map(|n| debug_assert_eq!(n.get(), 1)) }
    }

    /// Attempts to write some number of elements into the fifo. On success, returns the number of
    /// elements actually written.
    ///
    /// Wraps
    /// [zx_fifo_write](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_write.md).
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring `buf` is valid to write to for `count` elements.
    pub unsafe fn write_raw(&self, buf: *const W, count: usize) -> Result<NonZeroUsize, Status> {
        let mut actual_count = 0;
        // SAFETY: safety requirements for this call are upheld by our caller.
        let status = unsafe {
            sys::zx_fifo_write(
                self.raw_handle(),
                std::mem::size_of::<W>(),
                buf.cast::<u8>(),
                count,
                &mut actual_count,
            )
        };
        ok(status)?;
        debug_assert_ne!(actual_count, 0);
        // SAFETY: `zx_fifo_write` returning `ZX_OK` guarantees `actual_count > 0`.
        Ok(unsafe { NonZeroUsize::new_unchecked(actual_count) })
    }

    /// Attempts to read some elements out of the fifo. On success, returns the number of elements
    /// actually read.
    ///
    /// Wraps
    /// [zx_fifo_read](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_read.md).
    pub fn read(&self, buf: &mut [R]) -> Result<NonZeroUsize, Status> {
        // SAFETY: the pointer is valid for the length of the slice
        unsafe { self.read_raw(buf.as_mut_ptr(), buf.len()) }
    }

    /// Attempts to read a single element out of the fifo.
    ///
    /// Wraps
    /// [zx_fifo_read](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_read.md).
    pub fn read_one(&self) -> Result<R, Status> {
        let mut elem = MaybeUninit::uninit();

        // SAFETY: the reference is valid to write to, and this call will not read from the bytes.
        let valid_count = unsafe { self.read_raw(elem.as_mut_ptr(), 1)? };
        debug_assert_eq!(valid_count.get(), 1);

        // SAFETY: if the previous call succeeded, the kernel has initialized this value.
        Ok(unsafe { elem.assume_init() })
    }

    /// Attempts to read some number of elements out of the fifo. On success, returns a slice of
    /// initialized elements.
    ///
    /// Wraps
    /// [zx_fifo_read](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_read.md).
    pub fn read_uninit<'a>(&self, bytes: &'a mut [MaybeUninit<R>]) -> Result<&'a mut [R], Status> {
        // SAFETY: the slice is valid to write to for its entire length, and this call will not
        // read from the bytes
        let valid_count = unsafe { self.read_raw(bytes.as_mut_ptr().cast::<R>(), bytes.len())? };
        let (valid, _uninit) = bytes.split_at_mut(valid_count.get());

        // SAFETY: the kernel initialized all bytes, strip out MaybeUninit
        unsafe { Ok(std::slice::from_raw_parts_mut(valid.as_mut_ptr().cast::<R>(), valid.len())) }
    }

    /// Attempts to read some number of elements out of the fifo. On success, returns the number of
    /// elements actually read.
    ///
    /// Wraps
    /// [zx_fifo_read](https://fuchsia.dev/fuchsia-src/reference/syscalls/fifo_read.md).
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring `bytes` points to valid (albeit
    /// not necessarily initialized) memory at least `len` bytes long.
    pub unsafe fn read_raw(&self, buf: *mut R, count: usize) -> Result<NonZeroUsize, Status> {
        let mut actual_count = 0;
        // SAFETY: this call's invariants must be upheld by our caller.
        let status = unsafe {
            sys::zx_fifo_read(
                self.raw_handle(),
                std::mem::size_of::<R>(),
                buf.cast::<u8>(),
                count,
                &mut actual_count,
            )
        };
        ok(status)?;
        debug_assert_ne!(actual_count, 0);
        // SAFETY: `zx_fifo_read` returning `ZX_OK` guarantees `actual_count > 0`.
        Ok(unsafe { NonZeroUsize::new_unchecked(actual_count) })
    }
}

impl Fifo<UnspecifiedFifoElement> {
    /// Give a `Fifo` specific read/write types. The size of `R2` and `W2` must match
    /// the element size the underlying handle was created with for reads and writes to succeed.
    pub fn cast<R2, W2>(self) -> Fifo<R2, W2> {
        Fifo::<R2, W2>::from(self.0)
    }
}

impl<R, W> Fifo<R, W> {
    /// Convert a fifo from having a specific element type to a fifo without any element type that
    /// will not support reads or writes.
    pub fn downcast(self) -> Fifo {
        Fifo::from(self.0)
    }
}

impl<R: FromBytes + IntoBytes, W: FromBytes + IntoBytes> From<Fifo> for Fifo<R, W> {
    fn from(untyped: Fifo) -> Self {
        untyped.cast()
    }
}

impl<R, W> std::fmt::Debug for Fifo<R, W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let read_name = std::any::type_name::<R>();
        let (_, short_read_name) = read_name.rsplit_once("::").unwrap();
        let write_name = std::any::type_name::<W>();
        let (_, short_write_name) = write_name.rsplit_once("::").unwrap();
        f.debug_tuple(&format!("Fifo<{short_read_name}, {short_write_name}>"))
            .field(&self.0)
            .finish()
    }
}

impl<R, W> std::cmp::PartialEq for Fifo<R, W> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<R, W> std::cmp::Eq for Fifo<R, W> {}

impl<R, W> std::cmp::PartialOrd for Fifo<R, W> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.0.partial_cmp(&other.0)
    }
}
impl<R, W> std::cmp::Ord for Fifo<R, W> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl<R, W> std::hash::Hash for Fifo<R, W> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

/// The default element for fifos, does not support reading or writing. Only used for IPC transfer.
#[derive(Copy, Clone, Debug)]
pub struct UnspecifiedFifoElement;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_basic() {
        let (fifo1, fifo2) = Fifo::<[u8; 2]>::create(4).unwrap();

        // Trying to write less than one element should fail.
        assert_eq!(fifo1.write(&[]), Err(Status::OUT_OF_RANGE));

        // Should write one element "he"
        fifo1.write_one(b"he").unwrap();

        // Should write three elements "ll" "o " "wo" and drop the rest as it is full.
        assert_eq!(fifo1.write(&[*b"ll", *b"o ", *b"wo", *b"rl", *b"ds"]).unwrap().get(), 3);

        // Now that the fifo is full any further attempts to write should fail.
        assert_eq!(fifo1.write(&[*b"bl", *b"ah", *b"bl", *b"ah"]), Err(Status::SHOULD_WAIT));

        assert_eq!(fifo2.read_one().unwrap(), *b"he");

        // Read remaining 3 entries from the other end.
        let mut read_vec = vec![[0; 2]; 8];
        assert_eq!(fifo2.read(&mut read_vec).unwrap().get(), 3);
        assert_eq!(read_vec, &[*b"ll", *b"o ", *b"wo", [0, 0], [0, 0], [0, 0], [0, 0], [0, 0]]);

        // Reading again should fail as the fifo is empty.
        assert_eq!(fifo2.read(&mut read_vec), Err(Status::SHOULD_WAIT));
    }
}
