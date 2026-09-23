// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::kernel::types::PAddr;
use crate::vm::page::{VmPage, VmPageDoublyLinkedList};
use crate::vm::page_state::{VmPageState, page_state_to_string};
use crate::vm::physmap;
use crate::vm::pmm_node::PmmNode;
use core::ptr::NonNull;
use debug::{dprintf, ltracef};
use kprint::kprintln;
use page_bindings::{self, vm_page_state};
use pmm_arena_bindings as bindings;
use zx_status::Status;

use ::page as kernel_page;

const LOCAL_TRACE: u32 = 0;

// A possibly "lossy" estimate of the maximum number of page runs examined while performing a
// contiguous allocation.  See the comment where this counter is updated.
define_kcounter!(COUNTER_MAX_RUNS_EXAMINED, "vm.pmm.max_runs_examined", Max);

#[repr(C)]
#[derive(Debug, Clone)]
pub struct PmmArenaInfo {
    pub name: [u8; 16],
    pub flags: u32,
    pub base: PAddr,
    pub size: usize,
}

// Compile-time layout assertions against the C++ pmm_arena_info_t type via bindgen.
zr::static_assert!(
    core::mem::size_of::<PmmArenaInfo>() == core::mem::size_of::<bindings::pmm_arena_info_t>()
);
zr::static_assert!(
    core::mem::align_of::<PmmArenaInfo>() == core::mem::align_of::<bindings::pmm_arena_info_t>()
);
zr::static_assert!(
    core::mem::offset_of!(PmmArenaInfo, name)
        == core::mem::offset_of!(bindings::pmm_arena_info, name)
);
zr::static_assert!(
    core::mem::offset_of!(PmmArenaInfo, flags)
        == core::mem::offset_of!(bindings::pmm_arena_info, flags)
);
zr::static_assert!(
    core::mem::offset_of!(PmmArenaInfo, base)
        == core::mem::offset_of!(bindings::pmm_arena_info, base)
);
zr::static_assert!(
    core::mem::offset_of!(PmmArenaInfo, size)
        == core::mem::offset_of!(bindings::pmm_arena_info, size)
);

impl PmmArenaInfo {
    pub const DEFAULT: PmmArenaInfo =
        PmmArenaInfo { name: [0; 16], flags: 0, base: PAddr(0), size: 0 };
}

pub type PmmStateCount = [usize; VmPageState::COUNT];

pub const PMM_ARENA_FLAG_LO_MEM: u32 = bindings::PMM_ARENA_FLAG_LO_MEM;

/// A contiguous physical memory arena managed by the PMM.
#[repr(C)]
#[derive(Debug)]
pub struct PmmArena {
    info: PmmArenaInfo,
    page_array: *mut VmPage,
    search_hint: u64,
}

// Compile-time layout assertions against the C++ PmmArena type via bindgen.
zr::static_assert!(core::mem::size_of::<PmmArena>() == core::mem::size_of::<bindings::PmmArena>());
zr::static_assert!(
    core::mem::align_of::<PmmArena>() == core::mem::align_of::<bindings::PmmArena>()
);
zr::static_assert!(core::mem::offset_of!(PmmArena, info) == 0);
zr::static_assert!(core::mem::offset_of!(PmmArena, page_array) == 40);
zr::static_assert!(core::mem::offset_of!(PmmArena, search_hint) == 48);

/// Computes and returns the offset from |page_array| of the first element at or
/// after |offset| whose physical address alignment satisfies |alignment_log2|.
///
/// Note, the returned value may exceed the bounds of |page_array_|.
fn align(offset: u64, alignment_log2: u8, first_aligned_offset: u64) -> u64 {
    if offset < first_aligned_offset {
        return first_aligned_offset;
    }
    debug_assert!(alignment_log2 >= kernel_page::SHIFT as u8);
    // The "extra" alignment required above and beyond kPageSize alignment.
    let offset_alignment = (alignment_log2 - kernel_page::SHIFT as u8) as u64;
    let mask = (1u64 << offset_alignment) - 1;
    ((offset - first_aligned_offset + mask) & !mask) + first_aligned_offset
}

