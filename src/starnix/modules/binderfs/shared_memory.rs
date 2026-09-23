// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::thread::TransactionError;
use starnix_core::mm::memory::MemoryObject;
use starnix_logging::{log_error, log_trace};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::errors::Errno;
use starnix_uapi::math::round_up_to_increment;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{binder_uintptr_t, errno, error};
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use zerocopy::IntoBytes;
use zx;

/// A tracked memory buffer segment within [`SharedMemory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BinderBufferNode {
    /// Offset from the start of the shared memory region.
    pub offset: usize,
    /// Total size in bytes of this buffer segment.
    pub size: usize,
    /// Whether this segment is currently allocated to an active transaction.
    pub is_free: bool,
    /// Offset of the preceding adjacent buffer in address space, if any.
    pub prev_offset: Option<usize>,
    /// Offset of the succeeding adjacent buffer in address space, if any.
    pub next_offset: Option<usize>,
}

/// The mapped VMO shared between userspace and the binder driver.
///
/// The binder driver copies messages from one process to another, which essentially amounts to
/// a copy between VMOs. It is not possible to copy directly between VMOs without an intermediate
/// copy, and the binder driver must only perform one copy for performance reasons.
///
/// The memory allocated to a binder process is shared with the binder driver, and mapped into
/// the kernel's address space so that a VMO read operation can copy directly into the mapped VMO.
#[derive(Debug)]
pub struct SharedMemory {
    /// The address in kernel address space where the VMO is mapped.
    kernel_address: *mut u8,
    /// The address in user address space where the VMO is mapped.
    pub user_address: UserAddress,
    /// The length of the shared memory mapping in bytes.
    pub length: usize,
    /// Hash map of all buffer segments indexed by starting offset.
    /// Combined with `prev_offset` and `next_offset` in [`BinderBufferNode`], forms an intrusive
    /// doubly-linked list with O(1) predecessor and successor lookup during coalescing.
    buffers_by_offset: HashMap<usize, BinderBufferNode>,
    /// Index of all free buffer segments ordered by (size, offset).
    /// Used for O(log K) Best-Fit search during allocate.
    free_buffers_by_size: BTreeSet<(usize, usize)>,
}

/// The user buffers containing the data to send to the recipient of a binder transaction.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TransactionBuffers {
    /// The buffer containing the data of the transaction.
    pub data: UserBuffer,
    /// The buffer containing the offsets of objects inside the `data` buffer.
    pub offsets: UserBuffer,
    /// An optional buffer pointing to the security context of the client of the transaction.
    pub security_context: Option<UserBuffer>,
}

/// Contains the allocations for a transaction.
#[derive(Debug)]
pub struct SharedMemoryAllocation<'a> {
    pub data_buffer: SharedBuffer<'a, u8>,
    pub offsets_buffer: SharedBuffer<'a, binder_uintptr_t>,
    pub scatter_gather_buffer: SharedBuffer<'a, u8>,
    pub security_context_buffer: Option<SharedBuffer<'a, u8>>,
}

impl From<SharedMemoryAllocation<'_>> for TransactionBuffers {
    fn from(value: SharedMemoryAllocation<'_>) -> Self {
        Self {
            data: value.data_buffer.user_buffer(),
            offsets: value.offsets_buffer.user_buffer(),
            security_context: value.security_context_buffer.map(|x| x.user_buffer()),
        }
    }
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        log_trace!("Dropping shared memory allocation {:?}", self);
        let kernel_root_vmar = fuchsia_runtime::vmar_root_self();

        // SAFETY: This object hands out references to the mapped memory, but the borrow checker
        // ensures correct lifetimes.
        let res = unsafe { kernel_root_vmar.unmap(self.kernel_address as usize, self.length) };
        match res {
            Ok(()) => {}
            Err(status) => {
                log_error!("failed to unmap shared binder region from kernel: {:?}", status);
            }
        }
    }
}

// SAFETY: SharedMemory has exclusive ownership of the `kernel_address` pointer, so it is safe to
// send across threads.
unsafe impl Send for SharedMemory {}

impl SharedMemory {
    pub fn map(
        memory: &MemoryObject,
        user_address: UserAddress,
        length: usize,
    ) -> Result<Self, Errno> {
        if length == 0 {
            return error!(EINVAL);
        }

        // Map the VMO into the kernel's address space.
        let kernel_root_vmar = fuchsia_runtime::vmar_root_self();
        let kernel_address = memory
            .map_in_vmar(
                &kernel_root_vmar,
                0,
                0,
                length,
                zx::VmarFlags::PERM_READ | zx::VmarFlags::PERM_WRITE,
            )
            .map_err(|status| {
                log_error!("failed to map shared binder region in kernel: {:?}", status);
                errno!(ENOMEM)
            })?;

        let mut buffers_by_offset = HashMap::new();
        let mut free_buffers_by_size = BTreeSet::new();

        buffers_by_offset.insert(
            0,
            BinderBufferNode {
                offset: 0,
                size: length,
                is_free: true,
                prev_offset: None,
                next_offset: None,
            },
        );
        free_buffers_by_size.insert((length, 0));

        Ok(Self {
            kernel_address: kernel_address as *mut u8,
            user_address,
            length,
            buffers_by_offset,
            free_buffers_by_size,
        })
    }

