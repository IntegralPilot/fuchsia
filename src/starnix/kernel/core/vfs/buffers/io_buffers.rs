// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::{
    MemoryAccessorExt, NumberOfElementsRead, TaskMemoryAccessor, read_to_array,
    read_to_object_as_bytes, read_to_vec,
};
use crate::task::{CurrentTask, Task};
use smallvec::{SmallVec, smallvec};
use starnix_types::user_buffer::{UserBuffer, UserBuffers};
use starnix_uapi::errors::{ENOTSUP, Errno};
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{errno, error};
use std::mem::MaybeUninit;
use std::ops::{Deref, DerefMut};
use zerocopy::FromBytes;

/// The callback for `OutputBuffer::write_each`. The callback is passed the buffers to write to in
/// order, and must return for each, how many bytes has been written.
pub type OutputBufferCallback<'a> = dyn FnMut(&mut [MaybeUninit<u8>]) -> Result<usize, Errno> + 'a;

fn slice_to_maybe_uninit(buffer: &[u8]) -> &[MaybeUninit<u8>] {
    // SAFETY: &[u8] and &[MaybeUninit<u8>] have the same layout.
    unsafe { std::slice::from_raw_parts(buffer.as_ptr() as *const MaybeUninit<u8>, buffer.len()) }
}

pub trait Iovec: Sized {
    fn create(buffer: &UserBuffer) -> Self;
}

impl Iovec for syncio::zxio::iovec {
    fn create(buffer: &UserBuffer) -> Self {
        Self { iov_base: buffer.address.ptr() as *mut starnix_uapi::c_void, iov_len: buffer.length }
    }
}

impl Iovec for syncio::zxio::zx_iovec {
    fn create(buffer: &UserBuffer) -> Self {
        Self { buffer: buffer.address.ptr() as *mut starnix_uapi::c_void, capacity: buffer.length }
    }
}

impl Iovec for zx::sys::zx_iovec_t {
    fn create(buffer: &UserBuffer) -> Self {
        Self { buffer: buffer.address.ptr() as *const u8, capacity: buffer.length }
    }
}

const IOVECS_IN_HEAP_THRESHOLD: usize = 5;

/// Provides access to a slice of iovecs while retaining some reference.
pub struct IovecsRef<'a, I: Sized> {
    iovecs: SmallVec<[I; IOVECS_IN_HEAP_THRESHOLD]>,
    _marker: std::marker::PhantomData<&'a I>,
}

impl<'a, I: Iovec> IovecsRef<'a, I> {
    /// Returns the list of iovecs backing the buffer.
    ///
    /// Note that we use `IovecsRef<'_>` so that while `IovecsRef` is held,
    /// no other methods may be called on the `Buffer` since `IovecsRef`
    /// holds onto the mutable reference for the `Buffer`.
    fn new<B: Buffer + ?Sized>(buf: &'a mut B) -> Result<Self, Errno> {
        let mut iovecs = SmallVec::with_capacity(buf.segments_count()?);
        buf.peek_each_segment(&mut |buffer| iovecs.push(I::create(buffer)))?;
        Ok(IovecsRef { iovecs, _marker: Default::default() })
    }
}

impl<I> Deref for IovecsRef<'_, I> {
    type Target = [I];
    fn deref(&self) -> &Self::Target {
        &self.iovecs
    }
}

impl<I> DerefMut for IovecsRef<'_, I> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.iovecs
    }
}

pub type PeekBufferSegmentsCallback<'a> = dyn FnMut(&UserBuffer) + 'a;

/// A buffer.
///
/// Provides the common implementations for input and output buffers.
pub trait Buffer: std::fmt::Debug {
    /// Returns the number of segments, if the buffer supports I/O directly
    /// to/from individual segments.
    fn segments_count(&self) -> Result<usize, Errno>;

    /// Calls the callback with each segment backing this buffer.
    ///
    /// Each segment can be read from (if this is an `InputBuffer`) or written to (if this is an
    /// `OutputBuffer`) using either user copy routines or Zircon system calls.  If this is an
    /// `OutputBuffer`, any damage caused by bad addresses will be restricted to user addresses.
    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno>;

    /// Returns all the segments backing this `Buffer`.
    ///
    /// Note that we use `IovecsRef<'_>` so that while `IovecsRef` is held, no other methods may be
    /// called on this `Buffer` since `IovecsRef` holds onto the mutable reference for this
    /// `Buffer`.
    ///
    /// NOTE: The returned segments can only be accessed using user copy routines or Zircon system
    /// calls (see the comment above for `peek_each_segment). The pointers returned are not _valid_
    /// for any non-zero sized access (see Rust's std::ptr documentation).
    fn peek_all_segments_as_iovecs(&mut self) -> Result<IovecsRef<'_, syncio::zxio::iovec>, Errno> {
        IovecsRef::new(self)
    }
}