impl PmmArena {
    /// Creates an empty `PmmArena`.
    pub const fn new() -> Self {
        Self { info: PmmArenaInfo::DEFAULT, page_array: core::ptr::null_mut(), search_hint: 0 }
    }

    /// Initializes the arena and allocates memory for internal data structures.
    pub fn init(
        &mut self,
        selected_arena_base: u64,
        selected_arena_size: u64,
        selected_bookkeeping_base: u64,
        selected_bookkeeping_size: u64,
        node: &mut PmmNode,
    ) {
        debug_assert!(kernel_page::is_aligned(selected_arena_base as usize));
        debug_assert!(kernel_page::is_aligned(selected_arena_size as usize));
        debug_assert!(kernel_page::is_aligned(selected_bookkeeping_base as usize));
        debug_assert!(kernel_page::is_aligned(selected_bookkeeping_size as usize));

        let page_count = (selected_arena_size as usize) / kernel_page::SIZE;
        assert!(page_count > 0);
        debug_assert!(
            selected_bookkeeping_size as usize
                == kernel_page::round_up(page_count * core::mem::size_of::<VmPage>())
        );
        debug_assert!(selected_bookkeeping_size < selected_arena_size);

        dprintf!(
            INFO,
            "PMM: adding arena [{:#x}, {:#x})\n",
            selected_arena_base,
            selected_arena_base + selected_arena_size
        );
        // Intentionally similar to the logging in PmmNode::InitReservedRange().
        dprintf!(
            INFO,
            "PMM: reserved [{:#x}, {:#x}): bookkeeping\n",
            selected_bookkeeping_base,
            selected_bookkeeping_base + selected_bookkeeping_size
        );

        let mut name = [0; 16];
        name[0] = b'r';
        name[1] = b'a';
        name[2] = b'm';

        self.info = PmmArenaInfo {
            name,
            flags: 0,
            base: PAddr(selected_arena_base as usize),
            size: selected_arena_size as usize,
        };

        // get the kernel pointer and initialize the pages.
        let page_array_size = selected_bookkeeping_size as usize;
        self.page_array =
            physmap::paddr_to_physmap(PAddr(selected_bookkeeping_base as usize)).0 as *mut VmPage;

        ltracef!(
            "arena for base {:#x} size {:#x} page array at {:p} size {:#x}\n",
            self.base().0,
            self.size(),
            self.page_array,
            page_array_size
        );

        // Initialize all pages.
        // SAFETY: `self.page_array` points to mapped bookkeeping memory of
        // `page_count * sizeof(VmPage)`.
        unsafe {
            core::ptr::write_bytes(self.page_array, 0, page_count);
        }

        // We've just constructed `page_count` pages in the state vm_page_state::FREE.
        crate::vm::page::add_to_initial_count(
            VmPageState(page_bindings::vm_page_state::FREE),
            page_count as u64,
        );

        // compute the range of the array that backs the array itself.
        let array_start_index =
            ((selected_bookkeeping_base - selected_arena_base) as usize) / kernel_page::SIZE;
        let array_end_index = array_start_index + page_array_size / kernel_page::SIZE;

        ltracef!(
            "array_start_index {array_start_index}, array_end_index {array_end_index}, page_count \
             {page_count}\n"
        );
        debug_assert!(array_start_index < page_count && array_end_index <= page_count);

        // add all pages that aren't part of the page array to the free list
        // pages part of the free array go to the WIRED state.
        pin_init::stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let base = self.base().0;

        for (index, p) in self.as_slice_mut().iter_mut().enumerate() {
            p.paddr_priv = base + index * kernel_page::SIZE;
            if index >= array_start_index && index < array_end_index {
                // SAFETY: We have conceptual ownership of the freshly constructed arena page.
                unsafe { p.set_state(VmPageState(vm_page_state::WIRED)) };
            } else {
                // SAFETY: `list` is pinned on stack; obtaining mutable reference to the list is safe.
                unsafe { list.as_mut().get_unchecked_mut().push_back_raw(NonNull::from(p)) };
            }
        }

        // SAFETY: These pages are freshly constructed and therefore owned by us.
        unsafe {
            node.add_free_pages(list);
        }
    }