    /// Allocate a buffer of size `length` from this memory block using a Best-Fit strategy.
    fn allocate(&mut self, length: usize) -> Result<usize, TransactionError> {
        if length == 0 || length > self.length {
            return Err(TransactionError::Failure);
        }

        // Find the best-fit free chunk: smallest free chunk with size >= length.
        let &(chunk_size, chunk_offset) = self
            .free_buffers_by_size
            .range((length, 0)..)
            .next()
            .ok_or(TransactionError::Failure)?;

        self.free_buffers_by_size.remove(&(chunk_size, chunk_offset));

        let node = self.buffers_by_offset.get_mut(&chunk_offset).expect("buffer node must exist");
        let remainder_size = chunk_size - length;

        if remainder_size > 0 {
            let remainder_offset = chunk_offset + length;
            let old_next = node.next_offset;

            // Update allocated chunk at chunk_offset
            node.size = length;
            node.is_free = false;
            node.next_offset = Some(remainder_offset);

            // Insert new free chunk for remainder
            self.buffers_by_offset.insert(
                remainder_offset,
                BinderBufferNode {
                    offset: remainder_offset,
                    size: remainder_size,
                    is_free: true,
                    prev_offset: Some(chunk_offset),
                    next_offset: old_next,
                },
            );
            self.free_buffers_by_size.insert((remainder_size, remainder_offset));

            // Update next node's prev pointer
            if let Some(next_offset) = old_next {
                if let Some(next_node) = self.buffers_by_offset.get_mut(&next_offset) {
                    next_node.prev_offset = Some(remainder_offset);
                }
            }
        } else {
            node.is_free = false;
        }

        Ok(chunk_offset)
    }

    /// Allocates three buffers large enough to hold the requested data, offsets, and scatter-gather
    /// buffer lengths, inserting padding between data and offsets as needed. `offsets_length` and
    /// `sg_buffers_length` must be 8-byte aligned.
    ///
    /// NOTE: When `data_length` is zero, a minimum data buffer size of 8 bytes is still allocated.
    /// This is because clients expect their buffer addresses to be uniquely associated with a
    /// transaction. Returning the same address for different transactions will break oneway
    /// transactions that have no payload.
    pub fn allocate_buffers(
        &mut self,
        data_length: usize,
        offsets_length: usize,
        sg_buffers_length: usize,
        security_context_buffer_length: usize,
    ) -> Result<SharedMemoryAllocation<'_>, TransactionError> {
        // Round `data_length` up to the nearest multiple of 8, so that the offsets buffer is
        // aligned when we pack it next to the data buffer.
        let data_cap = round_up_to_increment(data_length, std::mem::size_of::<binder_uintptr_t>())?;
        // Ensure that we allocate at least 8 bytes, so that each buffer returned is uniquely
        // associated with a transaction. Otherwise, multiple zero-sized allocations will have the
        // same address and there will be no way of distinguishing which transaction they belong to.
        let data_cap = std::cmp::max(data_cap, std::mem::size_of::<binder_uintptr_t>());
        // Ensure that the offsets and buffers lengths are valid.
        if offsets_length % std::mem::size_of::<binder_uintptr_t>() != 0
            || sg_buffers_length % std::mem::size_of::<binder_uintptr_t>() != 0
            || security_context_buffer_length % std::mem::size_of::<binder_uintptr_t>() != 0
        {
            return Err(TransactionError::Malformed(errno!(EINVAL)));
        }
        let total_length = data_cap
            .checked_add(offsets_length)
            .and_then(|v| v.checked_add(sg_buffers_length))
            .and_then(|v| v.checked_add(security_context_buffer_length))
            .ok_or_else(|| errno!(EINVAL))?;
        let base_offset = self.allocate(total_length)?;
        let security_context_buffer = if security_context_buffer_length > 0 {
            Some(SharedBuffer::new(
                self,
                base_offset + data_cap + offsets_length + sg_buffers_length,
                security_context_buffer_length,
            )?)
        } else {
            None
        };

