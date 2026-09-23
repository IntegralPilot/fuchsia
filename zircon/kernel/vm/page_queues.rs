// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::event::{AutounsignalEvent, Event};
use crate::kernel::relaxed_atomic::{RelaxedAtomic, RelaxedAtomicU32, RelaxedAtomicU64};
use crate::kernel::thread::{Thread, ThreadPtr};
use crate::platform_rs::timer::DurationMono;
use crate::vm::page::{VmPageDoublyLinkedList, VmPagePtr};
use crate::vm::vm_cow_pages::VmCowPages;
use core::cell::UnsafeCell;
use core::ffi::CStr;
use core::marker::{PhantomData, PhantomPinned};
use core::mem::{MaybeUninit, offset_of};
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use fbl::RefPtr;
use ksync::{KMutex, LockToken, RawCriticalMutex, guarded};
use page_queues_bindings as bindings;
use pin_init::{PinInit, pin_data};

/// Used to identify the reason that aging is triggered, mostly for debugging and informational
/// purposes.
#[repr(i32)]
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum AgeReason {
    /// Aging occurred due to the maximum timeout being reached before any other reason could
    /// trigger.
    Timeout,
    /// The allowable ratio of active versus inactive pages was exceeded.
    ActiveRatio,
    /// An explicit call to RotatePagerBackedQueues caused aging. This would typically occur due to
    /// test code or via the kernel debug console.
    Manual,
}
zr::static_assert!(AgeReason::Timeout as i32 == bindings::PageQueues_AgeReason::Timeout as i32);
zr::static_assert!(
    AgeReason::ActiveRatio as i32 == bindings::PageQueues_AgeReason::ActiveRatio as i32
);
zr::static_assert!(AgeReason::Manual as i32 == bindings::PageQueues_AgeReason::Manual as i32);
zr::static_assert!(size_of::<AgeReason>() == size_of::<bindings::PageQueues_AgeReason>());

/// Describes any action to take when processing the LRU queue. This is applied to pages that would
/// otherwise have to be moved from the old LRU queue into the isolate queue.
#[repr(i32)]
#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum LruAction {
    None,
    EvictOnly,
    CompressOnly,
    EvictAndCompress,
}
zr::static_assert!(LruAction::None as i32 == bindings::PageQueues_LruAction::None as i32);
zr::static_assert!(LruAction::EvictOnly as i32 == bindings::PageQueues_LruAction::EvictOnly as i32);
zr::static_assert!(
    LruAction::CompressOnly as i32 == bindings::PageQueues_LruAction::CompressOnly as i32
);
zr::static_assert!(
    LruAction::EvictAndCompress as i32 == bindings::PageQueues_LruAction::EvictAndCompress as i32
);
zr::static_assert!(size_of::<LruAction>() == size_of::<bindings::PageQueues_LruAction>());

/// Helper struct to group queue length counts returned by [`PageQueues::queue_counts`].
#[repr(C)]
#[derive(Default, Debug, Clone)]
pub struct Counts {
    pub reclaim: [usize; NUM_RECLAIM],
    pub reclaim_isolate: usize,
    pub pager_backed_dirty: usize,
    pub anonymous: usize,
    pub wired: usize,
    pub anonymous_zero_fork: usize,
    pub failed_reclaim: usize,
    pub high_priority: usize,
}
zr::static_assert!(offset_of!(Counts, reclaim) == offset_of!(bindings::PageQueues_Counts, reclaim));
zr::static_assert!(
    offset_of!(Counts, reclaim_isolate) == offset_of!(bindings::PageQueues_Counts, reclaim_isolate)
);
zr::static_assert!(
    offset_of!(Counts, pager_backed_dirty)
        == offset_of!(bindings::PageQueues_Counts, pager_backed_dirty)
);
zr::static_assert!(
    offset_of!(Counts, anonymous) == offset_of!(bindings::PageQueues_Counts, anonymous)
);
zr::static_assert!(offset_of!(Counts, wired) == offset_of!(bindings::PageQueues_Counts, wired));
zr::static_assert!(
    offset_of!(Counts, anonymous_zero_fork)
        == offset_of!(bindings::PageQueues_Counts, anonymous_zero_fork)
);
zr::static_assert!(
    offset_of!(Counts, failed_reclaim) == offset_of!(bindings::PageQueues_Counts, failed_reclaim)
);
zr::static_assert!(
    offset_of!(Counts, high_priority) == offset_of!(bindings::PageQueues_Counts, high_priority)
);
zr::static_assert!(size_of::<Counts>() == size_of::<bindings::PageQueues_Counts>());

/// Helper struct to group reclaimable queue length counts returned by
/// [`PageQueues::get_reclaim_queue_counts`].
#[repr(C)]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct ReclaimCounts {
    pub total: usize,
    pub newest: usize,
    pub oldest: usize,
}
zr::static_assert!(
    offset_of!(ReclaimCounts, total) == offset_of!(bindings::PageQueues_ReclaimCounts, total)
);
zr::static_assert!(
    offset_of!(ReclaimCounts, newest) == offset_of!(bindings::PageQueues_ReclaimCounts, newest)
);
zr::static_assert!(
    offset_of!(ReclaimCounts, oldest) == offset_of!(bindings::PageQueues_ReclaimCounts, oldest)
);
zr::static_assert!(size_of::<ReclaimCounts>() == size_of::<bindings::PageQueues_ReclaimCounts>());

/// Helper struct to group active and inactive page counts returned by
/// [`PageQueues::get_active_inactive_counts`].
#[repr(C)]
#[derive(Default, Debug, Copy, Clone, PartialEq, Eq)]
pub struct ActiveInactiveCounts {
    /// Pages that would normally be available for eviction, but are presently considered active and
    /// so will not be evicted.
    pub active: usize,
    /// Pages that are available for eviction due to not presently being considered active.
    pub inactive: usize,
}
zr::static_assert!(
    offset_of!(ActiveInactiveCounts, active)
        == offset_of!(bindings::PageQueues_ActiveInactiveCounts, active)
);
zr::static_assert!(
    offset_of!(ActiveInactiveCounts, inactive)
        == offset_of!(bindings::PageQueues_ActiveInactiveCounts, inactive)
);
zr::static_assert!(
    size_of::<ActiveInactiveCounts>() == size_of::<bindings::PageQueues_ActiveInactiveCounts>()
);