    /// Initializes the arena for testing with the given info and page array.
    ///
    /// # Safety
    ///
    /// `page_array` must point to a valid array of `VmPage` structs of at least
    /// `info.size / PAGE_SIZE` elements, or be null if size is 0.
    pub unsafe fn init_for_test(&mut self, info: &PmmArenaInfo, page_array: *mut VmPage) {
        self.info = info.clone();
        self.page_array = page_array;
    }

    /// Returns a reference to the arena's info structure.
    pub const fn info(&self) -> &PmmArenaInfo {
        &self.info
    }

    /// Returns the name of the arena as a string slice.
    pub fn name(&self) -> &str {
        // SAFETY: `self.info.name` is a fixed 16-byte buffer.
        let bytes = unsafe { core::slice::from_raw_parts(self.info.name.as_ptr(), 16) };
        let len = bytes.iter().position(|&b| b == 0).unwrap_or(16);
        core::str::from_utf8(&bytes[..len]).unwrap_or("<invalid utf8>")
    }

    /// Returns the base physical address of the arena.
    pub const fn base(&self) -> PAddr {
        self.info.base
    }

    /// Returns the size in bytes of the arena.
    pub const fn size(&self) -> usize {
        self.info.size
    }

    /// Returns the end physical address of the arena.
    pub const fn end(&self) -> PAddr {
        PAddr(self.info.base.0 + self.info.size)
    }

    /// Returns the arena flags.
    pub const fn flags(&self) -> u32 {
        self.info.flags
    }

    /// Counts the number of pages in every state. For each page in the arena,
    /// increments the corresponding vm_page_state::*-indexed entry of
    /// |state_count|. Does not zero out the entries first.
    pub fn count_states(&self, state_count: &mut PmmStateCount) {
        for page in self.as_slice() {
            let state = page.state();
            state_count[state.index()] += 1;
        }
    }

    /// Returns a pointer to the page at `index`.
    ///
    /// # Safety
    ///
    /// `index` must be within bounds (`index < size() / PAGE_SIZE`).
    pub unsafe fn get_page(&self, index: usize) -> NonNull<VmPage> {
        // SAFETY: The caller ensures index is within bounds of page_array.
        unsafe { NonNull::new_unchecked(self.page_array.add(index)) }
    }

    /// Returns the index of `page` in the arena's page array.
    ///
    /// # Safety
    ///
    /// `page` must belong to this arena's `page_array`.
    pub unsafe fn get_index(&self, page: *const VmPage) -> usize {
        // SAFETY: Caller guarantees `page` points into `page_array`.
        unsafe { page.offset_from(self.page_array) as usize }
    }