/// Attempts to perform some I/O with the iovec segments of `Buffer`.
///
/// Returns `None` if the I/O can not be performed with iovecs (when unified
/// aspaces is disabled or the `Buffer` does not support I/O on its segments
/// directly).
///
/// NOTE: The segments can only be accessed using user copy routines or Zircon system calls (see the
/// comment above for `peek_each_segment). The pointers returned are not _valid_ for any non-zero
/// sized access (see Rust's std::ptr documentation).
pub fn with_iovec_segments<B: Buffer + ?Sized, I: Iovec, T>(
    data: &mut B,
    f: impl FnOnce(&mut [I]) -> Result<T, Errno>,
) -> Option<Result<T, Errno>> {
    match IovecsRef::new(data) {
        Ok(mut o) => Some(f(&mut o)),
        Err(e) => {
            if e.code == ENOTSUP {
                None
            } else {
                Some(Err(e))
            }
        }
    }
}

/// The OutputBuffer allows for writing bytes to a buffer.
/// A single OutputBuffer will only write up to MAX_RW_COUNT bytes which is the maximum size of a
/// single operation.
pub trait OutputBuffer: Buffer {
    /// Calls `callback` for each segment to write data for. `callback` must returns the number of
    /// bytes actually written. When it returns less than the size of the input buffer, the write
    /// is stopped.
    ///
    /// Returns the total number of bytes written.
    fn write_each(&mut self, callback: &mut OutputBufferCallback<'_>) -> Result<usize, Errno>;

    /// Returns the number of bytes available to be written into the buffer.
    fn available(&self) -> usize;

    /// Returns the number of bytes already written into the buffer.
    fn bytes_written(&self) -> usize;

    /// Fills this buffer with zeros.
    fn zero(&mut self) -> Result<usize, Errno>;

    /// Advance the output buffer by `length` bytes.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the length bytes are initialized.
    unsafe fn advance(&mut self, length: usize) -> Result<(), Errno>;

    /// Write the content of `buffer` into this buffer. If this buffer is too small, the write will
    /// be partial.
    ///
    /// Returns the number of bytes written in this buffer.
    fn write(&mut self, buffer: &[u8]) -> Result<usize, Errno> {
        let mut buffer = slice_to_maybe_uninit(buffer);

        self.write_each(&mut move |data| {
            let size = std::cmp::min(buffer.len(), data.len());
            let (to_clone, remaining) = buffer.split_at(size);
            data[0..size].clone_from_slice(to_clone);
            buffer = remaining;
            Ok(size)
        })
    }

    /// Write the content of `buffer` into this buffer. It is an error to pass a buffer larger than
    /// the number of bytes available in this buffer. In that case, the content of the buffer after
    /// the operation is unspecified.
    ///
    /// In case of success, always returns `buffer.len()`.
    fn write_all(&mut self, buffer: &[u8]) -> Result<usize, Errno> {
        let size = self.write(buffer)?;
        if size != buffer.len() { error!(EINVAL) } else { Ok(size) }
    }

    /// Write the content of the given `InputBuffer` into this buffer. The number of bytes written
    /// will be the smallest between the number of bytes available in this buffer and in the
    /// `InputBuffer`.
    ///
    /// Returns the number of bytes read and written.
    fn write_buffer(&mut self, input: &mut dyn InputBuffer) -> Result<usize, Errno> {
        self.write_each(&mut move |data| {
            let size = std::cmp::min(data.len(), input.available());
            input.read_exact(&mut data[0..size])
        })
    }
}

/// The callback for `InputBuffer::peek_each` and `InputBuffer::read_each`. The callback is passed
/// the buffers to write to in order, and must return for each, how many bytes has been read.

pub type InputBufferCallback<'a> = dyn FnMut(&[u8]) -> Result<usize, Errno> + 'a;

/// The InputBuffer allows for reading bytes from a buffer.
/// A single InputBuffer will only read up to MAX_RW_COUNT bytes which is the maximum size of a
/// single operation.
pub trait InputBuffer: Buffer {
    /// Calls `callback` for each segment to peek data from. `callback` must returns the number of
    /// bytes actually peeked. When it returns less than the size of the output buffer, the read
    /// is stopped.
    ///
    /// Returns the total number of bytes peeked.
    fn peek_each(&mut self, callback: &mut InputBufferCallback<'_>) -> Result<usize, Errno>;

    /// Returns the number of bytes available to be read from the buffer.
    fn available(&self) -> usize;

    /// Returns the number of bytes already read from the buffer.
    fn bytes_read(&self) -> usize;

    /// Clear the remaining content in the buffer. Returns the number of bytes swallowed. After this
    /// method returns, `available()` will returns 0. This does not touch the data in the buffer.
    fn drain(&mut self) -> usize;

    /// Consumes `length` bytes of data from this buffer.
    fn advance(&mut self, length: usize) -> Result<(), Errno>;

    /// Calls `callback` for each segment to read data from. `callback` must returns the number of
    /// bytes actually read. When it returns less than the size of the output buffer, the read
    /// is stopped.
    ///
    /// Returns the total number of bytes read.
    fn read_each(&mut self, callback: &mut InputBufferCallback<'_>) -> Result<usize, Errno> {
        let length = self.peek_each(callback)?;
        self.advance(length)?;
        Ok(length)
    }

    /// Read all the remaining content in this buffer and returns it as a `Vec`.
    fn read_all(&mut self) -> Result<Vec<u8>, Errno> {
        let result = self.peek_all()?;
        let drain_result = self.drain();
        assert!(result.len() == drain_result);
        Ok(result)
    }

    /// Peek all the remaining content in this buffer and returns it as a `Vec`.
    fn peek_all(&mut self) -> Result<Vec<u8>, Errno> {
        // SAFETY: self.peek returns the number of bytes read.
        unsafe {
            read_to_vec::<u8, _>(self.available(), |buf| self.peek(buf).map(NumberOfElementsRead))
        }
    }

    /// Peeks the content of this buffer into `buffer`.
    /// If `buffer` is too small, the read will be partial.
    /// If `buffer` is too large, the remaining bytes will be left untouched.
    ///
    /// Returns the number of bytes read from this buffer.
    fn peek(&mut self, buffer: &mut [MaybeUninit<u8>]) -> Result<usize, Errno> {
        let mut index = 0;
        self.peek_each(&mut move |data| {
            let data = slice_to_maybe_uninit(data);
            let size = std::cmp::min(buffer.len() - index, data.len());
            buffer[index..index + size].clone_from_slice(&data[..size]);
            index += size;
            Ok(size)
        })
    }

    /// Write the content of this buffer into `buffer`.
    /// If `buffer` is too small, the read will be partial.
    /// If `buffer` is too large, the remaining bytes will be left untouched.
    ///
    /// Returns the number of bytes read from this buffer.
    fn read(&mut self, buffer: &mut [MaybeUninit<u8>]) -> Result<usize, Errno> {
        let length = self.peek(buffer)?;
        self.advance(length)?;
        Ok(length)
    }

    /// Read the exact number of bytes required to fill buf.
    ///
    /// If `buffer` is larger than the number of available bytes, an error will be returned.
    ///
    /// In case of success, always returns `buffer.len()`.
    fn read_exact(&mut self, buffer: &mut [MaybeUninit<u8>]) -> Result<usize, Errno> {
        let size = self.read(buffer)?;
        if size != buffer.len() { error!(EINVAL) } else { Ok(size) }
    }
}

pub trait InputBufferExt: InputBuffer {
    /// Reads exactly `len` bytes into a returned `Vec`.
    ///
    /// Returns an error if `len` is larger than the number of available bytes.
    fn read_to_vec_exact(&mut self, len: usize) -> Result<Vec<u8>, Errno> {
        // SAFETY: `data.read_exact` returns `len` bytes on success.
        unsafe { read_to_vec::<u8, _>(len, |buf| self.read_exact(buf).map(NumberOfElementsRead)) }
    }