        Ok(SharedMemoryAllocation {
            data_buffer: SharedBuffer::new(self, base_offset, data_length)?,
            offsets_buffer: SharedBuffer::new(self, base_offset + data_cap, offsets_length)?,
            scatter_gather_buffer: SharedBuffer::new(
                self,
                base_offset + data_cap + offsets_length,
                sg_buffers_length,
            )?,
            security_context_buffer,
        })
    }

    // Reclaim the buffer so that it can be reused, coalescing adjacent free neighbors in O(1).
    pub fn free_buffer(&mut self, buffer: UserAddress) -> Result<(), Errno> {
        // Sanity check that the buffer being freed came from this memory region.
        if buffer < self.user_address || buffer >= (self.user_address + self.length)? {
            return error!(EINVAL);
        }
        let offset = buffer - self.user_address;

        let mut node = match self.buffers_by_offset.remove(&offset) {
            Some(node) if !node.is_free => node,
            Some(node) => {
                self.buffers_by_offset.insert(offset, node);
                return error!(EINVAL);
            }
            None => return error!(EINVAL),
        };
        node.is_free = true;

        // 1. Check predecessor
        if let Some(prev_offset) = node.prev_offset {
            if let Entry::Occupied(prev_entry) = self.buffers_by_offset.entry(prev_offset) {
                if prev_entry.get().is_free {
                    let prev_node = prev_entry.remove();
                    self.free_buffers_by_size.remove(&(prev_node.size, prev_offset));
                    node.offset = prev_offset;
                    node.size += prev_node.size;
                    node.prev_offset = prev_node.prev_offset;
                }
            }
        }

        // 2. Check successor
        if let Some(next_offset) = node.next_offset {
            if let Entry::Occupied(next_entry) = self.buffers_by_offset.entry(next_offset) {
                if next_entry.get().is_free {
                    let next_node = next_entry.remove();
                    self.free_buffers_by_size.remove(&(next_node.size, next_offset));
                    node.size += next_node.size;
                    node.next_offset = next_node.next_offset;
                }
            }
        }

        // 3. Update the successor's prev_offset pointer if one exists.
        if let Some(next_offset) = node.next_offset {
            if let Some(next_node) = self.buffers_by_offset.get_mut(&next_offset) {
                next_node.prev_offset = Some(node.offset);
            }
        }

        // 4. Insert the merged free node back into both indices.
        self.free_buffers_by_size.insert((node.size, node.offset));
        self.buffers_by_offset.insert(node.offset, node);

        Ok(())
    }
}

/// A buffer of memory allocated from a binder process' [`SharedMemory`].
#[derive(Debug)]
pub struct SharedBuffer<'a, T> {
    pub memory: &'a SharedMemory,
    /// Offset into the shared memory region where the buffer begins.
    pub offset: usize,
    /// The length of the buffer in bytes.
    pub length: usize,
    /// The underlying buffer.
    user_buffer: UserBuffer,
    // A zero-sized type that satisfies the compiler's need for the struct to reference `T`, which
    // is used in `as_mut_bytes` and `as_bytes`.
    _phantom_data: std::marker::PhantomData<T>,
}

impl<'a, T: IntoBytes> SharedBuffer<'a, T> {
    /// Creates a new `SharedBuffer`, which represents a sub-region of `memory` starting at `offset`
    /// bytes, with `length` bytes. Will return EFAULT if the sub-region is not within memory bounds.
    /// The caller is responsible for ensuring it is not aliased.
    fn new(memory: &'a SharedMemory, offset: usize, length: usize) -> Result<Self, Errno> {
        let memory_address = (memory.user_address + offset)?;
        // Validate that the entire buffer length is valid as well.
        let _ = (memory_address + length)?;
        let user_buffer = UserBuffer { address: memory_address, length: length };
        Ok(Self { memory, offset, length, user_buffer, _phantom_data: std::marker::PhantomData })
    }

    /// Returns a mutable slice of the buffer.
    pub fn as_mut_bytes(&mut self) -> &'a mut [T] {
        // SAFETY: `offset + length` was bounds-checked by `new`, and the memory region pointed to
        // was zero-allocated by mapping a new VMO in `allocate_buffers`.
        unsafe {
            std::slice::from_raw_parts_mut(
                self.memory.kernel_address.add(self.offset) as *mut T,
                self.length / std::mem::size_of::<T>(),
            )
        }
    }

    /// Returns an immutable slice of the buffer.
    pub fn as_bytes(&self) -> &'a [T] {
        // SAFETY: `offset + length` was bounds-checked by `new`, and the memory region pointed to
        // was zero-allocated by mapping a new VMO in `allocate_buffers`.
        unsafe {
            std::slice::from_raw_parts(
                self.memory.kernel_address.add(self.offset) as *const T,
                self.length / std::mem::size_of::<T>(),
            )
        }
    }

    /// The userspace address and length of the buffer.
    pub fn user_buffer(&self) -> UserBuffer {
        self.user_buffer
    }
}