    /// Finds a free run of contiguous pages.
    pub fn find_free_contiguous(
        &mut self,
        count: usize,
        mut alignment_log2: u8,
    ) -> Option<NonNull<VmPage>> {
        debug_assert!(count > 0);

        if alignment_log2 < kernel_page::SHIFT as u8 {
            alignment_log2 = kernel_page::SHIFT as u8;
        }

        // Number of pages in this arena.
        let arena_count = (self.size() / kernel_page::SIZE) as u64;
        let base = self.base();
        let align_bytes = 1u64 << alignment_log2;
        let aligned_base = (base.0 as u64 + align_bytes - 1) & !(align_bytes - 1);
        // Offset of the first page that satisfies the required alignment.
        let first_aligned_offset = (aligned_base - (base.0 as u64)) / (kernel_page::SIZE as u64);
        // Start our search at the hint so that we can skip over regions previously
        // known to be in use.
        let initial = self.search_hint;
        debug_assert!(initial < arena_count, "initial {initial}");
        let mut candidate = align(initial, alignment_log2, first_aligned_offset);
        // Keep track of how many runs of pages we examine before finding a
        // sufficiently long contiguous run.
        let mut num_runs_examined: i64 = 0;
        // Indicates whether we have wrapped around back to the start of the arena.
        let mut wrapped = false;
        let mut result: Option<NonNull<VmPage>> = None;

        // Keep searching until we've wrapped and "lapped" our initial starting point.
        while !wrapped || candidate < initial {
            ltracef!(
                "num_runs_examined={num_runs_examined} candidate={candidate} count={count} \
                 alignment_log2={alignment_log2} arena_count={arena_count} initial={initial}\n"
            );
            num_runs_examined += 1;
            if candidate.checked_add(count as u64).is_none_or(|end| end > arena_count) {
                if wrapped {
                    break;
                }
                wrapped = true;
                candidate = first_aligned_offset;
            } else {
                // Is the candidate region free?  Walk the pages of the region back to
                // front, stopping at the first non-free page.
                match self.find_last_non_free(candidate, count) {
                    Err(_) => {
                        // Candidate region is free.  We're done.
                        self.search_hint = (candidate + count as u64) % arena_count;
                        // SAFETY: `candidate` is within `arena_count`.
                        result = Some(unsafe { self.get_page(candidate as usize) });
                        debug_assert!(
                            candidate < arena_count,
                            "candidate={candidate} arena_count={arena_count}"
                        );
                        break;
                    }
                    Ok(last_non_free) => {
                        // Candidate region is not completely free.  Skip over the "broken" run,
                        // maintaining alignment.
                        candidate = align(last_non_free + 1, alignment_log2, first_aligned_offset);
                    }
                }
            }
        }

        // If called with preemption enabled, then the counter may fail to observe the true max.
        COUNTER_MAX_RUNS_EXAMINED.max(num_runs_examined);
        result
    }

    /// Returns a pointer to a specific page by physical address, or `None` if not in arena.
    pub fn find_specific(&self, pa: PAddr) -> Option<NonNull<VmPage>> {
        if !self.address_in_arena(pa) {
            return None;
        }
        let index = (pa.0 - self.base().0) / kernel_page::SIZE;
        debug_assert!(index < self.size() / kernel_page::SIZE);
        // SAFETY: index is within bounds of page_array.
        Some(unsafe { self.get_page(index) })
    }

    /// Returns whether `page` belongs to this arena.
    ///
    /// # Safety
    ///
    /// Caller must ensure `page` is a valid pointer.
    pub unsafe fn page_belongs_to_arena(&self, page: *const VmPage) -> bool {
        // SAFETY: Caller guarantees `page` is a valid pointer.
        let pa = unsafe { (*page).paddr() };
        self.address_in_arena(pa)
    }

    /// Returns whether `address` falls within this arena.
    pub const fn address_in_arena(&self, address: PAddr) -> bool {
        let base = self.info.base.0;
        address.0 >= base && address.0 < base + self.info.size
    }

