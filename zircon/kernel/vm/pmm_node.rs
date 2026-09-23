// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::counters::define_kcounter;
use crate::kernel::deadline::{Deadline, SlackMode, TimerSlack};
use crate::kernel::event::{AutounsignalEvent, Event};
use crate::kernel::thread::{AutoPreemptDisabler, THREAD_SIGNAL_SUSPEND};
use crate::kernel::types::PAddr;
use crate::vm::compression::VmCompression;
use crate::vm::evictor::Evictor;
use crate::vm::page::{VmPage, VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::page_queues::PageQueues;
use crate::vm::page_state::VmPageState;
use crate::vm::physmap::paddr_to_physmap;
use crate::vm::pmm_arena::{PmmArena, PmmArenaInfo, PmmStateCount, print_page_state_counts};
use crate::vm::pmm_checker::{CheckFailAction, PmmChecker};
use boot_options::BootOptions;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use debug::{dprintf, ltracef};
use fbl::{DoublyLinkedList, SinglyLinkedListNode};
use ksync::{KMutex, LockToken, RawMutex, guarded, kcell_init};
use pin_init::{PinInit, Wrapper, pin_data, pin_init};
use pmm_node_bindings as bindings;
use zx_status::Status;
use zx_types::{zx_duration_t, zx_instant_mono_t, zx_status_t};

const LOCAL_TRACE: u32 = 0;

// TODO(https://fxbug.dev/549318457): Ideally we would use an rng from the standard rand crate, but
// even the smallest rng source is 128 bits, and to maintain data layout compatibility with C++ it
// must be 64-bits. Once C++ no longer references the data structure directly this can be dropped,
// the C library rand_r no longer used and this definition removed.
const RAND_MAX: core::ffi::c_int = 0x7fff_ffff;

// TODO(https://fxbug.dev/521834554): Replicating the C++ version of this constant here until a
// Rust instrumentation is supported.
const ASAN_PMM_FREE_MAGIC: u8 = 0xfb;

define_kcounter!(PMM_ALLOC_FAILED, "vm.pmm.alloc.failed", Sum);
define_kcounter!(PMM_ALLOC_DELAYED, "vm.pmm.alloc.delayed", Sum);

pub type AllocFailureType = bindings::PmmNode_AllocFailure_Type;

/// This enum is used to specify whether a page, when freed, should have its reuse
/// (i.e. reallocation) delayed.  This feature exists to both improve the PMM checker's ability
/// to detect "bad DMAs" and to reduce the impact when they do occur.  When allocating a page,
/// reusing the most recently freed page often has performance benefits.  However, it can
/// amplify the impact of use-after-free bugs.  This feature enables part of the kernel to
/// express a preference on whether a page should be eligible for immediate reuse or not.  It's
/// a hint.
///
/// |Default| means no preference.  When specified, the page may or may not be immediately reused.
/// In some build/runtime configurations (e.g. kasan) delayed reuse is the default behavior.
///
/// |Yes| indicates that the PMM should delay the reuse of the page by placing it on the "cold" end
/// up the free list, thereby maximizing the amount of time before which it is reallocated.
pub use bindings::PmmOptDelayReuse;

// Flags for PMM allocation routines.
/// no restrictions on which arena to allocate from.
pub const ALLOC_FLAG_ANY: u32 = bindings::PMM_ALLOC_FLAG_ANY;
/// The caller is able to wait and retry this allocation and so pmm allocation functions are allowed
/// to return ZX_ERR_SHOULD_WAIT, as opposed to ZX_ERR_NO_MEMORY, to indicate that the caller should
/// wait and try again. This is intended for the PMM to tell callers who are able to wait that
/// memory is low. The caller should not infer anything about memory state if it is told to wait, as
/// the PMM may tell it to wait for any reason.
pub const ALLOC_FLAG_CAN_WAIT: u32 = bindings::PMM_ALLOC_FLAG_CAN_WAIT;

/// Tell this PmmNode that we've failed a user-visible allocation.  Calling this method will
/// (optionally) trigger an asynchronous OOM response. To improve diagnostics some information
/// about the source of the failure can be provided.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct AllocFailure {
    pub r#type: AllocFailureType,
    pub size: usize,
    pub free_count: u64,
}

impl Default for AllocFailure {
    fn default() -> Self {
        Self { r#type: AllocFailureType::None, size: 0, free_count: 0 }
    }
}

// Compile-time layout assertions against C++ AllocFailure
zr::static_assert!(
    core::mem::size_of::<AllocFailure>() == core::mem::size_of::<bindings::PmmNode_AllocFailure>()
);
zr::static_assert!(
    core::mem::align_of::<AllocFailure>()
        == core::mem::align_of::<bindings::PmmNode_AllocFailure>()
);
zr::static_assert!(
    core::mem::offset_of!(AllocFailure, r#type)
        == core::mem::offset_of!(bindings::PmmNode_AllocFailure, type_)
);
zr::static_assert!(
    core::mem::offset_of!(AllocFailure, size)
        == core::mem::offset_of!(bindings::PmmNode_AllocFailure, size)
);
zr::static_assert!(
    core::mem::offset_of!(AllocFailure, free_count)
        == core::mem::offset_of!(bindings::PmmNode_AllocFailure, free_count)
);

/// Controls the behavior of requests that have the PMM_ALLOC_FLAG_CAN_WAIT.
#[repr(u32)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ShouldWaitState {
    /// The PMM_ALLOC_FLAG_CAN_WAIT should never be followed and we will always attempt to perform
    /// the allocation, or fail with ZX_ERR_NO_MEMORY. This state is permanent and cannot be left.
    Never,
    /// Allocations do not need to be delayed, but the should_wait_free_pages_level should be
    /// monitored and once tripped should be delayed.
    OnceLevelTripped,
    /// State indicates that the level got tripped, and we should delay any allocations until the
    /// level is reset.
    UntilReset,
}

// Compile-time layout assertions against C++ ShouldWaitState
zr::static_assert!(
    core::mem::size_of::<ShouldWaitState>()
        == core::mem::size_of::<bindings::PmmNode_ShouldWaitState>()
);
zr::static_assert!(
    ShouldWaitState::Never as u32 == bindings::PmmNode_ShouldWaitState::Never as u32
);
zr::static_assert!(
    ShouldWaitState::OnceLevelTripped as u32
        == bindings::PmmNode_ShouldWaitState::OnceLevelTripped as u32
);
zr::static_assert!(
    ShouldWaitState::UntilReset as u32 == bindings::PmmNode_ShouldWaitState::UntilReset as u32
);

/// Waiter node for loaned page freeing synchronization.
#[derive(fbl::SinglyLinkedListContainable)]
#[pin_data]
#[repr(C)]
pub struct FreeLoanedPagesHolderWaiter {
    #[sll_node]
    pub node: fbl::SinglyLinkedListNode<FreeLoanedPagesHolderWaiter>,
    #[pin]
    pub event: Event,
}

// Compile-time layout assertions against C++ Waiter
zr::static_assert!(
    core::mem::size_of::<FreeLoanedPagesHolderWaiter>()
        == core::mem::size_of::<bindings::FreeLoanedPagesHolder_Waiter>()
);
zr::static_assert!(
    core::mem::align_of::<FreeLoanedPagesHolderWaiter>()
        == core::mem::align_of::<bindings::FreeLoanedPagesHolder_Waiter>()
);

/// Object for managing freeing of loaned pages via a temporary holding object. Can be instantiated
/// on the stack and then passed into different PmmNode methods, the object itself has no publicly
/// available methods.
/// This object is not thread safe, and multiple threads must not pass the same instance of this
/// object into PmmNode methods.
/// A given FreeLoanedPagesHolder, as described in |FinishFreeLoanedPages|, may only be used for a
/// single call to |FinishFreeLoanedPages|, after which it is 'dead', and may not be passed to any
/// other PmmNode methods.
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct FreeLoanedPagesHolder {
    /// A given FreeLoanedPagesHolder interval is only allowed to be used once to return pages to
    /// the PMM, this tracks whether this has happened or not.
    /// Only permitting a single instance of freeing simplifies any need to reason about a single
    /// FreeLoanedPagesHolder repeatedly having pages moved into it and free'd to the PMM
    /// concurrently with attempts to wait on it.
    /// Although the lock cannot be annotated, this member is guarded by the relevant
    /// PmmNode::loaned_list_lock_.
    pub used: bool,
    /// List of pages presently owned by this object. Every page in this list is defined to be in
    /// the ALLOC state with |owner| set to this object.
    /// Although the lock cannot be annotated, this member is guarded by the relevant
    /// PmmNode::loaned_list_lock_.
    #[pin]
    pub pages: VmPageDoublyLinkedList,
    /// Maintain a list of waiters to be notified once pages have been freed. The Waiter object
    /// itself is stack allocated in the WithLoanedPage method and registered into this list.
    /// Having this be a list of Events of single waiting thread, instead of a single Event
    /// with a list of waiting threads, allows the waiters to retain a reference to the FLPH
    /// object while waiting. This ensures that once FinishFreeLoanedPages performs the signal
    /// on the waiters, the FLPH object can be safely destroyed.
    pub waiters: fbl::SinglyLinkedList<*mut FreeLoanedPagesHolderWaiter>,
}

// Compile-time layout assertions against C++ FreeLoanedPagesHolder
zr::static_assert!(
    core::mem::size_of::<FreeLoanedPagesHolder>()
        == core::mem::size_of::<bindings::FreeLoanedPagesHolder>()
);
zr::static_assert!(
    core::mem::align_of::<FreeLoanedPagesHolder>()
        == core::mem::align_of::<bindings::FreeLoanedPagesHolder>()
);
zr::static_assert!(
    core::mem::offset_of!(FreeLoanedPagesHolder, used)
        == core::mem::offset_of!(bindings::FreeLoanedPagesHolder, used_)
);
zr::static_assert!(
    core::mem::offset_of!(FreeLoanedPagesHolder, pages)
        == core::mem::offset_of!(bindings::FreeLoanedPagesHolder, pages_)
);
zr::static_assert!(
    core::mem::offset_of!(FreeLoanedPagesHolder, waiters)
        == core::mem::offset_of!(bindings::FreeLoanedPagesHolder, waiters_)
);

impl FreeLoanedPagesHolder {
    /// Creates a new pinned initializer for `FreeLoanedPagesHolder`.
    pub fn init() -> impl pin_init::PinInit<Self, core::convert::Infallible> {
        pin_init::pin_init!(&_this in Self {
            used: false,
            pages <- fbl::DoublyLinkedList::new(),
            waiters: fbl::SinglyLinkedList::new(),
        })
    }
}

#[pin_init::pinned_drop]
impl PinnedDrop for FreeLoanedPagesHolder {
    fn drop(self: core::pin::Pin<&mut Self>) {
        assert!(self.pages.is_empty());
        assert!(self.waiters.is_empty());
    }
}

unsafe extern "C" {
    fn asan_poison_shadow(address: usize, size: usize, value: u8);
    fn asan_unpoison_shadow(address: usize, size: usize);
    fn rand_r(seed: *mut usize) -> core::ffi::c_int;
    fn cpp_global_prng_draw(buffer: *mut u8, len: usize);
}

fn asan_poison_page(page: &VmPage, value: u8) {
    if cfg!(sanitize = "address") {
        // SAFETY: FFI call to C++ asan shim.
        unsafe {
            asan_poison_shadow(paddr_to_physmap(page.paddr()).0, page::SIZE, value);
        }
    }
}

fn asan_unpoison_page(page: &VmPage) {
    if cfg!(sanitize = "address") {
        // SAFETY: FFI call to C++ asan shim.
        unsafe {
            asan_unpoison_shadow(paddr_to_physmap(page.paddr()).0, page::SIZE);
        }
    }
}

fn return_pages_to_free_list(
    target_list: &mut VmPageDoublyLinkedList,
    to_free: &mut VmPageDoublyLinkedList,
    delay_reuse: PmmOptDelayReuse,
) {
    if delay_reuse == PmmOptDelayReuse::Yes || cfg!(sanitize = "address") {
        target_list.splice(to_free);
    } else {
        target_list.cursor_front_mut().splice(to_free);
    }
}

/// Per-NUMA node collection of physical memory arenas and bookkeeping.
#[repr(C)]
#[guarded]
pub struct PmmNode {
    canary: fbl::Canary<{ fbl::magic(b"PNOD") }>,

    #[mutex]
    lock: KMutex<RawMutex>,

    #[guarded_by(lock)]
    arena_cumulative_size: u64,
    // This is both an atomic and guarded by lock as we would like modifications to require the
    // lock, as logic in the system relies on the free_count not changing whilst the lock is
    // held, but also be an atomic so it can be correctly read without the lock.
    #[guarded_by(lock)]
    free_count: AtomicU64,
    #[guarded_by(loaned_list_lock)]
    free_loaned_count: AtomicU64,
    #[guarded_by(loaned_list_lock)]
    loaned_count: AtomicU64,
    #[guarded_by(loaned_list_lock)]
    loan_cancelled_count: AtomicU64,

    /// Free pages where !loaned.
    #[guarded_by(lock)]
    #[pin]
    free_list: VmPageDoublyLinkedList,
    #[mutex]
    loaned_list_lock: KMutex<RawMutex>,
    /// Free pages where loaned && !loan_cancelled.
    #[guarded_by(loaned_list_lock)]
    #[pin]
    free_loaned_list: VmPageDoublyLinkedList,

    /// The pages comprising the memory temporarily used during phys hand-off,
    /// populated on Init(). It is the responsibility of EndHandoff() to free this
    /// list.
    #[pin]
    phys_handoff_temporary_list: UnsafeCell<VmPageDoublyLinkedList>,

    /// The pages comprising the page-aligned regions of memory that we expect to
    /// turn into VMOs to hand-off to userspace - as determined by
    /// PhysHandoff::IsPhysVmoType() - populated and marked as wired on Init().
    ///
    /// It is expected that this memory will be unwired and turned into VMOs by the
    /// end of the phys hand-off phase, and it is the responsibility of
    /// PmmNode::EndHandoff() to ensure afterward that this list is empty.
    #[guarded_by(lock)]
    #[pin]
    phys_handoff_vmo_list: VmPageDoublyLinkedList,

    /// The pages intended to be permanently reserved.
    #[guarded_by(lock)]
    #[pin]
    permanently_reserved_list: VmPageDoublyLinkedList,

    #[guarded_by(lock)]
    should_wait: ShouldWaitState,

    /// Below this number of free pages the PMM will transition into delaying allocations.
    #[guarded_by(lock)]
    should_wait_free_pages_level: u64,

    /// The event acts a gate keeper for waking up threads waiting for allocations one at time.
    /// The event gets signalled when there MAY be pages available.
    #[pin]
    may_allocate_evt: AutounsignalEvent,

    /// Indicates whether a PMM alloc call has ever failed with ZX_ERR_NO_MEMORY. Used to trigger
    /// an OOM response.  See |MemoryWatchdog::WorkerThread|.
    alloc_failed_no_mem: AtomicBool,

    /// A record of the first time an allocation failure is reported to aid in diagnostics.
    #[guarded_by(lock)]
    first_alloc_failure: AllocFailure,

    /// If mem_signal is not null, then once the available free memory falls outside of the
    /// defined lower and upper bound the signal is raised. This is a one-shot signal and is
    /// cleared after firing.
    #[guarded_by(lock)]
    mem_signal: *mut Event,
    #[guarded_by(lock)]
    mem_signal_lower_bound: u64,
    #[guarded_by(lock)]
    mem_signal_upper_bound: u64,

    #[pin]
    page_queues: UnsafeCell<PageQueues>,

    #[pin]
    evictor: UnsafeCell<Evictor>,

    #[mutex]
    compression_lock: KMutex<RawMutex>,
    /// The page_compression is a lazily initialized RefPtr to keep the PmmNode constructor
    /// simple, at the cost needing to hold a lock to read the RefPtr. To avoid unnecessarily
    /// contending on the main pmm lock, use a separate one.
    #[guarded_by(compression_lock)]
    page_compression: Option<fbl::RefPtr<VmCompression>>,

    /// Indicates whether pages should have a pattern filled into them when they are freed. This
    /// value can only transition from false->true, and never back to false again. Once this
    /// value is set, the fill size in checker may no longer be changed, and it becomes safe
    /// to call FillPattern even without the lock held.
    /// This is an atomic to allow for reading this outside of the lock, but modifications only
    /// happen with the lock held.
    #[guarded_by(lock, loaned_list_lock)]
    free_fill_enabled: AtomicBool,
    /// Indicates whether it is known that all pages in the free list have had a pattern filled
    /// into them. This value can only transition from false->true, and never back to false
    /// again. Once this value is set the action and armed state in checker may no longer be
    /// changed, and it becomes safe to call AssertPattern even without the lock held.
    #[guarded_by(lock, loaned_list_lock)]
    // TODO(https://fxbug.dev/562635905): guarded_by currently does not support multiple locks and
    // so fails to event generate the wrapping UnsafeCell around this method. For now manually put
    // the UnsafeCell in and require callers to manually manage the locking.
    all_free_pages_filled: UnsafeCell<bool>,
    #[pin]
    checker: UnsafeCell<PmmChecker>,

    /// The rng state for random waiting on allocations. This allows us to use rand_r, which
    /// requires no further thread synchronization, unlike rand().
    #[guarded_by(lock)]
    random_should_wait_seed: usize,

    #[guarded_by(lock)]
    used_arena_count: usize,
    #[guarded_by(lock)]
    arenas: [PmmArena; PmmNode::ARENA_COUNT],