/// Used to represent and return page backlink information acquired whilst holding the page queue
/// lock. As a VMO may not destruct while it has pages in it, the cow RefPtr will always be valid,
/// although the page and offset contained here are not synchronized and must be separately
/// validated before use. This can be done by acquiring the returned vmo's lock and then validating
/// that the page is still contained at the offset.
#[derive(Clone)]
pub struct VmoBacklink {
    pub cow: Option<RefPtr<VmCowPages>>,
    pub page: VmPagePtr,
    pub offset: u64,
}

impl core::fmt::Debug for VmoBacklink {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("VmoBacklink")
            .field("cow", &self.cow.as_ref().map(|c| c.as_raw()))
            .field("page", &self.page)
            .field("offset", &self.offset)
            .finish()
    }
}

/// Specifies the indices for both the page_queues and the page_queue_counts
#[repr(transparent)]
#[derive(Default, Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd)]
struct PageQueue(u8);

impl PageQueue {
    const NONE: Self = Self(0);
    const ANONYMOUS: Self = Self(1);
    const WIRED: Self = Self(2);
    const HIGH_PRIORITY: Self = Self(3);
    const ANONYMOUS_ZERO_FORK: Self = Self(4);
    const PAGER_BACKED_DIRTY: Self = Self(5);
    const FAILED_RECLAIM: Self = Self(6);
    const RECLAIM_ISOLATE: Self = Self(7);
    const RECLAIM_BASE: Self = Self(8);
    const RECLAIM_LAST: Self = Self(Self::RECLAIM_BASE.0 + NUM_RECLAIM as u8 - 1);
    const NUM_QUEUES: Self = Self(Self::RECLAIM_LAST.0 + 1);
}

// Ensure that the reclaim queue counts are always at the end.
zr::static_assert!(PageQueue::RECLAIM_LAST.0 + 1 == PageQueue::NUM_QUEUES.0);

/// The number of reclamation queues is slightly arbitrary, but to be useful you want at least 3
/// representing
///  * Very new pages that you probably don't want to evict as doing so probably implies you are in
///    swap death
///  * Slightly old pages that could be evicted if needed
///  * Very old pages that you'd be happy to evict
///
/// With two active queues 8 page queues are used so that there is some fidelity of information
/// in the inactive queues. Additional queues have reduced value as sufficiently old pages
/// quickly become equivalently unlikely to be used in the future.
pub const NUM_RECLAIM: usize = 8;
zr::static_assert!(NUM_RECLAIM == bindings::PageQueues_kNumReclaim);

/// Two active queues are used to allow for better fidelity of active information. This prevents
/// a race between aging once and needing to collect/harvest age information.
pub const NUM_ACTIVE_QUEUES: usize = 2;
zr::static_assert!(NUM_ACTIVE_QUEUES == bindings::PageQueues_kNumActiveQueues);

/// The amount of pages that will have to move around the queues before the active/inactive
/// ratio is re-checked. This therefore represents how much error the active ratio aging process
/// might have, or how delayed the MRU generation might be. In the worst case once the active
/// ratio is triggered this value is how much page data needs to then change queues before the
/// aging process happens.
const MB: usize = 1024 * 1024;
pub const ACTIVE_INACTIVE_ERROR_MARGIN: usize = (2 * MB) / page::SIZE;
zr::static_assert!(ACTIVE_INACTIVE_ERROR_MARGIN == bindings::PageQueues_kActiveInactiveErrorMargin);

// Needs to be at least one non-active queue
zr::static_assert!(NUM_RECLAIM > NUM_ACTIVE_QUEUES);

/// In addition to active and inactive, we want to consider some of the queues as 'oldest' to
/// provide an additional way to limit eviction. Presently the processing of the LRU queue to
/// make room for aging is not integrated with the Evictor, and so will not trigger eviction,
/// therefore to have a non-zero number of pages ever appear in an oldest queue for eviction the
/// last two queues are considered the oldest.
pub const NUM_OLDEST_QUEUES: usize = 2;
zr::static_assert!(NUM_OLDEST_QUEUES == bindings::PageQueues_kNumOldestQueues);
zr::static_assert!(NUM_OLDEST_QUEUES + NUM_ACTIVE_QUEUES <= NUM_RECLAIM);

/// Number of different isolate queues that are available. Different isolate queues allow for
/// separating isolate pages into different buckets such that more nuanced choices on what page
/// to reclaim can be made.
///
/// We use 2 queues to separate "Don't Need" pages (high reclamation priority, index 0) from
/// standard aged pages (standard reclamation priority, index 1).
pub const ISOLATE_QUEUE_DONT_NEED: usize = 0;
zr::static_assert!(ISOLATE_QUEUE_DONT_NEED == bindings::PageQueues_kIsolateQueueDontNeed);
pub const ISOLATE_QUEUE_STANDARD: usize = 1;
zr::static_assert!(ISOLATE_QUEUE_STANDARD == bindings::PageQueues_kIsolateQueueStandard);
pub const NUM_ISOLATE_QUEUES: usize = 2;
zr::static_assert!(NUM_ISOLATE_QUEUES == bindings::PageQueues_kNumIsolateQueues);

zr::static_assert!(ISOLATE_QUEUE_DONT_NEED < NUM_ISOLATE_QUEUES);
zr::static_assert!(ISOLATE_QUEUE_STANDARD < NUM_ISOLATE_QUEUES);
zr::static_assert!(ISOLATE_QUEUE_DONT_NEED < ISOLATE_QUEUE_STANDARD);

pub const DEFAULT_MIN_MRU_ROTATE_TIME: DurationMono = DurationMono::from_seconds(5);
zr::static_assert!(
    DEFAULT_MIN_MRU_ROTATE_TIME.into_nanos() == bindings::PageQueues_kDefaultMinMruRotateTime
);
pub const DEFAULT_MAX_MRU_ROTATE_TIME: DurationMono = DurationMono::from_seconds(5);
zr::static_assert!(
    DEFAULT_MAX_MRU_ROTATE_TIME.into_nanos() == bindings::PageQueues_kDefaultMaxMruRotateTime
);

/// This is presently an arbitrary constant, since the min and max mru rotate time are currently
/// fixed at the same value, meaning that the active ratio can not presently trigger, or
/// prevent, aging.
pub const DEFAULT_ACTIVE_RATIO_MULTIPLIER: u64 = 0;
zr::static_assert!(
    DEFAULT_ACTIVE_RATIO_MULTIPLIER == bindings::PageQueues_kDefaultActiveRatioMultiplier
);