    /// Reads up to `limit` bytes into a returned `Vec`.
    fn read_to_vec_limited(&mut self, limit: usize) -> Result<Vec<u8>, Errno> {
        // SAFETY: `data.read` returns the number of bytes read.
        unsafe { read_to_vec::<u8, _>(limit, |buf| self.read(buf).map(NumberOfElementsRead)) }
    }

    /// Reads bytes into the array.
    ///
    /// Returns an error if `N` is larger than the number of available bytes.
    fn read_to_array<const N: usize>(&mut self) -> Result<[u8; N], Errno> {
        // SAFETY: `data.read_exact` returns `N` bytes on success.
        unsafe {
            read_to_array::<_, _, N>(|buf| {
                self.read_exact(buf).map(|bytes_read| debug_assert_eq!(bytes_read, buf.len()))
            })
        }
    }

    /// Interprets the buffer as an object.
    ///
    /// Returns an error if the buffer does not have enough bytes to represent the
    /// object.
    fn read_to_object<T: FromBytes>(&mut self) -> Result<T, Errno> {
        // SAFETY: the callback returns successfully only if the required number of
        // bytes were read.
        unsafe {
            read_to_object_as_bytes(|buf| {
                if self.read(buf)? != buf.len() { error!(EINVAL) } else { Ok(()) }
            })
        }
    }
}

impl InputBufferExt for dyn InputBuffer + '_ {}
impl<T: InputBuffer> InputBufferExt for T {}

/// An OutputBuffer that write data to user space memory through a `TaskMemoryAccessor`.
pub struct UserBuffersOutputBuffer<'a, M> {
    mm: &'a M,
    buffers: UserBuffers,
    available: usize,
    bytes_written: usize,
}

impl<'a, M> std::fmt::Debug for UserBuffersOutputBuffer<'a, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserBuffersOutputBuffer")
            .field("buffers", &self.buffers)
            .field("available", &self.available)
            .field("bytes_written", &self.bytes_written)
            .finish()
    }
}