    phantom: core::marker::PhantomData<core::marker::PhantomPinned>,
}

// Compile-time layout assertions against the C++ PmmNode type via bindgen.
zr::static_assert!(core::mem::size_of::<PmmNode>() == core::mem::size_of::<bindings::PmmNode>());
zr::static_assert!(core::mem::align_of::<PmmNode>() == core::mem::align_of::<bindings::PmmNode>());
zr::static_assert!(
    core::mem::offset_of!(PmmNode, canary) == core::mem::offset_of!(bindings::PmmNode, canary_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, lock) == core::mem::offset_of!(bindings::PmmNode, lock_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, arena_cumulative_size)
        == core::mem::offset_of!(bindings::PmmNode, arena_cumulative_size_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, free_count)
        == core::mem::offset_of!(bindings::PmmNode, free_count_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, free_loaned_count)
        == core::mem::offset_of!(bindings::PmmNode, free_loaned_count_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, loaned_count)
        == core::mem::offset_of!(bindings::PmmNode, loaned_count_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, loan_cancelled_count)
        == core::mem::offset_of!(bindings::PmmNode, loan_cancelled_count_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, free_list)
        == core::mem::offset_of!(bindings::PmmNode, free_list_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, loaned_list_lock)
        == core::mem::offset_of!(bindings::PmmNode, loaned_list_lock_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, free_loaned_list)
        == core::mem::offset_of!(bindings::PmmNode, free_loaned_list_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, phys_handoff_temporary_list)
        == core::mem::offset_of!(bindings::PmmNode, phys_handoff_temporary_list_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, phys_handoff_vmo_list)
        == core::mem::offset_of!(bindings::PmmNode, phys_handoff_vmo_list_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, permanently_reserved_list)
        == core::mem::offset_of!(bindings::PmmNode, permanently_reserved_list_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, should_wait)
        == core::mem::offset_of!(bindings::PmmNode, should_wait_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, should_wait_free_pages_level)
        == core::mem::offset_of!(bindings::PmmNode, should_wait_free_pages_level_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, may_allocate_evt)
        == core::mem::offset_of!(bindings::PmmNode, may_allocate_evt_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, alloc_failed_no_mem)
        == core::mem::offset_of!(bindings::PmmNode, alloc_failed_no_mem_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, first_alloc_failure)
        == core::mem::offset_of!(bindings::PmmNode, first_alloc_failure_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, mem_signal)
        == core::mem::offset_of!(bindings::PmmNode, mem_signal_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, mem_signal_lower_bound)
        == core::mem::offset_of!(bindings::PmmNode, mem_signal_lower_bound_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, mem_signal_upper_bound)
        == core::mem::offset_of!(bindings::PmmNode, mem_signal_upper_bound_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, page_queues)
        == core::mem::offset_of!(bindings::PmmNode, page_queues_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, evictor) == core::mem::offset_of!(bindings::PmmNode, evictor_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, compression_lock)
        == core::mem::offset_of!(bindings::PmmNode, compression_lock_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, page_compression)
        == core::mem::offset_of!(bindings::PmmNode, page_compression_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, free_fill_enabled)
        == core::mem::offset_of!(bindings::PmmNode, free_fill_enabled_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, all_free_pages_filled)
        == core::mem::offset_of!(bindings::PmmNode, all_free_pages_filled_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, checker) == core::mem::offset_of!(bindings::PmmNode, checker_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, random_should_wait_seed)
        == core::mem::offset_of!(bindings::PmmNode, random_should_wait_seed_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, used_arena_count)
        == core::mem::offset_of!(bindings::PmmNode, used_arena_count_)
);
zr::static_assert!(
    core::mem::offset_of!(PmmNode, arenas) == core::mem::offset_of!(bindings::PmmNode, arenas_)
);

unsafe impl Sync for PmmNode {}
unsafe impl Send for PmmNode {}

impl PmmNode {
    /// Arenas are allocated from the node itself to avoid any boot allocations. Walking linearly
    /// through them at run time should also be fairly efficient.
    const ARENA_COUNT: usize = bindings::PmmNode_kArenaCount;
    /// Bit constants to fold a vm_page_t pointer into a uint32 and back. The format from LSB
    /// to MSB is : zero-bits | arena-index | page-index |. Where arena-index is 4 bits wide and
    /// zero-bits is 3 bits wide. This limits the number of pages per arena to 2^25.
    const ARENA_BITS: i32 = bindings::PmmNode_kArenaBits;
    pub const INDEX_ZERO_BITS: i32 = bindings::PmmNode_kIndexZeroBits;
    const MAX_PAGES_PER_ARENA: usize = bindings::PmmNode_kMaxPagesPerArena;
    const ARENA_MASK: u32 = bindings::PmmNode_kArenaMask;

    /// Creates an in-place pinned initializer for `PmmNode`.
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        pin_init!(Self {
            canary: fbl::Canary::new(),
            lock <- KMutex::init(),
            arena_cumulative_size: 0.into(),
            free_count: AtomicU64::new(0).into(),
            free_loaned_count: AtomicU64::new(0).into(),
            loaned_count: AtomicU64::new(0).into(),
            loan_cancelled_count: AtomicU64::new(0).into(),
            free_list <- kcell_init(fbl::DoublyLinkedList::new()),
            loaned_list_lock <- KMutex::init(),
            free_loaned_list <- kcell_init(fbl::DoublyLinkedList::new()),
            phys_handoff_temporary_list <- UnsafeCell::pin_init(fbl::DoublyLinkedList::new()),
            phys_handoff_vmo_list <- kcell_init(fbl::DoublyLinkedList::new()),
            permanently_reserved_list <- kcell_init(fbl::DoublyLinkedList::new()),
            should_wait: ShouldWaitState::OnceLevelTripped.into(),
            should_wait_free_pages_level: 0.into(),
            may_allocate_evt <- AutounsignalEvent::init_signaled(),
            alloc_failed_no_mem: AtomicBool::new(false),
            first_alloc_failure: AllocFailure::default().into(),
            mem_signal: core::ptr::null_mut::<Event>().into(),
            mem_signal_lower_bound: 0.into(),
            mem_signal_upper_bound: 0.into(),
            page_queues <- UnsafeCell::pin_init(PageQueues::init()),
            evictor <- UnsafeCell::pin_init(Evictor::init()),
            compression_lock <- KMutex::init(),
            page_compression: None.into(),
            free_fill_enabled: AtomicBool::new(false),
            all_free_pages_filled: UnsafeCell::new(false),
            checker <- UnsafeCell::pin_init(PmmChecker::init()),
            random_should_wait_seed: 0.into(),
            used_arena_count: 0.into(),
            arenas: [const { PmmArena::new() }; PmmNode::ARENA_COUNT].into(),
            phantom: core::marker::PhantomData,
        })
    }

    /// Domain-specific conversion: returns raw pointer for `PmmNode`.
    pub fn as_raw(&self) -> *mut bindings::PmmNode {
        (self as *const Self).cast_mut().cast()
    }