/// Allocated pages that are part of the cow pages in a VmObjectPaged can be placed in a page queue.
/// The page queues provide a way to
///  * Classify and group pages across VMO boundaries
///  * Retrieve the VMO that a page is contained in (via a back reference stored in the vm_page_t)
///
/// Once a page has been placed in a page queue its queue_node becomes owned by the page queue and
/// must not be used until the page has been [`PageQueues::remove`]'d. It is not sufficient to call
/// list_delete on the queue_node yourself as this operation is not atomic and needs to be performed
/// whilst holding the [`PageQueues`] `list_lock`.
#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct PageQueues {
    /// The `list_lock` is used to protect the linked lists queues as these cannot be implemented
    /// with atomics. A few related members are also protected with this lock, such as the isolate
    /// cursor. The purpose of this separate spinlock, compared to the general `lock`, is so that
    /// latency sensitive operations, such as adding / removing pages, that only need to modify the
    /// list, can happen without false contention with other page queues operations.
    #[mutex]
    list_lock: KMutex<RawCriticalMutex>,

    /// General lock used to protect all logic and members that are not part of critical latency
    /// sensitive operations.
    /// Where both locks need to be acquired, `lock` must be acquired prior to the `list_lock`.
    #[mutex]
    lock: KMutex<RawCriticalMutex>,

    /// Externally supplied event that we should signal anytime aging occurs.
    #[guarded_by(lock)]
    aging_event: *mut Event,

    /// Records whether or not the active aging via the mru thread should be disabled or not.
    #[guarded_by(lock)]
    aging_disabled: bool,

    /// Time at which the `mru_gen` was last incremented.
    // TODO(https://fxbug.dev/549881466): This should be a InstantMono (which is just an i64), but
    // Rust does not yet support arbitrary atomic types like this, even when an #[repr(transparent)]
    last_age_time: AtomicI64,

    /// Reason the last aging event happened, this is purely for informational/debugging purposes.
    /// Initialized to Timeout as a somewhat arbitrary choice.
    #[guarded_by(lock)]
    last_age_reason: AgeReason,

    /// Tracks whether the active ratio has been tripped and should contribute as an aging trigger.
    /// This is stored as a boolean so that it is sticky in the advent of a race with additional
    /// modifications to the page queues. Were this not sticky then, in the absence of a debounce
    /// threshold, we could repeatedly trigger the active ratio on and off, causing the aging
    /// thread to repeatedly wake up, miss the trigger, and do nothing.
    #[guarded_by(lock)]
    active_ratio_triggered: bool,

    /// Used to signal the mru thread that it should wake up and check if the mru generation needs
    /// incrementing. This must be signaled if `active_ratio_triggered` transitions false->true or
    /// if the `lru_gen` is incremented. Over signalling is safe, just less efficient. Only the mru
    /// thread is permitted to wait on this.
    #[pin]
    mru_event: AutounsignalEvent,

    /// Used to signal the lru thread that it should wake up and check if the lru queue needs
    /// processing. This must be signaled if the `mru_gen` is modified such that the lru queue
    /// needs processing. Over signalling is safe, just less efficient. Only the lru thread is
    /// permitted to wait on this.
    #[pin]
    lru_event: AutounsignalEvent,

    /// What to do with pages when processing the LRU queue.
    #[guarded_by(lock)]
    lru_action: LruAction,

    /// The page queues are placed into an array, indexed by page queue, for consistency and
    /// uniformity of access. This does mean that the list for PageQueueNone does not actually have
    /// any pages in it, and should always be empty.
    /// The reclaimable queues are the more complicated as, unlike the other categories, pages can
    /// be in one of the queues, and can move around. The reclaimable queues themselves store pages
    /// that are roughly grouped by their last access time. The relationship is not precise as
    /// pages are not moved between queues unless it becomes strictly necessary. This is in
    /// contrast to the queue counts that are always up to date.
    ///
    /// What this means is that the `VmPage::page_queue` index is always up to do date, and the
    /// `page_queue_counts` represent an accurate count of pages with that `VmPage::page_queue`
    /// index, but counting the pages actually in the linked list may not yield the correct number.
    ///
    /// New reclaimable pages are always placed into the queue associated with the MRU generation.
    /// If they get accessed the `VmPage::page_queue` gets updated along with the counts. At some
    /// point the LRU queue will get processed and this will cause pages to get relocated to their
    /// correct list.
    ///
    /// Consider the following example:
    ///
    /// ```text
    ///  LRU  MRU            LRU  MRU            LRU   MRU            LRU   MRU        MRU  LRU
    ///    |  |                |  |                |     |              |     |            |  |
    ///    |  |    Insert A    |  |    Age         |     |  Touch A     |     |  Age       |  |
    ///    V  v    Queue=2     v  v    Queue=2     v     v  Queue=3     v     v  Queue=3   v  v
    /// [][ ][ ][] -------> [][ ][a][] -------> [][ ][a][ ] -------> [][ ][a][ ] -------> [ ][ ][a][]
    /// ```
    ///
    /// At this point page A, in its `VmPage`, has its queue marked as 3, and the
    /// `page_queue_counts` are {0,0,1,0}, but the page itself remains in the linked list for queue
    /// 2. If the LRU queue is then processed to increment it we would do.
    ///
    /// ```text
    ///  MRU  LRU             MRU  LRU            MRU    LRU
    ///    |  |                 |    |              |      |
    ///    |  |       Move LRU  |    |    Move LRU  |      |
    ///    V  v       Queue=3   v    v    Queue=3   v      v
    ///   [ ][ ][a][] -------> [ ][][a][] -------> [][ ][][a]
    /// ```
    ///
    /// In the second processing of the LRU queue it gets noticed that the page, based on
    /// `VmPage::page_queue`, is in the wrong queue and gets moved into the correct one.
    ///
    /// For specifics on how LRU and MRU generations map to LRU and MRU queues, see comments on
    /// `lru_gen` and `mru_gen`.
    #[guarded_by(list_lock)]
    #[pin]
    page_queues: [VmPageDoublyLinkedList; PageQueue::NUM_QUEUES.0 as usize],

    /// When a page is in the PageQueueReclaimIsolate state, instead of being in the `page_queues`
    /// list it is in, potentially one of several different, `isolate_queues` lists. This is just
    /// an implementation simplification as there's no need to 'save' the memory of the unused
    /// list_node_t in the other `page_queues` array. Note that PageQueueReclaimIsolate state is
    /// not a queue index, i.e. a page will have the PageQueueReclaimIsolate state irrespective
    /// of which `isolate_queues` list it is in.
    /// Pages in the PageQueueReclaimIsolate state are always exactly in the `isolate_queues` list,
    /// and similarly any page in the `isolate_queues` list is exactly in the
    /// PageQueueReclaimIsolate state. In this way the `isolate_queues` are considered reclaimable,
    /// and are part of active/inactive tracking, but do not support the
    /// [`PageQueues::mark_accessed`] fastpath.
    #[guarded_by(list_lock)]
    #[pin]
    isolate_queues: [VmPageDoublyLinkedList; NUM_ISOLATE_QUEUES],

    /// The generation counts are monotonic increasing counters and used to represent the effective
    /// age of the oldest and newest reclaimable queues. The page queues themselves are treated as
    /// a fixed size circular buffer that the generations map onto. This means all pages in the
    /// system have an age somewhere in `[lru_gen, mru_gen]` and so the lru and mru generations
    /// cannot drift apart by more than [`PageQueues::NUM_RECLAIM`], otherwise there would not
    /// be enough queues.
    /// A pages age being between `[lru_gen, mru_gen]` is not an invariant as
    /// [`PageQueues::mark_accessed`] can race and mark pages as being in an invalid queue. This
    /// race will get noticed when the LRU queue is processed and the page will get updated at that
    /// point to have a valid queue. Importantly, whilst pages can think they are in a queue that
    /// is invalid, only valid linked lists in the `page_queues` will ever have pages in them.
    /// This invariant is easy to enforce as the `page_queues` are updated under a lock.
    /// These are atomic so they can be safely read without the lock held, however they are always
    /// modified with the lock hold.
    lru_gen: AtomicU64,
    mru_gen: AtomicU64,

    /// Tracks the counts of pages in each queue in O(1) time complexity. As pages are moved
    /// between queues, the corresponding source and destination counts are decremented and
    /// incremented, respectively.
    ///
    /// The first entry of the array is left special: it logically represents pages not in any
    /// queue. For simplicity, it is initialized to zero rather than the total number of pages in
    /// the system. Consequently, the value of this entry is a negative number with absolute value
    /// equal to the total number of pages in all queues. This approach avoids unnecessary branches
    /// when updating counts.
    page_queue_counts: [AtomicUsize; PageQueue::NUM_QUEUES.0 as usize],

    /// Count for how many pages have moved queue without us recalculating the active ratio. This
    /// is a [`RelaxedAtomic`] to allow for completely skipping lock acquisition in
    /// [`PageQueues::mark_accessed`], except when the ratio actually needs to be recalculated.
    lazy_active_ratio_aging_skips: RelaxedAtomicU64,

    /// Tracks the number of consecutive LRU queue processing iterations that skipped sweeping due
    /// to active unloans.
    consecutive_skipped_sweeps: RelaxedAtomicU32,

    /// Track the mru and lru threads and have a signalling mechanism to shut them down.
    shutdown_threads: AtomicBool,
    #[guarded_by(lock)]
    mru_thread: *mut Thread,
    #[guarded_by(lock)]
    lru_thread: *mut Thread,

    /// Debug compressor is only available when debug asserts are also enabled. This ensures it can
    /// never have an impact on production builds.
    ///
    /// As the #[guarded] macro does not support #[cfg] members in #[guarded_by] annotates this is
    /// manually wrapped in an UnsafeCell and its access is controlled by list_lock.
    #[cfg(debug_assertions)]
    debug_compressor: UnsafeCell<Option<kalloc::Box<super::debug_compressor::DebugCompressor>>>,

    /// Queue rotation parameters. These are not locked as they are only read by the mru thread,
    /// and are set before the mru thread is started.
    min_mru_rotate_time: UnsafeCell<DurationMono>,
    max_mru_rotate_time: UnsafeCell<DurationMono>,

    /// Determines if anonymous zero page forks are placed in the zero fork queue or in the
    /// reclaimable queue.
    zero_fork_is_reclaimable: RelaxedAtomic<AtomicBool>,

    /// Determines if anonymous pages are placed in the reclaimable queues, or in their own non
    /// aging anonymous queues.
    anonymous_is_reclaimable: RelaxedAtomic<AtomicBool>,

    /// Current active ratio multiplier.
    #[guarded_by(lock)]
    active_ratio_multiplier: i64,

    phantom: PhantomData<PhantomPinned>,
}

