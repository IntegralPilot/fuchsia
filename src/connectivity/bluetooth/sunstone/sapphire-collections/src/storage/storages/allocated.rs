// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(feature = "std")]

use core::ptr::NonNull;
use std::alloc::Layout;

use crate::AllocError;
use crate::storage::Storage;

/// Storage backing that uses the global allocator.
#[derive(Default, Clone, Copy, Debug)]
pub struct Global;

// SAFETY: The allocated pointers are resolved to themselves, and we delegate
// to the standard library's global allocator which guarantees safety properties.
unsafe impl Storage for Global {
    type Handle = NonNull<()>;

    unsafe fn resolve(&self, handle: Self::Handle) -> NonNull<()> {
        handle
    }

    fn allocate(&mut self, layout: Layout) -> Result<(Self::Handle, usize), AllocError> {
        if layout.size() == 0 {
            return Ok((NonNull::dangling(), 0));
        }
        // SAFETY: layout size is guaranteed to be non-zero.
        let buf = unsafe { std::alloc::alloc(layout) };
        let buf = NonNull::new(buf).ok_or(AllocError)?;
        Ok((buf.cast(), layout.size()))
    }

    unsafe fn deallocate(&mut self, layout: Layout, handle: Self::Handle) {
        if layout.size() == 0 {
            return;
        }
        // SAFETY: handle was allocated by this allocator with layout.
        unsafe { std::alloc::dealloc(handle.as_ptr().cast(), layout) };
    }

    unsafe fn grow(
        &mut self,
        old_layout: Layout,
        new_layout: Layout,
        handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError> {
        assert!(old_layout.align() >= new_layout.align());
        if new_layout.size() == 0 {
            return Ok((NonNull::dangling(), 0));
        }
        if old_layout.size() == 0 {
            return self.allocate(new_layout);
        }
        // SAFETY: handle was allocated by this allocator with old_layout.
        let buf =
            unsafe { std::alloc::realloc(handle.as_ptr().cast(), old_layout, new_layout.size()) };
        let buf = NonNull::new(buf).ok_or(AllocError)?;
        Ok((buf.cast(), new_layout.size()))
    }

    unsafe fn shrink(
        &mut self,
        old_layout: Layout,
        new_layout: Layout,
        handle: Self::Handle,
    ) -> Result<(Self::Handle, usize), AllocError> {
        assert!(old_layout.align() >= new_layout.align());
        if new_layout.size() == 0 {
            return Ok((NonNull::dangling(), 0));
        }
        if old_layout.size() == 0 {
            return self.allocate(new_layout);
        }
        // SAFETY: handle was allocated by this allocator with old_layout.
        let buf =
            unsafe { std::alloc::realloc(handle.as_ptr().cast(), old_layout, new_layout.size()) };
        let buf = NonNull::new(buf).ok_or(AllocError)?;
        Ok((buf.cast(), new_layout.size()))
    }
}
