// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::pmm_arena::PmmArenaInfo;
use super::pmm_node::PmmNode;
use crate::kernel::types::PAddr;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_queues::PageQueues;
use core::mem::MaybeUninit;
use core::pin::Pin;
use pmm_bindings as bindings;
use zx_status::Status;

// Flags for PMM allocation routines.
pub const ALLOC_FLAG_ANY: u32 = bindings::PMM_ALLOC_FLAG_ANY;
pub const ALLOC_FLAG_CAN_WAIT: u32 = bindings::PMM_ALLOC_FLAG_CAN_WAIT;

// One of the members of the PmmNode has a zero sized type, which is not FFI compliant, however we
// are separately validating the equivalence of the final layout of the C++ and Rust objects so
// this is okay.
#[allow(improper_ctypes)]
unsafe extern "C" {
    // C++ name mangled form of `PmmNode Pmm::node_`
    #[link_name = "_ZN3Pmm5node_E"]
    static PMM_NODE: PmmNode;
}

/// Allocates a single physical page from the PMM.
pub fn alloc_page(flags: u32) -> Result<(VmPagePtr, PAddr), Status> {
    let mut page = core::ptr::null_mut();
    let mut paddr: bindings::zx_paddr_t = 0;
    // SAFETY: FFI call passing valid stack addresses to store the page pointer and physical
    // address.
    let status = unsafe { bindings::cpp_pmm_alloc_page(flags, &mut page, &mut paddr) };
    Status::ok(status)?;
    // SAFETY: `page` is a valid page pointer returned by the PMM on success.
    let page_ptr = unsafe { VmPagePtr::from_ffi(page) }.ok_or(Status::NO_MEMORY)?;
    Ok((page_ptr, PAddr(paddr)))
}

/// Frees a single physical page back to the PMM.
///
/// # Safety
///
/// Caller must ensure `page` is a valid allocated PMM page that has not already been freed.
pub unsafe fn free_page(page: VmPagePtr) {
    // SAFETY: Caller guarantees `page` is a valid allocated PMM page.
    unsafe { bindings::cpp_pmm_free_page(page.as_ffi()) };
}

/// Frees every page on `list` back to the PMM, emptying the list.  The PMM lock
/// is taken once for the whole list rather than once per page.
///
/// # Safety
///
/// Caller must ensure every page on the list is a valid allocated PMM page that
/// has not already been freed.
pub unsafe fn free_list(list: Pin<&mut VmPageDoublyLinkedList>) {
    if list.is_empty() {
        return;
    }
    // SAFETY: `DoublyLinkedList` is `repr(C)` and mirrors `VmPageDoublyLinkedList`
    // -- a bare head pointer with the same sentinel encoding -- so C++ can drain it
    // in place.  The caller guarantees the pages.
    unsafe {
        bindings::cpp_pmm_free_list(
            (list.get_unchecked_mut() as *mut VmPageDoublyLinkedList).cast(),
        )
    };
}

/// Converts a physical address to a `VmPagePtr`.
pub fn paddr_to_vm_page(paddr: PAddr) -> Option<VmPagePtr> {
    let raw = unsafe { bindings::cpp_paddr_to_vm_page(paddr.0) };
    // SAFETY: cpp_paddr_to_vm_page returns a valid VmPagePtr, or null.
    unsafe { VmPagePtr::from_ffi(raw) }
}

/// Allocate count pages of physical memory, adding to the tail of the passed list.
/// The list must be initialized.
/// Note that if PMM_ALLOC_FLAG_CAN_WAIT is passed in then this could always return
/// ZX_ERR_SHOULD_WAIT. Since there is no way to wait until an arbitrary number of pages can be
/// allocated (see comment on |pmm_wait_till_should_retry_single_alloc|) passing
/// PMM_ALLOC_FLAG_CAN_WAIT here should be used as an optimistic fast path, and the caller should
/// have a fallback of allocating single pages.
pub fn alloc_pages(
    count: usize,
    flags: u32,
    list: Pin<&mut VmPageDoublyLinkedList>,
) -> Result<(), Status> {
    // SAFETY: FFI call passing pointer to `list`.
    let status = unsafe {
        bindings::cpp_pmm_alloc_pages(
            count,
            flags,
            (list.get_unchecked_mut() as *mut VmPageDoublyLinkedList).cast(),
        )
    };
    Status::ok(status)
}