// Compile-time layout assertions against the C++ PageQueues type via bindgen.
zr::static_assert!(
    core::mem::size_of::<PageQueues>() == core::mem::size_of::<bindings::PageQueues>()
);
zr::static_assert!(
    core::mem::align_of::<PageQueues>() == core::mem::align_of::<bindings::PageQueues>()
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, list_lock)
        == core::mem::offset_of!(bindings::PageQueues, list_lock_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, lock) == core::mem::offset_of!(bindings::PageQueues, lock_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, aging_event)
        == core::mem::offset_of!(bindings::PageQueues, aging_event_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, aging_disabled)
        == core::mem::offset_of!(bindings::PageQueues, aging_disabled_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, last_age_time)
        == core::mem::offset_of!(bindings::PageQueues, last_age_time_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, last_age_reason)
        == core::mem::offset_of!(bindings::PageQueues, last_age_reason_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, active_ratio_triggered)
        == core::mem::offset_of!(bindings::PageQueues, active_ratio_triggered_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, mru_event)
        == core::mem::offset_of!(bindings::PageQueues, mru_event_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, lru_event)
        == core::mem::offset_of!(bindings::PageQueues, lru_event_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, lru_action)
        == core::mem::offset_of!(bindings::PageQueues, lru_action_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, page_queues)
        == core::mem::offset_of!(bindings::PageQueues, page_queues_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, isolate_queues)
        == core::mem::offset_of!(bindings::PageQueues, isolate_queues_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, lru_gen)
        == core::mem::offset_of!(bindings::PageQueues, lru_gen_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, mru_gen)
        == core::mem::offset_of!(bindings::PageQueues, mru_gen_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, page_queue_counts)
        == core::mem::offset_of!(bindings::PageQueues, page_queue_counts_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, lazy_active_ratio_aging_skips)
        == core::mem::offset_of!(bindings::PageQueues, lazy_active_ratio_aging_skips_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, consecutive_skipped_sweeps)
        == core::mem::offset_of!(bindings::PageQueues, consecutive_skipped_sweeps_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, shutdown_threads)
        == core::mem::offset_of!(bindings::PageQueues, shutdown_threads_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, mru_thread)
        == core::mem::offset_of!(bindings::PageQueues, mru_thread_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, lru_thread)
        == core::mem::offset_of!(bindings::PageQueues, lru_thread_)
);
#[cfg(debug_assertions)]
zr::static_assert!(
    core::mem::offset_of!(PageQueues, debug_compressor)
        == core::mem::offset_of!(bindings::PageQueues, debug_compressor_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, min_mru_rotate_time)
        == core::mem::offset_of!(bindings::PageQueues, min_mru_rotate_time_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, max_mru_rotate_time)
        == core::mem::offset_of!(bindings::PageQueues, max_mru_rotate_time_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, zero_fork_is_reclaimable)
        == core::mem::offset_of!(bindings::PageQueues, zero_fork_is_reclaimable_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, anonymous_is_reclaimable)
        == core::mem::offset_of!(bindings::PageQueues, anonymous_is_reclaimable_)
);
zr::static_assert!(
    core::mem::offset_of!(PageQueues, active_ratio_multiplier)
        == core::mem::offset_of!(bindings::PageQueues, active_ratio_multiplier_)
);

