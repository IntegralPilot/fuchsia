// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::vm::page::VmPageDoublyLinkedList;
use crate::vm::pmm;
use core::ffi;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::{self, NonNull};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::zx_status_t;

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn cpp_page_cache_create(reserve_pages: usize, out_cache: *mut *mut ffi::c_void)
    -> zx_status_t;
    fn cpp_page_cache_delete(cache: *mut ffi::c_void);
    fn cpp_page_cache_reserve_pages(cache: *const ffi::c_void) -> usize;
    fn cpp_page_cache_alloc(
        cache: *mut ffi::c_void,
        count: usize,
        out_pages: *mut VmPageDoublyLinkedList,
        out_available_pages: *mut usize,
    ) -> zx_status_t;
    fn cpp_page_cache_free(cache: *mut ffi::c_void, pages: *mut VmPageDoublyLinkedList);
}

/// An RAII wrapper around `VmPageDoublyLinkedList` that automatically frees remaining pages
/// back to the PMM upon destruction.
#[pin_data(PinnedDrop)]
pub struct PageList {
    #[pin]
    pages: VmPageDoublyLinkedList,
}

impl PageList {
    /// Creates an in-place initializer for a new, empty `PageList`.
    pub fn new() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            pages <- VmPageDoublyLinkedList::new(),
        })
    }

    /// Returns a shared reference to the underlying `VmPageDoublyLinkedList`.
    pub fn list(&self) -> &VmPageDoublyLinkedList {
        &self.pages
    }

    /// Returns a mutable reference to the underlying `VmPageDoublyLinkedList`.
    pub fn list_mut(&mut self) -> &mut VmPageDoublyLinkedList {
        &mut self.pages
    }
}

impl Deref for PageList {
    type Target = VmPageDoublyLinkedList;

    fn deref(&self) -> &Self::Target {
        &self.pages
    }
}

impl DerefMut for PageList {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.pages
    }
}

#[pinned_drop]
impl PinnedDrop for PageList {
    fn drop(self: Pin<&mut Self>) {
        if !self.pages.is_empty() {
            // SAFETY: Pages in `self.pages` are valid allocated PMM pages that have not
            // already been freed.
            unsafe {
                pmm::free_list(self.project().pages);
            }
        }
    }
}

/// The number of pages remaining in the per-CPU cache after an allocation request.
pub type AvailablePages = usize;

/// A wrapper around the C++ `page_cache::PageCache`.
pub struct PageCache {
    ptr: NonNull<ffi::c_void>,
}

// SAFETY: `PageCache` internally synchronizes per-CPU caches and fallback locking.
unsafe impl Send for PageCache {}
unsafe impl Sync for PageCache {}

impl PageCache {
    /// Creates a new page cache with the given number of reserve pages per CPU.
    pub fn new(reserve_pages: usize) -> Result<Self, Status> {
        let mut ptr = ptr::null_mut();
        // SAFETY: Calling C++ FFI to create a PageCache.
        let status = unsafe { cpp_page_cache_create(reserve_pages, &mut ptr) };
        Status::ok(status)?;
        let ptr = NonNull::new(ptr).ok_or(Status::NO_MEMORY)?;
        Ok(Self { ptr })
    }

    /// Returns the number of reserve pages per CPU configured for this cache.
    pub fn reserve_pages(&self) -> usize {
        // SAFETY: `self.ptr` is valid.
        unsafe { cpp_page_cache_reserve_pages(self.ptr.as_ptr()) }
    }

    /// Allocates `count` pages from the page cache into `out_pages`.
    pub fn alloc(
        &self,
        count: usize,
        mut out_pages: Pin<&mut PageList>,
    ) -> Result<AvailablePages, Status> {
        let mut available_pages = 0;
        // SAFETY: `out_pages` is pinned in memory; we access its inner list without moving it.
        let list = unsafe { out_pages.as_mut().get_unchecked_mut() };
        // SAFETY: Calling C++ FFI with valid PageCache pointer and valid out_pages pointer.
        let status = unsafe {
            cpp_page_cache_alloc(self.ptr.as_ptr(), count, &mut list.pages, &mut available_pages)
        };
        Status::ok(status)?;
        Ok(available_pages)
    }

    /// Frees pages back to the page cache. Excess pages are returned to the PMM.
    pub fn free(&self, mut pages: Pin<&mut PageList>) {
        // SAFETY: `pages` is pinned in memory; we access its inner list without moving it.
        let list = unsafe { pages.as_mut().get_unchecked_mut() };
        // SAFETY: Calling C++ FFI with valid PageCache pointer and valid pages pointer.
        unsafe { cpp_page_cache_free(self.ptr.as_ptr(), &mut list.pages) };
    }
}

impl Drop for PageCache {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is a valid pointer allocated by `cpp_page_cache_create`.
        unsafe { cpp_page_cache_delete(self.ptr.as_ptr()) };
    }
}

/// In-tree kernel unit tests for `page_cache`.
#[cfg(ktest)]
#[unittest::suite(name = "page_cache_rust")]
mod tests {
    use super::{PageCache, PageList};
    use pin_init::stack_pin_init;
    use unittest::{expect_eq, expect_false, expect_true, unwrap_ok};

    /// Tests basic page cache allocation and freeing.
    #[test]
    fn test_page_cache_alloc_and_free() {
        let cache = unwrap_ok!(PageCache::new(8));
        expect_eq!(cache.reserve_pages(), 8);

        stack_pin_init!(let pages = PageList::new());
        expect_true!(pages.is_empty());

        let available_pages = unwrap_ok!(cache.alloc(4, pages.as_mut()));
        expect_false!(pages.is_empty());
        expect_eq!(available_pages, 8);

        cache.free(pages.as_mut());
        expect_true!(pages.is_empty());
    }

    /// Tests that PageList drop automatically frees unconsumed pages back to PMM.
    #[test]
    fn test_page_list_drop_frees_pages() {
        let cache = unwrap_ok!(PageCache::new(4));

        {
            stack_pin_init!(let pages = PageList::new());
            let _available_pages = unwrap_ok!(cache.alloc(2, pages.as_mut()));
            expect_false!(pages.is_empty());
            // Dropping `pages` without calling `cache.free`:
            // `PageList::drop` must safely return pages to the PMM without leaking.
        }
    }
}
