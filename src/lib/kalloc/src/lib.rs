// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![no_std]

#[cfg(not(is_kernel))]
extern crate alloc;

use core::alloc::Layout;
use core::ptr::NonNull;

mod allocator;
mod boxed;

pub use allocator::{AllocError, Allocator, DefaultAllocator, NoOpAllocator};
pub use boxed::Box;

/// Fallible allocation matching standard alloc interface.
///
/// # Safety
///
/// - `layout` must have non-zero size.
#[cfg(not(is_kernel))]
pub unsafe fn alloc(layout: Layout) -> Option<NonNull<u8>> {
    debug_assert!(layout.size() > 0);
    let ptr = unsafe { alloc::alloc::alloc(layout) };
    NonNull::new(ptr)
}

/// Fallible deallocation matching standard alloc interface.
///
/// # Safety
///
/// - `ptr` is a block of memory currently allocated via this allocator and,
/// - `layout` is the same layout that was used to allocate that block of memory.
#[cfg(not(is_kernel))]
pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    debug_assert!(layout.size() > 0);
    unsafe { alloc::alloc::dealloc(ptr, layout) }
}

/// Fallible reallocation matching standard alloc interface.
///
/// # Safety
///
/// - `ptr` is allocated via this allocator,
/// - `layout` is the same layout that was used to allocate that block of memory,
/// - `new_size` is greater than zero,
/// - `new_size`, when rounded up to the nearest multiple of `layout.align()`, does not overflow isize.
#[cfg(not(is_kernel))]
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> Option<NonNull<u8>> {
    debug_assert!(layout.size() > 0);
    debug_assert!(new_size > 0);
    let ptr = unsafe { alloc::alloc::realloc(ptr, layout, new_size) };
    NonNull::new(ptr)
}

/// Fallible zeroed allocation matching standard alloc interface.
///
/// # Safety
///
/// - `layout` must have non-zero size.
#[cfg(not(is_kernel))]
pub unsafe fn alloc_zeroed(layout: Layout) -> Option<NonNull<u8>> {
    debug_assert!(layout.size() > 0);
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
    NonNull::new(ptr)
}

/// Fallible allocation using kernel malloc.
///
/// # Safety
///
/// - `layout` must have non-zero size.
#[cfg(is_kernel)]
pub unsafe fn alloc(layout: Layout) -> Option<NonNull<u8>> {
    debug_assert!(layout.size() > 0);
    unsafe extern "C" {
        fn malloc(size: usize) -> *mut core::ffi::c_void;
        fn memalign(alignment: usize, size: usize) -> *mut core::ffi::c_void;
    }
    let ptr = if layout.align() <= 8 {
        unsafe { malloc(layout.size()) as *mut u8 }
    } else {
        unsafe { memalign(layout.align(), layout.size()) as *mut u8 }
    };
    NonNull::new(ptr)
}

/// Fallible deallocation using kernel free.
///
/// # Safety
///
/// - `ptr` is a block of memory currently allocated via this allocator and,
/// - `layout` is the same layout that was used to allocate that block of memory.
#[cfg(is_kernel)]
pub unsafe fn dealloc(ptr: *mut u8, layout: Layout) {
    debug_assert!(layout.size() > 0);
    unsafe extern "C" {
        fn free(ptr: *mut core::ffi::c_void);
    }
    unsafe { free(ptr as *mut core::ffi::c_void) }
}

/// Fallible reallocation using kernel realloc.
///
/// # Safety
///
/// - `ptr` is allocated via this allocator,
/// - `layout` is the same layout that was used to allocate that block of memory,
/// - `new_size` is greater than zero,
/// - `new_size`, when rounded up to the nearest multiple of `layout.align()`, does not overflow isize.
#[cfg(is_kernel)]
pub unsafe fn realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> Option<NonNull<u8>> {
    debug_assert!(layout.size() > 0);
    debug_assert!(new_size > 0);

    // Zircon kernel's cmpctmalloc doesn't implement a C-linkage `realloc`, so we
    // emulate it using alloc, copy, and dealloc.
    let new_layout = Layout::from_size_align(new_size, layout.align()).ok()?;
    let new_ptr = unsafe { crate::alloc(new_layout) };

    if let Some(new_p) = new_ptr {
        let copy_size = core::cmp::min(layout.size(), new_size);
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, new_p.as_ptr(), copy_size);
            crate::dealloc(ptr, layout);
        }
    }
    new_ptr
}

/// Fallible zeroed allocation using kernel calloc.
///
/// # Safety
///
/// - `layout` must have non-zero size.
#[cfg(is_kernel)]
pub unsafe fn alloc_zeroed(layout: Layout) -> Option<NonNull<u8>> {
    debug_assert!(layout.size() > 0);
    if layout.align() <= 8 {
        unsafe extern "C" {
            fn calloc(nmemb: usize, size: usize) -> *mut core::ffi::c_void;
        }
        let ptr = unsafe { calloc(1, layout.size()) as *mut u8 };
        NonNull::new(ptr)
    } else {
        let ptr = unsafe { crate::alloc(layout) };
        if let Some(p) = ptr {
            unsafe {
                core::ptr::write_bytes(p.as_ptr(), 0, layout.size());
            }
        }
        ptr
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kalloc_alloc() {
        let layout = Layout::from_size_align(10, 1).unwrap();
        unsafe {
            let ptr = crate::alloc(layout).unwrap();
            crate::dealloc(ptr.as_ptr(), layout);
        }
    }

    #[test]
    fn test_kalloc_realloc() {
        let layout = Layout::from_size_align(10, 1).unwrap();
        unsafe {
            let ptr = crate::alloc(layout).unwrap();
            let new_ptr = crate::realloc(ptr.as_ptr(), layout, 20).unwrap();
            crate::dealloc(new_ptr.as_ptr(), Layout::from_size_align(20, 1).unwrap());
        }
    }

    #[test]
    fn test_kalloc_alloc_zeroed() {
        let layout = Layout::from_size_align(10, 1).unwrap();
        unsafe {
            let ptr = crate::alloc_zeroed(layout).unwrap();
            let slice = core::slice::from_raw_parts(ptr.as_ptr(), 10);
            assert!(slice.iter().all(|&b| b == 0));
            crate::dealloc(ptr.as_ptr(), layout);
        }
    }
}