/// Allocate a run of contiguous pages, aligned on log2 byte boundary (0-31).
/// Return the base address of the run in the physical address pointer and
/// append the allocate page structures to the tail of the passed in list.
pub fn alloc_contiguous(
    count: usize,
    flags: u32,
    align_log2: u8,
    list: Pin<&mut VmPageDoublyLinkedList>,
) -> Result<PAddr, Status> {
    let mut pa: bindings::zx_paddr_t = 0;
    // SAFETY: FFI call passing stack pointer for `pa` and pointer to `list`.
    let status = unsafe {
        bindings::cpp_pmm_alloc_contiguous(
            count,
            flags,
            align_log2,
            &mut pa,
            (list.get_unchecked_mut() as *mut VmPageDoublyLinkedList).cast(),
        )
    };
    Status::ok(status)?;
    Ok(PAddr(pa))
}

/// Returns the number of physical memory arenas.
pub fn num_arenas() -> usize {
    // SAFETY: No preconditions.
    unsafe { bindings::cpp_pmm_num_arenas() }
}

// Fills |buffer| with PmmArenaInfo objects starting at |offset| arena, ordered by base address.
// For example, passing an |offset| of 1 would skip the 1st arena.
//
// Returns OUT_OF_RANGE if |offset| would yield an invalid range or |buffer| is too large.
//
// Returns BUFFER_TOO_SMALL if the |buffer| is too small.
pub fn get_arena_info(
    offset: usize,
    buffer: &mut [MaybeUninit<PmmArenaInfo>],
) -> Result<&mut [PmmArenaInfo], Status> {
    // SAFETY: FFI call passing point to |buffer|.
    let status = unsafe {
        bindings::cpp_pmm_get_arena_info(
            buffer.len(),
            offset as u64,
            buffer.as_mut_ptr().cast(),
            buffer.len() * core::mem::size_of::<PmmArenaInfo>(),
        )
    };
    Status::ok(status)?;
    // SAFETY: cpp_pmm_get_arena_info fully initializes this buffer on success.
    unsafe { Ok(buffer.assume_init_mut()) }
}

/// Returns the static `PageQueues` instance associated with the PMM.
pub fn page_queues() -> &'static PageQueues {
    // SAFETY: No preconditions.
    let queues = unsafe { bindings::cpp_pmm_page_queues() };
    let queues: *const PageQueues = queues.cast();
    // SAFETY: `cpp_pmm_page_queues` returns a valid static pointer to the global PmmNode's
    // PageQueues.
    unsafe { queues.as_ref_unchecked() }
}

/// Returns a reference to the global `PmmNode` instance.
pub fn node() -> &'static PmmNode {
    unsafe { &PMM_NODE }
}

/// Returns the `VmCompression` instance if compression is enabled.
pub fn get_page_compression() -> Option<&'static crate::vm::compression::VmCompression> {
    let raw = unsafe { bindings::cpp_pmm_get_page_compression() };
    if raw.is_null() {
        None
    } else {
        // SAFETY: `raw` points to a live `VmCompression` owned by the global PmmNode.
        Some(unsafe { &*(raw as *const crate::vm::compression::VmCompression) })
    }
}

/// Unit tests for PMM.
#[cfg(ktest)]
#[unittest::suite(name = "pmm_rust")]
mod pmm_rust {
    use super::{
        alloc_contiguous, alloc_page, free_list, free_page, node, num_arenas, paddr_to_vm_page,
    };
    use crate::kernel::types::PAddr;
    use crate::vm::physmap::paddr_to_physmap;
    use crate::vm::pmm_arena::PmmArenaInfo;
    use pin_init::stack_pin_init;
    use unittest::{
        assert_eq, assert_err, assert_ge, assert_gt, assert_ne, assert_true, unwrap_ok,
    };
    use zx_status::Status;