impl<'a, M: TaskMemoryAccessor> UserBuffersOutputBuffer<'a, M> {
    fn new_inner(mm: &'a M, mut buffers: UserBuffers) -> Result<Self, Errno> {
        let available = UserBuffer::cap_buffers_to_max_rw_count(
            mm.maximum_valid_address().ok_or_else(|| errno!(EINVAL))?,
            &mut buffers,
        )?;
        // Reverse the buffers as the element will be removed as they are handled.
        buffers.reverse();
        Ok(Self { mm, buffers, available, bytes_written: 0 })
    }

    fn write_each_inner<B: AsRef<[u8]>, F: FnMut(usize) -> Result<B, Errno>>(
        &mut self,
        mut callback: F,
    ) -> Result<usize, Errno> {
        let mut bytes_written = 0;
        while let Some(mut buffer) = self.buffers.pop() {
            if buffer.is_null() {
                continue;
            }

            let bytes = callback(buffer.length)?;
            let bytes = bytes.as_ref();

            bytes_written += self.mm.write_memory(buffer.address, bytes)?;
            let bytes_len = bytes.len();
            buffer.advance(bytes_len)?;
            self.available -= bytes_len;
            self.bytes_written += bytes_len;
            if !buffer.is_empty() {
                self.buffers.push(buffer);
                break;
            }
        }
        Ok(bytes_written)
    }
}

impl<'a> UserBuffersOutputBuffer<'a, CurrentTask> {
    pub fn unified_new(task: &'a CurrentTask, buffers: UserBuffers) -> Result<Self, Errno> {
        Self::new_inner(task, buffers)
    }

    pub fn unified_new_at(
        task: &'a CurrentTask,
        address: UserAddress,
        length: usize,
    ) -> Result<Self, Errno> {
        Self::unified_new(task, smallvec![UserBuffer { address, length }])
    }
}

impl<'a> UserBuffersOutputBuffer<'a, Task> {
    pub fn syscall_new(task: &'a Task, buffers: UserBuffers) -> Result<Self, Errno> {
        Self::new_inner(task, buffers)
    }
}

impl<'a, M: TaskMemoryAccessor> Buffer for UserBuffersOutputBuffer<'a, M> {
    fn segments_count(&self) -> Result<usize, Errno> {
        Ok(self.buffers.len())
    }

    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno> {
        // This `UserBuffersOutputBuffer` made sure that each segment only pointed
        // to valid user-space address ranges on creation so each `buffer` is
        // safe to write to.
        for buffer in self.buffers.iter().rev() {
            if buffer.is_null() {
                continue;
            }
            callback(buffer)
        }

        Ok(())
    }
}

impl<'a, M: TaskMemoryAccessor> OutputBuffer for UserBuffersOutputBuffer<'a, M> {
    fn write(&mut self, mut bytes: &[u8]) -> Result<usize, Errno> {
        self.write_each_inner(|buflen| {
            let bytes_len = std::cmp::min(bytes.len(), buflen);
            let (to_write, remaining) = bytes.split_at(bytes_len);
            bytes = remaining;
            Ok(to_write)
        })
    }

    fn write_each(&mut self, callback: &mut OutputBufferCallback<'_>) -> Result<usize, Errno> {
        self.write_each_inner(|buflen| {
            // SAFETY: `callback` returns the number of bytes read on success.
            unsafe {
                read_to_vec::<u8, _>(buflen, |buf| {
                    let result = callback(buf)?;
                    if result > buflen {
                        return error!(EINVAL);
                    }
                    Ok(NumberOfElementsRead(result))
                })
            }
        })
    }

    fn available(&self) -> usize {
        self.available
    }

    fn bytes_written(&self) -> usize {
        self.bytes_written
    }

    fn zero(&mut self) -> Result<usize, Errno> {
        let mut bytes_written = 0;
        while let Some(mut buffer) = self.buffers.pop() {
            if buffer.is_null() {
                continue;
            }

            let count = self.mm.zero(buffer.address, buffer.length)?;
            buffer.advance(count)?;
            bytes_written += count;

            self.available -= count;
            self.bytes_written += count;

            if !buffer.is_empty() {
                self.buffers.push(buffer);
                break;
            }
        }

        Ok(bytes_written)
    }

    unsafe fn advance(&mut self, mut length: usize) -> Result<(), Errno> {
        if length > self.available() {
            return error!(EINVAL);
        }

        while let Some(mut buffer) = self.buffers.pop() {
            if buffer.is_null() {
                continue;
            }

            let advance_by = std::cmp::min(length, buffer.length);
            buffer.advance(advance_by)?;
            self.available -= advance_by;
            self.bytes_written += advance_by;
            if !buffer.is_empty() {
                self.buffers.push(buffer);
                break;
            }
            length -= advance_by;
        }

        Ok(())
    }
}

/// An InputBuffer that read data from user space memory through a `TaskMemoryAccessor`.
pub struct UserBuffersInputBuffer<'a, M> {
    mm: &'a M,
    buffers: UserBuffers,
    available: usize,
    bytes_read: usize,
}