    /// Return the slice of arenas from the built-in array that are known to be active. Used in
    /// loops that iterate across all arenas.
    fn active_arenas<'a>(&'a self, token: &'a LockToken<'_, PmmNodeLockClass>) -> &'a [PmmArena] {
        // SAFETY: The lock token proves that either the lock protecting these fields is held, or
        // the caller has determined it is safe.
        unsafe { &self.arenas.get(token)[..*self.used_arena_count.get(token)] }
    }

    /// Converts the number returned by page_to_index() back to a VmPagePtr pointer.
    /// It does not check for invalid indexes such as 0.
    ///
    /// Note: This method is faster than page_to_index, about the cost of some basic math
    ///       and bit manipulation.
    ///
    /// # Safety
    ///
    /// The `index` must be a valid PMM page index.
    pub unsafe fn index_to_page(&self, index: u32) -> VmPagePtr {
        let index = index >> Self::INDEX_ZERO_BITS;
        let arena_ix = (index & Self::ARENA_MASK) as usize;
        let page_ix = (index >> Self::ARENA_BITS) as usize;
        // SAFETY: Active arenas are initialized during early boot and arena metadata is immutable.
        unsafe {
            let token = LockToken::new();
            VmPagePtr::new(self.active_arenas(&token)[arena_ix].get_page(page_ix - 1))
        }
    }

    /// Returns compressed representation a page_t*, with the following characteristics:
    /// - zeros in the last INDEZ_ZERO_BITS bits, used by clients to store metadata.
    /// - The value 0 is never returned, it can be used as "no page" marker.
    ///
    /// Note: This method needs to traverse (up to) all the memory pools so it's cost is
    ///       low but not trivial.
    pub fn page_to_index(&self, page: VmPagePtr) -> u32 {
        let page_raw = page.as_raw();
        // SAFETY: Active arenas are initialized during early boot and arena metadata is immutable.
        let token = unsafe { LockToken::new() };
        for (arena_ix, a) in self.active_arenas(&token).iter().enumerate() {
            // SAFETY: `page` is a valid VmPagePtr so `page_raw` is a valid pointer.
            if unsafe { a.page_belongs_to_arena(page_raw) } {
                // SAFETY: `page_raw` belongs to this arena's `page_array`.
                let page_ix = (unsafe { a.get_index(page_raw) } + 1) as u32;
                return ((page_ix << Self::ARENA_BITS) | (arena_ix as u32))
                    << Self::INDEX_ZERO_BITS;
            }
        }
        0
    }

    /// Converts the number returned by page_to_index() back to a PAddr.
    /// It does not check for invalid indexes such as 0 or kIndexReserved0.
    ///
    /// Note: This method is faster than page_to_index().paddr() as the VmPagePtr itself does not
    /// have to be de-referenced, saving a memory load.
    ///
    /// # Safety
    ///
    /// The `index` must be a valid PMM page index.
    pub unsafe fn index_to_paddr(&self, index: u32) -> PAddr {
        let index = index >> Self::INDEX_ZERO_BITS;
        let arena_ix = (index & Self::ARENA_MASK) as usize;
        let page_ix = (index >> Self::ARENA_BITS) as usize;
        // SAFETY: Active arenas are initialized during early boot and arena metadata is immutable.
        unsafe {
            let token = LockToken::new();
            let base = self.active_arenas(&token)[arena_ix].base().0;
            PAddr(base + (page_ix - 1) * page::SIZE)
        }
    }

    /// Ends phys handoff by freeing temporary allocations.
    pub fn end_handoff(&self) {
        unsafe {
            self.free_list(
                Pin::new_unchecked(&mut *self.phys_handoff_temporary_list.get()),
                PmmOptDelayReuse::Default,
            );
            assert!((*self.phys_handoff_temporary_list.get()).is_empty());
        }
    }

    /// Converts physical address to VmPagePtr if it belongs to any active arena.
    pub fn paddr_to_page(&self, addr: PAddr) -> Option<VmPagePtr> {
        // SAFETY: Active arenas are initialized during early boot and arena metadata is immutable.
        let token = unsafe { LockToken::new() };
        for a in self.active_arenas(&token) {
            if a.address_in_arena(addr) {
                let index = (addr.0 - a.base().0) / page::SIZE;
                // SAFETY: index is within arena bounds.
                let page_ptr = unsafe { a.get_page(index) };
                return Some(VmPagePtr::new(page_ptr));
            }
        }
        None
    }

    /// Allocates a single physical page from this node.
    pub fn alloc_page(&self, alloc_flags: u32) -> Result<VmPagePtr, Status> {
        debug_assert!(crate::kernel::thread::current_memory_allocation_state_is_enabled());
        let page_ptr: NonNull<VmPage>;
        let free_list_had_fill_pattern;

        {
            let _preempt_disable = AutoPreemptDisabler::new();
            ksync::lock!(let mut guard = self.lock.lock());
            let token = guard.as_mut().token_mut();
            // SAFETY: lock is held.
            free_list_had_fill_pattern = unsafe { *self.all_free_pages_filled.get() };

            if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
                && self.should_delay_allocation_locked(token)
            {
                PMM_ALLOC_DELAYED.add(1);
                return Err(Status::SHOULD_WAIT);
            }

            // SAFETY: token proves lock is held.
            let free_list = unsafe { self.free_list.get_mut(token) };
            let Some(p) = free_list.pop_front() else {
                // Allocation failures from the regular free list are likely to become user-visible.
                self.report_alloc_failure_locked(
                    token,
                    AllocFailure { r#type: AllocFailureType::Pmm, size: 1, free_count: 0 },
                );
                return Err(Status::NO_MEMORY);
            };
            page_ptr = p;

            // SAFETY: page_ptr is a valid VmPage popped from free_list.
            unsafe {
                self.alloc_page_helper_locked(page_ptr);
            }
            self.decrement_free_count_locked(token, 1);
        }

        let vmp = VmPagePtr::new(page_ptr);
        if free_list_had_fill_pattern {
            // SAFETY: vmp is a valid VmPagePtr.
            unsafe {
                self.checker().assert_pattern(vmp);
            }
        }

        Ok(vmp)
    }

    /// Allocates `count` physical pages, adding them to the tail of `list`.
    pub fn alloc_pages(
        &self,
        count: usize,
        alloc_flags: u32,
        mut list: Pin<&mut VmPageDoublyLinkedList>,
    ) -> Result<(), Status> {
        ltracef!("count {}\n", count);
        debug_assert!(crate::kernel::thread::current_memory_allocation_state_is_enabled());
        if count == 0 {
            return Ok(());
        } else if count == 1 {
            let page = self.alloc_page(alloc_flags)?;
            // SAFETY: list is pinned.
            unsafe {
                list.as_mut().get_unchecked_mut().push_back_raw(page.as_non_null());
            }
            return Ok(());
        }

        let free_list_had_fill_pattern;
        // Holds the pages that we pull out of the PMMs free list. These pages may still need to
        // have their pattern checked (based on the bool above) before being appended to |list| and
        // returned to the caller.
        pin_init::stack_pin_init!(let alloc_list = DoublyLinkedList::<NonNull<VmPage>>::new());
        {
            let _preempt_disable = AutoPreemptDisabler::new();
            ksync::lock!(let mut guard = self.lock.lock());
            let token = guard.as_mut().token_mut();
            // SAFETY: lock is held.
            free_list_had_fill_pattern = unsafe { *self.all_free_pages_filled.get() };

            // SAFETY: token proves lock is held.
            let free_count = unsafe { self.free_count.get(token) }.load(Ordering::Relaxed);

            if count as u64 > free_count {
                // SAFETY: token proves lock is held.
                if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
                    && unsafe { *self.should_wait.get(token) } != ShouldWaitState::Never
                {
                    PMM_ALLOC_DELAYED.add(1);
                    return Err(Status::SHOULD_WAIT);
                }
                // Allocation failures from the regular free list are likely to become user-visible.
                self.report_alloc_failure_locked(
                    token,
                    AllocFailure { r#type: AllocFailureType::Pmm, size: count, free_count },
                );
                return Err(Status::NO_MEMORY);
            }

            self.decrement_free_count_locked(token, count as u64);

            if (alloc_flags & ALLOC_FLAG_CAN_WAIT) != 0
                && self.should_delay_allocation_locked(token)
            {
                self.increment_free_count_locked(token, count as u64);
                PMM_ALLOC_DELAYED.add(1);
                return Err(Status::SHOULD_WAIT);
            }

            // SAFETY: token proves lock is held.
            let free_list = unsafe { self.free_list.get_mut(token) };
            let mut cursor = free_list.cursor_front_mut();
            for _ in 0..count {
                let page = cursor.get().expect("free_count was checked");
                let page_ptr = NonNull::from(page);
                // SAFETY: page_ptr is a valid VmPage pointer.
                unsafe {
                    self.alloc_page_helper_locked(page_ptr);
                }
                cursor.move_next();
            }
            // SAFETY: alloc_list is pinned.
            let mut alloc_cursor =
                unsafe { alloc_list.as_mut().get_unchecked_mut().cursor_back_mut() };
            cursor.split_before(&mut alloc_cursor);
        }

        // Check the pages we are allocating before appending them into the user's allocation list.
        // Do this check before since we must not existing pages in the user's allocation list, as
        // they are completely arbitrary pages and there's no reason to expect a fill pattern in
        // them.
        if free_list_had_fill_pattern {
            for page in alloc_list.iter() {
                let vmp = VmPagePtr::new(NonNull::from(page));
                // SAFETY: page points to valid VmPage in list.
                unsafe {
                    self.checker().assert_pattern(vmp);
                }
            }
        }

        // Append the checked list onto the user provided list.
        // SAFETY: Splicing into list.
        unsafe {
            list.as_mut().get_unchecked_mut().splice(alloc_list.as_mut().get_unchecked_mut());
        }
        Ok(())
    }

    /// Allocates physical pages in the specific address range.
    pub fn alloc_range(
        &self,
        mut address: PAddr,
        count: usize,
        mut list: Pin<&mut VmPageDoublyLinkedList>,
    ) -> Result<(), Status> {
        ltracef!("address {:#x}, count {}\n", address.0, count);

        debug_assert!(crate::kernel::thread::current_memory_allocation_state_is_enabled());
        // On error scenarios we will free the list, so make sure the caller didn't leave anything
        // in there.
        debug_assert!(list.is_empty());
        if count == 0 {
            return Ok(());
        }

        address = PAddr(page::round_down(address.0));
        let mut allocated = 0usize;
        let free_list_had_fill_pattern;

        {
            let _preempt_disable = AutoPreemptDisabler::new();
            ksync::lock!(let mut guard = self.lock.lock());
            let token = guard.as_mut().token_mut();
            // SAFETY: lock is held.
            free_list_had_fill_pattern = unsafe { *self.all_free_pages_filled.get() };

            // SAFETY: token proves lock is held.
            let num_arenas = unsafe { *self.used_arena_count.get(token) };
            // walk through the arenas, looking to see if the physical page belongs to it.
            for arena_idx in 0..num_arenas {
                loop {
                    if allocated >= count {
                        break;
                    }
                    // SAFETY: token proves lock is held, arena_idx < num_arenas.
                    let in_arena =
                        unsafe { self.arenas.get(token) }[arena_idx].address_in_arena(address);
                    if !in_arena {
                        break;
                    }
                    // SAFETY: token proves lock is held, arena_idx < num_arenas.
                    let page_nonnull =
                        unsafe { self.arenas.get(token) }[arena_idx].find_specific(address);
                    let Some(page_nonnull) = page_nonnull else {
                        break;
                    };
                    let page_ptr = page_nonnull;

                    // As we hold lock_, we can assume that any page in the FREE state is owned by
                    // us, and protected by lock_, and so should is_free() be true we will be
                    // allowed to assume it is in the free list, remove it from said list, and
                    // allocate it.
                    // SAFETY: page_ptr is a valid VmPage in arena.
                    let page = unsafe { page_ptr.as_ref() };
                    if !page.is_free() {
                        break;
                    }

                    // We never allocate loaned pages for caller of AllocRange()
                    if page.is_loaned() {
                        break;
                    }

                    // SAFETY: Reading container linkage while holding PmmNode lock.
                    debug_assert!(unsafe { (*page.queue_node.get()).in_container() });

                    // SAFETY: page is in free_list and list is pinned.
                    unsafe {
                        self.free_list.get_mut(token).erase(page);
                        self.alloc_page_helper_locked(page_ptr);
                        list.as_mut().get_unchecked_mut().push_back_raw(page_ptr);
                    }
                    allocated += 1;
                    address = PAddr(address.0 + page::SIZE);
                    self.decrement_free_count_locked(token, 1);
                }
                if allocated == count {
                    break;
                }
            }
            if allocated < count {
                // We were not able to allocate the entire run, free these pages. As we allocated
                // these pages under this lock acquisition, the fill status is whatever it was
                // before, i.e. the status of whether free pages have all been filled. No need to
                // request delayed reuse as these pages were allocated just now, but never used.
                // SAFETY: Freeing partially allocated pages back to node.
                self.free_list_locked(
                    token,
                    list,
                    free_list_had_fill_pattern,
                    PmmOptDelayReuse::Default,
                );
                return Err(Status::NOT_FOUND);
            }
        }

        if free_list_had_fill_pattern {
            for page in list.iter() {
                let vmp = VmPagePtr::new(NonNull::from(page));
                // SAFETY: page points to valid VmPage in list.
                unsafe {
                    self.checker().assert_pattern(vmp);
                }
            }
        }

        Ok(())
    }

    /// Allocate a run of contiguous pages, aligned on log2 byte boundary (0-31).
    /// Return the base address of the run in the physical address pointer and
    /// append the allocate page structures to the tail of the passed in list.
    pub fn alloc_contiguous(
        &self,
        count: usize,
        alloc_flags: u32,
        mut alignment_log2: u8,
        mut list: Pin<&mut VmPageDoublyLinkedList>,
    ) -> Result<PAddr, Status> {
        debug_assert!(crate::kernel::thread::current_memory_allocation_state_is_enabled());
        ltracef!("count {}, align {}\n", count, alignment_log2);

        // Forbid zero size contiguous allocations because we are obligated to provide the physical
        // address on success, but have no sensible value to give.
        if count == 0 {
            return Err(Status::INVALID_ARGS);
        }
        if (alignment_log2 as usize) < page::SHIFT {
            alignment_log2 = page::SHIFT as u8;
        }
        debug_assert!((alloc_flags & ALLOC_FLAG_CAN_WAIT) == 0);

        let _preempt_disable = AutoPreemptDisabler::new();
        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();

        // SAFETY: token proves lock is held.
        let num_arenas = unsafe { *self.used_arena_count.get(token) };
        for arena_idx in 0..num_arenas {
            // find_free_contiguous will search the arena for FREE pages. As we hold lock, any pages
            // in the FREE state are assumed to be owned by us, and would only be modified if lock
            // were held.
            // SAFETY: token proves lock is held, arena_idx < num_arenas.
            let p_nonnull = unsafe { self.arenas.get_mut(token) }[arena_idx]
                .find_free_contiguous(count, alignment_log2);
            let Some(p_nonnull) = p_nonnull else {
                continue;
            };
            let p = p_nonnull.as_ptr();

            // SAFETY: p is a valid page pointer returned by find_free_contiguous.
            let pa = unsafe { (*p).paddr() };

            let mut curr_pa = pa;
            // remove the pages from the run out of the free list.
            for _ in 0..count {
                // SAFETY: token proves lock is held, arena_idx < num_arenas.
                let curr_nonnull =
                    unsafe { self.arenas.get(token) }[arena_idx].find_specific(curr_pa);
                let Some(curr_nonnull) = curr_nonnull else {
                    panic!("arena find_specific failed on page allocated from it");
                };
                let curr_p = curr_nonnull;
                // SAFETY: curr_p is valid VmPage in arena and was verified free.
                unsafe {
                    let page = curr_p.as_ref();
                    debug_assert!(page.is_free());
                    // Loaned pages are never returned by FindFreeContiguous() above.
                    debug_assert!(!page.is_loaned());
                    debug_assert!((*page.queue_node.get()).in_container());

                    // Atomically (that is, in a single lock acquisition) remove this page from both
                    // the free list and FREE state, ensuring it is owned by us.
                    self.free_list.get_mut(token).erase(page);
                    page.set_state(VmPageState(page_bindings::vm_page_state::ALLOC));
                    (*page.state_union.get()).alloc.owner = core::ptr::null_mut();

                    self.decrement_free_count_locked(token, 1);
                    asan_unpoison_page(page);
                    let vmp = VmPagePtr::new(curr_p);
                    self.checker().assert_pattern(vmp);

                    list.as_mut().get_unchecked_mut().push_back_raw(curr_p);
                }
                curr_pa = PAddr(curr_pa.0 + page::SIZE);
            }
            return Ok(pa);
        }

        // We could potentially move contents of non-pinned pages out of the way for critical
        // contiguous allocations, but for now...
        ltracef!("couldn't find run\n");
        Err(Status::NOT_FOUND)
    }

    /// Frees a single physical page back to this node.
    ///
    /// # Safety
    ///
    /// Caller guarantees that page is valid and they are the owner.
    pub unsafe fn free_page(&self, page: VmPagePtr, delay_reuse: PmmOptDelayReuse) {
        let _preempt_disable = AutoPreemptDisabler::new();
        debug_assert!(unsafe { !page.is_loaned() });
        let fill = self.is_free_fill_enabled_racy();
        let page_ptr = page.as_non_null();
        if fill {
            // SAFETY: page is valid.
            unsafe {
                self.checker().fill_pattern(page);
            }
        }

        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();

        // SAFETY: page_ptr is valid.
        unsafe {
            let page_ref = page_ptr.as_ref();
            // pages freed individually shouldn't be in a queue.
            debug_assert!(!(*page_ref.queue_node.get()).in_container());
            self.free_page_helper_locked(token, page_ptr, fill);
            self.increment_free_count_locked(token, 1);

            if delay_reuse == PmmOptDelayReuse::Yes || cfg!(sanitize = "address") {
                self.free_list.get_mut(token).push_back_raw(page_ptr);
            } else {
                self.free_list.get_mut(token).push_front_raw(page_ptr);
            }
        }
    }

    /// Frees every page on `list` back to this node.
    ///
    /// # Safety
    ///
    /// Caller guarantees that all pages in list are valid and they are the owner.
    pub unsafe fn free_list(
        &self,
        mut list: Pin<&mut VmPageDoublyLinkedList>,
        delay_reuse: PmmOptDelayReuse,
    ) {
        let _preempt_disable = AutoPreemptDisabler::new();
        let fill = self.is_free_fill_enabled_racy();
        if fill {
            for page in list.iter() {
                let vmp = VmPagePtr::new(NonNull::from(page));
                // SAFETY: page in list is valid.
                unsafe {
                    self.checker().fill_pattern(vmp);
                }
            }
        }

        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();

        self.free_list_locked(token, list.as_mut(), fill, delay_reuse);
    }

    /// Return count of unallocated physical pages in this node.
    pub fn count_free_pages(&self) -> u64 {
        let token = unsafe { LockToken::new() };
        // SAFETY: Reading free_count.
        unsafe { self.free_count.get(&token) }.load(Ordering::Relaxed)
    }

    /// Return count of unallocated loaned physical pages in this node.
    pub fn count_loaned_free_pages(&self) -> u64 {
        let token = unsafe { LockToken::new() };
        unsafe { self.free_loaned_count.get(&token) }.load(Ordering::Relaxed)
    }

    /// Return count of pages which are presently loaned with the loan cancelled.
    pub fn count_loan_cancelled_pages(&self) -> u64 {
        let token = unsafe { LockToken::new() };
        unsafe { self.loan_cancelled_count.get(&token) }.load(Ordering::Relaxed)
    }

    /// Return count of loaned pages that are not free.
    pub fn count_loaned_not_free_pages(&self) -> u64 {
        let _preempt_disable = AutoPreemptDisabler::new();
        ksync::lock!(let loaned_guard = self.loaned_list_lock.lock());
        let loaned_token = loaned_guard.token();
        ksync::lock!(let _free_guard = self.lock.lock());
        unsafe { self.loaned_count.get(loaned_token) }.load(Ordering::Relaxed)
            - unsafe { self.free_loaned_count.get(loaned_token) }.load(Ordering::Relaxed)
    }

    /// Return count of loaned pages in this node.
    pub fn count_loaned_pages(&self) -> u64 {
        let token = unsafe { LockToken::new() };
        unsafe { self.loaned_count.get(&token) }.load(Ordering::Relaxed)
    }

    /// Return amount of physical memory in this node, in bytes.
    pub fn count_total_bytes(&self) -> u64 {
        let token = unsafe { LockToken::new() };
        // SAFETY: Reading arena_cumulative_size.
        unsafe { *self.arena_cumulative_size.get(&token) }
    }

    /// Enable the free fill checker with the specified fill size and action, and begin filling
    /// freed pages (including freed loaned pages) going forward.  See |PmmChecker| for definition
    /// of fill size.
    ///
    /// Note, pages freed piror to calling this method will remain unfilled.  To fill them, call
    /// |FillFreePagesAndArm|.
    ///
    /// Returns true if the checker was enabled with the requested fill_size, or |false| otherwise.
    pub fn enable_free_page_filling(&self, fill_size: usize, action: CheckFailAction) -> bool {
        // Require both locks so we can manipulate free_fill_enabled.
        ksync::lock!(let _loaned_guard = self.loaned_list_lock.lock());
        ksync::lock!(let _free_guard = self.lock.lock());
        if self.free_fill_enabled.load(Ordering::SeqCst) {
            // Checker is already enabled.
            return false;
        }
        // SAFETY: Both loaned_list_lock and lock are held, providing synchronized exclusive access
        // to checker.
        let checker = unsafe { &mut *self.checker.get() };
        checker.set_fill_size(fill_size);
        checker.set_action(action);
        // As free_fill_enabled may be examined outside of the lock, ensure the manipulations to
        // checker complete first by performing a release. See is_free_fill_enabled_racy for where
        // the acquire is performed.
        self.free_fill_enabled.store(true, Ordering::Release);
        true
    }

    /// Fill all free pages (both non-loaned and loaned) with a pattern and arm the checker.  See
    /// |PmmChecker|.
    ///
    /// This is a no-op if the checker is not enabled.  See |EnableFreePageFilling|
    pub fn fill_free_pages_and_arm(&self) {
        // Require both locks so we can process both of the free lists and modify
        // all_free_pages_filled.
        ksync::lock!(let mut loaned_guard = self.loaned_list_lock.lock());
        let loaned_token = loaned_guard.as_mut().token_mut();
        ksync::lock!(let mut free_guard = self.lock.lock());
        let free_token = free_guard.as_mut().token_mut();

        if !self.free_fill_enabled.load(Ordering::SeqCst) {
            return;
        }

        // SAFETY: free_token proves lock is held.
        let free_list = unsafe { self.free_list.get_mut(free_token) };
        for page in free_list.iter() {
            let vmp = VmPagePtr::new(NonNull::from(page));
            // SAFETY: page in free_list is valid.
            unsafe {
                self.checker().fill_pattern(vmp);
            }
        }
        // SAFETY: loaned_token proves lock is held.
        let free_loaned_list = unsafe { self.free_loaned_list.get_mut(loaned_token) };
        for page in free_loaned_list.iter() {
            let vmp = VmPagePtr::new(NonNull::from(page));
            // SAFETY: page in free_loaned_list is valid.
            unsafe {
                self.checker().fill_pattern(vmp);
            }
        }

        // SAFETY: Both loaned_list_lock and lock are held, providing synchronized exclusive access
        // to checker.
        let checker = unsafe { &mut *self.checker.get() };
        // Now that every page has been filled, we can arm the checker.
        checker.arm();
        // SAFETY: Both loaned_list_lock and lock are held.
        unsafe { *self.all_free_pages_filled.get() = true };
        checker.print_status_stdout();
    }

    /// Configures the free memory bounds and allows for setting a one shot signal as well as a
    /// level where allocations should start being delayed.
    ///
    /// The event is signaled once the number of PMM free pages falls outside of the range given by
    /// |free_lower_bound| and |free_upper_bound|. As the event is one shot, one signaled this must
    /// be called again to configure a new range. If the number of free pages is already outside the
    /// requested bound then this method fails (returns false) and no event is setup. In this case
    /// the caller should recalculate a correct bounds and try again.
    ///
    /// In addition to exiting the provided memory bounds, the event will also get signaled on the
    /// first time an allocation fails (i.e. the first time at which has_alloc_failed_no_mem would
    /// return true).
    ///
    /// |delay_allocations_level| is the number of PMM free pages below which the PMM will
    /// transition to delaying allocations that can wait, i.e. those with PMM_ALLOC_FLAG_CAN_WAIT.
    /// This transition is sticky, and even if pages are freed to go back above this line,
    /// allocations will remain delayed until this method is called again to re-set the level. For
    /// this reason, and since there is only a single common Event, the |delay_allocations_level|
    /// must either be <= the |free_lower_bound|, ensuring that the caller will have been notified
    /// and can respond by freeing memory and/or setting a new level, or |delay_allocations_level|
    /// can be UINT64_MAX, indicating allocations should start and remain delayed.
    ///
    /// # Safety
    ///
    /// Caller ensures that `event` lives either until this method is called again or the PmmNode is
    /// destroyed.
    pub unsafe fn set_free_memory_signal(
        &self,
        free_lower_bound: u64,
        free_upper_bound: u64,
        delay_allocations_pages: u64,
        event: *mut Event,
    ) -> bool {
        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();

        // Ensure delay allocations is valid.
        debug_assert!(
            delay_allocations_pages <= free_lower_bound || delay_allocations_pages == u64::MAX
        );
        let free_count = self.count_free_pages();
        if free_count < free_lower_bound || free_count > free_upper_bound {
            return false;
        }
        // SAFETY: token proves lock is held.
        unsafe {
            if delay_allocations_pages == u64::MAX {
                self.trip_free_pages_level_locked(token);
            } else if *self.should_wait.get(token) == ShouldWaitState::UntilReset {
                self.may_allocate_evt.signal();
                *self.should_wait.get_mut(token) = ShouldWaitState::OnceLevelTripped;
            }
            *self.should_wait_free_pages_level.get_mut(token) = delay_allocations_pages;
            *self.mem_signal_lower_bound.get_mut(token) = free_lower_bound;
            *self.mem_signal_upper_bound.get_mut(token) = free_upper_bound;
            *self.mem_signal.get_mut(token) = event;
        }
        true
    }

    /// Waits the system to exit low memory state and then attempts to allocate.
    ///
    /// To prevent herding problem, and because allocation compete for the `PmmNode::lock_` anyway,
    /// only one thread is woken up a time, only if the previous thread successfully allocated.
    ///
    /// In normal conditions,  when the system is in low memory state, this method will return
    /// `ZX_ERR_TIMED_OUT` if the system didn't transition fast enough. If we run into a TOC to TOU,
    /// for the system race `ZX_ERR_SHOULD_WAIT` will be returned, that is the system transitioned
    /// out and back into low memory state before we managed to perform the allocation.
    ///
    /// If `BootOptions::Get()->pmm_alloc_random_wait` is true, then the system
    /// may return spurious `ZX_ERR_SHOULD_WAIT`, in such cases, if the system is
    /// not in a low memory state, a thread is woken up anyway, so forward
    /// progress can be made.
    ///
    /// If |suspendable| is true, the wait will terminate early with
    /// `ZX_ERR_INTERNAL_INTR_RETRY` if the thread is suspended. If false, suspension is ignored and
    /// the wait continues.
    pub fn wait_for_single_page_allocation(
        &self,
        deadline: Deadline,
        suspendable: bool,
    ) -> Result<VmPagePtr, Status> {
        let mut skip_wait = false;
        {
            ksync::lock!(let guard = self.lock.lock());
            let token = guard.token();
            // If we have been instructed to never wait, skip waiting on the event entirely to
            // prevent blocking. The Never state is final.
            // SAFETY: token proves lock is held.
            if unsafe { *self.should_wait.get(token) } == ShouldWaitState::Never {
                skip_wait = true;
            }
        }

        if !skip_wait {
            // Ignore the suspend signal if not suspendable, and retry the wait if interrupted.
            let signal_mask = if suspendable { 0 } else { THREAD_SIGNAL_SUSPEND };
            let mut wait_result;
            loop {
                wait_result = self.may_allocate_evt.wait_mask(&deadline, signal_mask);
                if wait_result == Err(Status::INTERRUPTED_RETRY) && !suspendable {
                    continue;
                }
                break;
            }

            // Let the caller handle the error and retry if necessary.
            // This could be `ZX_ERR_TIMED_OUT`, `ZX_ERR_INTERNAL_INTR_KILLED` (thread killed)
            // or `ZX_ERR_INTERNAL_INTR_RETRY` (thread suspended, if suspendable is true).
            wait_result?;
        }

        // Try to allocate the page now, it may fail sporadically, since there is no guarantee that
        // by the time we attempt to allocate the pages are still available.
        let res = self.alloc_page(ALLOC_FLAG_CAN_WAIT);

        // Normally we would only signal in the `ZX_OK` case, i.e. when we are in an allocation-able
        // state. Otherwise we would wake up another thread just for it to receive
        // `ZX_ERR_SHOULD_WAIT` and immediately go back to waiting on the event. However, we also
        // signal in these cases:
        //
        // 1) `should_wait_ == Never`: We unconditionally signal to ensure all waiting threads are
        //    woken up (cascading signal) and no new threads block.
        //
        // 2) `pmm_alloc_random_should_wait`: In the random wait mode, we may block in a non low
        //    memory state, which will lead to threads getting blocked, and no one kicking them out.
        //    The unblocking chain is triggered by the system moving OUT of a low memory state,
        //    which would signal the event. To avoid checking the boot option explicitly, we always
        //    signal if the allocation returned `ZX_ERR_SHOULD_WAIT` and we are not in the
        //    `UntilReset` state, which leads to one extra spurious wake-up.
        let mut should_signal = false;
        {
            ksync::lock!(let guard = self.lock.lock());
            let token = guard.token();
            // SAFETY: token proves lock is held.
            unsafe {
                if *self.should_wait.get(token) == ShouldWaitState::Never {
                    should_signal = true;
                } else if *self.should_wait.get(token) != ShouldWaitState::UntilReset {
                    should_signal = res.is_ok() || res == Err(Status::SHOULD_WAIT);
                }
            }
        }

        if should_signal {
            self.may_allocate_evt.signal();
        }

        res
    }

    /// Tells the node to stop returning SHOULD_WAIT.
    pub fn stop_returning_should_wait(&self) {
        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();
        // SAFETY: token proves lock is held.
        unsafe {
            *self.should_wait.get_mut(token) = ShouldWaitState::Never;
        }
        self.may_allocate_evt.signal();
    }

    /// Returns whether an allocation has failed with NO_MEMORY.
    pub fn has_alloc_failed_no_mem(&self) -> bool {
        self.alloc_failed_no_mem.load(Ordering::Relaxed)
    }

    /// Retrieves information from the first allocation failure.
    pub fn get_first_alloc_failure(&self) -> AllocFailure {
        ksync::lock!(let guard = self.lock.lock());
        let token = guard.token();
        // SAFETY: token proves lock is held.
        unsafe { *self.first_alloc_failure.get(token) }
    }

    /// This method should be called when the PMM fails to allocate in a user-visible way and will
    /// (optionally) trigger an asynchronous OOM response.
    pub fn report_alloc_failure(&self, failure: AllocFailure) {
        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();
        self.report_alloc_failure_locked(token, failure);
    }

    /// Frees all pages in the given list and places them in the loaned state available to be
    /// returned from AllocLoanedPage.
    ///
    /// |delay_reuse| controls whether the newly loaned pages are eligile for immediate or delayed
    /// reuse.
    ///
    /// # Safety
    ///
    /// Caller guarantees that all pages in list are valid and they are the owner.
    pub unsafe fn begin_loan(
        &self,
        list: Pin<&mut VmPageDoublyLinkedList>,
        delay_reuse: PmmOptDelayReuse,
    ) {
        let _preempt_disable = AutoPreemptDisabler::new();
        let fill = self.is_free_fill_enabled_racy();
        if fill {
            for page in list.iter() {
                let vmp = VmPagePtr::new(NonNull::from(page));
                // SAFETY: page in list is valid.
                unsafe {
                    self.checker().fill_pattern(vmp);
                }
            }
        }

        ksync::lock!(let mut guard = self.loaned_list_lock.lock());
        let token = guard.as_mut().token_mut();

        let mut loaned_count = 0u64;
        for page in list.iter() {
            debug_assert!(!page.is_loaned());
            debug_assert!(!page.is_free());
            page.set_is_loaned();
            loaned_count += 1;
            debug_assert!(!page.is_loan_cancelled());
        }

        self.increment_loaned_count_locked(token, loaned_count);
        // Callers of begin_loan() generally won't want the pages loaned to them; the intent is to
        // loan to the rest of the system, so go ahead and free also.  Some callers will basically
        // choose between pmm_begin_loan() and pmm_free().
        // SAFETY: Modifying list under lock.
        unsafe {
            self.free_loaned_list_locked(
                token,
                list.get_unchecked_mut(),
                fill,
                delay_reuse,
                |_| {},
            );
        }
    }

    /// Marks a page that had been previously provided to BeginLoan as cancelled. This page may be
    /// in the FREE_LOANED state, or presently in use.
    ///
    /// This call prevents the page from being reused for any new purpose until EndLoan(). For
    /// presently-FREE_LOANED pages, this removes the pages from free_loaned_list_. For
    /// presently-used pages, this specifies that the page will not be added to free_loaned_list_
    /// when later freed. Once this page is FREE_LOANED (to be ensured by the caller via
    /// PhysicalPageProvider reclaim of the pages), the loan can be ended with EndLoan().
    ///
    /// # Safety
    ///
    /// Caller guarantees that page is valid
    pub unsafe fn cancel_loan(&self, page: VmPagePtr) {
        let _preempt_disable = AutoPreemptDisabler::new();
        // Require both locks in order to iterate the arenas and manipulate the loaned list.
        ksync::lock!(let mut loaned_guard = self.loaned_list_lock.lock());
        let loaned_token = loaned_guard.as_mut().token_mut();
        ksync::lock!(let _arena_guard = self.lock.lock());

        let page_raw = page.as_raw();
        // SAFETY: page is a valid VmPagePtr.
        unsafe {
            let page_ref = &*page_raw;
            debug_assert!(page_ref.is_loaned());
            debug_assert!(!page_ref.is_free());
            // We can assert this because of PageSource's overlapping request handling.
            let was_cancelled = page_ref.is_loan_cancelled();
            debug_assert!(!was_cancelled);
            page_ref.set_is_loan_cancelled();
            self.increment_loan_cancelled_count_locked(loaned_token, 1);
            if page_ref.is_free_loaned() {
                // Currently in free_loaned_list.
                debug_assert!((*page_ref.queue_node.get()).in_container());
                // Remove from free_loaned_list to prevent any new use until after end_loan.
                self.free_loaned_list.get_mut(loaned_token).erase(page_ref);
                self.decrement_free_loaned_count_locked(loaned_token, 1);
            }
        }
    }

    /// Allocates the page to the caller as a regular non-loaned page. Must currently be:
    ///  * Loaned (via BeginLoan).
    ///  * Have had its loan cancelled (via CancelLoan).
    ///  * Be in the FREE_LOANED state.
    ///
    /// # Safety
    ///
    /// Caller guarantees that page is valid
    pub unsafe fn end_loan(&self, page: VmPagePtr) {
        let free_list_had_fill_pattern;
        let page_ptr = page.as_non_null();

        {
            let _preempt_disable = AutoPreemptDisabler::new();
            // Require both locks in order to manipulate loaned pages and the regular free list.
            ksync::lock!(let mut loaned_guard = self.loaned_list_lock.lock());
            let loaned_token = loaned_guard.as_mut().token_mut();
            ksync::lock!(let _free_guard = self.lock.lock());

            // SAFETY: loaned_list_lock and lock are held.
            free_list_had_fill_pattern = unsafe { *self.all_free_pages_filled.get() };

            // SAFETY: page is a valid VmPagePtr.
            unsafe {
                let page_ref = page_ptr.as_ref();
                // PageSource serializing such that there's only one request to
                // PageProvider in flight at a time for any given page is the main
                // reason we can assert these instead of needing to check these.
                debug_assert!(page_ref.is_loaned());
                debug_assert!(page_ref.is_loan_cancelled());
                debug_assert!(page_ref.is_free_loaned());
                // Already not in free_loaned_list_ (because loan_cancelled
                // already).
                debug_assert!(!(*page_ref.queue_node.get()).in_container());

                page_ref.clear_is_loaned();
                page_ref.clear_is_loan_cancelled();
                // Change the state to regular FREE. When this page was made
                // FREE_LOANED all of the pmm checker filling and asan work was
                // done, so we are safe to just change the state without using a
                // helper.
                page_ref.set_state(VmPageState(page_bindings::vm_page_state::FREE));

                self.alloc_page_helper_locked(page_ptr);

                self.decrement_loan_cancelled_count_locked(loaned_token, 1);
                self.decrement_loaned_count_locked(loaned_token, 1);
            }
        }

        if free_list_had_fill_pattern {
            // SAFETY: page is a valid VmPagePtr.
            unsafe {
                self.checker().assert_pattern(page);
            }
        }
    }

    /// Allocates a single page from the loaned pages list. The allocated page will always have
    /// is_loaned() being true, and must be returned by either FreeLoanedPage or FreeLoanedList. If
    /// there are not loaned pages available ZX_ERR_UNAVAILABLE is returned, as an absence of loaned
    /// pages does not constitute an out of memory scenario.
    /// The provided callback must transition the page into a state such that it has a valid
    /// backlink, i.e. it is in the OBJECT state with an owner set, prior to returning.
    /// During the execution of the callback the page contents must *not* be modified.
    pub fn alloc_loaned_page<F: FnOnce(VmPagePtr)>(
        &self,
        allocated: F,
    ) -> Result<VmPagePtr, Status> {
        debug_assert!(crate::kernel::thread::current_memory_allocation_state_is_enabled());
        let _preempt_disable = AutoPreemptDisabler::new();
        let free_list_had_fill_pattern;
        let page_ptr: NonNull<VmPage>;

        {
            ksync::lock!(let mut guard = self.loaned_list_lock.lock());
            let token = guard.as_mut().token_mut();
            // SAFETY: loaned_list_lock is held.
            free_list_had_fill_pattern = unsafe { *self.all_free_pages_filled.get() };

            // SAFETY: token proves lock is held.
            let free_loaned_list = unsafe { self.free_loaned_list.get_mut(token) };
            let Some(p) = free_loaned_list.pop_front() else {
                // Does not count as out of memory, so do not report an allocation failure, just
                // tell the caller we are out of resources.
                return Err(Status::NO_RESOURCES);
            };
            page_ptr = p;

            // SAFETY: page_ptr is a valid VmPage in free_loaned_list.
            unsafe {
                self.alloc_loaned_page_helper_locked(page_ptr);
                self.decrement_free_loaned_count_locked(token, 1);
                let page_vm = VmPagePtr::new(page_ptr);
                // Run the callback while still holding the lock.
                allocated(page_vm);
                let page_ref = page_ptr.as_ref();
                // Before we drop the loaned list lock the page is expected to be in the object
                // state with a back pointer.
                debug_assert!(
                    page_ref.state() == VmPageState(page_bindings::vm_page_state::OBJECT)
                        && !page_ref.get_object().is_null()
                );
            }
        }

        let vmp = VmPagePtr::new(page_ptr);
        if free_list_had_fill_pattern {
            // SAFETY: vmp is a valid VmPagePtr.
            unsafe {
                self.checker().assert_pattern(vmp);
            }
        }

        Ok(vmp)
    }

    /// Begins freeing a loaned page that was previously allocated by AllocLoanPage by moving into a
    /// holding object. It is an error to attempt to free a non loaned page. When this method is
    /// called the |page| must have a valid backlink (i.e. be in the OBJECT state with an owner
    /// set). This backlink should be removed by the |release_page| callback, which is invoked
    /// under the loaned pages lock, prior to transition the page into the holding state. The caller
    /// *must*, at some point in the future, complete the page freeing process by passing the
    /// provided |flph| into a |FinishFreeLoanedPages| call.
    ///
    /// # Safety
    ///
    /// Caller guarantees that page is valid, loaned and owned by them.
    pub unsafe fn begin_free_loaned_page<F: FnOnce(VmPagePtr)>(
        &self,
        page: VmPagePtr,
        release_page: F,
        mut flph: Pin<&mut FreeLoanedPagesHolder>,
    ) {
        let _preempt_disable = AutoPreemptDisabler::new();
        debug_assert!(unsafe { page.is_loaned() });
        let page_ptr = page.as_non_null();
        // SAFETY: caller guarantees page is valid and owned by them.
        unsafe {
            let page_ref = page_ptr.as_ref();
            // On entry we require that the page has a valid backlink.
            debug_assert!(
                page_ref.state() == VmPageState(page_bindings::vm_page_state::OBJECT)
                    && !page_ref.get_object().is_null()
            );
        }

        ksync::lock!(let guard = self.loaned_list_lock.lock());
        let _token = guard.token();

        release_page(page);

        // SAFETY: caller guarantees valid page.
        unsafe {
            let p = page_ptr.as_ref();
            // pages freed individually shouldn't be in a queue.
            debug_assert!(!(*p.queue_node.get()).in_container());
            debug_assert!(!flph.used);
            p.set_state(VmPageState(page_bindings::vm_page_state::ALLOC));
            (*p.state_union.get()).alloc.owner =
                (flph.as_mut().get_unchecked_mut() as *mut FreeLoanedPagesHolder).cast();
            flph.as_mut().get_unchecked_mut().pages.push_front_raw(page_ptr);
        }
    }

    /// Completes the freeing of any loaned pages in |flph|, after which |flph| is allowed to be
    /// destructed. Once this method is called on a given |flph| that object is effectively 'dead'
    /// and is not allowed to be passed to any PmmNode methods.
    pub fn finish_free_loaned_pages(&self, mut flph: Pin<&mut FreeLoanedPagesHolder>) {
        if flph.pages.is_empty() {
            return;
        }

        let fill = self.is_free_fill_enabled_racy();
        if fill {
            for page in flph.pages.iter() {
                let vmp = VmPagePtr::new(NonNull::from(page));
                // SAFETY: valid page in flph.
                unsafe {
                    self.checker().fill_pattern(vmp);
                }
            }
        }

        let _preempt_disable = AutoPreemptDisabler::new();
        ksync::lock!(let mut guard = self.loaned_list_lock.lock());
        let token = guard.as_mut().token_mut();
        // SAFETY: Modifying flph under lock.
        let flph_mut = unsafe { flph.as_mut().get_unchecked_mut() };
        debug_assert!(!flph_mut.used);
        flph_mut.used = true;

        // Why default and not "yes"?  The primary reason to delay reuse is to mitigate "bad DMA"
        // involving previously pinned pages.  Because the pages we're about to free had been on
        // loan and because we do not pin loaned pages (see
        // |VmCowPages::ReplacePagesWithNonLoanedLocked|) we have no reason to delay their reuse.
        let delay_reuse = PmmOptDelayReuse::Default;

        let expected_owner = (flph_mut as *mut FreeLoanedPagesHolder).cast();
        self.free_loaned_list_locked(token, &mut flph_mut.pages, fill, delay_reuse, |page| {
            debug_assert!(page.state() == VmPageState(page_bindings::vm_page_state::ALLOC));
            // SAFETY: page is in ALLOC state with owner.
            unsafe {
                debug_assert!((*page.state_union.get()).alloc.owner == expected_owner);
                (*page.state_union.get()).alloc.owner = core::ptr::null_mut();
            }
        });

        // With the pager owners all cleared, no more waiters can come along so we can wake all the
        // existing ones up.
        while let Some(waiter_ptr) = flph_mut.waiters.pop_front() {
            // SAFETY: waiter_ptr is a valid FreeLoanedPagesHolderWaiter on waiter thread stack.
            unsafe {
                (*waiter_ptr).event.signal();
            }
        }
    }

    /// Add new pages to the free queue. Used when bootstrapping a PmmArena.
    ///
    /// # Safety
    ///
    /// Caller guarantees that all pages in |list| are valid and owned by them.
    pub unsafe fn add_free_pages(&mut self, mut list: Pin<&mut VmPageDoublyLinkedList>) {
        ltracef!("list {:p}\n", list.as_ref().get_ref() as *const _);

        // SAFETY: called at boot time as arenas are brought online, no locks are acquired
        let mut token = unsafe { LockToken::new() };

        let mut free_count = 0u64;
        // SAFETY: pop_front does not move data outside the list.
        while let Some(page) = unsafe { list.as_mut().get_unchecked_mut().pop_front() } {
            // SAFETY: `page` comes from valid VmPage array.
            unsafe {
                debug_assert!(!page.as_ref().is_loaned());
                debug_assert!(!page.as_ref().is_loan_cancelled());
                debug_assert!(page.as_ref().is_free());
                self.free_list.get_mut(&mut token).push_back_raw(page);
            }
            free_count += 1;
        }
        // SAFETY: called at boot time as arenas are brought online.
        unsafe { self.free_count.get(&token) }.fetch_add(free_count, Ordering::Relaxed);
        assert!(unsafe { self.free_count.get(&token) }.load(Ordering::Relaxed) != 0);
        self.may_allocate_evt.signal();

        ltracef!(
            "free count now {}\n",
            unsafe { self.free_count.get(&token) }.load(Ordering::Relaxed)
        );
    }

    /// Retrieve access to the page queues.
    pub fn page_queues(&self) -> &PageQueues {
        // SAFETY: page_queues is valid for the lifetime of PmmNode.
        unsafe { &*self.page_queues.get() }
    }

    fn free_list_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLockClass>,
        list: Pin<&mut VmPageDoublyLinkedList>,
        already_filled: bool,
        delay_reuse: PmmOptDelayReuse,
    ) {
        let mut count = 0u64;
        for page in list.iter() {
            let page_ptr = NonNull::from(page);
            debug_assert!(!page.is_loaned());
            // SAFETY: page_ptr is valid.
            unsafe {
                self.free_page_helper_locked(token, page_ptr, already_filled);
            }
            count += 1;
        }

        // SAFETY: token proves lock is held.
        unsafe {
            return_pages_to_free_list(
                self.free_list.get_mut(token),
                list.get_unchecked_mut(),
                delay_reuse,
            );
        }

        self.increment_free_count_locked(token, count);
    }

    /// Calls the provided function, passing |page| back into it, serialized with any other calls to
    /// |alloc_loaned_page|, |begin_free_loaned_page| and |finish_free_loaned_pages|. This allows
    /// caller to know that while the |with_page| callback is running there are no in progress calls
    /// to these methods, and that the page is not presently in holding object, i.e. it is either
    /// fully owned by the PmmNode, or fully owned by an object.
    pub fn with_loaned_page<F: FnOnce(VmPagePtr)>(&self, page: VmPagePtr, with_page: F) {
        // Technically users could race with |with_loaned_page| and re-allocate the page after it
        // gets migrated to the PmmNode, and then place it back in a new FLPH before a stable state
        // can be observed. Such behavior almost certainly represents a kernel bug, so if we detect
        // multiple iterations to track the page down we generate a warning.
        let mut iterations: u32 = 0;
        loop {
            // Intentionally allocate a new waiter every iteration so that its destructor can detect
            // if it has been left in a list incorrectly between iterations.
            pin_init::stack_pin_init!(let waiter = pin_init::pin_init!(FreeLoanedPagesHolderWaiter {
                node: SinglyLinkedListNode::new(),
                event <- Event::init_unsignaled(),
            }));
            {
                let _preempt_disable = AutoPreemptDisabler::new();
                ksync::lock!(let guard = self.loaned_list_lock.lock());
                unsafe {
                    debug_assert!(page.is_loaned());
                    if page.state() != VmPageState(page_bindings::vm_page_state::ALLOC)
                        || (*page.as_ref().state_union.get()).alloc.owner.is_null()
                    {
                        with_page(page);
                        return;
                    }
                    let flph_ptr = (*page.as_ref().state_union.get())
                        .alloc
                        .owner
                        .cast::<FreeLoanedPagesHolder>();
                    (*flph_ptr).waiters.push_front_raw(waiter.as_ref().get_ref()
                        as *const FreeLoanedPagesHolderWaiter
                        as *mut FreeLoanedPagesHolderWaiter);
                }
                // After placing waiter in the list and dropping the loaned_list_lock_ we must not
                // manipulate the intrusive list node in the Waiter, as it is now owned by the FLPH.
            }
            if iterations > 0 {
                kprint::kprintln!(
                    "WARNING: Required multiple attempts {iterations} to track down loaned page \
                     {:p}",
                    page.as_raw()
                );
            }

            // Now that the lock is dropped, wait on the event.
            let _ = waiter.event.wait_infinite();

            // Grab the loaned_list_lock to ensure that the finished_loaned_free_pages path has
            // finished holding any reference to our event.
            ksync::lock!(let guard = self.loaned_list_lock.lock());

            iterations = iterations.wrapping_add(1);
        }
    }

    /// Begins freeing multiple pages that were allocated by |alloc_loaned_page| by moving into a
    /// holding object. It is an error to attempt to free any non loaned pages. When this method is
    /// called all pages in the array must have a valid backlink (i.e. be in the OBJECT state with
    /// an owner set), and the |release_list| method must remove the backlink from all pages, and
    /// place them in the provided list in the same order.
    /// The caller *must*, at some point in the future, complete the page freeing process by passing
    /// the provided |flph| into a |finish_free_loaned_pages| call.
    ///
    /// # Safety
    ///
    /// Caller guarantees that all |pages| are valid, loaned and owned by them.
    pub unsafe fn begin_free_loaned_array<
        F: FnOnce(&[VmPagePtr], Pin<&mut VmPageDoublyLinkedList>),
    >(
        &self,
        pages: &[VmPagePtr],
        release_list: F,
        mut flph: Pin<&mut FreeLoanedPagesHolder>,
    ) {
        let _preempt_disable = AutoPreemptDisabler::new();
        // On entry we expect all pages to have a backlink.
        for p in pages {
            // SAFETY: valid pages slice.
            unsafe {
                let page_ref = &*p.as_raw();
                debug_assert!(
                    page_ref.state() == VmPageState(page_bindings::vm_page_state::OBJECT)
                        && !page_ref.get_object().is_null()
                );
            }
        }

        ksync::lock!(let guard = self.loaned_list_lock.lock());
        let _token = guard.token();
        debug_assert!(!flph.used);

        pin_init::stack_pin_init!(let free_list = DoublyLinkedList::<NonNull<VmPage>>::new());
        release_list(pages, free_list.as_mut());

        // Validate that the callback populated the free list correctly.
        let mut expected = 0usize;
        // SAFETY: free_list is pinned on stack and exclusively accessed.
        for p in unsafe { free_list.as_mut().get_unchecked_mut().iter() } {
            // SAFETY: p is in ALLOC state and we hold loaned_list_lock.
            unsafe {
                p.set_state(VmPageState(page_bindings::vm_page_state::ALLOC));
                (*p.state_union.get()).alloc.owner =
                    (flph.as_mut().get_unchecked_mut() as *mut FreeLoanedPagesHolder).cast();
            }
            debug_assert!(pages[expected].as_non_null() == NonNull::from(p));
            expected += 1;
        }
        debug_assert_eq!(expected, pages.len());

        // SAFETY: Splicing into flph.pages.
        unsafe {
            flph.as_mut()
                .get_unchecked_mut()
                .pages
                .cursor_front_mut()
                .splice(free_list.as_mut().get_unchecked_mut());
        }
    }

    fn free_loaned_list_locked<F: FnMut(&VmPage)>(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        list: &mut VmPageDoublyLinkedList,
        already_filled: bool,
        delay_reuse: PmmOptDelayReuse,
        mut validator: F,
    ) {
        let mut count = 0u64;
        let mut cursor = list.cursor_front_mut();
        while let Some(page) = cursor.get() {
            let page_ptr = NonNull::from(page);
            validator(page);
            // SAFETY: page_ptr is valid.
            unsafe {
                debug_assert!(page.is_loaned());
                self.free_loaned_page_helper_locked(token, page_ptr, already_filled);
                if page.is_loan_cancelled() {
                    // Loaned cancelled pages do not go back on the free list.
                    cursor.erase();
                } else {
                    count += 1;
                    cursor.move_next();
                }
            }
        }

        // SAFETY: token proves lock is held.
        unsafe {
            return_pages_to_free_list(self.free_loaned_list.get_mut(token), list, delay_reuse);
        }

        self.increment_free_loaned_count_locked(token, count);
    }

    /// Unwires a page that was previously in WIRED state.
    pub fn unwire_page(&self, page: VmPagePtr) {
        ksync::lock!(let mut guard = self.lock.lock());
        let token = guard.as_mut().token_mut();
        // SAFETY: page is a valid VmPagePtr.
        unsafe {
            let page_ref = &*page.as_raw();
            assert_eq!(page_ref.state(), VmPageState(page_bindings::vm_page_state::WIRED));
            if (*page_ref.queue_node.get()).in_container() {
                self.phys_handoff_vmo_list.get_mut(token).erase(page_ref);
            }
            page_ref.set_state(VmPageState(page_bindings::vm_page_state::ALLOC));
        }
    }

    /// Returns the number of active arenas.
    pub fn num_arenas(&self) -> usize {
        ksync::lock!(let guard = self.lock.lock());
        let token = guard.token();
        // SAFETY: token proves lock is held.
        unsafe { *self.used_arena_count.get(token) }
    }

    /// Fills |buffer| with PmmArenaInfo objects starting at |offset| arena, ordered by base
    /// address. For example, passing an |offset| of 1 would skip the 1st arena.
    ///
    /// Returns OUT_OF_RANGE if |offset| would yield an invalid.
    pub fn get_arena_info<'a>(
        &self,
        offset: usize,
        buffer: &'a mut [MaybeUninit<PmmArenaInfo>],
    ) -> Result<&'a mut [PmmArenaInfo], Status> {
        ksync::lock!(let guard = self.lock.lock());
        let token = guard.token();
        let active = self.active_arenas(token);
        let active_len = active.len();

        if buffer.is_empty() || (offset >= active_len) {
            return Err(Status::OUT_OF_RANGE);
        }

        let count = (active_len - offset).min(buffer.len());
        let buffer = &mut buffer[0..count];

        // Skip the first |offset| elements and copy the next elements.
        for off in 0..buffer.len() {
            unsafe {
                buffer[off].as_mut_ptr().write(active[off + offset].info().clone());
            }
        }
        // SAFETY: we have fully initialized the buffer.
        unsafe { Ok(buffer.assume_init_mut()) }
    }

    /// Prints free megabytes to stdout.
    pub fn dump_free(&self) {
        const MB: u64 = 1024 * 1024;
        let megabytes_free = self.count_free_pages() * (page::SIZE as u64) / MB;
        kprint::kprintln!(" {} free MBs", megabytes_free);
    }

    /// Dumps arena and page state diagnostics.
    pub fn dump(&self, is_panic: bool) {
        // No lock analysis here, as we want to just go for it in the panic case without the lock.
        let dump_inner = |token: &LockToken<'_, PmmNodeLockClass>| {
            // SAFETY: token proves lock is held or synthesized in panic.
            let free_count = unsafe { self.free_count.get(token) }.load(Ordering::Relaxed);
            let loaned_token = unsafe { LockToken::new() };
            let free_loaned_count =
                unsafe { self.free_loaned_count.get(&loaned_token) }.load(Ordering::Relaxed);
            // SAFETY: token proves lock is held or synthesized in panic.
            let total_size = unsafe { *self.arena_cumulative_size.get(token) };
            kprint::kprintln!(
                "pmm node {:p}: free_count {} ({} bytes), free_loaned_count: {} ({} bytes), total \
                 size {}\n",
                self as *const _,
                free_count,
                free_count * (page::SIZE as u64),
                free_loaned_count,
                free_loaned_count * (page::SIZE as u64),
                total_size
            );
            let mut count_sum = PmmStateCount::default();
            for a in self.active_arenas(token) {
                a.dump(false, false, Some(&mut count_sum));
            }
            kprint::kprintln!("Totals\n");
            print_page_state_counts(&count_sum);
        };

        if is_panic {
            // SAFETY: In panic context, synthesize lock token.
            let token = unsafe { LockToken::new() };
            dump_inner(&token);
        } else {
            ksync::lock!(let guard = self.lock.lock());
            let token = guard.token();
            dump_inner(token);
        }
    }

    /// Retrieve any page compression instance. If this returns non-null then it's return value will
    /// not change and the result can be cached.
    pub fn get_page_compression(&self) -> Option<&VmCompression> {
        ksync::lock!(let guard = self.compression_lock.lock());
        let token = guard.token();
        // SAFETY: token proves lock is held. Once `page_compression` is set to `Some`, it is never
        // modified or cleared for the lifetime of `self`.
        unsafe { self.page_compression.get(token).as_ref().map(|ptr| &*fbl::RefPtr::as_ptr(ptr)) }
    }

    /// Set the page compression instance. Returns an error if one has already been set.
    pub fn set_page_compression(
        &self,
        compression: fbl::RefPtr<VmCompression>,
    ) -> Result<(), Status> {
        ksync::lock!(let mut guard = self.compression_lock.lock());
        let token = guard.as_mut().token_mut();
        // SAFETY: token proves lock is held.
        unsafe {
            if self.page_compression.get(token).is_some() {
                return Err(Status::ALREADY_EXISTS);
            }
            *self.page_compression.get_mut(token) = Some(compression);
        }
        Ok(())
    }

    /// Returns a reference to the evictor.
    pub fn evictor(&self) -> &Evictor {
        // SAFETY: evictor is valid for the lifetime of PmmNode.
        unsafe { &*self.evictor.get() }
    }

    /// Returns a reference to the free fill checker.
    pub fn checker(&self) -> &PmmChecker {
        // SAFETY: checker is valid for the lifetime of PmmNode.
        unsafe { &*self.checker.get() }
    }

    /// Returns the global failed allocation count across all CPUs.
    pub fn get_alloc_failed_count() -> i64 {
        PMM_ALLOC_FAILED.sum_across_all_cpus()
    }

    /// If randomly waiting on allocations is enabled, this re-seeds from the global prng, otherwise
    /// it does nothing.
    pub fn seed_random_should_wait(&self) {
        if cfg!(debug_assertions) {
            ksync::lock!(let mut guard = self.lock.lock());
            let token = guard.as_mut().token_mut();
            // SAFETY: cpp_global_prng_draw writes size bytes into buffer.
            unsafe {
                cpp_global_prng_draw(
                    (self.random_should_wait_seed.get_mut(token) as *mut usize).cast(),
                    core::mem::size_of::<usize>(),
                );
            }
        }
    }

    /// Synchronously walk the PMM's free list (and free loaned list) and validate each page.  This
    /// is an incredibly expensive operation and should only be used for debugging purposes.
    pub fn check_all_free_pages(&self) {
        // Require both locks so we can process both of the free lists. This is an infrequent manual
        // operation and does not need to be optimized to avoid holding both locks at once.
        ksync::lock!(let mut loaned_guard = self.loaned_list_lock.lock());
        let loaned_token = loaned_guard.as_mut().token_mut();
        ksync::lock!(let mut free_guard = self.lock.lock());
        let free_token = free_guard.as_mut().token_mut();

        if !self.checker().is_armed() {
            return;
        }

        let mut free_page_count = 0u64;
        let mut free_loaned_page_count = 0u64;

        dprintf!(INFO, "PMM: checking free list...\n");
        // SAFETY: free_token proves lock is held.
        let free_list = unsafe { self.free_list.get_mut(free_token) };
        for page in free_list.iter() {
            let vmp = VmPagePtr::new(NonNull::from(page));
            // SAFETY: page in free_list is valid.
            unsafe {
                self.checker().assert_pattern(vmp);
            }
            free_page_count += 1;
        }
        dprintf!(INFO, "PMM: done checking free list\n");

        dprintf!(INFO, "PMM: checking free loaned list...\n");
        // SAFETY: loaned_token proves lock is held.
        let free_loaned_list = unsafe { self.free_loaned_list.get_mut(loaned_token) };
        for page in free_loaned_list.iter() {
            let vmp = VmPagePtr::new(NonNull::from(page));
            // SAFETY: page in free_loaned_list is valid.
            unsafe {
                self.checker().assert_pattern(vmp);
            }
            free_loaned_page_count += 1;
        }
        dprintf!(INFO, "PMM: done checking free loaned list\n");

        // SAFETY: free_token proves lock is held.
        assert_eq!(
            free_page_count,
            unsafe { self.free_count.get(free_token) }.load(Ordering::Relaxed)
        );
        assert_eq!(
            free_loaned_page_count,
            unsafe { self.free_loaned_count.get(loaned_token) }.load(Ordering::Relaxed)
        );
    }

    /// Synchronously walk the PMM's free list (and free loaned list) and poison each page.
    #[cfg(sanitize = "address")]
    pub fn poison_all_free_pages(&self) {
        // Require both locks so we can process both of the free lists. This is an infrequent manual
        // operation and does not need to be optimized to avoid holding both locks at once.
        ksync::lock!(let mut loaned_guard = self.loaned_list_lock.lock());
        let loaned_token = loaned_guard.as_mut().token_mut();
        ksync::lock!(let mut free_guard = self.lock.lock());
        let free_token = free_guard.as_mut().token_mut();

        // SAFETY: free_token proves lock is held.
        let free_list = unsafe { self.free_list.get_mut(free_token) };
        for page in free_list.iter() {
            asan_poison_page(page, ASAN_PMM_FREE_MAGIC);
        }
        // SAFETY: loaned_token proves lock is held.
        let free_loaned_list = unsafe { self.free_loaned_list.get_mut(loaned_token) };
        for page in free_loaned_list.iter() {
            asan_poison_page(page, ASAN_PMM_FREE_MAGIC);
        }
    }

    /// This method is racy as it allows us to read free_fill_enabled_ without holding the lock. If
    /// we receive a value of 'true', then as there is no mechanism to re-set it to false, we
    /// know it is still true. If we receive the value of 'false', then it could still become
    /// 'true' later. The intent of this method is to allow for filling the free pattern outside
    /// of the lock in most cases, and in the unlikely event of a race during the checker being
    /// armed, the pattern can resort to being filled inside the lock.
    fn is_free_fill_enabled_racy(&self) -> bool {
        // Read with acquire semantics to ensure that any modifications to checker_ are visible
        // before changes to free_fill_enabled_. See EnableFreePageFilling for where the
        // release is performed.
        self.free_fill_enabled.load(Ordering::Acquire)
    }

    fn should_delay_allocation_locked(&self, token: &mut LockToken<'_, PmmNodeLockClass>) -> bool {
        // SAFETY: token proves lock is held.
        unsafe {
            if *self.should_wait.get(token) == ShouldWaitState::UntilReset {
                return true;
            }
            if *self.should_wait.get(token) == ShouldWaitState::Never {
                return false;
            }
        }
        // See pmm_check_alloc_random_should_wait in pmm.rs for an assertion that random should wait
        // is only enabled if debug_assertions.
        if cfg!(debug_assertions) && BootOptions::get().pmm_alloc_random_should_wait {
            // SAFETY: token proves lock is held.
            let seed = unsafe { self.random_should_wait_seed.get_mut(token) };
            // SAFETY: rand_r takes a valid pointer.
            let val = unsafe { rand_r((seed as *mut usize).cast()) };
            if val < (RAND_MAX / 10) {
                return true;
            }
        }
        false
    }

    /// This method should be called when the PMM fails to allocate in a user-visible way and will
    /// (optionally) trigger an asynchronous OOM response.
    fn report_alloc_failure_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLockClass>,
        failure: AllocFailure,
    ) {
        PMM_ALLOC_FAILED.add(1);

        // Update before signaling the MemoryWatchdog to ensure it observes the update.
        //
        // |alloc_failed_no_mem| latches so only need to invoke the callback once.  We could call it
        // on every failure, but that's wasteful and we don't want to spam any underlying Event (or
        // the thread lock or the MemoryWatchdog).
        let first_time = !self.alloc_failed_no_mem.swap(true, Ordering::Relaxed);
        if first_time {
            let mut first = failure;
            // Record the free_count_ only for non-Pmm types. For PMM alloc failures, we know
            // exactly what the free count was at the time, because we use that to determine whether
            // the allocation should fail.
            if failure.r#type != AllocFailureType::Pmm {
                // SAFETY: token proves lock is held.
                first.free_count = unsafe { self.free_count.get(token) }.load(Ordering::Relaxed);
            }
            // SAFETY: token proves lock is held.
            unsafe {
                *self.first_alloc_failure.get_mut(token) = first;
            }
        }
        // SAFETY: token proves lock is held.
        if first_time && unsafe { !self.mem_signal.get(token).is_null() } {
            self.signal_free_memory_change_locked(token);
        }
    }

    fn signal_free_memory_change_locked(&self, token: &mut LockToken<'_, PmmNodeLockClass>) {
        // SAFETY: token proves lock is held.
        let event_ptr = unsafe { *self.mem_signal.get(token) };
        debug_assert!(!event_ptr.is_null());
        // SAFETY: event_ptr was set by caller and is non-null.
        unsafe {
            (*event_ptr).signal();
            *self.mem_signal.get_mut(token) = core::ptr::null_mut();
        }
    }

    fn trip_free_pages_level_locked(&self, token: &mut LockToken<'_, PmmNodeLockClass>) {
        // SAFETY: token proves lock is held.
        unsafe {
            if *self.should_wait.get(token) == ShouldWaitState::OnceLevelTripped {
                *self.should_wait.get_mut(token) = ShouldWaitState::UntilReset;
                let _ = self.may_allocate_evt.unsignal();
            }
        }
    }

    fn increment_free_count_locked(&self, token: &mut LockToken<'_, PmmNodeLockClass>, count: u64) {
        // SAFETY: token proves lock is held.
        let new_free_count = unsafe {
            self.free_count.get(token).fetch_add(count, Ordering::Relaxed);
            self.free_count.get(token).load(Ordering::Relaxed)
        };
        // SAFETY: token proves lock is held.
        unsafe {
            if !self.mem_signal.get(token).is_null()
                && new_free_count > *self.mem_signal_upper_bound.get(token)
            {
                self.signal_free_memory_change_locked(token);
            }
        }
    }

    fn decrement_free_count_locked(&self, token: &mut LockToken<'_, PmmNodeLockClass>, count: u64) {
        // SAFETY: token proves lock is held.
        let new_free_count = unsafe {
            debug_assert!(self.free_count.get(token).load(Ordering::Relaxed) >= count);
            self.free_count.get(token).fetch_sub(count, Ordering::Relaxed);
            self.free_count.get(token).load(Ordering::Relaxed)
        };
        // SAFETY: token proves lock is held.
        unsafe {
            if *self.should_wait.get(token) == ShouldWaitState::OnceLevelTripped
                && new_free_count < *self.should_wait_free_pages_level.get(token)
            {
                self.trip_free_pages_level_locked(token);
            }
            if !self.mem_signal.get(token).is_null()
                && new_free_count < *self.mem_signal_lower_bound.get(token)
            {
                self.signal_free_memory_change_locked(token);
            }
        }
    }

    fn increment_free_loaned_count_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        count: u64,
    ) {
        unsafe { self.free_loaned_count.get(token) }.fetch_add(count, Ordering::Relaxed);
    }

    fn decrement_free_loaned_count_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        count: u64,
    ) {
        debug_assert!(
            unsafe { self.free_loaned_count.get(token) }.load(Ordering::Relaxed) >= count
        );
        unsafe { self.free_loaned_count.get(token) }.fetch_sub(count, Ordering::Relaxed);
    }

    fn increment_loaned_count_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        count: u64,
    ) {
        unsafe { self.loaned_count.get(token) }.fetch_add(count, Ordering::Relaxed);
    }

    fn decrement_loaned_count_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        count: u64,
    ) {
        debug_assert!(unsafe { self.loaned_count.get(token) }.load(Ordering::Relaxed) >= count);
        unsafe { self.loaned_count.get(token) }.fetch_sub(count, Ordering::Relaxed);
    }

    fn increment_loan_cancelled_count_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        count: u64,
    ) {
        unsafe { self.loan_cancelled_count.get(token) }.fetch_add(count, Ordering::Relaxed);
    }

    fn decrement_loan_cancelled_count_locked(
        &self,
        token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        count: u64,
    ) {
        debug_assert!(
            unsafe { self.loan_cancelled_count.get(token) }.load(Ordering::Relaxed) >= count
        );
        unsafe { self.loan_cancelled_count.get(token) }.fetch_sub(count, Ordering::Relaxed);
    }

    unsafe fn alloc_page_helper_locked(&self, page: NonNull<VmPage>) {
        // SAFETY: page is a valid VmPage pointer.
        unsafe {
            let p = page.as_ref();
            ltracef!(
                "allocating page {:p}, pa {:#x}, prev state {:?}\n",
                page,
                p.paddr().0,
                p.state()
            );
            asan_unpoison_page(p);
            debug_assert!(p.is_free() && !p.is_loaned());
            // Here we transition the page from FREE->ALLOC, completing the transfer of ownership
            // from the PmmNode to the stack. This must be done under lock, and more specifically
            // the same lock acquisition that removes the page from the free list, as both being the
            // free list, or being in the ALLOC state, indicate ownership by the PmmNode.
            p.set_state(VmPageState(page_bindings::vm_page_state::ALLOC));
            // Used by the FLPH for loaned pages, but cleared here for consistency to ensure no
            // stale pointers that could be accidentally referenced.
            (*p.state_union.get()).alloc.owner = core::ptr::null_mut();
        }
    }

    unsafe fn alloc_loaned_page_helper_locked(&self, page: NonNull<VmPage>) {
        // SAFETY: page is a valid VmPage pointer.
        unsafe {
            let p = page.as_ref();
            ltracef!(
                "allocating loaned page {:p}, pa {:#x}, prev state {:?}\n",
                page,
                p.paddr().0,
                p.state()
            );
            asan_unpoison_page(p);
            debug_assert!(p.is_free_loaned() && p.is_loaned());
            // Here we transition the page from FREE_LOANED->ALLOC, completing the transfer of
            // ownership from the PmmNode to the stack. This must be done under loaned_pages_lock,
            // and more specifically the same loaned_pages_lock acquisition that removes the page
            // from the free list, as both being the free list, or being in the ALLOC state,
            // indicate ownership by the PmmNode.
            p.set_state(VmPageState(page_bindings::vm_page_state::ALLOC));
            (*p.state_union.get()).alloc.owner = core::ptr::null_mut();
        }
    }

    unsafe fn free_page_helper_locked(
        &self,
        _token: &mut LockToken<'_, PmmNodeLockClass>,
        page: NonNull<VmPage>,
        already_filled: bool,
    ) {
        // SAFETY: page is a valid VmPage pointer.
        unsafe {
            let p = page.as_ref();
            ltracef!("page {:p} state {:?} paddr {:#x}\n", page, p.state(), p.paddr().0);
            debug_assert!(!p.is_free());
            debug_assert!(!p.is_free_loaned());
            debug_assert!(
                p.state() != VmPageState(page_bindings::vm_page_state::OBJECT)
                    || (p.get_pin_count() == 0 && p.get_object().is_null())
            );
            // mark it free. This makes the page owned the PmmNode, even though it may not be in any
            // page
            // list, since the page is findable via the arena, and so we must ensure to:
            // 1. Be performing set_state here under the lock
            // 2. Place the page in the free list and cease referring to the page before ever
            //    dropping
            // lock
            p.set_state(VmPageState(page_bindings::vm_page_state::FREE));
            // This page cannot be loaned.
            debug_assert!(!p.is_loaned());
            // The caller may have called RacyFreeFillEnabled and potentially already filled a
            // pattern, however if it raced with enabling of free filling we may still need to fill
            // the pattern. This should be unlikely, and since free filling can never be turned back
            // off there is no race in the other direction.
            if self.free_fill_enabled.load(Ordering::SeqCst) && !already_filled {
                let vmp = VmPagePtr::new(page);
                self.checker().fill_pattern(vmp);
            }
            asan_poison_page(p, ASAN_PMM_FREE_MAGIC);
        }
    }

    unsafe fn free_loaned_page_helper_locked(
        &self,
        _token: &mut LockToken<'_, PmmNodeLoanedListLockClass>,
        page: NonNull<VmPage>,
        already_filled: bool,
    ) {
        // SAFETY: page is a valid VmPage pointer.
        unsafe {
            let p = page.as_ref();
            ltracef!("page {:p} state {:?} paddr {:#x}\n", page, p.state(), p.paddr().0);
            debug_assert!(!p.is_free());
            debug_assert!(
                p.state() != VmPageState(page_bindings::vm_page_state::OBJECT)
                    || p.get_pin_count() == 0
            );
            debug_assert!(
                p.state() != VmPageState(page_bindings::vm_page_state::ALLOC)
                    || (*p.state_union.get()).alloc.owner.is_null()
            );
            // mark it free. This makes the page owned the PmmNode and even though it may not be in
            // any page list, since the page is findable via the arena we must ensure the following
            // happens:
            // 1. We hold loaned_list_lock preventing pages from transition to/from loaned
            // 2. This page is loaned and hence will not be considered by an arena traversal that
            //    holds lock
            // 3. Perform set_state here under the loaned_list_lock
            // 4. Place the page in the loaned_free_list and cease referring to the page before ever
            //    dropping the loaned_list_lock.
            p.set_state(VmPageState(page_bindings::vm_page_state::FREE_LOANED));
            // The caller may have called IsFreeFillEnabledRacy and potentially already filled a
            // pattern, however if it raced with enabling of free filling we may still need to fill
            // the pattern. This should be unlikely, and since free filling can never be turned back
            //off there is no race in the other direction. As we hold lock we can safely perform a
            // relaxed read.
            if !already_filled && self.free_fill_enabled.load(Ordering::SeqCst) {
                let vmp = VmPagePtr::new(page);
                self.checker().fill_pattern(vmp);
            }
            asan_poison_page(p, ASAN_PMM_FREE_MAGIC);
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_end_handoff(node: *mut PmmNode) {
    // SAFETY: node is a valid PmmNode pointer.
    let node = unsafe { &*node };
    node.end_handoff();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_alloc_page(
    node: *mut PmmNode,
    alloc_flags: u32,
    out_page: *mut *mut page_bindings::vm_page_t,
) -> zx_status_t {
    // SAFETY: node and out_page are valid pointers.
    let node = unsafe { &*node };
    match node.alloc_page(alloc_flags) {
        Ok(page) => {
            unsafe { *out_page = page.as_ffi() };
            zx_types::ZX_OK
        }
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_alloc_pages(
    node: *mut PmmNode,
    count: usize,
    alloc_flags: u32,
    list: *mut VmPageDoublyLinkedList,
) -> zx_status_t {
    // SAFETY: node and list are valid and pinned.
    let node = unsafe { &*node };
    let list = unsafe { Pin::new_unchecked(&mut *list) };
    match node.alloc_pages(count, alloc_flags, list) {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_alloc_range(
    node: *mut PmmNode,
    address: u64,
    count: usize,
    list: *mut VmPageDoublyLinkedList,
) -> zx_status_t {
    // SAFETY: node and list are valid and pinned.
    let node = unsafe { &*node };
    let list = unsafe { Pin::new_unchecked(&mut *list) };
    match node.alloc_range(PAddr(address as usize), count, list) {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_alloc_contiguous(
    node: *mut PmmNode,
    count: usize,
    alloc_flags: u32,
    alignment_log2: u8,
    pa: *mut u64,
    list: *mut VmPageDoublyLinkedList,
) -> zx_status_t {
    // SAFETY: node, pa, and list are valid and pinned.
    let node = unsafe { &*node };
    let list = unsafe { Pin::new_unchecked(&mut *list) };
    match node.alloc_contiguous(count, alloc_flags, alignment_log2, list) {
        Ok(out_pa) => {
            unsafe { *pa = out_pa.0 as u64 };
            zx_types::ZX_OK
        }
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_free_page(
    node: *mut PmmNode,
    page: *mut page_bindings::vm_page_t,
    delay_reuse: PmmOptDelayReuse,
) {
    // SAFETY: node and page are valid.
    let node = unsafe { &*node };
    let page = unsafe { VmPagePtr::from_ffi(page) }.expect("null page");
    unsafe { node.free_page(page, delay_reuse) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_free_list(
    node: *mut PmmNode,
    list: *mut VmPageDoublyLinkedList,
    delay_reuse: PmmOptDelayReuse,
) {
    // SAFETY: node and list are valid and pinned.
    let node = unsafe { &*node };
    let list = unsafe { Pin::new_unchecked(&mut *list) };
    unsafe { node.free_list(list, delay_reuse) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_with_loaned_page(
    node: *mut PmmNode,
    page: *mut page_bindings::vm_page_t,
    with_page: unsafe extern "C" fn(*mut page_bindings::vm_page_t, *mut core::ffi::c_void),
    cookie: *mut core::ffi::c_void,
) {
    // SAFETY: node and page are valid.
    let node = unsafe { &*node };
    let page = unsafe { VmPagePtr::from_ffi(page) }.expect("null page");
    node.with_loaned_page(page, |p| {
        // SAFETY: FFI callback with cookie.
        unsafe { with_page(p.as_ffi(), cookie) };
    });
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_alloc_loaned_page(
    node: *mut PmmNode,
    allocated: unsafe extern "C" fn(*mut page_bindings::vm_page_t, *mut core::ffi::c_void),
    cookie: *mut core::ffi::c_void,
    out_page: *mut *mut page_bindings::vm_page_t,
) -> zx_status_t {
    // SAFETY: node and out_page are valid.
    let node = unsafe { &*node };
    let res = node.alloc_loaned_page(|p| {
        unsafe { allocated(p.as_ffi(), cookie) };
    });
    match res {
        Ok(page) => {
            unsafe { *out_page = page.as_ffi() };
            zx_types::ZX_OK
        }
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_begin_free_loaned_page(
    node: *mut PmmNode,
    page: *mut page_bindings::vm_page_t,
    release_page: unsafe extern "C" fn(*mut page_bindings::vm_page_t, *mut core::ffi::c_void),
    cookie: *mut core::ffi::c_void,
    flph: *mut FreeLoanedPagesHolder,
) {
    // SAFETY: node, page, and flph are valid.
    let node = unsafe { &*node };
    let page = unsafe { VmPagePtr::from_ffi(page) }.expect("null page");
    let flph = unsafe { Pin::new_unchecked(&mut *flph) };
    unsafe {
        node.begin_free_loaned_page(
            page,
            |p| {
                release_page(p.as_ffi(), cookie);
            },
            flph,
        );
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_finish_free_loaned_pages(
    node: *mut PmmNode,
    flph: *mut FreeLoanedPagesHolder,
) {
    // SAFETY: node and flph are valid.
    let node = unsafe { &*node };
    let flph = unsafe { Pin::new_unchecked(&mut *flph) };
    node.finish_free_loaned_pages(flph);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_begin_free_loaned_array(
    node: *mut PmmNode,
    pages: *mut *mut page_bindings::vm_page_t,
    count: usize,
    release_list: unsafe extern "C" fn(
        *mut *mut page_bindings::vm_page_t,
        usize,
        *mut VmPageDoublyLinkedList,
        *mut core::ffi::c_void,
    ),
    cookie: *mut core::ffi::c_void,
    flph: *mut FreeLoanedPagesHolder,
) {
    // SAFETY: node, pages, and flph are valid.
    let node = unsafe { &*node };
    let raw_slice = unsafe { core::slice::from_raw_parts(pages, count) };
    let ptr_slice: &[VmPagePtr] = unsafe { core::mem::transmute(raw_slice) };
    let flph = unsafe { Pin::new_unchecked(&mut *flph) };
    unsafe {
        node.begin_free_loaned_array(
            ptr_slice,
            |pages_slice, free_list| {
                release_list(
                    pages,
                    pages_slice.len(),
                    (free_list.get_unchecked_mut() as *mut VmPageDoublyLinkedList).cast(),
                    cookie,
                );
            },
            flph,
        );
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_unwire_page(
    node: *mut PmmNode,
    page: *mut page_bindings::vm_page_t,
) {
    // SAFETY: node and page are valid.
    let node = unsafe { &*node };
    let page = unsafe { VmPagePtr::from_ffi(page) }.expect("null page");
    node.unwire_page(page);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_begin_loan(
    node: *mut PmmNode,
    page_list: *mut VmPageDoublyLinkedList,
    delay_reuse: PmmOptDelayReuse,
) {
    // SAFETY: node and page_list are valid and pinned.
    let node = unsafe { &*node };
    let page_list = unsafe { Pin::new_unchecked(&mut *page_list) };
    unsafe { node.begin_loan(page_list, delay_reuse) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_cancel_loan(
    node: *mut PmmNode,
    page: *mut page_bindings::vm_page_t,
) {
    // SAFETY: node and page are valid.
    let node = unsafe { &*node };
    let page = unsafe { VmPagePtr::from_ffi(page) }.expect("null page");
    unsafe { node.cancel_loan(page) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_end_loan(
    node: *mut PmmNode,
    page: *mut page_bindings::vm_page_t,
) {
    // SAFETY: node and page are valid.
    let node = unsafe { &*node };
    let page = unsafe { VmPagePtr::from_ffi(page) }.expect("null page");
    unsafe { node.end_loan(page) };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_set_free_memory_signal(
    node: *mut PmmNode,
    free_lower_bound: u64,
    free_upper_bound: u64,
    delay_allocations_pages: u64,
    event: *mut Event,
) -> bool {
    // SAFETY: node is valid and caller guarantees event is valid.
    unsafe {
        let node = &*node;
        node.set_free_memory_signal(
            free_lower_bound,
            free_upper_bound,
            delay_allocations_pages,
            event,
        )
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_wait_for_single_page_allocation(
    node: *mut PmmNode,
    deadline: zx_instant_mono_t,
    slack_amount: zx_duration_t,
    slack_mode: SlackMode,
    suspendable: bool,
    out_page: *mut *mut page_bindings::vm_page_t,
) -> zx_status_t {
    // SAFETY: node and out_page are valid.
    let node = unsafe { &*node };
    let slack =
        TimerSlack::new(crate::platform_rs::timer::DurationUnknown(slack_amount), slack_mode);
    let deadline = Deadline::new(crate::platform_rs::timer::InstantUnknown(deadline), slack);
    match node.wait_for_single_page_allocation(deadline, suspendable) {
        Ok(page) => {
            unsafe { *out_page = page.as_ffi() };
            zx_types::ZX_OK
        }
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_stop_returning_should_wait(node: *mut PmmNode) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.stop_returning_should_wait();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_count_free_pages(node: *const PmmNode) -> u64 {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.count_free_pages()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_count_loaned_free_pages(node: *const PmmNode) -> u64 {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.count_loaned_free_pages()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_count_loan_cancelled_pages(node: *const PmmNode) -> u64 {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.count_loan_cancelled_pages()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_count_loaned_not_free_pages(node: *const PmmNode) -> u64 {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.count_loaned_not_free_pages()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_count_loaned_pages(node: *const PmmNode) -> u64 {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.count_loaned_pages()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_count_total_bytes(node: *const PmmNode) -> u64 {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.count_total_bytes()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_dump_free(node: *const PmmNode) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.dump_free();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_dump(node: *const PmmNode, is_panic: bool) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.dump(is_panic);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_get_arena_info(
    node: *const PmmNode,
    count: usize,
    i: u64,
    buffer: *mut PmmArenaInfo,
    buffer_size: usize,
) -> zx_status_t {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    let num_arenas = node.num_arenas();
    if count == 0 || (count + i as usize > num_arenas) || (i as usize >= num_arenas) {
        return zx_types::ZX_ERR_OUT_OF_RANGE;
    }
    if buffer_size < count * core::mem::size_of::<PmmArenaInfo>() {
        return zx_types::ZX_ERR_BUFFER_TOO_SMALL;
    }
    // SAFETY: Validated count is not 0 and buffer is at least large enough.
    let slice =
        unsafe { core::slice::from_raw_parts_mut(buffer as *mut MaybeUninit<PmmArenaInfo>, count) };
    // SAFETY: buffer is checked against buffer_size in get_arena_info_raw.
    match node.get_arena_info(i as usize, slice) {
        Ok(result) => {
            if result.len() < count {
                zx_types::ZX_ERR_BUFFER_TOO_SMALL
            } else {
                zx_types::ZX_OK
            }
        }
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_set_page_compression(
    node: *mut PmmNode,
    compression: *mut VmCompression,
) -> zx_status_t {
    // SAFETY: node is valid. compression was passed from RefPtr::release().
    let node = unsafe { &*node };
    let ref_ptr = unsafe { fbl::RefPtr::from_raw(compression) };
    match node.set_page_compression(ref_ptr) {
        Ok(()) => zx_types::ZX_OK,
        Err(status) => status.into_raw(),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_fill_free_pages_and_arm(node: *mut PmmNode) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.fill_free_pages_and_arm();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_check_all_free_pages(node: *mut PmmNode) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.check_all_free_pages();
}

#[unsafe(no_mangle)]
#[cfg(sanitize = "address")]
unsafe extern "C" fn rust_pmm_node_poison_all_free_pages(node: *mut PmmNode) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.poison_all_free_pages();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_enable_free_page_filling(
    node: *mut PmmNode,
    fill_size: usize,
    action: u8,
) -> bool {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    let action = match action {
        1 => CheckFailAction::Panic,
        _ => CheckFailAction::Oops,
    };
    node.enable_free_page_filling(fill_size, action)
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_get_alloc_failed_count() -> i64 {
    PmmNode::get_alloc_failed_count()
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_seed_random_should_wait(node: *mut PmmNode) {
    // SAFETY: node is valid.
    let node = unsafe { &*node };
    node.seed_random_should_wait();
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_report_alloc_failure(
    node: *mut PmmNode,
    failure: *const AllocFailure,
) {
    // SAFETY: node and failure are valid.
    let node = unsafe { &*node };
    let failure = unsafe { *failure };
    node.report_alloc_failure(failure);
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_get_first_alloc_failure(
    node: *const PmmNode,
    out_failure: *mut AllocFailure,
) {
    // SAFETY: node and out_failure are valid.
    let node = unsafe { &*node };
    let failure = node.get_first_alloc_failure();
    unsafe { *out_failure = failure };
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_pmm_node_add_free_pages(
    node: *mut PmmNode,
    list: *mut VmPageDoublyLinkedList,
) {
    // SAFETY: Caller guarantees these are not null and are pinned.
    unsafe {
        (*node).add_free_pages(Pin::new_unchecked(&mut *list));
    }
}

/// Unit tests for PmmNode.
#[cfg(ktest)]
#[unittest::suite(name = "pmm_node_rust")]
mod pmm_node_rust {
    use super::{
        ALLOC_FLAG_ANY, ALLOC_FLAG_CAN_WAIT, AllocFailure, AllocFailureType, FreeLoanedPagesHolder,
        PmmNode, PmmOptDelayReuse,
    };
    use crate::kernel::deadline::{Deadline, DurationMono, TimerSlack};
    use crate::kernel::thread;
    use crate::platform_rs::timer::InstantMono;
    use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
    use crate::vm::page_state::VmPageState;
    use crate::vm::page_state::bindings::vm_page_state;
    use crate::vm::physical_page_borrowing_config::ScopedLoaningEnabled;
    use crate::vm::physmap::paddr_to_physmap;
    use core::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use pin_init::{stack_pin_init, stack_try_pin_init};
    use unittest::{
        assert_gt, assert_ok, assert_true, expect_eq, expect_false, expect_ne, expect_ok,
        expect_true, unwrap_ok,
    };
    use zx_status::Status;

    /// Helper class for managing a PmmNode with real pages. alloc_range and alloc_contiguous are
    /// not supported by the managed PmmNode object. Only a single instance can exist at a time.
    #[pin_data(PinnedDrop)]
    pub struct ManagedPmmNode {
        #[pin]
        node: PmmNode,
        #[pin]
        event: Event,
        /// VMO that we will use to have a valid backlink for any loaned pages that get allocated.
        vmo: fbl::RefPtr<crate::vm::vm_object_paged::VmObjectPaged>,
        /// An optional scanner disable that is instantiated should any loaned pages get allocated.
        /// This is needed as our backlinks, while valid pointers, will confuse reclamation if it
        /// tries to reclaim using them.
        scanner_disable: core::cell::RefCell<Option<crate::vm::scanner::AutoVmScannerDisable>>,
    }

    impl ManagedPmmNode {
        pub const NUM_PAGES: usize = 64;
        pub const DEFAULT_MEM_EVENT_LOWER_BOUND: u64 = (Self::NUM_PAGES / 2) as u64;
        pub const DEFAULT_SHOULD_WAIT_LEVEL: u64 = (Self::NUM_PAGES / 4) as u64;

        pub const DEFAULT_LOW_MEM_ALLOC: usize =
            Self::NUM_PAGES - Self::DEFAULT_SHOULD_WAIT_LEVEL as usize + 1;
        pub const DEFAULT_MEM_EVENT_ALLOC: usize =
            Self::NUM_PAGES - Self::DEFAULT_MEM_EVENT_LOWER_BOUND as usize + 1;

        pub fn init() -> impl PinInit<Self, Status> {
            pin_init!(&_this in Self {
                node <- PmmNode::init(),
                event <- Event::init_unsignaled(),
                vmo: crate::vm::vm_object_paged::VmObjectPaged::create(0, 0, 0)?,
                scanner_disable: core::cell::RefCell::new(None),
            }? Status)
        }

        pub fn setup(self: Pin<&mut Self>) -> Result<(), Status> {
            pin_init::stack_pin_init!(let list = VmPageDoublyLinkedList::new());
            crate::vm::pmm::alloc_pages(Self::NUM_PAGES, 0, list.as_mut())?;
            for page in list.iter() {
                // TODO: Prevent this page state from allowing AllocContiguous() to potentially find
                // run of FREE pages involving some of these pages.
                // SAFETY: Setting page state for initialized test pages.
                unsafe {
                    page.set_state(VmPageState(vm_page_state::FREE));
                }
            }
            // SAFETY: Destructuring pinned ManagedPmmNode during setup.
            let this = unsafe { self.get_unchecked_mut() };
            // SAFETY: Pages were allocated and transitioned to FREE state above.
            unsafe { this.node.add_free_pages(list.as_mut()) };

            assert!(this.node.enable_free_page_filling(
                page::SIZE,
                crate::vm::pmm_checker::CheckFailAction::Panic
            ));
            this.node.fill_free_pages_and_arm();

            let result = this.reset_default_mem_event();
            assert!(result);

            Ok(())
        }

        pub fn is_event_signaled(&self) -> bool {
            self.event.wait(&crate::kernel::deadline::Deadline::infinite_past()).is_ok()
        }

        pub fn unsignal_event(&self) {
            let _ = self.event.unsignal();
        }

        pub fn reset_default_mem_event(&self) -> bool {
            self.set_free_memory_signal(
                Self::DEFAULT_MEM_EVENT_LOWER_BOUND,
                u64::MAX,
                Self::DEFAULT_SHOULD_WAIT_LEVEL,
            )
        }

        pub fn set_free_memory_signal(
            &self,
            lower_bound: u64,
            higher_bound: u64,
            delay_pages: u64,
        ) -> bool {
            // SAFETY: self.event is pinned and valid.
            unsafe {
                self.node.set_free_memory_signal(
                    lower_bound,
                    higher_bound,
                    delay_pages,
                    &self.event as *const Event as *mut Event,
                )
            }
        }

        pub fn node(&self) -> &PmmNode {
            &self.node
        }

        pub fn alloc_loaned_pages(
            &self,
            count: usize,
            pages: &mut [Option<VmPagePtr>],
        ) -> Result<(), Status> {
            let mut scanner_disable = self.scanner_disable.borrow_mut();
            if scanner_disable.is_none() {
                *scanner_disable = Some(crate::vm::scanner::AutoVmScannerDisable::new());
            }
            let cow = self.vmo.debug_get_cow_pages().ok_or(Status::INTERNAL)?;
            for i in 0..count {
                let result = self.node.alloc_loaned_page(|page| {
                    // SAFETY: Initializing loaned page backlink and state for test.
                    unsafe {
                        page.set_state(VmPageState(vm_page_state::OBJECT));
                        page.as_ref().set_object(core::ptr::null_mut());
                        page.as_ref().set_page_offset(0);
                        crate::vm::pmm::node().page_queues().set_reclaim(page, &cow, 0);
                    }
                });
                match result {
                    Ok(page) => {
                        pages[i] = Some(page);
                    }
                    Err(status) => {
                        for p in pages.iter().take(i).flatten() {
                            self.free_loaned_page(*p);
                        }
                        return Err(status);
                    }
                }
            }
            Ok(())
        }

        pub fn free_loaned_page(&self, page: VmPagePtr) {
            pin_init::stack_pin_init!(let flph = FreeLoanedPagesHolder::init());
            // SAFETY: page was allocated as loaned and flph is a valid pinned holder.
            unsafe {
                self.node.begin_free_loaned_page(
                    page,
                    |p| crate::vm::pmm::node().page_queues().remove(p),
                    flph.as_mut(),
                );
            }
            self.node.finish_free_loaned_pages(flph.as_mut());
        }
    }

    #[pin_init::pinned_drop]
    impl PinnedDrop for ManagedPmmNode {
        fn drop(self: core::pin::Pin<&mut Self>) {
            // SAFETY: Destructuring pinned ManagedPmmNode during drop.
            let this = unsafe { self.get_unchecked_mut() };
            pin_init::stack_pin_init!(let list = VmPageDoublyLinkedList::new());
            let status = this.node.alloc_pages(Self::NUM_PAGES, 0, list.as_mut());
            assert_eq!(status, Ok(()));
            for page in list.iter() {
                // SAFETY: Resetting page state to ALLOC so they can be freed to pmm.
                unsafe {
                    page.set_state(VmPageState(vm_page_state::ALLOC));
                }
            }
            // SAFETY: list contains valid allocated pages to return to pmm.
            unsafe { crate::vm::pmm::free_list(list) };
        }
    }

    /// Tests simple creation and destruction.
    #[test]
    fn smoke() {
        stack_pin_init!(let _pmm = PmmNode::init());
    }

    /// Allocates more than one page and frees them.
    #[test]
    fn node_multi_alloc() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());
        let alloc_count = ManagedPmmNode::NUM_PAGES / 2;
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());

        let status = node.node().alloc_pages(alloc_count, 0, list.as_mut());
        expect_ok!(status, "pmm_alloc_pages a few pages");
        expect_eq!(alloc_count, list.iter().count(), "pmm_alloc_pages a few pages list count");

        let status = node.node().alloc_pages(alloc_count, 0, list.as_mut());
        expect_ok!(status, "pmm_alloc_pages a few pages");
        expect_eq!(2 * alloc_count, list.iter().count(), "pmm_alloc_pages a few pages list count");

        // SAFETY: list contains pages allocated from node.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Allocates one page from the bulk allocation api.
    #[test]
    fn node_singleton_list() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());

        let status = node.node().alloc_pages(1, 0, list.as_mut());
        expect_ok!(status, "pmm_alloc_pages a few pages");
        expect_eq!(1, list.iter().count(), "pmm_alloc_pages a few pages list count");

        // SAFETY: list contains pages allocated from node.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Loans pages, borrows, cancels, reclaims, and ends the loan.
    #[test]
    fn node_loan_borrow_cancel_reclaim_end() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        let _cleanup = ScopedLoaningEnabled::new(true);

        stack_pin_init!(let list = VmPageDoublyLinkedList::new());

        const LOAN_COUNT: usize = ManagedPmmNode::NUM_PAGES * 3 / 4;
        const NOT_LOAN_COUNT: usize = ManagedPmmNode::NUM_PAGES - LOAN_COUNT;
        let mut paddr = [crate::kernel::types::PAddr(0); LOAN_COUNT];

        let status = node.node().alloc_pages(LOAN_COUNT, 0, list.as_mut());
        expect_ok!(status, "pmm_alloc_pages a few pages");
        expect_eq!(LOAN_COUNT, list.iter().count(), "pmm_alloc_pages correct # pages");

        for (i, page) in list.iter().enumerate() {
            paddr[i] = page.paddr();
        }

        for page in list.iter() {
            expect_false!(page.is_loaned());
            expect_false!(page.is_loan_cancelled());
        }
        // SAFETY: list contains valid allocated pages to loan.
        unsafe {
            node.node().begin_loan(list.as_mut(), PmmOptDelayReuse::Default);
        }
        for page in list.iter() {
            expect_true!(page.is_loaned());
            expect_false!(page.is_loan_cancelled());
        }

        expect_eq!(LOAN_COUNT as u64, node.node().count_loaned_pages());
        expect_eq!(NOT_LOAN_COUNT as u64, node.node().count_free_pages());
        expect_eq!(LOAN_COUNT as u64, node.node().count_loaned_free_pages());
        expect_eq!(0, node.node().count_loan_cancelled_pages());
        expect_eq!(0, node.node().count_loaned_not_free_pages());

        expect_eq!(0, list.iter().count());
        let mut loaned_pages = [None; LOAN_COUNT];
        let status = node.alloc_loaned_pages(LOAN_COUNT, &mut loaned_pages);
        expect_ok!(status, "pmm_alloc_pages PMM_ALLOC_FLAG_LOANED");

        for p in loaned_pages.iter() {
            let p = p.unwrap();
            let mut i = 0;
            while i < LOAN_COUNT {
                // SAFETY: p is a valid loaned page pointer.
                if paddr[i] == unsafe { p.paddr() } {
                    break;
                }
                i += 1;
            }
            expect_ne!(LOAN_COUNT, i);
        }

        for p in loaned_pages.iter() {
            let p = p.unwrap();
            // SAFETY: p is a valid loaned page pointer.
            expect_true!(unsafe { p.is_loaned() });
            // SAFETY: p is a valid loaned page pointer.
            expect_false!(unsafe { p.is_loan_cancelled() });
            // SAFETY: p is a valid loaned page pointer.
            unsafe {
                node.node().cancel_loan(p);
            }
            // SAFETY: p is a valid loaned page pointer.
            expect_true!(unsafe { p.is_loaned() });
            // SAFETY: p is a valid loaned page pointer.
            expect_true!(unsafe { p.is_loan_cancelled() });
        }

        expect_eq!(LOAN_COUNT as u64, node.node().count_loaned_pages());
        expect_eq!(NOT_LOAN_COUNT as u64, node.node().count_free_pages());
        expect_eq!(0, node.node().count_loaned_free_pages());
        expect_eq!(LOAN_COUNT as u64, node.node().count_loan_cancelled_pages());
        expect_eq!(LOAN_COUNT as u64, node.node().count_loaned_not_free_pages());

        for p in loaned_pages.iter() {
            node.free_loaned_page(p.unwrap());
        }

        expect_eq!(LOAN_COUNT as u64, node.node().count_loaned_pages());
        expect_eq!(NOT_LOAN_COUNT as u64, node.node().count_free_pages());
        expect_eq!(0, node.node().count_loaned_free_pages());
        expect_eq!(LOAN_COUNT as u64, node.node().count_loan_cancelled_pages());
        expect_eq!(LOAN_COUNT as u64, node.node().count_loaned_not_free_pages());

        expect_eq!(0, list.iter().count());
        let mut extra_loaned = [None; NOT_LOAN_COUNT + 1];
        let status = node.alloc_loaned_pages(NOT_LOAN_COUNT + 1, &mut extra_loaned);
        expect_true!(status == Err(Status::NO_RESOURCES), "try to allocate a loan_cancelled page");

        expect_eq!(0, list.iter().count());
        let status = node.node().alloc_pages(NOT_LOAN_COUNT, ALLOC_FLAG_ANY, list.as_mut());
        expect_ok!(status, "allocate all the not-loaned pages");

        for page in list.iter() {
            let paddr_page = page.paddr();
            expect_false!(page.is_loaned());
            let mut i = 0;
            while i < LOAN_COUNT {
                if paddr[i] == paddr_page {
                    break;
                }
                i += 1;
            }
            expect_eq!(LOAN_COUNT, i);
        }

        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }

        expect_eq!(0, list.iter().count());
        for j in 0..LOAN_COUNT {
            let page = loaned_pages[j].unwrap();
            // SAFETY: page is a valid loaned page pointer.
            expect_eq!(paddr[j].0, unsafe { page.paddr() }.0);
            // SAFETY: page is a valid loaned page pointer.
            unsafe {
                node.node().end_loan(page);
            }
            // SAFETY: page is a valid page pointer.
            expect_false!(unsafe { page.is_loaned() });
            // SAFETY: page is a valid page pointer.
            expect_false!(unsafe { page.is_loan_cancelled() });
            // SAFETY: list is pinned on stack, push_back_raw does not move list.
            unsafe { list.as_mut().get_unchecked_mut().push_back_raw(page.as_non_null()) };
        }

        // SAFETY: list contains unloaned allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }

        expect_eq!(0, node.node().count_loaned_pages());
        expect_eq!(ManagedPmmNode::NUM_PAGES as u64, node.node().count_free_pages());
        expect_eq!(0, node.node().count_loaned_free_pages());
        expect_eq!(0, node.node().count_loan_cancelled_pages());
        expect_eq!(0, node.node().count_loaned_not_free_pages());

        expect_eq!(0, list.iter().count());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status, "allocate all pages");
        expect_eq!(ManagedPmmNode::NUM_PAGES, list.iter().count());

        for page in list.iter() {
            expect_false!(page.is_loaned());
            expect_false!(page.is_loan_cancelled());
        }

        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }

        expect_eq!(0, node.node().count_loaned_pages());
        expect_eq!(ManagedPmmNode::NUM_PAGES as u64, node.node().count_free_pages());
        expect_eq!(0, node.node().count_loaned_free_pages());
        expect_eq!(0, node.node().count_loan_cancelled_pages());
        expect_eq!(0, node.node().count_loaned_not_free_pages());
    }

    /// Allocates too many pages and makes sure it fails nicely.
    #[test]
    fn node_oversized_alloc() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());

        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES + 1, 0, list.as_mut());
        expect_true!(status == Err(Status::NO_MEMORY), "pmm_alloc_pages failed to alloc");
        expect_true!(list.is_empty(), "pmm_alloc_pages list is empty");
    }

    /// Check that free memory events work correctly.
    #[test]
    fn node_free_mem_event() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        let free_count = node.node().count_free_pages();
        assert_gt!(free_count, 0);

        // Setting an event range that does not include the current free count should be invalid.
        expect_false!(node.set_free_memory_signal(free_count + 1, u64::MAX, 0));
        expect_false!(node.set_free_memory_signal(0, free_count - 1, 0));

        // The range can be inclusive of the current free count.
        expect_true!(node.set_free_memory_signal(free_count, u64::MAX, 0));
        expect_true!(node.set_free_memory_signal(0, free_count, 0));

        // Reset back to the default event.
        expect_true!(node.reset_default_mem_event());

        // Should never have triggered the event up to this point.
        expect_false!(node.is_event_signaled());

        // Allocate all but 1 of the pages to trigger the event.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());

        for _i in 1..ManagedPmmNode::DEFAULT_MEM_EVENT_ALLOC {
            let page = unwrap_ok!(node.node().alloc_page(0));
            // SAFETY: mutating pinned list without moving it.
            unsafe { list.as_mut().get_unchecked_mut().push_back_raw(page.as_non_null()) };
        }
        // Should not have triggered the event yet.
        expect_false!(node.is_event_signaled());

        // Allocate the last page, this should put us over the limit and set the event.
        {
            let page = unwrap_ok!(node.node().alloc_page(0));
            // SAFETY: mutating pinned list without moving it.
            unsafe { list.as_mut().get_unchecked_mut().push_back_raw(page.as_non_null()) };
        }
        expect_true!(node.is_event_signaled());
        node.unsignal_event();

        // Events are one-shot, and so putting a page back and allocating it again should not
        // re-trigger the event.
        // SAFETY: popping from pinned list without moving list.
        let pop_page = unsafe { list.as_mut().get_unchecked_mut().pop_front().unwrap() };
        // SAFETY: pop_page is a valid pointer.
        unsafe {
            node.node().free_page(VmPagePtr::new(pop_page), PmmOptDelayReuse::Default);
        }
        {
            let page = unwrap_ok!(node.node().alloc_page(0));
            // SAFETY: mutating pinned list without moving it.
            unsafe { list.as_mut().get_unchecked_mut().push_back_raw(page.as_non_null()) };
        }
        expect_false!(node.is_event_signaled());

        // Set a new free range that should trip as we return the pages back.
        expect_true!(node.set_free_memory_signal(0, (ManagedPmmNode::NUM_PAGES - 1) as u64, 0));

        // Take one page off the list as our final page.
        // SAFETY: popping from pinned list without moving list.
        let page_raw = unsafe { list.as_mut().get_unchecked_mut().pop_front().unwrap() };
        let page = VmPagePtr::new(page_raw);

        // Return the rest of the list.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
        // Event should not have tripped yet.
        expect_false!(node.is_event_signaled());

        // Return the last page, should trip.
        // SAFETY: page was allocated and is owned by this test.
        unsafe {
            node.node().free_page(page, PmmOptDelayReuse::Default);
        }
        expect_true!(node.is_event_signaled());
    }

    /// Checks sync allocation failure when the node crosses a threshold.
    #[test]
    fn node_low_mem_alloc_failure() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());

        // Put the node in an oom state and make sure allocation fails.
        let status =
            node.node().alloc_pages(ManagedPmmNode::DEFAULT_LOW_MEM_ALLOC, 0, list.as_mut());
        expect_ok!(status);
        // Should also have been signaled.
        expect_true!(node.is_event_signaled());

        let result = node.node().alloc_page(ALLOC_FLAG_CAN_WAIT);
        expect_true!(result == Err(Status::SHOULD_WAIT));

        // Waiting for an allocation should block.
        expect_true!(
            node.node().wait_for_single_page_allocation(
                Deadline::after_mono(DurationMono::from_millis(10), TimerSlack::none()),
                true
            ) == Err(Status::TIMED_OUT)
        );

        // Free the list.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }

        // Allocations will still be delayed until we reset the trigger.
        let result = node.node().alloc_page(ALLOC_FLAG_CAN_WAIT);
        expect_true!(result == Err(Status::SHOULD_WAIT));

        expect_true!(node.reset_default_mem_event());

        // Allocations should work again.
        {
            let alloc_page =
                node.node().wait_for_single_page_allocation(Deadline::infinite_past(), true);
            assert_true!(alloc_page != Err(Status::TIMED_OUT));
            if let Ok(page) = alloc_page {
                // SAFETY: page was allocated and is owned by this test.
                unsafe {
                    node.node().free_page(page, PmmOptDelayReuse::Default);
                }
            }
        }

        // Reset the signal.
        node.unsignal_event();
        // Set a threshold such that a single allocation should trip into the low mem state.
        expect_true!(node.set_free_memory_signal(
            ManagedPmmNode::NUM_PAGES as u64,
            u64::MAX,
            ManagedPmmNode::NUM_PAGES as u64
        ));

        // Signal should not yet be set, and allocations should not be delayed.
        expect_false!(node.is_event_signaled());
        {
            let alloc_page =
                node.node().wait_for_single_page_allocation(Deadline::infinite_past(), true);
            assert_true!(alloc_page != Err(Status::TIMED_OUT));
            if let Ok(page) = alloc_page {
                // SAFETY: page was allocated and is owned by this test.
                unsafe {
                    node.node().free_page(page, PmmOptDelayReuse::Default);
                }
            }
        }

        // Allocate a single page and validate that allocations are now delayed.
        assert_ok!(node.node().alloc_pages(1, 0, list.as_mut()));
        let result = node.node().alloc_page(ALLOC_FLAG_CAN_WAIT);
        expect_true!(result == Err(Status::SHOULD_WAIT));
        expect_true!(
            node.node().wait_for_single_page_allocation(
                Deadline::after_mono(DurationMono::from_millis(10), TimerSlack::none()),
                true
            ) == Err(Status::TIMED_OUT)
        );

        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Test reporting allocation failures and latching the first failure.
    #[test]
    fn node_alloc_failure_reporting() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Initially, no allocation failure should be recorded.
        expect_false!(node.node().has_alloc_failed_no_mem());
        let initial_failure = node.node().get_first_alloc_failure();
        expect_true!(initial_failure.r#type == AllocFailureType::None);
        expect_eq!(0, initial_failure.size);

        // Report a first allocation failure.
        let failure1 = AllocFailure { r#type: AllocFailureType::Heap, size: 1024, free_count: 10 };
        node.node().report_alloc_failure(failure1);

        expect_true!(node.node().has_alloc_failed_no_mem());
        let recorded_failure = node.node().get_first_alloc_failure();
        expect_true!(recorded_failure.r#type == AllocFailureType::Heap);
        expect_eq!(1024, recorded_failure.size);
        expect_eq!(ManagedPmmNode::NUM_PAGES as u64, recorded_failure.free_count);

        // Report a second allocation failure with different parameters.
        let failure2 = AllocFailure { r#type: AllocFailureType::Pmm, size: 4096, free_count: 5 };
        node.node().report_alloc_failure(failure2);

        // The node should still retain the first recorded failure.
        let latched_failure = node.node().get_first_alloc_failure();
        expect_true!(latched_failure.r#type == AllocFailureType::Heap);
        expect_eq!(1024, latched_failure.size);
        expect_eq!(ManagedPmmNode::NUM_PAGES as u64, latched_failure.free_count);
    }

    /// Test that deliberately putting into a no alloc state (and back out) works.
    #[test]
    fn node_explicit_should_wait() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        // Allocations that can wait should be blocked.
        let result = node.node().alloc_page(ALLOC_FLAG_CAN_WAIT);
        expect_true!(result == Err(Status::SHOULD_WAIT));
        expect_true!(
            node.node().wait_for_single_page_allocation(
                Deadline::after_mono(DurationMono::from_millis(10), TimerSlack::none()),
                true
            ) == Err(Status::TIMED_OUT)
        );

        // A regular allocation should work.
        let result = unwrap_ok!(node.node().alloc_page(0));
        // SAFETY: result is a valid allocated page.
        unsafe {
            node.node().free_page(result, PmmOptDelayReuse::Default);
        }

        // Changing the delayed threshold should re-enable allocations.
        expect_true!(node.reset_default_mem_event());

        {
            let alloc_page =
                node.node().wait_for_single_page_allocation(Deadline::infinite_past(), true);
            assert_true!(alloc_page != Err(Status::TIMED_OUT));
            if let Ok(page) = alloc_page {
                // SAFETY: page was allocated and is owned by this test.
                unsafe {
                    node.node().free_page(page, PmmOptDelayReuse::Default);
                }
            }
        }
    }

    struct PmmWaiterArgs {
        node: *const PmmNode,
        timeout_count: *const AtomicI32,
        no_memory_count: *const AtomicI32,
    }

    // SAFETY: Raw pointers point to valid test data living for the test duration.
    unsafe impl Send for PmmWaiterArgs {}
    // SAFETY: Raw pointers point to valid test data living for the test duration.
    unsafe impl Sync for PmmWaiterArgs {}

    extern "C" fn pmm_waiter_thread(arg: *mut core::ffi::c_void) -> i32 {
        // SAFETY: arg is a valid pointer to PmmWaiterArgs passed during thread spawn.
        let args = unsafe { &*(arg as *const PmmWaiterArgs) };
        // SAFETY: PmmWaiterArgs fields point to valid objects living for the test duration.
        let node = unsafe { &*args.node };
        // SAFETY: PmmWaiterArgs fields point to valid objects living for the test duration.
        let timeout_count = unsafe { &*args.timeout_count };
        // SAFETY: PmmWaiterArgs fields point to valid objects living for the test duration.
        let no_memory_count = unsafe { &*args.no_memory_count };

        let result = node.wait_for_single_page_allocation(
            Deadline::after_mono(DurationMono::from_seconds(2), TimerSlack::none()),
            true,
        );
        match result {
            Err(Status::TIMED_OUT) => {
                timeout_count.fetch_add(1, Ordering::Relaxed);
            }
            Err(Status::NO_MEMORY) => {
                no_memory_count.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: page was allocated by wait_for_single_page_allocation.
            Ok(page) => unsafe {
                node.free_page(page, PmmOptDelayReuse::Default);
            },
            _ => {}
        }
        0
    }

    /// Verifies that WaitForSinglePageAllocation does not block after StopReturningShouldWait.
    #[test]
    fn node_stop_returning_should_wait() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status);

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        let timeout_count = AtomicI32::new(0);
        let no_memory_count = AtomicI32::new(0);
        let args = PmmWaiterArgs {
            node: node.node() as *const _,
            timeout_count: &timeout_count as *const _,
            no_memory_count: &no_memory_count as *const _,
        };

        // Start a thread that will wait.
        // SAFETY: args outlives the spawned thread which is joined before test exit.
        let thread = unwrap_ok!(unsafe {
            thread::spawn(
                c"pmm waiter".as_ptr(),
                pmm_waiter_thread,
                &args as *const _ as *mut core::ffi::c_void,
            )
        });

        // Give the thread time to block.
        let _ = thread::sleep_relative(DurationMono::from_millis(100));

        // Stop returning should wait. This should wake up the thread.
        node.node().stop_returning_should_wait();

        // Wait for the thread to complete.
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.join(InstantMono::INFINITE) };

        // Verify that the thread did not time out.
        expect_eq!(timeout_count.load(Ordering::Relaxed), 0);
        // Verify that the thread failed with NO_MEMORY.
        expect_eq!(no_memory_count.load(Ordering::Relaxed), 1);

        // Second call: may_allocate_evt_ might be unsignaled but since should_wait_ is Never, it
        // should not wait.
        let alloc_page2 = node.node().wait_for_single_page_allocation(
            Deadline::after_mono(DurationMono::from_millis(10), TimerSlack::none()),
            true,
        );
        expect_true!(alloc_page2 == Err(Status::NO_MEMORY));

        // Clean up.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Verifies that all threads blocked on WaitForSinglePageAllocation are woken up.
    #[test]
    fn node_stop_returning_should_wait_concurrent() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status);

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        let timeout_count = AtomicI32::new(0);
        let no_memory_count = AtomicI32::new(0);
        let args = PmmWaiterArgs {
            node: node.node() as *const _,
            timeout_count: &timeout_count as *const _,
            no_memory_count: &no_memory_count as *const _,
        };

        const NUM_WAITERS: usize = 3;
        let mut threads = [None; NUM_WAITERS];

        for t in threads.iter_mut() {
            // SAFETY: args outlives the spawned threads which are joined before test exit.
            let thread = unwrap_ok!(unsafe {
                thread::spawn(
                    c"pmm waiter".as_ptr(),
                    pmm_waiter_thread,
                    &args as *const _ as *mut core::ffi::c_void,
                )
            });
            *t = Some(thread);
        }

        // Give threads time to block.
        let _ = thread::sleep_relative(DurationMono::from_millis(100));

        // Stop returning should wait.
        node.node().stop_returning_should_wait();

        // Wait for all threads to complete.
        for t in threads.iter() {
            // SAFETY: t contains a valid Thread handle.
            let _ = unsafe { t.unwrap().join(InstantMono::INFINITE) };
        }

        // Verify that NO threads timed out.
        expect_eq!(timeout_count.load(Ordering::Relaxed), 0);
        // Verify that all threads failed with NO_MEMORY.
        expect_eq!(no_memory_count.load(Ordering::Relaxed), NUM_WAITERS as i32);

        // Clean up.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    struct PmmSuspendKillWaiterArgs {
        node: *const PmmNode,
        suspendable: bool,
        timeout: DurationMono,
        result: *const AtomicI32,
    }

    // SAFETY: Raw pointers point to valid test data living for the test duration.
    unsafe impl Send for PmmSuspendKillWaiterArgs {}
    // SAFETY: Raw pointers point to valid test data living for the test duration.
    unsafe impl Sync for PmmSuspendKillWaiterArgs {}

    extern "C" fn pmm_suspend_kill_waiter_thread(arg: *mut core::ffi::c_void) -> i32 {
        // SAFETY: arg is a valid pointer to PmmSuspendKillWaiterArgs passed during thread spawn.
        let args = unsafe { &*(arg as *const PmmSuspendKillWaiterArgs) };
        // SAFETY: PmmSuspendKillWaiterArgs fields point to valid objects living for the test
        // duration.
        let node = unsafe { &*args.node };
        // SAFETY: PmmSuspendKillWaiterArgs fields point to valid objects living for the test
        // duration.
        let result = unsafe { &*args.result };

        let res = node.wait_for_single_page_allocation(
            Deadline::after_mono(args.timeout, TimerSlack::none()),
            args.suspendable,
        );
        // SAFETY: page was allocated by wait_for_single_page_allocation.
        let res = res.map(|page| unsafe {
            node.free_page(page, PmmOptDelayReuse::Default);
        });
        let status = Status::result_into_raw(res);
        result.store(status, Ordering::Relaxed);
        0
    }

    /// Verifies that suspendable WaitForSinglePageAllocation is interrupted by suspension.
    #[test]
    fn node_suspendable_wait() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status);

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        let result = AtomicI32::new(zx_types::ZX_OK);
        let args = PmmSuspendKillWaiterArgs {
            node: node.node() as *const _,
            suspendable: true,
            timeout: DurationMono::from_seconds(5),
            result: &result as *const _,
        };

        // Start a thread that will wait in a suspendable state.
        // SAFETY: args outlives the spawned thread which is joined before test exit.
        let thread = unwrap_ok!(unsafe {
            thread::spawn(
                c"pmm suspendable waiter".as_ptr(),
                pmm_suspend_kill_waiter_thread,
                &args as *const _ as *mut core::ffi::c_void,
            )
        });

        // Give the thread time to block.
        let _ = thread::sleep_relative(DurationMono::from_millis(100));

        // Suspend the thread.
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.suspend() };

        // Wait for the thread to complete (it should exit immediately due to suspension).
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.join(InstantMono::INFINITE) };

        // Verify that the thread returned ZX_ERR_INTERNAL_INTR_RETRY.
        expect_eq!(result.load(Ordering::Relaxed), Status::INTERRUPTED_RETRY.into_raw());

        // Clean up.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Verifies that non-suspendable WaitForSinglePageAllocation ignores suspend signals.
    #[test]
    fn node_non_suspendable_wait() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status);

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        // Use a short 200ms timeout so the test completes quickly.
        let result = AtomicI32::new(zx_types::ZX_OK);
        let args = PmmSuspendKillWaiterArgs {
            node: node.node() as *const _,
            suspendable: false,
            timeout: DurationMono::from_millis(200),
            result: &result as *const _,
        };

        // Start a thread that will wait in a non-suspendable state.
        // SAFETY: args outlives the spawned thread which is joined before test exit.
        let thread = unwrap_ok!(unsafe {
            thread::spawn(
                c"pmm non-suspendable waiter".as_ptr(),
                pmm_suspend_kill_waiter_thread,
                &args as *const _ as *mut core::ffi::c_void,
            )
        });

        // Give the thread time to block.
        let _ = thread::sleep_relative(DurationMono::from_millis(50));

        // Suspend the thread (which should be ignored by the Wait loop).
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.suspend() };

        // Wait for the thread to complete (it should wait out the full 200ms timeout).
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.join(InstantMono::INFINITE) };

        // Verify that the thread returned ZX_ERR_TIMED_OUT instead of ZX_ERR_INTERNAL_INTR_RETRY.
        expect_eq!(result.load(Ordering::Relaxed), Status::TIMED_OUT.into_raw());

        // Clean up.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Verifies that WaitForSinglePageAllocation is interrupted when the thread is killed.
    #[test]
    fn node_killed_wait() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status);

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        // Use a long timeout so the test doesn't time out.
        let result = AtomicI32::new(zx_types::ZX_OK);
        let args = PmmSuspendKillWaiterArgs {
            node: node.node() as *const _,
            suspendable: true,
            timeout: DurationMono::from_seconds(5),
            result: &result as *const _,
        };

        // Start a thread that will wait.
        // SAFETY: args outlives the spawned thread which is joined before test exit.
        let thread = unwrap_ok!(unsafe {
            thread::spawn(
                c"pmm killed waiter".as_ptr(),
                pmm_suspend_kill_waiter_thread,
                &args as *const _ as *mut core::ffi::c_void,
            )
        });

        // Give the thread time to block.
        let _ = thread::sleep_relative(DurationMono::from_millis(100));

        // Kill the thread.
        // SAFETY: thread is a valid Thread handle.
        unsafe { thread.kill() };

        // Wait for the thread to complete.
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.join(InstantMono::INFINITE) };

        // Verify that the thread returned ZX_ERR_INTERNAL_INTR_KILLED.
        expect_eq!(result.load(Ordering::Relaxed), zx_types::ZX_ERR_INTERNAL_INTR_KILLED);

        // Clean up.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Verifies that non-suspendable WaitForSinglePageAllocation is interrupted when killed.
    #[test]
    fn node_suspend_then_killed_wait() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        // Allocate all pages to ensure AllocPage fails with NO_MEMORY later.
        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        let status = node.node().alloc_pages(ManagedPmmNode::NUM_PAGES, 0, list.as_mut());
        expect_ok!(status);

        // Place the node directly into a state that forbids allocations.
        expect_true!(node.set_free_memory_signal(0, ManagedPmmNode::NUM_PAGES as u64, u64::MAX));

        // Use a long timeout so the test doesn't time out naturally.
        let result = AtomicI32::new(zx_types::ZX_OK);
        let args = PmmSuspendKillWaiterArgs {
            node: node.node() as *const _,
            suspendable: false,
            timeout: DurationMono::from_seconds(5),
            result: &result as *const _,
        };

        // Start a thread that will wait in a non-suspendable state.
        // SAFETY: args outlives the spawned thread which is joined before test exit.
        let thread = unwrap_ok!(unsafe {
            thread::spawn(
                c"pmm suspend-then-killed waiter".as_ptr(),
                pmm_suspend_kill_waiter_thread,
                &args as *const _ as *mut core::ffi::c_void,
            )
        });

        // Give the thread time to block.
        let _ = thread::sleep_relative(DurationMono::from_millis(100));

        // Suspend the thread (which should be ignored).
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.suspend() };

        // Give it some time to ensure it's still blocked.
        let _ = thread::sleep_relative(DurationMono::from_millis(50));

        // Now kill the thread.
        // SAFETY: thread is a valid Thread handle.
        unsafe { thread.kill() };

        // Wait for the thread to complete.
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.join(InstantMono::INFINITE) };

        // Verify that the thread returned ZX_ERR_INTERNAL_INTR_KILLED.
        expect_eq!(result.load(Ordering::Relaxed), zx_types::ZX_ERR_INTERNAL_INTR_KILLED);

        // Clean up.
        // SAFETY: list contains allocated pages.
        unsafe {
            node.node().free_list(list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    /// Verifies that AllocPages appends to an existing list without re-running the checker.
    #[test]
    fn alloc_append() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        stack_pin_init!(let alloc_list = VmPageDoublyLinkedList::new());

        // Allocate a single page into the list first.
        assert_ok!(node.node().alloc_pages(1, 0, alloc_list.as_mut()));

        // Zero the page as a modification.
        let front_pa = alloc_list.front().unwrap().paddr();
        let p_vaddr = paddr_to_physmap(front_pa);
        // SAFETY: front_pa is a valid page allocated from PMM, so its physmap mapping is valid for
        // page::SIZE bytes.
        let p = unsafe { core::slice::from_raw_parts_mut(p_vaddr.0 as *mut u8, page::SIZE) };
        p.fill(0);

        // Now append more pages to the list. If this runs the checker on the page already in the
        // list that we modified then it will panic.
        expect_ok!(node.node().alloc_pages(ManagedPmmNode::NUM_PAGES / 2, 0, alloc_list.as_mut()));

        // SAFETY: alloc_list contains allocated pages.
        unsafe {
            node.node().free_list(alloc_list.as_mut(), PmmOptDelayReuse::Default);
        }
    }

    struct WithLoanedPageWaiterArgs {
        node: *const PmmNode,
        page: VmPagePtr,
        completed: *const AtomicBool,
        seen_free_loaned: *const AtomicBool,
    }

    // SAFETY: Raw pointers point to valid test data living for the test duration.
    unsafe impl Send for WithLoanedPageWaiterArgs {}
    // SAFETY: Raw pointers point to valid test data living for the test duration.
    unsafe impl Sync for WithLoanedPageWaiterArgs {}

    extern "C" fn with_loaned_page_waiter_thread(arg: *mut core::ffi::c_void) -> i32 {
        // SAFETY: arg is a valid pointer to WithLoanedPageWaiterArgs passed during thread spawn.
        let args = unsafe { &*(arg as *const WithLoanedPageWaiterArgs) };
        // SAFETY: WithLoanedPageWaiterArgs fields point to valid objects living for the test
        // duration.
        let node = unsafe { &*args.node };
        // SAFETY: WithLoanedPageWaiterArgs fields point to valid objects living for the test
        // duration.
        let completed = unsafe { &*args.completed };
        // SAFETY: WithLoanedPageWaiterArgs fields point to valid objects living for the test
        // duration.
        let seen_free_loaned = unsafe { &*args.seen_free_loaned };

        node.with_loaned_page(args.page, |p| {
            // SAFETY: p is a valid loaned page pointer.
            seen_free_loaned.store(unsafe { p.is_free_loaned() }, Ordering::Relaxed);
            completed.store(true, Ordering::Relaxed);
        });
        0
    }

    /// Verifies `with_loaned_page` waiting and non-waiting behavior.
    #[test]
    fn node_with_loaned_page() {
        stack_try_pin_init!(let node = ManagedPmmNode::init());
        let mut node = unwrap_ok!(node);
        assert_ok!(node.as_mut().setup());

        let _cleanup = ScopedLoaningEnabled::new(true);

        stack_pin_init!(let list = VmPageDoublyLinkedList::new());
        assert_ok!(node.node().alloc_pages(1, 0, list.as_mut()));
        // SAFETY: list contains valid allocated page to loan.
        unsafe {
            node.node().begin_loan(list.as_mut(), PmmOptDelayReuse::Default);
        }

        let mut loaned_pages = [None; 1];
        assert_ok!(node.alloc_loaned_pages(1, &mut loaned_pages));
        let page = loaned_pages[0].unwrap();

        // 1. Page is in OBJECT state (not ALLOC); `with_loaned_page` should run immediately.
        let mut called_object = false;
        node.node().with_loaned_page(page, |p| {
            assert_eq!(p.state(), VmPageState(vm_page_state::OBJECT));
            called_object = true;
        });
        expect_true!(called_object);

        // 2. Begin freeing the loaned page into a `FreeLoanedPagesHolder`.
        // The page is now in ALLOC state with non-null `alloc.owner` pointing to `flph`.
        stack_pin_init!(let flph = FreeLoanedPagesHolder::init());
        // SAFETY: page was allocated as loaned and flph is a valid pinned holder.
        unsafe {
            node.node().begin_free_loaned_page(
                page,
                |p| crate::vm::pmm::node().page_queues().remove(p),
                flph.as_mut(),
            );
        }

        let completed = AtomicBool::new(false);
        let seen_free_loaned = AtomicBool::new(false);
        let args = WithLoanedPageWaiterArgs {
            node: node.node() as *const _,
            page,
            completed: &completed as *const _,
            seen_free_loaned: &seen_free_loaned as *const _,
        };

        // Start a thread that calls `with_loaned_page`. It must block until
        // `finish_free_loaned_pages`.
        // SAFETY: args outlives the spawned thread which is joined before test exit.
        let thread = unwrap_ok!(unsafe {
            thread::spawn(
                c"with_loaned_page waiter".as_ptr(),
                with_loaned_page_waiter_thread,
                &args as *const _ as *mut core::ffi::c_void,
            )
        });

        // Give the thread time to enter `with_loaned_page` and wait on `flph`.
        let _ = thread::sleep_relative(DurationMono::from_millis(50));

        // The callback must NOT have run yet because `flph` still holds the page.
        expect_false!(completed.load(Ordering::Relaxed));

        // Finish freeing loaned pages, which wakes up the waiter thread.
        node.node().finish_free_loaned_pages(flph.as_mut());

        // Wait for the thread to complete.
        // SAFETY: thread is a valid Thread handle.
        let _ = unsafe { thread.join(InstantMono::INFINITE) };

        expect_true!(completed.load(Ordering::Relaxed));
        expect_true!(seen_free_loaned.load(Ordering::Relaxed));

        // 3. Cancel the loan so the page is removed from `free_loaned_list`.
        // SAFETY: page is a valid loaned page pointer.
        unsafe {
            node.node().cancel_loan(page);
        }

        // Test `with_loaned_page` when the page is in ALLOC state with a null `alloc.owner`
        // (i.e. not owned by a `FreeLoanedPagesHolder`). It should run immediately without
        // dereferencing a null pointer.
        // SAFETY: page is exclusively owned by this test while loan is cancelled.
        unsafe {
            page.set_state(VmPageState(vm_page_state::ALLOC));
            (*page.as_ref().state_union.get()).alloc.owner = core::ptr::null_mut();
        }
        let mut called_alloc_null_owner = false;
        node.node().with_loaned_page(page, |p| {
            assert_eq!(p.state(), VmPageState(vm_page_state::ALLOC));
            called_alloc_null_owner = true;
        });
        expect_true!(called_alloc_null_owner);

        // Restore FREE_LOANED state to end the loan and return the page to the node.
        // SAFETY: restoring page state before ending loan and freeing page.
        unsafe {
            page.set_state(VmPageState(vm_page_state::FREE_LOANED));
            node.node().end_loan(page);
            node.node().free_page(page, PmmOptDelayReuse::Default);
        }
    }
}