    /// Dumps arena status, page states, and free ranges to the kernel log.
    pub fn dump(
        &self,
        dump_pages: bool,
        dump_free_ranges: bool,
        counts_sum: Option<&mut PmmStateCount>,
    ) {
        let name = self.name();
        let base = self.base().0;
        let size = self.size();
        let flags = self.flags();
        let this_ptr = self as *const Self;
        let page_array_ptr = self.page_array;
        let search_hint = self.search_hint;

        kprintln!(
            "  arena {:p}: name '{:s}' base {:#x} size ({:#x}) flags {:#x}",
            this_ptr,
            name,
            base,
            size,
            flags
        );
        kprintln!("\tpage_array {:p} search_hint {:u}", page_array_ptr, search_hint);

        if dump_pages {
            for page in self.as_slice() {
                // SAFETY: It is safe to dump page state and union metadata while inspecting arena pages.
                unsafe { page.dump() };
            }
        }

        // count the number of pages in every state.
        let mut state_count: PmmStateCount = [0; VmPageState::COUNT];
        self.count_states(&mut state_count);

        if let Some(sum) = counts_sum {
            for (sum_item, state_item) in sum.iter_mut().zip(state_count.iter()) {
                *sum_item += *state_item;
            }
        }

        print_page_state_counts(&state_count);

        // dump the free pages.
        if dump_free_ranges {
            kprintln!("\tfree ranges:");
            let mut last: isize = -1;
            for (i, page) in self.as_slice().iter().enumerate() {
                if page.is_free() {
                    if last == -1 {
                        last = i as isize;
                    }
                } else {
                    if last != -1 {
                        let start = base + (last as usize) * kernel_page::SIZE;
                        let end = base + i * kernel_page::SIZE;
                        kprintln!("\t\t{:#x} - {:#x}", start, end);
                    }
                    last = -1;
                }
            }
            if last != -1 {
                let start = base + (last as usize) * kernel_page::SIZE;
                let end = base + size;
                kprintln!("\t\t{:#x} - {:#x}", start, end);
            }
        }
    }

    /// Walks the region defined by |offset| and |count| and returns the index of
    /// the last non-free page or ZX_ERR_NOT_FOUND if all pages are free.
    ///
    /// It is an error if the range specified by |offset| and |count| is not
    /// completely contained within the arena.
    ///
    /// A loaned page is considered non-free for purposes of contiguous memory
    /// allocation.
    pub fn find_last_non_free(&self, offset: u64, count: usize) -> Result<u64, Status> {
        let pages = self.as_slice();
        let mut i = offset + (count as u64) - 1;
        loop {
            let page = &pages[i as usize];
            if !page.is_free() {
                return Ok(i);
            }
            if i == offset {
                break;
            }
            i -= 1;
        }
        Err(Status::NOT_FOUND)
    }

    /// Returns a slice over all pages in the arena.
    pub fn as_slice(&self) -> &[VmPage] {
        // SAFETY: `page_array` points to `size() / PAGE_SIZE` valid VmPage elements.
        unsafe { core::slice::from_raw_parts(self.page_array, self.size() / kernel_page::SIZE) }
    }

    /// Returns a mutable slice over all pages in the arena.
    pub fn as_slice_mut(&mut self) -> &mut [VmPage] {
        // SAFETY: `page_array` points to `size() / PAGE_SIZE` valid VmPage elements.
        unsafe { core::slice::from_raw_parts_mut(self.page_array, self.size() / kernel_page::SIZE) }
    }
}

impl Default for PmmArena {
    fn default() -> Self {
        Self::new()
    }
}

/// Prints page state counts to the kernel log.
pub fn print_page_state_counts(state_count: &PmmStateCount) {
    kprintln!("\tpage states:");
    for (i, count) in state_count.iter().enumerate().take(VmPageState::COUNT) {
        let state = VmPageState(unsafe {
            core::mem::transmute::<u8, page_bindings::vm_page_state>(i as u8)
        });
        let name = page_state_to_string(state);
        let bytes = count * kernel_page::SIZE;
        kprintln!("\t\t{:<12s} {:<16u} ({:u} bytes)", name, count, bytes);
    }
}

// -------------------------------------------------------------------------------------------------
// C++ FFI Export Functions
// -------------------------------------------------------------------------------------------------

/// # Safety
///
/// The caller must ensure `arena` points to an allocated `PmmArena` and `node` is a valid
/// `PmmNode`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pmm_arena_init(
    arena: *mut PmmArena,
    selected_arena_base: u64,
    selected_arena_size: u64,
    selected_bookkeeping_base: u64,
    selected_bookkeeping_size: u64,
    node: *mut PmmNode,
) {
    // SAFETY: The caller ensures `arena` and `node` are valid pointers.
    let arena = unsafe { &mut *arena };
    let node = unsafe { &mut *node };
    arena.init(
        selected_arena_base,
        selected_arena_size,
        selected_bookkeeping_base,
        selected_bookkeeping_size,
        node,
    );
}