    /// Allocates a single page, translates it to a vm_page_t and frees it.
    #[test]
    fn smoke() {
        let (page, pa) = unwrap_ok!(alloc_page(0), "pmm_alloc single page");
        assert_ne!(pa.0, 0, "pmm_alloc single page");

        let page2 = paddr_to_vm_page(pa);
        assert_true!(page2 == Some(page), "paddr_to_vm_page on single page");

        unsafe { free_page(page) };
    }

    /// Allocates one page and frees it.
    #[test]
    fn alloc_contiguous_one() {
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let count = 1usize;
        let pa = unwrap_ok!(
            alloc_contiguous(count, 0, page::SHIFT as u8, list.as_mut()),
            "pmm_alloc_contiguous returned failure\n"
        );
        assert_eq!(count, list.iter().count(), "pmm_alloc_contiguous list size is wrong");
        assert_ne!(paddr_to_physmap(pa).0, 0);
        unsafe { free_list(list) };
    }

    /// Tests getting arena info.
    #[test]
    fn get_arena_info() {
        let n_arenas = num_arenas();
        assert_gt!(n_arenas, 0);

        let mut oversize_buffer = unwrap_ok!(
            kalloc::Box::<[PmmArenaInfo]>::try_new_uninit_slice(n_arenas + 1)
                .map_err(|_| Status::NO_MEMORY)
        );
        let mut buffer = unwrap_ok!(
            kalloc::Box::<[PmmArenaInfo]>::try_new_uninit_slice(n_arenas)
                .map_err(|_| Status::NO_MEMORY)
        );

        // Asking for none.
        let result = super::get_arena_info(0, &mut buffer[0..0]);
        assert_err!(result, Status::OUT_OF_RANGE);

        // Asking for more than exist.
        let result = super::get_arena_info(0, &mut oversize_buffer[..]);
        assert_err!(result, Status::OUT_OF_RANGE);

        // Attempting to skip them all.
        let result = super::get_arena_info(n_arenas, &mut buffer[0..1]);
        assert_err!(result, Status::OUT_OF_RANGE);

        // Asking for one.
        let result = super::get_arena_info(0, &mut buffer[0..1]);
        let _ = unwrap_ok!(result);

        // Asking for them all.
        let result = super::get_arena_info(0, &mut buffer[..]);
        let buffer = unwrap_ok!(result);

        // See they are in ascending order by base.
        let mut prev = PAddr(0);
        for (i, arena) in buffer.iter().enumerate() {
            if i == 0 {
                assert_ge!(arena.base.0, prev.0);
            } else {
                assert_gt!(arena.base.0, prev.0);
            }
            prev = arena.base;
            assert_gt!(arena.size, 0);
        }
    }

    /// Tests converting between pages and indexes.
    #[test]
    fn page_to_from_index() {
        // Assert that indexes have zero bits, can roundtrip and are distinct for distinct pages.
        let (page0, _pa0) = unwrap_ok!(alloc_page(0), "pmm_alloc single page");
        let index0 = node().page_to_index(page0);
        assert_ne!(0, index0);
        let zero_bits_mask = (1u32 << PmmNode::INDEX_ZERO_BITS) - 1;
        assert_eq!(0, index0 & zero_bits_mask);
        let same_page = unsafe { node().index_to_page(index0) };
        assert_true!(page0 == same_page);
        assert_eq!(unsafe { page0.paddr() }.0, unsafe { node().index_to_paddr(index0).0 });

        let (page1, _pa1) = unwrap_ok!(alloc_page(0), "pmm_alloc single page");
        let index1 = node().page_to_index(page1);
        assert_ne!(0, index1);

        assert_ne!(index0, index1);

        unsafe { free_page(page0) };
        unsafe { free_page(page1) };
    }
}