// SAFETY: `PageQueues` is internally synchronized via its own `lock` and `list_lock`, and the
// pointers it holds only reference objects that outlive it.
unsafe impl Send for PageQueues {}
// SAFETY: `PageQueues` methods operate on shared references `&self` concurrently across threads,
// with all mutable state protected by its internal locks or held in atomics.
unsafe impl Sync for PageQueues {}

#[pin_init::pinned_drop]
impl PinnedDrop for PageQueues {
    fn drop(self: Pin<&mut Self>) {
        // The remainder of the C++ destructor, i.e. tearing down the locks, the events and the
        // queues themselves, is performed by the drop glue of the individual members.
        // SAFETY: `self` is a live, pinned `PageQueues` for the duration of this call.
        unsafe { bindings::cpp_page_queues_stop_threads(self.as_raw()) };

        // SAFETY: We hold an exclusive reference to `self` and, having just stopped the mru and lru
        // threads, there can be no other references to the queues, so the `list_lock` does not need
        // to be acquired to inspect them.
        unsafe {
            let token = LockToken::new();
            for queue in self.page_queues.get(&token) {
                debug_assert!(queue.is_empty());
            }
        }
        for (i, count) in self.page_queue_counts.iter().enumerate() {
            let count = count.load(Ordering::Relaxed);
            debug_assert_eq!(count, 0, "i={i} count={count}");
        }
    }
}

impl PageQueues {
    pub fn init() -> impl PinInit<Self, core::convert::Infallible> {
        zr::pin_init_ffi!(bindings::cpp_page_queues_init)
    }

    /// Domain-specific conversion: returns raw pointer for `PageQueues`.
    pub fn as_raw(&self) -> *mut bindings::PageQueues {
        (self as *const Self).cast_mut().cast()
    }

    // All Set operations places a page, which must not currently be in a page queue, into the
    // specified queue. The backlink information of |object| and |page_offset| must be specified and
    // valid. If the page is either removed from the referenced object, or moved to a different
    // offset, the backlink information must be updated either by calling ChangeObjectOffsetLocked,
    // or removing the page completely from the queues.