/// # Safety
///
/// The caller must ensure `arena` points to a valid `PmmArena` and `info` is a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pmm_arena_init_for_test(
    arena: *mut PmmArena,
    info: *const PmmArenaInfo,
    page_array: *mut page_bindings::vm_page_t,
) {
    // SAFETY: The caller ensures `arena` and `info` are valid pointers.
    let arena = unsafe { &mut *arena };
    let info = unsafe { &*info };
    unsafe {
        arena.init_for_test(info, page_array.cast());
    }
}

/// # Safety
///
/// The caller must ensure `arena` points to an initialized `PmmArena`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pmm_arena_find_specific(
    arena: *const PmmArena,
    pa: bindings::zx_paddr_t,
) -> *mut page_bindings::vm_page_t {
    // SAFETY: The caller ensures `arena` points to an initialized `PmmArena`.
    let arena = unsafe { &*arena };
    arena.find_specific(PAddr(pa)).map_or(core::ptr::null_mut(), |p| p.as_ptr().cast())
}

/// # Safety
///
/// The caller must ensure `arena` points to an initialized `PmmArena`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pmm_arena_find_free_contiguous(
    arena: *mut PmmArena,
    count: usize,
    alignment_log2: u8,
) -> *mut page_bindings::vm_page_t {
    // SAFETY: The caller ensures `arena` points to an initialized `PmmArena`.
    let arena = unsafe { &mut *arena };
    arena
        .find_free_contiguous(count, alignment_log2)
        .map_or(core::ptr::null_mut(), |p| p.as_ptr().cast())
}

/// # Safety
///
/// The caller must ensure `arena` and `state_count` (array of `VmPageState::COUNT` usize) are
/// valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pmm_arena_count_states(
    arena: *const PmmArena,
    state_count: *mut usize,
) {
    // SAFETY: The caller ensures `arena` and `state_count` are valid.
    let arena = unsafe { &*arena };
    let counts = unsafe { &mut *(state_count as *mut PmmStateCount) };
    arena.count_states(counts);
}

/// # Safety
///
/// The caller must ensure `arena` is valid. If non-null, `counts_sum` must point to
/// `PmmStateCount`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_pmm_arena_dump(
    arena: *const PmmArena,
    dump_pages: bool,
    dump_free_ranges: bool,
    counts_sum: *mut usize,
) {
    // SAFETY: The caller ensures `arena` is valid.
    let arena = unsafe { &*arena };
    let counts_sum_ref = if counts_sum.is_null() {
        None
    } else {
        Some(unsafe { &mut *(counts_sum as *mut PmmStateCount) })
    };
    arena.dump(dump_pages, dump_free_ranges, counts_sum_ref);
}

/// # Safety
///
/// The caller must ensure `state_count` points to an array of `VmPageState::COUNT` usize.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_print_page_state_counts(state_count: *const usize) {
    // SAFETY: The caller ensures `state_count` points to a valid PmmStateCount array.
    let counts = unsafe { &*(state_count as *const PmmStateCount) };
    print_page_state_counts(counts);
}

/// Unit tests for PmmArena.
#[cfg(ktest)]
#[unittest::suite(name = "pmm_arena_rust")]
mod pmm_arena_rust {
    use super::{PmmArena, PmmArenaInfo};
    use crate::kernel::types::PAddr;
    use crate::vm::page::VmPage;
    use crate::vm::page_state::VmPageState;
    use ::page as kernel_page;
    use core::ptr::NonNull;
    use page_bindings::vm_page_state;
    use unittest::{assert_true, unwrap_ok};
    use zx_status::Status;