impl<'a, M> std::fmt::Debug for UserBuffersInputBuffer<'a, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserBuffersInputBuffer")
            .field("buffers", &self.buffers)
            .field("available", &self.available)
            .field("bytes_read", &self.bytes_read)
            .finish()
    }
}

impl<'a, M: TaskMemoryAccessor> UserBuffersInputBuffer<'a, M> {
    fn new_inner(mm: &'a M, mut buffers: UserBuffers) -> Result<Self, Errno> {
        let available = UserBuffer::cap_buffers_to_max_rw_count(
            mm.maximum_valid_address().ok_or_else(|| errno!(EINVAL))?,
            &mut buffers,
        )?;
        // Reverse the buffers as the element will be removed as they are handled.
        buffers.reverse();
        Ok(Self { mm, buffers, available, bytes_read: 0 })
    }

    fn peek_each_inner<F: FnMut(&UserBuffer, usize) -> Result<usize, Errno>>(
        &mut self,
        mut callback: F,
    ) -> Result<usize, Errno> {
        let mut read = 0;
        for buffer in self.buffers.iter().rev() {
            if buffer.is_null() {
                continue;
            }

            let result = callback(buffer, read)?;
            if result > buffer.length {
                return error!(EINVAL);
            }
            read += result;
            if result != buffer.length {
                break;
            }
        }
        Ok(read)
    }
}

impl<'a> UserBuffersInputBuffer<'a, CurrentTask> {
    pub fn unified_new(task: &'a CurrentTask, buffers: UserBuffers) -> Result<Self, Errno> {
        Self::new_inner(task, buffers)
    }

    pub fn unified_new_at(
        task: &'a CurrentTask,
        address: UserAddress,
        length: usize,
    ) -> Result<Self, Errno> {
        Self::unified_new(task, smallvec![UserBuffer { address, length }])
    }
}

impl<'a> UserBuffersInputBuffer<'a, Task> {
    pub fn syscall_new(task: &'a Task, buffers: UserBuffers) -> Result<Self, Errno> {
        Self::new_inner(task, buffers)
    }
}

impl<'a, M: TaskMemoryAccessor> Buffer for UserBuffersInputBuffer<'a, M> {
    fn segments_count(&self) -> Result<usize, Errno> {
        Ok(self.buffers.iter().filter(|b| b.is_null()).count())
    }

    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno> {
        // This `UserBuffersInputBuffer` made sure that each segment only pointed
        // to valid user-space address ranges on creation so each `buffer` is
        // safe to read from.
        for buffer in self.buffers.iter().rev() {
            if buffer.is_null() {
                continue;
            }
            callback(buffer)
        }

        Ok(())
    }
}

impl<'a, M: TaskMemoryAccessor> InputBuffer for UserBuffersInputBuffer<'a, M> {
    fn peek(&mut self, uninit_bytes: &mut [MaybeUninit<u8>]) -> Result<usize, Errno> {
        self.peek_each_inner(|buffer, read_so_far| {
            let read_to = &mut uninit_bytes[read_so_far..];
            let read_count = std::cmp::min(buffer.length, read_to.len());
            let read_to = &mut read_to[..read_count];
            let read_bytes = self.mm.read_memory(buffer.address, read_to)?;
            debug_assert_eq!(read_bytes.len(), read_count);
            Ok(read_count)
        })
    }

    fn peek_each(&mut self, callback: &mut InputBufferCallback<'_>) -> Result<usize, Errno> {
        self.peek_each_inner(|buffer, _read_so_far| {
            let bytes = self.mm.read_memory_to_vec(buffer.address, buffer.length)?;
            callback(&bytes)
        })
    }

    fn drain(&mut self) -> usize {
        let result = self.available;
        self.bytes_read += self.available;
        self.available = 0;
        self.buffers.clear();
        result
    }

    fn advance(&mut self, mut length: usize) -> Result<(), Errno> {
        if length > self.available {
            return error!(EINVAL);
        }
        self.available -= length;
        self.bytes_read += length;
        while let Some(mut buffer) = self.buffers.pop() {
            if length < buffer.length {
                buffer.advance(length)?;
                self.buffers.push(buffer);
                return Ok(());
            }
            length -= buffer.length;
            if length == 0 {
                return Ok(());
            }
        }
        if length != 0 { error!(EINVAL) } else { Ok(()) }
    }

    fn available(&self) -> usize {
        self.available
    }
    fn bytes_read(&self) -> usize {
        self.bytes_read
    }
}

/// An OutputBuffer that write data to an internal buffer.
#[derive(Debug)]
pub struct VecOutputBuffer {
    buffer: Vec<u8>,
    // Used to keep track of the requested capacity. `Vec::with_capacity` may
    // allocate more than the requested capacity so we can't rely on
    // `Vec::capacity` to return the expected capacity.
    capacity: usize,
}

impl VecOutputBuffer {
    pub fn new(capacity: usize) -> Self {
        Self { buffer: Vec::with_capacity(capacity), capacity }
    }