    /// Places `page` into the wired queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, but not yet assigned to a PageQueue.
    pub unsafe fn set_wired(&self, page: VmPagePtr, cow: &VmCowPages, offset: u64) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // not attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_set_wired(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            )
        }
    }

    /// Places `page` into the anonymous queue.
    ///
    /// `skip_reclaim` controls whether reclaiming the page should be forcibly skipped regardless
    /// of whether anonymous pages are considered reclaimable in general.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, but not yet assigned to a PageQueue.
    pub unsafe fn set_anonymous(
        &self,
        page: VmPagePtr,
        cow: &VmCowPages,
        offset: u64,
        skip_reclaim: bool,
    ) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // not attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_set_anonymous(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
                skip_reclaim,
            )
        }
    }

    /// Places `page` into the general reclaimable queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, but not yet assigned to a PageQueue.
    pub unsafe fn set_reclaim(&self, page: VmPagePtr, cow: &VmCowPages, offset: u64) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // not attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_set_reclaim(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            )
        }
    }

    /// Places `page` into the pager backed dirty queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, but not yet assigned to a PageQueue.
    pub unsafe fn set_pager_backed_dirty(&self, page: VmPagePtr, cow: &VmCowPages, offset: u64) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // not attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_set_pager_backed_dirty(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            )
        }
    }

    /// Places `page` into the anonymous zero fork queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, but not yet assigned to a PageQueue.
    pub unsafe fn set_anonymous_zero_fork(&self, page: VmPagePtr, cow: &VmCowPages, offset: u64) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // not attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_set_anonymous_zero_fork(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            )
        }
    }

    /// Places `page` into the high priority queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, but not yet assigned to a PageQueue.
    pub unsafe fn set_high_priority(&self, page: VmPagePtr, cow: &VmCowPages, offset: u64) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // not attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_set_high_priority(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            )
        }
    }

    // All Move operations change the queue that a page is considered to be in, but do not change
    // the object or offset backlink information. The page must currently be in a valid page
    // queue.

    /// Moves `page` to the wired queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_to_wired(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_move_to_wired(self.as_raw(), page.as_ffi()) }
    }

    /// Moves `page` to the anonymous queue.
    ///
    /// `skip_reclaim` controls whether reclaiming the page should be forcibly skipped regardless
    /// of whether anonymous pages are considered reclaimable in general.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_to_anonymous(&self, page: VmPagePtr, skip_reclaim: bool) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_move_to_anonymous(self.as_raw(), page.as_ffi(), skip_reclaim)
        }
    }

    /// Moves `page` to the general reclaimable queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_to_reclaim(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_move_to_reclaim(self.as_raw(), page.as_ffi()) }
    }

    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_to_reclaim_dont_need(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_move_to_reclaim_dont_need(self.as_raw(), page.as_ffi()) }
    }

    /// Moves `page` to the pager backed dirty queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_to_pager_backed_dirty(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_move_to_pager_backed_dirty(self.as_raw(), page.as_ffi())
        }
    }

    /// Moves `page` to the high priority queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_to_high_priority(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_move_to_high_priority(self.as_raw(), page.as_ffi()) }
    }

    /// If a page is presently in the anonymous (or reclaim queue, depending if anonymous pages are
    /// reclaimable) moves it to the appropriate anonymous zero fork queue (exact queue depends on
    /// whether zero forks are reclaimable or not). If the page is not in the anonymous queue then
    /// it is not modified.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn move_anonymous_to_anonymous_zero_fork(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_move_anonymous_to_anonymous_zero_fork(
                self.as_raw(),
                page.as_ffi(),
            )
        }
    }

    /// Indicates that page has failed a compression attempted, and moves it to a separate queue to
    /// prevent it from being considered part of the reclaim set, which makes it neither active nor
    /// inactive. The specified page must be in the page queues, but if not presently in a reclaim
    /// queue this method will do nothing.
    /// TODO(https://fxbug.dev/42138396): Determine whether/how pages are moved back into the
    /// reclaim pool and either further generalize this to support pager backed, or specialize
    /// FailedReclaim to be explicitly only anonymous.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO, and assigned to this PageQueue.
    pub unsafe fn compress_failed(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_compress_failed(self.as_raw(), page.as_ffi()) }
    }

    /// Changes the backlink information for a page and should only be called by the page owner
    /// under its lock (that is the VMO lock). The page must currently be in a valid page queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO (and that VMOs lock is held), and
    /// assigned to this PageQueue.
    pub unsafe fn change_object_offset(&self, page: VmPagePtr, cow: &VmCowPages, offset: u64) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_change_object_offset(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            )
        }
    }

    /// Changes the backlink information for an array of pages.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `pages` are owned by a VMO, assigned to this PageQueue, and that
    /// `pages` and `offsets` have matching lengths.
    pub unsafe fn change_object_offset_array(
        &self,
        pages: &[VmPagePtr],
        cow: &VmCowPages,
        offsets: &[u64],
    ) {
        assert_eq!(pages.len(), offsets.len());
        if pages.is_empty() {
            return;
        }
        // SAFETY: `self` is valid, `pages` and `offsets` have matching lengths, and the caller
        // guarantees preconditions on `pages`.
        unsafe {
            bindings::cpp_page_queues_change_object_offset_array(
                self.as_raw(),
                pages.as_ptr().cast_mut().cast(),
                cow.as_raw().cast(),
                offsets.as_ptr(),
                pages.len(),
            );
        }
    }

    /// Externally locked variant of `change_object_offset` that can be used for more efficient
    /// batch operations. In addition to the annotated lock, the VMO lock of the owner is also
    /// required to be held.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO (and its lock is held) and assigned to
    /// this PageQueue.`_token` proves the page queues `list_lock` is held.
    pub unsafe fn change_object_offset_locked_list(
        &self,
        _token: &LockToken<'_, PageQueuesListLockClass>,
        page: VmPagePtr,
        cow: &VmCowPages,
        offset: u64,
    ) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object and relevant locks are held.
        unsafe {
            bindings::cpp_page_queues_change_object_offset_locked_list(
                self.as_raw(),
                page.as_ffi(),
                cow.as_raw().cast(),
                offset,
            );
        }
    }

    /// Removes the page from any page list and returns ownership of the queue_node.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn remove(&self, page: VmPagePtr) {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_remove(self.as_raw(), page.as_ffi()) }
    }

    /// Batched version of `remove` that also places all the pages in the specified list.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `pages` are owned by a VMO and assigned to this PageQueue.
    pub unsafe fn remove_array_into_list(
        &self,
        pages: &[VmPagePtr],
        mut out_list: Pin<&mut VmPageDoublyLinkedList>,
    ) {
        if pages.is_empty() {
            return;
        }
        let list_ptr: *mut bindings::VmPageDoublyLinkedList =
            (unsafe { out_list.as_mut().get_unchecked_mut() } as *mut VmPageDoublyLinkedList)
                .cast();
        // SAFETY: `self.as_raw()` is valid, and caller guarantees `pages` are attached.
        unsafe {
            bindings::cpp_page_queues_remove_array_into_list(
                self.as_raw(),
                pages.as_ptr().cast_mut().cast(),
                pages.len(),
                list_ptr,
            );
        }
    }

    /// Tells the page queue this page has been accessed, and it should have its position in the
    /// queues updated.
    ///
    /// A page that is not in a reclaim queue is ignored, so this is safe to call
    /// for any page that is owned by a VMO.
    ///
    ///  # Safety
    ///
    /// The caller must guarantee that `page` is owned by a VMO for the duration of the operation.
    pub unsafe fn mark_accessed(&self, page: VmPagePtr) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer and `page`
        // is a valid page.
        unsafe { bindings::cpp_page_queues_mark_accessed(self.as_raw(), page.as_ffi()) }
    }

    /// Provides access to the underlying lock, allowing _Locked variants to be called. Use of this
    /// is highly discouraged as the underlying lock is a CriticalMutex which disables
    /// preemption. Preferably *Array variations should be used, but this provides a higher
    /// performance mechanism when needed.
    pub fn get_lock(&self) -> &KMutex<PageQueuesListLockClass, RawCriticalMutex> {
        let lock = unsafe { bindings::cpp_page_queues_get_lock(self.as_raw()) };
        // SAFETY: The returned lock pointer is `&self.list_lock_`, valid for the lifetime of
        // `self`.
        unsafe { &*lock.cast::<KMutex<PageQueuesListLockClass, RawCriticalMutex>>() }
    }

    /// Returns a string representation of the given `AgeReason`.
    pub fn string_from_age_reason(reason: AgeReason) -> &'static CStr {
        let ptr = unsafe { bindings::cpp_page_queues_string_from_age_reason(reason as i32) };
        // SAFETY: `cpp_page_queues_string_from_age_reason` returns a pointer to a static C string
        // literal.
        unsafe { CStr::from_ptr(ptr) }
    }

    /// Performs a manually requested aging event. This ignores usual aging triggers / restrictions
    /// and waits, if necessary, for aging to be possible and then performs it.
    /// Only for tests and debugging.
    pub fn rotate_reclaim_queues(&self) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_rotate_reclaim_queues(self.as_raw()) }
    }

    /// Moves a page from from the anonymous zero fork queue into the anonymous queue and returns
    /// the backlink information. If the zero fork queue is empty then a nullopt is returned,
    /// otherwise if it has_value the vmo field may be null to indicate that the vmo is running
    /// its destructor (see VmoBacklink for more details).
    pub fn pop_anonymous_zero_fork(&self) -> Option<VmoBacklink> {
        let mut out = MaybeUninit::<bindings::PageQueuesVmoBacklink>::uninit();
        // SAFETY: `self.as_raw()` is a valid pointer and `out` is valid for writing.
        let has_value = unsafe {
            bindings::cpp_page_queues_pop_anonymous_zero_fork(self.as_raw(), out.as_mut_ptr())
        };
        if has_value {
            let out = unsafe { out.assume_init() };
            let page = unsafe { VmPagePtr::from_ffi(out.page).expect("null page provided") };
            let cow = unsafe { VmCowPages::from_raw(out.cow.cast()) };
            Some(VmoBacklink { cow, page, offset: out.offset })
        } else {
            None
        }
    }

    /// Looks at the isolate queues and returns backlink information of the first page found. If the
    /// isolate queue is empty then LRU queues up to |lowest_queue| epochs from the most recent will
    /// be processed to attempt to fill the isolate list. If no page was found a nullopt is
    /// returned, otherwise if it has_value the vmo field may be null to indicate that the vmo
    /// is running its destructor (see VmoBacklink for more details). If a page is returned its
    /// location in the reclaim queue is not modified.
    pub fn peek_isolate(&self, lowest_queue: usize) -> Option<VmoBacklink> {
        let mut out = MaybeUninit::<bindings::PageQueuesVmoBacklink>::uninit();
        // SAFETY: `self.as_raw()` is a valid pointer and `out` is valid for writing.
        let has_value = unsafe {
            bindings::cpp_page_queues_peek_isolate(self.as_raw(), lowest_queue, out.as_mut_ptr())
        };
        if has_value {
            let out = unsafe { out.assume_init() };
            let page = unsafe { VmPagePtr::from_ffi(out.page).expect("null page provided") };
            let cow = unsafe { VmCowPages::from_raw(out.cow.cast()) };
            Some(VmoBacklink { cow, page, offset: out.offset })
        } else {
            None
        }
    }

    /// Can be called while the |page| is known to be in the loaned state. This method checks if it
    /// is in the page queues, and if so returns a reference to the cow pages that owns it.
    /// The page must be 'owned' by the caller, in so far as the page->state() is guaranteed to not
    /// be changing.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is valid and in the loaned state with state not changing.
    pub unsafe fn get_cow_for_loaned_page(&self, page: VmPagePtr) -> Option<VmoBacklink> {
        let mut out = MaybeUninit::<bindings::PageQueuesVmoBacklink>::uninit();
        // SAFETY: `self.as_raw()` is valid, and caller guarantees preconditions on `page`.
        let has_value = unsafe {
            bindings::cpp_page_queues_get_cow_for_loaned_page(
                self.as_raw(),
                page.as_ffi(),
                out.as_mut_ptr(),
            )
        };
        if has_value {
            let out = unsafe { out.assume_init() };
            let page = unsafe { VmPagePtr::from_ffi(out.page).expect("null page provided") };
            let cow = unsafe { VmCowPages::from_raw(out.cow.cast()) };
            Some(VmoBacklink { cow, page, offset: out.offset })
        } else {
            None
        }
    }

    /// Returns just the reclaim queue counts. Called from the zx_object_get_info() syscall.
    pub fn get_reclaim_queue_counts(&self) -> ReclaimCounts {
        let mut counts: MaybeUninit<ReclaimCounts> = MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` is valid and `counts` is valid for writing.
        unsafe {
            bindings::cpp_page_queues_get_reclaim_queue_counts(
                self.as_raw(),
                counts.as_mut_ptr().cast(),
            );
            counts.assume_init()
        }
    }

    /// Returns the counts of pages in the various queues.
    pub fn queue_counts(&self) -> Counts {
        let mut counts: MaybeUninit<Counts> = MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer, and `counts` is valid for
        // writing.
        unsafe {
            bindings::cpp_page_queues_queue_counts(self.as_raw(), counts.as_mut_ptr().cast());
        }
        // SAFETY: `cpp_page_queues_queue_counts` certainly wrote out `counts`.
        unsafe { counts.assume_init() }
    }

    /// Returns the counts of active and inactive pages.
    pub fn get_active_inactive_counts(&self) -> ActiveInactiveCounts {
        let mut counts: MaybeUninit<ActiveInactiveCounts> = MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` is valid and `counts` is valid for writing.
        unsafe {
            bindings::cpp_page_queues_get_active_inactive_counts(
                self.as_raw(),
                counts.as_mut_ptr().cast(),
            );
            counts.assume_init()
        }
    }

    /// Dumps debug information about the page queues.
    pub fn dump(&self) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_dump(self.as_raw()) }
    }

    /// Returns a global count of all pages compressed at the point of LRU change. This is a global
    /// method and will include stats from every PageQueues that has been instantiated.
    pub fn get_lru_pages_compressed() -> u64 {
        // SAFETY: C++ function is safe to call globally.
        unsafe { bindings::cpp_page_queues_get_lru_pages_compressed() }
    }

    /// Enables reclamation of anonymous pages by causing them to be placed into the reclaimable
    /// queue instead of the dedicated anonymous queue. The |zero_forks| parameter controls
    /// whether the anonymous zero forks should also go into the general reclaimable queue or
    /// not. Any pages already placed into the anonymous queues will be moved over, and there is
    /// no way to disable this once enabled.
    pub fn enable_anonymous_reclaim(&self, zero_forks: bool) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_enable_anonymous_reclaim(self.as_raw(), zero_forks) }
    }

    /// Returns whether or not the reclaim queues only include pager backed pages or not.
    pub fn reclaim_is_only_pager_backed(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_reclaim_is_only_pager_backed(self.as_raw()) }
    }

    /// Returns true if `page` is in an isolate queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn is_page_reclaimable(page: VmPagePtr) -> bool {
        // SAFETY: The caller guarantees `page` is attached to a VM object.
        unsafe { bindings::cpp_page_queues_is_page_reclaimable(page.as_ffi()) }
    }

    // These query functions are marked debug as it is generally a racy way to determine a pages
    // state and these are exposed for the purpose of writing tests or asserts against the
    // PageQueues.

    /// This takes an optional output parameter that, if the function returns true, will contain the
    /// index of the queue that the page was in.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_reclaim(&self, page: VmPagePtr) -> Option<usize> {
        let mut age = 0;
        // SAFETY: `self` and `&mut age` are valid for required accesses, and the caller
        // guarantees `page` is attached to a VM object per function safety preconditions.
        let is_reclaim = unsafe {
            bindings::cpp_page_queues_debug_page_is_reclaim(self.as_raw(), page.as_ffi(), &mut age)
        };
        if is_reclaim { Some(age) } else { None }
    }

    /// Returns whether `page` is in the reclaim isolate queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_reclaim_isolate(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_debug_page_is_reclaim_isolate(self.as_raw(), page.as_ffi())
        }
    }

    /// Returns whether `page` is in the pager backed dirty queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_pager_backed_dirty(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_debug_page_is_pager_backed_dirty(self.as_raw(), page.as_ffi())
        }
    }

    /// Returns whether `page` is in an anonymous queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_anonymous(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_debug_page_is_anonymous(self.as_raw(), page.as_ffi()) }
    }

    /// Returns whether `page` is in the anonymous zero fork queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_anonymous_zero_fork(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_debug_page_is_anonymous_zero_fork(
                self.as_raw(),
                page.as_ffi(),
            )
        }
    }

    /// Returns whether `page` is in any anonymous queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_any_anonymous(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_debug_page_is_any_anonymous(self.as_raw(), page.as_ffi())
        }
    }

    /// Returns whether `page` is in the wired queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_wired(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe { bindings::cpp_page_queues_debug_page_is_wired(self.as_raw(), page.as_ffi()) }
    }

    /// Returns whether `page` is in the high priority queue.
    ///
    /// # Safety
    ///
    /// The caller must guarantee `page` is owned by a VMO and assigned to this PageQueue.
    pub unsafe fn debug_page_is_high_priority(&self, page: VmPagePtr) -> bool {
        // SAFETY: `self` is valid for required accesses, and the caller guarantees `page` is
        // attached to a VM object per function safety preconditions.
        unsafe {
            bindings::cpp_page_queues_debug_page_is_high_priority(self.as_raw(), page.as_ffi())
        }
    }

    // These methods are public so that the scanner can call. Once the scanner is an object that can
    // be friended, and not a collection of anonymous functions, these can be made private.

    /// Creates any threads for queue management. This needs to be done separately to construction
    /// as there is a recursive dependency where creating threads will need to manipulate pages,
    /// which will call back into the page queues.
    /// Delaying thread creation is fine as these threads are purely for aging and eviction
    /// management, which is not needed during early kernel boot.
    /// Failure to start the threads may cause operations such as RotatePagerBackedQueues to block
    /// indefinitely as they might attempt to offload work to a nonexistent thread. This issue is
    /// only relevant for unittests that may wish to avoid starting the threads for some tests.
    /// It is the responsibility of the caller to only call this once, otherwise it will panic.
    pub fn start_threads(
        &self,
        min_mru_rotate_time: DurationMono,
        max_mru_rotate_time: DurationMono,
    ) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe {
            bindings::cpp_page_queues_start_threads(
                self.as_raw(),
                min_mru_rotate_time.into_nanos(),
                max_mru_rotate_time.into_nanos(),
            )
        }
    }

    /// Initializes and starts the debug compression, which attempts to immediately compress a
    /// random subset of pages added to the page queues. It is an error to call this if there is
    /// no compressor or if not running in debug mode.
    pub fn start_debug_compressor(&self) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_start_debug_compressor(self.as_raw()) }
    }

    /// Sets the active ratio multiplier.
    pub fn set_active_ratio_multiplier(&self, multiplier: u32) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_set_active_ratio_multiplier(self.as_raw(), multiplier) }
    }

    /// Sets the action to take when processing the LRU queue.
    pub fn set_lru_action(&self, action: LruAction) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_set_lru_action(self.as_raw(), action as i32) }
    }

    /// Disables the active aging system.
    /// Controls to enable and disable the active aging system. These must be called alternately and
    /// not in parallel. That is, it is an error to call DisableAging twice without calling
    /// EnableAging in between. Similar for EnableAging.
    pub fn disable_aging(&self) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_disable_aging(self.as_raw()) }
    }

    /// Enables the active aging system.
    /// Controls to enable and disable the active aging system. These must be called alternately and
    /// not in parallel. That is, it is an error to call DisableAging twice without calling
    /// EnableAging in between. Similar for EnableAging.
    pub fn enable_aging(&self) {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        unsafe { bindings::cpp_page_queues_enable_aging(self.as_raw()) }
    }

    /// Register an Event that will be signalled every time aging occurs. This can be used to know
    /// if if peek_reclaim might now return items (due to aging having occurred) where it had
    /// previously ceased.
    /// Only a single Event may be registered at a time and the Event is assumed to live as long as
    /// the PageQueues object. A None can be passed in to unregister an Event, otherwise it is
    /// an error to attempt to register over the top of an existing event.
    ///
    /// # Safety
    ///
    /// If an `Event` is provided, it is assumed to live as long as the `PageQueues` object.
    pub unsafe fn set_aging_event(&self, event: Option<NonNull<Event>>) {
        let event_ptr = event.map_or(core::ptr::null_mut(), |e| e.as_ptr()) as *mut bindings::Event;
        // SAFETY: `self.as_raw()` is valid, and caller guarantees event lifetime.
        unsafe { bindings::cpp_page_queues_set_aging_event(self.as_raw(), event_ptr) }
    }

    // Debug method to retrieve a reference to any lru thread. Intended for use
    // during tests / debugging and hence bypass the lock normally needed to read this member. It is
    // up to the caller to know if these objects are alive or not.
    pub fn debug_get_lru_thread(&self) -> Option<ThreadPtr> {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        let raw = unsafe { bindings::cpp_page_queues_debug_get_lru_thread(self.as_raw()) };
        // SAFETY: If non-null, `raw` points to a live kernel thread.
        unsafe { ThreadPtr::from_raw(raw.cast()) }
    }

    // Debug method to retrieve a reference to any mru thread. Intended for use
    // during tests / debugging and hence bypass the lock normally needed to read this member. It is
    // up to the caller to know if these objects are alive or not.
    pub fn debug_get_mru_thread(&self) -> Option<ThreadPtr> {
        // SAFETY: `self.as_raw()` returns a valid `PageQueues` pointer.
        let raw = unsafe { bindings::cpp_page_queues_debug_get_mru_thread(self.as_raw()) };
        // SAFETY: If non-null, `raw` points to a live kernel thread.
        unsafe { ThreadPtr::from_raw(raw.cast()) }
    }
}