    unsafe fn set_page_state_range(state: vm_page_state, start: NonNull<VmPage>, count: usize) {
        for i in 0..count {
            // SAFETY: Caller guarantees start points to an array with at least count elements.
            let page = unsafe { &*start.as_ptr().add(i) };
            // SAFETY: Caller guarantees ownership of the range.
            unsafe { page.set_state(VmPageState(state)) };
        }
    }

    /// Tests finding free contiguous pages in arena.
    #[test]
    fn find_free_contiguous() {
        const K_NUM_PAGES: usize = 8;
        let base = 0x1001000;
        let mut name = [0u8; 16];
        name[..10].copy_from_slice(b"test arena");
        let info = PmmArenaInfo {
            name,
            flags: 0,
            base: PAddr(base),
            size: K_NUM_PAGES * kernel_page::SIZE,
        };

        let mut page_array: [VmPage; K_NUM_PAGES] = core::array::from_fn(|_| VmPage::default());
        let mut arena = PmmArena::new();
        // SAFETY: page_array is valid for K_NUM_PAGES elements.
        unsafe { arena.init_for_test(&info, page_array.as_mut_ptr()) };

        // page_array is as follow (0 == free, 1 == allocated):
        //
        // [00000000]
        //
        // Ask for some sizes and alignments that can't possibly succeed.
        let k_page_shift = kernel_page::SHIFT as u8;
        assert_true!(arena.find_free_contiguous(K_NUM_PAGES + 1, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(K_NUM_PAGES + 2, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(K_NUM_PAGES + 3, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(K_NUM_PAGES + 4, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(1, 24).is_none()); // 16MB aligned
        assert_true!(arena.find_free_contiguous(1, 25).is_none()); // 32MB aligned
        assert_true!(arena.find_free_contiguous(1, 26).is_none()); // 64MB aligned
        assert_true!(arena.find_free_contiguous(1, 27).is_none()); // 128MB aligned

        // [00000000]
        //
        // Ask for 4 pages, aligned on a 2-page boundary. See that the first page is skipped.
        let result = arena.find_free_contiguous(4, k_page_shift + 1);
        assert_true!(result.map(|p| p.as_ptr()) == Some(core::ptr::addr_of_mut!(page_array[1])));
        let result = unwrap_ok!(result.ok_or(Status::NO_MEMORY));
        // SAFETY: result points to page_array[1] with 4 elements available.
        unsafe { set_page_state_range(vm_page_state::ALLOC, result, 4) };

        // [01111000]
        //
        // Ask for various sizes and see that they all fail.
        assert_true!(arena.find_free_contiguous(4, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(5, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(6, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(7, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(8, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(9, k_page_shift).is_none());

        // [01111000]
        //
        // Ask for 3 pages.
        let result = arena.find_free_contiguous(3, k_page_shift);
        let result = unwrap_ok!(result.ok_or(Status::NO_MEMORY));
        assert_true!(result.as_ptr() == core::ptr::addr_of_mut!(page_array[5]));
        // SAFETY: result points to page_array[5] with 3 elements available.
        unsafe { set_page_state_range(vm_page_state::ALLOC, result, 3) };

        // [01111111]
        //
        // Ask for various sizes and see that they all fail.
        assert_true!(arena.find_free_contiguous(2, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(3, k_page_shift).is_none());
        assert_true!(arena.find_free_contiguous(4, k_page_shift).is_none());

        // [01111111]
        //
        // Ask for the last remaining page.
        let result = arena.find_free_contiguous(1, k_page_shift);
        assert_true!(result.map(|p| p.as_ptr()) == Some(core::ptr::addr_of_mut!(page_array[0])));
        let result = unwrap_ok!(result.ok_or(Status::NO_MEMORY));
        // SAFETY: result points to page_array[0] with 1 element available.
        unsafe { set_page_state_range(vm_page_state::ALLOC, result, 1) };

        // [11111111]
        //
        // See there are none left.
        assert_true!(arena.find_free_contiguous(1, k_page_shift).is_none());
    }
}