    pub fn data(&self) -> &[u8] {
        &self.buffer
    }

    pub fn reset(&mut self) {
        self.buffer.clear()
    }
}

impl From<VecOutputBuffer> for Vec<u8> {
    fn from(data: VecOutputBuffer) -> Self {
        data.buffer
    }
}

impl Buffer for VecOutputBuffer {
    fn segments_count(&self) -> Result<usize, Errno> {
        Ok(1)
    }

    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno> {
        let current_len = self.buffer.len();
        let buffer = &mut self.buffer.spare_capacity_mut()[..self.capacity - current_len];
        callback(&UserBuffer {
            address: UserAddress::from(buffer.as_mut_ptr() as u64),
            length: buffer.len(),
        });

        Ok(())
    }
}

impl OutputBuffer for VecOutputBuffer {
    fn write_each(&mut self, callback: &mut OutputBufferCallback<'_>) -> Result<usize, Errno> {
        let current_len = self.buffer.len();
        let written =
            callback(&mut self.buffer.spare_capacity_mut()[..self.capacity - current_len])?;
        if current_len + written > self.capacity {
            return error!(EINVAL);
        }
        // SAFETY: the vector is now initialized for an extra `written` bytes.
        unsafe { self.buffer.set_len(current_len + written) }
        Ok(written)
    }

    fn available(&self) -> usize {
        self.capacity - self.buffer.len()
    }

    fn bytes_written(&self) -> usize {
        self.buffer.len()
    }

    fn zero(&mut self) -> Result<usize, Errno> {
        let zeroed = self.capacity - self.buffer.len();
        self.buffer.resize(self.capacity, 0);
        Ok(zeroed)
    }

    unsafe fn advance(&mut self, length: usize) -> Result<(), Errno> {
        if length > self.available() {
            return error!(EINVAL);
        }

        self.capacity -= length;
        let current_len = self.buffer.len();
        // SAFETY: We checked that length <= self.available(), and we updated self.capacity.
        // self.available() is self.capacity - self.buffer.len().
        // So length <= self.capacity - self.buffer.len()
        // self.buffer.len() + length <= self.capacity.
        // The buffer has at least self.capacity capacity (see VecOutputBuffer::new).
        unsafe { self.buffer.set_len(current_len + length) };
        Ok(())
    }
}

/// An InputBuffer that read data from an internal buffer.
#[derive(Debug)]
pub struct VecInputBuffer {
    buffer: Vec<u8>,

    // Invariant: `bytes_read <= buffer.len()` at all times.
    bytes_read: usize,
}

impl VecInputBuffer {
    pub fn new(buffer: &[u8]) -> Self {
        Self { buffer: buffer.to_vec(), bytes_read: 0 }
    }
}

impl From<Vec<u8>> for VecInputBuffer {
    fn from(buffer: Vec<u8>) -> Self {
        Self { buffer, bytes_read: 0 }
    }
}

impl Buffer for VecInputBuffer {
    fn segments_count(&self) -> Result<usize, Errno> {
        Ok(1)
    }

    fn peek_each_segment(
        &mut self,
        callback: &mut PeekBufferSegmentsCallback<'_>,
    ) -> Result<(), Errno> {
        let buffer = &self.buffer[self.bytes_read..];
        callback(&UserBuffer {
            address: UserAddress::from(buffer.as_ptr() as u64),
            length: buffer.len(),
        });

        Ok(())
    }
}

impl InputBuffer for VecInputBuffer {
    fn peek_each(&mut self, callback: &mut InputBufferCallback<'_>) -> Result<usize, Errno> {
        let read = callback(&self.buffer[self.bytes_read..])?;
        if self.bytes_read + read > self.buffer.len() {
            return error!(EINVAL);
        }
        debug_assert!(self.bytes_read <= self.buffer.len());
        Ok(read)
    }
    fn advance(&mut self, length: usize) -> Result<(), Errno> {
        if length > self.buffer.len() {
            return error!(EINVAL);
        }
        self.bytes_read += length;
        debug_assert!(self.bytes_read <= self.buffer.len());
        Ok(())
    }
    fn available(&self) -> usize {
        self.buffer.len() - self.bytes_read
    }
    fn bytes_read(&self) -> usize {
        self.bytes_read
    }
    fn drain(&mut self) -> usize {
        let result = self.available();
        self.bytes_read += result;
        result
    }
}

impl VecInputBuffer {
    /// Read an object from userspace memory and increment the read position.
    ///
    /// Returns an error if there is not enough available bytes compared to the size of `T`.
    pub fn read_object<T: FromBytes>(&mut self) -> Result<T, Errno> {
        let size = std::mem::size_of::<T>();
        let end = self.bytes_read + size;
        if end > self.buffer.len() {
            return error!(EINVAL);
        }
        let obj =
            T::read_from_bytes(&self.buffer[self.bytes_read..end]).map_err(|_| errno!(EINVAL))?;
        self.bytes_read = end;
        debug_assert!(self.bytes_read <= self.buffer.len());
        Ok(obj)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mm::{MemoryAccessor as _, PAGE_SIZE};
    use crate::testing::*;
    use usercopy::slice_to_maybe_uninit_mut;

    #[test]
    fn test_data_input_buffer() {
        let mut executor = fuchsia_async::TestExecutor::new();
        executor.run_singlethreaded(async {
            spawn_kernel_and_run(async |current_task| {
                let page_size = *PAGE_SIZE;
                let addr = map_memory(&current_task, UserAddress::default(), 64 * page_size);

                let data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();
                let mm = current_task.deref();
                mm.write_memory(addr, &data).expect("failed to write test data");

                let input_iovec = smallvec![
                    UserBuffer { address: addr, length: 25 },
                    UserBuffer {
                        address: (addr + 64usize).expect("Memory mapped OOB!"),
                        length: 12
                    },
                ];

                // Test incorrect callback.
                {
                    let mut input_buffer =
                        UserBuffersInputBuffer::unified_new(&current_task, input_iovec.clone())
                            .expect("UserBuffersInputBuffer");
                    assert!(input_buffer.peek_each(&mut |data| Ok(data.len() + 1)).is_err());
                }

                // Test drain
                {
                    let mut input_buffer =
                        UserBuffersInputBuffer::unified_new(&current_task, input_iovec.clone())
                            .expect("UserBuffersInputBuffer");
                    assert_eq!(input_buffer.available(), 37);
                    assert_eq!(input_buffer.bytes_read(), 0);
                    assert_eq!(input_buffer.drain(), 37);
                    assert_eq!(input_buffer.available(), 0);
                    assert_eq!(input_buffer.bytes_read(), 37);
                }

                // Test read_all
                {
                    let mut input_buffer =
                        UserBuffersInputBuffer::unified_new(&current_task, input_iovec.clone())
                            .expect("UserBuffersInputBuffer");
                    assert_eq!(input_buffer.available(), 37);
                    assert_eq!(input_buffer.bytes_read(), 0);
                    let buffer = input_buffer.read_all().expect("read_all");
                    assert_eq!(input_buffer.available(), 0);
                    assert_eq!(input_buffer.bytes_read(), 37);
                    assert_eq!(buffer.len(), 37);
                    assert_eq!(&data[..25], &buffer[..25]);
                    assert_eq!(&data[64..76], &buffer[25..37]);
                }

                // Test read
                {
                    let mut input_buffer =
                        UserBuffersInputBuffer::unified_new(&current_task, input_iovec)
                            .expect("UserBuffersInputBuffer");
                    let mut buffer = [0; 50];
                    assert_eq!(input_buffer.available(), 37);
                    assert_eq!(input_buffer.bytes_read(), 0);
                    assert_eq!(
                        input_buffer
                            .read_exact(slice_to_maybe_uninit_mut(&mut buffer[0..20]))
                            .expect("read"),
                        20
                    );
                    assert_eq!(input_buffer.available(), 17);
                    assert_eq!(input_buffer.bytes_read(), 20);
                    assert_eq!(
                        input_buffer
                            .read_exact(slice_to_maybe_uninit_mut(&mut buffer[20..37]))
                            .expect("read"),
                        17
                    );
                    assert!(
                        input_buffer
                            .read_exact(slice_to_maybe_uninit_mut(&mut buffer[37..]))
                            .is_err()
                    );
                    assert_eq!(input_buffer.available(), 0);
                    assert_eq!(input_buffer.bytes_read(), 37);
                    assert_eq!(&data[..25], &buffer[..25]);
                    assert_eq!(&data[64..76], &buffer[25..37]);
                }
            })
            .await;
        });
    }

    #[test]
    fn test_data_output_buffer() {
        let mut executor = fuchsia_async::TestExecutor::new();
        executor.run_singlethreaded(async {
            spawn_kernel_and_run(async |current_task| {
                let page_size = *PAGE_SIZE;
                let addr = map_memory(&current_task, UserAddress::default(), 64 * page_size);

                let output_iovec = smallvec![
                    UserBuffer { address: addr, length: 25 },
                    UserBuffer {
                        address: (addr + 64usize).expect("Memory was mapped OOB!"),
                        length: 12
                    },
                ];

                let _mm = current_task.deref();
                let data: Vec<u8> = (0..1024).map(|i| (i % 256) as u8).collect();

                // Test incorrect callback.
                {
                    let mut output_buffer =
                        UserBuffersOutputBuffer::unified_new(&current_task, output_iovec.clone())
                            .expect("UserBuffersOutputBuffer");
                    assert!(output_buffer.write_each(&mut |data| Ok(data.len() + 1)).is_err());
                }

                // Test write
                {
                    let mut output_buffer =
                        UserBuffersOutputBuffer::unified_new(&current_task, output_iovec)
                            .expect("UserBuffersOutputBuffer");
                    assert_eq!(output_buffer.available(), 37);
                    assert_eq!(output_buffer.bytes_written(), 0);
                    assert_eq!(output_buffer.write_all(&data[0..20]).expect("write"), 20);
                    assert_eq!(output_buffer.available(), 17);
                    assert_eq!(output_buffer.bytes_written(), 20);
                    assert_eq!(output_buffer.write_all(&data[20..37]).expect("write"), 17);
                    assert_eq!(output_buffer.available(), 0);
                    assert_eq!(output_buffer.bytes_written(), 37);
                    assert!(output_buffer.write_all(&data[37..50]).is_err());

                    let buffer = current_task
                        .read_memory_to_array::<128>(addr)
                        .expect("failed to write test data");
                    assert_eq!(&data[0..25], &buffer[0..25]);
                    assert_eq!(&data[25..37], &buffer[64..76]);
                }
            })
            .await;
        });
    }

    #[::fuchsia::test]
    fn test_vec_input_buffer() {
        let mut input_buffer = VecInputBuffer::new(b"helloworld");
        assert!(input_buffer.peek_each(&mut |data| Ok(data.len() + 1)).is_err());

        let mut input_buffer = VecInputBuffer::new(b"helloworld");
        assert_eq!(input_buffer.bytes_read(), 0);
        assert_eq!(input_buffer.available(), 10);
        assert_eq!(input_buffer.drain(), 10);
        assert_eq!(input_buffer.bytes_read(), 10);
        assert_eq!(input_buffer.available(), 0);

        let mut input_buffer = VecInputBuffer::new(b"helloworld");
        assert_eq!(input_buffer.bytes_read(), 0);
        assert_eq!(input_buffer.available(), 10);
        assert_eq!(&input_buffer.read_all().expect("read_all"), b"helloworld");
        assert_eq!(input_buffer.bytes_read(), 10);
        assert_eq!(input_buffer.available(), 0);

        let mut input_buffer = VecInputBuffer::new(b"helloworld");
        let mut buffer = [0; 5];
        assert_eq!(
            input_buffer.read_exact(slice_to_maybe_uninit_mut(&mut buffer)).expect("read"),
            5
        );
        assert_eq!(input_buffer.bytes_read(), 5);
        assert_eq!(input_buffer.available(), 5);
        assert_eq!(&buffer, b"hello");
        assert_eq!(
            input_buffer.read_exact(slice_to_maybe_uninit_mut(&mut buffer)).expect("read"),
            5
        );
        assert_eq!(input_buffer.bytes_read(), 10);
        assert_eq!(input_buffer.available(), 0);
        assert_eq!(&buffer, b"world");
        assert!(input_buffer.read_exact(slice_to_maybe_uninit_mut(&mut buffer)).is_err());

        // Test read_object
        let mut input_buffer = VecInputBuffer::new(b"hello");
        assert_eq!(input_buffer.bytes_read(), 0);
        let buffer: [u8; 3] = input_buffer.read_object().expect("read_object");
        assert_eq!(&buffer, b"hel");
        assert_eq!(input_buffer.bytes_read(), 3);
        let buffer: [u8; 2] = input_buffer.read_object().expect("read_object");
        assert_eq!(&buffer, b"lo");
        assert_eq!(input_buffer.bytes_read(), 5);
        assert!(input_buffer.read_object::<[u8; 1]>().is_err());
        assert_eq!(input_buffer.bytes_read(), 5);

        let mut input_buffer = VecInputBuffer::new(b"hello");
        assert_eq!(input_buffer.bytes_read(), 0);
        assert!(input_buffer.read_object::<[u8; 100]>().is_err());
        assert_eq!(input_buffer.bytes_read(), 0);
    }

    #[::fuchsia::test]
    fn test_vec_output_buffer() {
        let mut output_buffer = VecOutputBuffer::new(10);
        assert!(output_buffer.write_each(&mut |data| Ok(data.len() + 1)).is_err());
        assert_eq!(output_buffer.bytes_written(), 0);
        assert_eq!(output_buffer.available(), 10);
        assert_eq!(output_buffer.write_all(b"hello").expect("write"), 5);
        assert_eq!(output_buffer.bytes_written(), 5);
        assert_eq!(output_buffer.available(), 5);
        assert_eq!(output_buffer.data(), b"hello");
        assert_eq!(output_buffer.write_all(b"world").expect("write"), 5);
        assert_eq!(output_buffer.bytes_written(), 10);
        assert_eq!(output_buffer.available(), 0);
        assert_eq!(output_buffer.data(), b"helloworld");
        assert!(output_buffer.write_all(b"foo").is_err());
        let data: Vec<u8> = output_buffer.into();
        assert_eq!(data, b"helloworld".to_vec());
    }

    #[::fuchsia::test]
    fn test_vec_write_buffer() {
        let mut input_buffer = VecInputBuffer::new(b"helloworld");
        let mut output_buffer = VecOutputBuffer::new(20);
        assert_eq!(output_buffer.write_buffer(&mut input_buffer).expect("write_buffer"), 10);
        assert_eq!(output_buffer.data(), b"helloworld");
    }
}
