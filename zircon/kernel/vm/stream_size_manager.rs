// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::kernel::event::Event;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU64, Ordering};
use fbl::{
    DoublyLinkedList, DoublyLinkedListContainable, DoublyLinkedListNode, Recyclable, RefPtr,
    ref_counted,
};
use ksync::{KMutex, KMutexGuard, LockToken, PhantomMutex, RawMutex, guarded};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{ZX_OK, zx_status_t};

/// Tag for the write queue in `StreamSizeManager`.
struct WriteQueueTag;

/// Tag for the read queue in `StreamSizeManager`.
struct ReadQueueTag;

/// The type of operation being managed by `StreamSizeManager`.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationType {
    Write = 0,
    Read = 1,
    SetSize = 2,
    Append = 3,
}

/// `Operation` is a structure to ensure operations related to stream size are committed in order.
/// `Operation` is intended to be used as a stack-allocated structure.
///
/// Currently, an operation maps 1:1 with the thread it is executing on and thus, can be
/// considered owned by that thread.
///
/// Notes:
///  * The initialization, destruction, and immutable properties of this type are only
///    thread-compatible.
///  * The type must either be committed or cancelled before destruction. Otherwise, the destructor
///    will panic.
#[guarded]
#[repr(C)]
#[derive(DoublyLinkedListContainable)]
#[pin_data(PinnedDrop)]
pub struct Operation {
    #[dll_node(tag = WriteQueueTag)]
    write_node: DoublyLinkedListNode<Operation>,

    #[dll_node(tag = ReadQueueTag)]
    read_node: DoublyLinkedListNode<Operation>,

    #[pin]
    ready_event: Event,

    #[mutex(StreamSizeManagerLockClass)]
    phantom_lock: KMutex<PhantomMutex>,

    parent: *const StreamSizeManager,

    #[guarded_by(phantom_lock)]
    valid: bool,

    #[guarded_by(phantom_lock)]
    op_type: OperationType,

    /// Holds the target size. For appends, this will only be valid once the operation is at the
    /// head of the queue.
    #[guarded_by(phantom_lock)]
    size: u64,

    /// Tracks any stream size updated this operation performed. This is used to ensure that an
    /// operation is not cancelled or shrunk beyond any published stream size updates.
    #[cfg(debug_assertions)]
    committed_stream_size: AtomicU64,
}

impl Operation {
    /// Constructs a `PinInit` initializer for `Operation`.
    pub fn init<'a>(
        parent: &'a StreamSizeManager,
    ) -> impl PinInit<Self, core::convert::Infallible> + 'a {
        pin_init!(Self {
            write_node: DoublyLinkedListNode::new(),
            read_node: DoublyLinkedListNode::new(),
            ready_event <- Event::init_unsignaled(),
            phantom_lock: KMutex::new(PhantomMutex),
            parent: parent as *const StreamSizeManager,
            valid: false.into(),
            op_type: OperationType::Read.into(),
            size: 0.into(),
            #[cfg(debug_assertions)]
            committed_stream_size: AtomicU64::new(0),
        })
    }

    /// Returns a reference to the parent stream size manager's mutex lock.
    #[inline]
    pub fn lock(&self) -> &KMutex<StreamSizeManagerLockClass, RawMutex> {
        // SAFETY: `self.parent` points to a live, pinned `StreamSizeManager` that outlives this
        // operation because the operation holds a reference/borrow to its parent.
        unsafe { (*self.parent).lock() }
    }

    /// Returns a reference to the parent `StreamSizeManager` while holding the manager's lock.
    #[inline]
    fn parent_locked<'a>(
        &self,
        _token: &LockToken<'a, StreamSizeManagerLockClass>,
    ) -> &'a StreamSizeManager {
        // SAFETY: `self.parent` points to a live `StreamSizeManager` initialized during
        // `Operation::init`. The caller holds the manager's lock as proven by the lock token with
        // lifetime `'a`.
        unsafe { &*self.parent }
    }

    /// Indicates whether the operation is valid.
    #[inline]
    pub fn is_valid_locked(&self, token: &LockToken<'_, StreamSizeManagerLockClass>) -> bool {
        *self.guard_phantom_lock(token).valid()
    }

    /// Resets validity of the operation. This operation is private as it is assumed to only be
    /// called when it is correct to do so by the StreamSizeManager.
    #[inline]
    fn reset(&self, token: &mut LockToken<'_, StreamSizeManagerLockClass>) {
        *self.guard_phantom_lock_mut(token).valid_mut() = false;
    }

    fn initialize(
        &self,
        parent: *const StreamSizeManager,
        size: u64,
        op_type: OperationType,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
    ) {
        debug_assert!(!*self.guard_phantom_lock(token).valid());
        debug_assert!(core::ptr::eq(self.parent, parent));

        let mut guard = self.guard_phantom_lock_mut(token);
        *guard.valid_mut() = true;
        *guard.size_mut() = size;
        *guard.op_type_mut() = op_type;
    }

    /// Gets the operation's type while holding the parent lock.
    #[inline]
    pub fn get_type_locked(
        &self,
        token: &LockToken<'_, StreamSizeManagerLockClass>,
    ) -> OperationType {
        *self.guard_phantom_lock(token).op_type()
    }

    /// Gets the stream size that the operation will expand to once it is completed.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    pub fn get_size_locked(&self, token: &LockToken<'_, StreamSizeManagerLockClass>) -> u64 {
        let guard = self.guard_phantom_lock(token);
        debug_assert!(*guard.valid());
        // Reading the size for `Append` operations should only occur after it's been initialized.
        debug_assert!(*guard.op_type() != OperationType::Append || *guard.size() > 0);

        *guard.size()
    }

    /// Shrinks the size of the operation.
    ///
    /// Only size shrinks are allowed, since the concurrency of other operations are gated on the
    /// largest potential size of operations in front of it.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    ///  * The `new_size` passed in must be greater than 0.
    ///  * The `new_size` passed in must be less than or equal to the current size.
    ///  * This must only be called for `OperationType::Append` and `OperationType::Write` ops.
    pub fn shrink_size_locked(
        &self,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
        new_size: u64,
    ) {
        let mut guard = self.guard_phantom_lock_mut(token);
        debug_assert!(*guard.valid());
        // If `new_size` is 0, the operation should be cancelled instead.
        debug_assert!(new_size > 0);
        // This function may only be called on expanding stream write operations.
        assert!(
            *guard.op_type() == OperationType::Append || *guard.op_type() == OperationType::Write
        );
        assert!(new_size <= *guard.size());
        #[cfg(debug_assertions)]
        debug_assert!(new_size >= self.committed_stream_size.load(Ordering::Relaxed));

        *guard.size_mut() = new_size;
    }

    /// Commits the operation's effects on the stream size.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    pub fn commit_locked(&self, token: &mut LockToken<'_, StreamSizeManagerLockClass>) {
        debug_assert!(self.is_valid_locked(token));
        self.parent_locked(token).commit_and_dequeue_operation_locked(self, token);
    }

    /// Cancels the operation and does not commit any changes to the stream size.
    ///
    /// Note:
    ///  * This may only be called on a valid operation.
    ///  * This must only be called when holding the parent `StreamSizeManager` lock.
    pub fn cancel_locked(&self, token: &mut LockToken<'_, StreamSizeManagerLockClass>) {
        debug_assert!(self.is_valid_locked(token));
        #[cfg(debug_assertions)]
        debug_assert!(self.committed_stream_size.load(Ordering::Relaxed) == 0);
        self.parent_locked(token).dequeue_operation_locked(self, token);
    }

    #[cfg(debug_assertions)]
    fn debug_assert_valid_progress(&self, new_stream_size: u64) {
        // In the original C++ implementation, `Operation`'s fields (`valid_`, `type_`, `size_`)
        // were plain member variables without `TA_GUARDED` annotations, while thread-safety
        // annotations (`TA_REQ(lock())`) were placed only on mutating or querying methods.
        // `UpdateStreamSizeFromProgress` was intentionally unannotated because it executes on the
        // thread performing the streaming write without holding the `StreamSizeManager` lock.
        // Because `Operation` is stack-allocated on and owned 1:1 by the executing thread, that
        // thread could safely inspect its own `valid_`, `type_`, and `size_` without a lock.
        //
        // In Rust, these fields are wrapped in `KCell` guarded by `phantom_lock` so that other
        // threads traversing queues in `StreamSizeManager` can only inspect them while holding the
        // lock. On the operation's owning thread, no concurrent writes to `valid`, `op_type`, or
        // `size` can occur during streaming I/O. Therefore, we can construct a temporary
        // `LockToken` using `unsafe { LockToken::new() }` solely to inspect these fields for debug
        // assertions.
        //
        // SAFETY: `update_stream_size_from_progress` is only called by the thread that owns this
        // stack-allocated `Operation`. No other thread modifies `valid`, `op_type`, or `size`, and
        // the constructed `LockToken` does not escape this function scope.
        let token = unsafe { LockToken::<StreamSizeManagerLockClass>::new() };
        let guard = self.guard_phantom_lock(&token);
        debug_assert!(*guard.valid());
        let op_type = *guard.op_type();
        debug_assert!(op_type == OperationType::Write || op_type == OperationType::Append);
        debug_assert!(new_stream_size <= *guard.size());
    }

    /// Updates the stream size when progress is made from the operation. Once this has been called
    /// it is invalid to call `cancel_locked` or to call `shrink_size_locked` with a size less than
    /// the stream size provided here.
    ///
    /// Note:
    ///  * This may only be called on a valid `Append` or `Write` operation.
    ///  * The stream size must be larger than the current stream size.
    pub fn update_stream_size_from_progress(&self, new_stream_size: u64) {
        // SAFETY: `self.parent` points to a live `StreamSizeManager` instance. Atomic updates
        // to `stream_size` handle internal thread synchronization.
        let parent = unsafe { &*self.parent };
        #[cfg(debug_assertions)]
        self.debug_assert_valid_progress(new_stream_size);
        debug_assert!(new_stream_size > parent.get_stream_size());
        #[cfg(debug_assertions)]
        self.committed_stream_size.store(new_stream_size, Ordering::Relaxed);
        parent.set_stream_size(new_stream_size);
    }
}

#[pinned_drop]
impl PinnedDrop for Operation {
    fn drop(self: Pin<&mut Self>) {
        let me = unsafe { self.get_unchecked_mut() };
        debug_assert!(
            !*me.valid.get_inner_mut(),
            "Operation destructed without cancelling or committing!"
        );
    }
}

/// `StreamSizeManager` is a struct that helps coordinate multiple, potentially concurrent changes
/// to a VMO's stream size without needing to serialize the I/O of those operations. This is done by
/// maintaining queues of outstanding operations, allowing concurrent execution of the operations,
/// and then committing the stream size effects of those operations in a particular order. This idea
/// is similar to the re-order buffer in Tomasulo's algorithm.
///
/// There are 2 ordering queues: the read queue and the write queue. Both queues hold their
/// respective namesake operations as well as shrink operations.
///
/// Read operations are permitted to read up to the smallest outstanding stream size, which can be
/// found as the minimum of the current stream size and all shrink operations. Upon completion,
/// reads will always commit without blocking behind other operations.
///
/// Write operations may extend stream size. Upon completion, a write will block until it is the
/// head of the write queue if the smallest outstanding stream size is less than its target size.
///
/// Set size operations are treated differently, depending on whether the operation will expand or
/// shrink the stream size. When expanding, set size ops are treated as write operations of the same
/// target size (see above). When shrinking, set size ops are treated as shrink operations and will
/// block until it is the head if any read or write operations that operate beyond the target size
/// are queued in front of the set size.
#[guarded]
#[ref_counted]
#[derive(Recyclable)]
#[pin_data]
#[repr(C)]
pub struct StreamSizeManager {
    #[pin]
    #[mutex]
    lock: KMutex<RawMutex>,

    // These queues are usually very shallow, unless stream clients call many operations
    // concurrently.
    #[pin]
    #[guarded_by(lock)]
    write_q: DoublyLinkedList<*mut Operation, WriteQueueTag>,

    #[pin]
    #[guarded_by(lock)]
    read_q: DoublyLinkedList<*mut Operation, ReadQueueTag>,

    // `stream_size` is not guarded by a lock because the queues above maintain that only one
    // operation can ever be mutating `stream_size` at any given point.
    //
    // Accessing this value should be done via `get_stream_size` and `set_stream_size`.
    stream_size: AtomicU64,
}

impl StreamSizeManager {
    /// Create a `StreamSizeManager` with its initial stream size set to `stream_size`.
    pub fn create(stream_size: u64) -> Result<RefPtr<Self>, Status> {
        fbl::pin_make_ref_counted!(Self {
            lock <- KMutex::init(),
            write_q <- ksync::KCell::pin_init(DoublyLinkedList::new()),
            read_q <- ksync::KCell::pin_init(DoublyLinkedList::new()),
            stream_size: AtomicU64::new(stream_size),
        })
        .map_err(|_: kalloc::AllocError| Status::NO_MEMORY)
    }

    /// Returns a reference to the stream size manager's mutex lock.
    #[inline]
    pub fn lock(&self) -> &KMutex<StreamSizeManagerLockClass, RawMutex> {
        &self.lock
    }

    /// Returns the current stream size.
    pub fn get_stream_size(&self) -> u64 {
        // Loads from the operation the stream size must be ordered with the acquire ordering to
        // ensure that all memory operations from the VMO (i.e. reads) after the load are not
        // reordered before reading the stream size. Otherwise, reads from the VMO before acquiring
        // stream size may not see data that was written to the VMO just before stream size was
        // updated (via `SetStreamSize`).
        self.stream_size.load(Ordering::Acquire)
    }

    /// Updates the stream size to a new value.
    ///
    /// Note that this function should only be called by internal functions, as stream size should
    /// only be modified by one operation at a time. This is enforced by the queues.
    fn set_stream_size(&self, new_stream_size: u64) {
        // Stores to the stream size must be ordered with release ordering to ensure that all memory
        // operations (i.e. writes) to the VMO are visible *before* updating stream size. Readers
        // must see valid data in the VMO if the region being read is within stream size. See
        // `GetStreamSize` as well.
        self.stream_size.store(new_stream_size, Ordering::Release);
    }

    fn push_write_queue_locked(
        &self,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
        op: &Operation,
    ) {
        let mut guard = self.guard_lock_mut(token);
        // SAFETY: `op` is pinned for the duration of the operation and inserted into `write_q`.
        unsafe {
            guard
                .write_q_mut()
                .get_unchecked_mut()
                .push_back_raw(core::ptr::from_ref(op).cast_mut());
        }
    }

    fn push_read_queue_locked(
        &self,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
        op: &Operation,
    ) {
        let mut guard = self.guard_lock_mut(token);
        // SAFETY: `op` is pinned for the duration of the operation and inserted into `read_q`.
        unsafe {
            guard
                .read_q_mut()
                .get_unchecked_mut()
                .push_back_raw(core::ptr::from_ref(op).cast_mut());
        }
    }

    fn dequeue_write_queue_locked(
        &self,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
        op: &Operation,
    ) {
        let mut guard = self.guard_lock_mut(token);
        // SAFETY: `op` is in `write_q`.
        unsafe {
            Self::dequeue_from_list::<WriteQueueTag>(guard.write_q_mut().get_unchecked_mut(), op);
        }
    }

    fn dequeue_read_queue_locked(
        &self,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
        op: &Operation,
    ) {
        let mut guard = self.guard_lock_mut(token);
        // SAFETY: `op` is in `read_q`.
        unsafe {
            Self::dequeue_from_list::<ReadQueueTag>(guard.read_q_mut().get_unchecked_mut(), op);
        }
    }

    /// Marks and registers the beginning of a read operation.
    ///
    /// Returns the maximum size of the stream that should be read.
    pub fn begin_read_locked(
        &self,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
        target_size: u64,
        op: &Operation,
    ) -> u64 {
        // Allow reads up to the smallest outstanding size.
        // Other concurrent, in-flight operations may or may not complete before this read, so it
        // is okay to be more conservative here and only read up to the guaranteed valid region.
        let mut stream_size_limit = self.get_stream_size();
        {
            let guard = self.guard_lock(token);
            for item in guard.read_q().iter() {
                if item.get_type_locked(token) == OperationType::SetSize {
                    stream_size_limit =
                        core::cmp::min(item.get_size_locked(token), stream_size_limit);
                }
            }
        }
        stream_size_limit = core::cmp::min(target_size, stream_size_limit);

        op.initialize(self, stream_size_limit, OperationType::Read, token);
        self.push_read_queue_locked(token, op);

        stream_size_limit
    }

    /// Marks and registers the beginning of a write operation.
    ///
    /// If the write results in an expansion of the stream size, returns the previous stream size
    /// from which the write expands. The gap from the previous stream size to where the write
    /// begins likely needs to be zeroed out.
    ///
    /// Notes:
    ///  * This function may block until other conflicting operations complete.
    ///  * This function may drop and reacquire the lock guarded by `guard`.
    pub fn begin_write_locked(
        &self,
        guard: &mut Pin<&mut KMutexGuard<'_, StreamSizeManagerLockClass, RawMutex>>,
        target_size: u64,
        op: &Operation,
    ) -> Option<u64> {
        let mut prev_stream_size = None;

        op.initialize(self, target_size, OperationType::Write, guard.as_mut().token_mut());
        self.push_write_queue_locked(guard.as_mut().token_mut(), op);

        // Check if there are any set size operations in front of this that sets the stream size
        // smaller than `target_size`.
        let mut block_due_to_set = false;
        for item in fbl::ReverseIterator::<*mut Operation, WriteQueueTag>::from_element(op).skip(1)
        {
            if item.get_type_locked(guard.token()) == OperationType::SetSize
                && item.get_size_locked(guard.token()) < target_size
            {
                block_due_to_set = true;
                break;
            }
        }

        // If this write can potentially create a scenario where it expands stream, block until it
        // is the head of the queue.
        if block_due_to_set || target_size > self.get_stream_size() {
            self.block_until_head_locked(guard, op);

            // Must re-read the stream size here, since `block_until_head_locked` would have dropped
            // the lock, and stream size may have been modified by the operations in front of this
            // one.
            let cur_stream_size = self.get_stream_size();
            if target_size > cur_stream_size {
                prev_stream_size = Some(cur_stream_size);
            }
        }

        prev_stream_size
    }

    /// Marks and registers the beginning of an append operation.
    ///
    /// Notes:
    ///  * This function may block until other conflicting operations complete.
    ///  * This function may drop and reacquire the lock guarded by `guard`.
    ///  * `append_size` must be greater than 0.
    pub fn begin_append_locked(
        &self,
        guard: &mut Pin<&mut KMutexGuard<'_, StreamSizeManagerLockClass, RawMutex>>,
        append_size: usize,
        op: &Operation,
    ) -> Result<(), Status> {
        debug_assert!(append_size > 0);

        // Temporarily initialize an `Append` operation with zero size until the size is known.
        op.initialize(self, 0, OperationType::Append, guard.as_mut().token_mut());
        self.push_write_queue_locked(guard.as_mut().token_mut(), op);

        // Block until head if there are any of the following operations preceding this one:
        //   * Appends or writes that exceed the current stream size.
        //   * Set size
        //
        // Effectively, this checks for any stream size modifying operations.
        let mut should_block = false;

        // It's okay to read the stream size once here, since the lock is held. This means that
        // stream size can only be increased if the front-most stream size modifying operation is
        // an expanding write or append. Not re-reading stream size and seeing a potentially smaller
        // stream size here is valid, since it will only pessimize (i.e. blocking until head) this
        // operation for a very small number of cases within an extremely narrow timing window.
        // There are no correctness issues with pessimization. Since the pessimizing case is so
        // rare, prefer reading once over continuously re-reading the atomic in a loop.
        let mut cur_stream_size = self.get_stream_size();

        for item in fbl::ReverseIterator::<*mut Operation, WriteQueueTag>::from_element(op).skip(1)
        {
            let item_op_type = item.get_type_locked(guard.token());
            if item_op_type == OperationType::SetSize
                || item_op_type == OperationType::Append
                || (item_op_type == OperationType::Write
                    && item.get_size_locked(guard.token()) > cur_stream_size)
            {
                should_block = true;
                break;
            }
        }

        if should_block {
            self.block_until_head_locked(guard, op);

            // Must re-read the stream size here, since `block_until_head_locked` would have dropped
            // the lock, and stream size may have been modified by the operations in front of this
            // one.
            cur_stream_size = self.get_stream_size();
        }

        // Using the previously read stream size. In the case where this operation blocked until
        // it was the head, stream size was re-read with the lock held and with this operation at
        // the head of the queue (no other stream size mutating operations can proceed before this).
        // In all other cases, the previous loop verified that no stream size mutating operations
        // are in front of this operation.
        let Some(new_size) = cur_stream_size.checked_add(append_size as u64) else {
            // Dequeue operation since this change should not be committed.
            self.dequeue_operation_locked(op, guard.as_mut().token_mut());
            return Err(Status::OUT_OF_RANGE);
        };
        *op.guard_phantom_lock_mut(guard.as_mut().token_mut()).size_mut() = new_size;

        Ok(())
    }

    /// Marks and registers the beginning of an operation to set the stream size to a target size.
    ///
    /// Note that this function may drop and reacquire the lock guarded by `guard`.
    pub fn begin_set_stream_size_locked(
        &self,
        guard: &mut Pin<&mut KMutexGuard<'_, StreamSizeManagerLockClass, RawMutex>>,
        target_size: u64,
        op: &Operation,
    ) {
        op.initialize(self, target_size, OperationType::SetSize, guard.as_mut().token_mut());
        self.push_write_queue_locked(guard.as_mut().token_mut(), op);
        self.push_read_queue_locked(guard.as_mut().token_mut(), op);

        // Block until head if there are any of the following operations preceding this one:
        //   * Appends or writes that exceed either the current stream size or the target size.
        //      - If it exceeds the current stream size, the overlap is in the region in which the
        //        set size will zero content and the write will commit data.
        //      - If it exceeds the target size, the overlap is in the region in which the set size
        //        will invalidate pages/data and the write will commit data.
        //   * Reads that are reading at or beyond target size.
        //   * Set size
        let mut should_block = false;

        // It's okay to read the stream size once here, since the lock is held. This means that
        // stream size can only be increased if the front-most stream size modifying operation is
        // an expanding write or append. Not re-reading stream size and seeing a potentially smaller
        // stream size here is valid, since it will only pessimize (i.e. blocking until head) this
        // operation for a very small number of cases within an extremely narrow timing window.
        // There are no correctness issues with pessimization. Since the pessimizing case is so
        // rare, prefer reading once over continuously re-reading the atomic in a loop.
        let cur_stream_size = self.get_stream_size();

        for item in fbl::ReverseIterator::<*mut Operation, WriteQueueTag>::from_element(op).skip(1)
        {
            let item_op_type = item.get_type_locked(guard.token());
            if item_op_type == OperationType::SetSize
                || item_op_type == OperationType::Append
                || (item_op_type == OperationType::Write
                    && item.get_size_locked(guard.token())
                        > core::cmp::min(cur_stream_size, target_size))
            {
                should_block = true;
                break;
            }
        }

        if !should_block {
            for item in
                fbl::ReverseIterator::<*mut Operation, ReadQueueTag>::from_element(op).skip(1)
            {
                if item.get_type_locked(guard.token()) == OperationType::Read
                    && item.get_size_locked(guard.token()) > target_size
                {
                    should_block = true;
                    break;
                }
            }
        }

        if should_block {
            self.block_until_head_locked(guard, op);
        }
    }

    /// Blocks until the provided operation is at the head of the queue.
    ///
    /// Note that this function will drop the lock guarded by `guard` while blocking and
    /// reacquires the lock after.
    fn block_until_head_locked(
        &self,
        guard: &mut Pin<&mut KMutexGuard<'_, StreamSizeManagerLockClass, RawMutex>>,
        op: &Operation,
    ) {
        debug_assert!(core::ptr::eq(op.parent, self));

        if op.write_node.in_container() {
            while op.is_valid_locked(guard.token()) {
                let guard_lock = self.guard_lock(guard.token());
                let is_front =
                    guard_lock.write_q().front().map(|f| core::ptr::eq(f, op)).unwrap_or(false);
                if is_front {
                    break;
                }
                guard.as_mut().call_unlocked(|| {
                    let _ = op.ready_event.wait_infinite();
                });
            }
        }

        if op.read_node.in_container() {
            while op.is_valid_locked(guard.token()) {
                let guard_lock = self.guard_lock(guard.token());
                let is_front =
                    guard_lock.read_q().front().map(|f| core::ptr::eq(f, op)).unwrap_or(false);
                if is_front {
                    break;
                }
                guard.as_mut().call_unlocked(|| {
                    let _ = op.ready_event.wait_infinite();
                });
            }
        }
    }

    fn commit_and_dequeue_operation_locked(
        &self,
        op: &Operation,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
    ) {
        if !op.is_valid_locked(token) {
            debug_assert!(!op.write_node.in_container());
            debug_assert!(!op.read_node.in_container());
            return;
        }

        match op.get_type_locked(token) {
            OperationType::Write => {
                let token_ref = &*token;
                self.set_stream_size(core::cmp::max(
                    op.get_size_locked(token_ref),
                    self.get_stream_size(),
                ));
            }
            OperationType::Append | OperationType::SetSize => {
                let token_ref = &*token;
                self.set_stream_size(op.get_size_locked(token_ref));
            }
            OperationType::Read => {
                // No-op
            }
        }

        self.dequeue_operation_locked(op, token);
        debug_assert!(!op.write_node.in_container());
        debug_assert!(!op.read_node.in_container());
    }

    /// Dequeues an `Operation`. This must only be called internally, once an `Operation` is
    /// committed or cancelled.
    fn dequeue_operation_locked(
        &self,
        op: &Operation,
        token: &mut LockToken<'_, StreamSizeManagerLockClass>,
    ) {
        debug_assert!(op.is_valid_locked(token));
        debug_assert!(core::ptr::eq(op.parent, self));

        match op.get_type_locked(token) {
            OperationType::Write | OperationType::Append => {
                debug_assert!(op.write_node.in_container());
                self.dequeue_write_queue_locked(token, op);
            }
            OperationType::Read => {
                debug_assert!(op.read_node.in_container());
                self.dequeue_read_queue_locked(token, op);
            }
            OperationType::SetSize => {
                debug_assert!(op.read_node.in_container());
                debug_assert!(op.write_node.in_container());
                self.dequeue_write_queue_locked(token, op);
                self.dequeue_read_queue_locked(token, op);
            }
        }

        // Just in case, signal the ready event of `op` in case another thread is blocking on it.
        //
        // Note that this should never usually occur, since only the owning thread of the operation
        // should be blocking or dequeueing.
        op.ready_event.signal();
        op.reset(token);
        debug_assert!(!op.write_node.in_container());
        debug_assert!(!op.read_node.in_container());
    }

    fn dequeue_from_list<Tag>(list: &mut DoublyLinkedList<*mut Operation, Tag>, op: &Operation)
    where
        Operation: DoublyLinkedListContainable<Operation, Tag>,
    {
        let is_head = list.front().map(|f| core::ptr::eq(f, op)).unwrap_or(false);
        let next_op_ptr: Option<*const Operation> =
            fbl::ForwardIterator::<*mut Operation, Tag>::from_element(op)
                .nth(1)
                .map(|next| next as *const Operation);

        // SAFETY: `op` is guaranteed to be contained within `list`.
        unsafe {
            let _ = list.erase(op);
        }

        // If the current operation is now at the head of the list, signal to the next operation
        // that it should wake to complete its task after this one finishes dequeueing.
        if is_head && let Some(next_ptr) = next_op_ptr {
            // SAFETY: `next_ptr` was in `list` and is now the new head.
            unsafe {
                (*next_ptr).ready_event.signal();
            }
        }
    }
}

// C FFI exports for C++ callers

/// Creates a new `StreamSizeManager` via C FFI.
///
/// # Safety
///
/// `out_ptr` must be a valid, writable pointer to receive the raw `StreamSizeManager` pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_stream_size_manager_create(
    stream_size: u64,
    out_ptr: *mut *mut StreamSizeManager,
) -> zx_status_t {
    match StreamSizeManager::create(stream_size) {
        Ok(mgr) => {
            // SAFETY: Caller guarantees `out_ptr` is valid and writable.
            unsafe {
                *out_ptr = RefPtr::into_raw(mgr).cast_mut();
            }
            ZX_OK
        }
        Err(status) => status.into_raw(),
    }
}

/// Recycles a `StreamSizeManager` raw pointer when its reference count drops to zero.
///
/// # Safety
///
/// `stream_size_manager` must be a non-null, valid pointer to a `StreamSizeManager` previously
/// created by Rust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_stream_size_manager_recycle(
    stream_size_manager: *mut StreamSizeManager,
) {
    // SAFETY: Caller guarantees `stream_size_manager` is a valid pointer whose refcount reached
    // zero.
    unsafe {
        StreamSizeManager::recycle(NonNull::new_unchecked(stream_size_manager));
    }
}

/// Returns the current stream size of `StreamSizeManager`.
///
/// # Safety
///
/// `stream_size_manager` must point to a live `StreamSizeManager` instance.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rust_stream_size_manager_get_stream_size(
    stream_size_manager: *const StreamSizeManager,
) -> u64 {
    // SAFETY: `stream_size_manager` is valid and non-null.
    unsafe { (*stream_size_manager).get_stream_size() }
}

#[cfg(ktest)]
/// Unit tests for `StreamSizeManager`.
#[unittest::suite(name = "stream_size_manager")]
mod tests {
    use super::{Operation, StreamSizeManager};
    use unittest::{assert_ok, expect_eq, expect_false, expect_true, unwrap_ok};

    /// Tests creating a StreamSizeManager and querying its initial size.
    #[test]
    fn create_and_get_size() {
        let ssm = unwrap_ok!(StreamSizeManager::create(1024));
        expect_eq!(ssm.get_stream_size(), 1024);
    }

    /// Tests begin_read_locked and committing a read operation.
    #[test]
    fn read_operation() {
        let ssm = unwrap_ok!(StreamSizeManager::create(2048));
        pin_init::stack_pin_init!(let op = Operation::init(&ssm));

        ksync::lock!(let mut guard = ssm.lock().lock());
        let limit = ssm.begin_read_locked(guard.as_mut().token_mut(), 4096, &op);
        expect_eq!(limit, 2048);
        expect_true!(op.is_valid_locked(guard.token()));
        expect_eq!(op.get_size_locked(guard.token()), 2048);

        op.commit_locked(guard.as_mut().token_mut());
        expect_false!(op.is_valid_locked(guard.token()));
        expect_eq!(ssm.get_stream_size(), 2048);
    }

    /// Tests begin_write_locked, shrinking the operation size, and committing.
    #[test]
    fn write_and_shrink_operation() {
        let ssm = unwrap_ok!(StreamSizeManager::create(100));
        pin_init::stack_pin_init!(let op = Operation::init(&ssm));

        ksync::lock!(let mut guard = ssm.lock().lock());
        let prev = ssm.begin_write_locked(&mut guard, 500, &op);
        expect_true!(prev == Some(100));
        expect_true!(op.is_valid_locked(guard.token()));
        expect_eq!(op.get_size_locked(guard.token()), 500);

        op.shrink_size_locked(guard.as_mut().token_mut(), 300);
        expect_eq!(op.get_size_locked(guard.token()), 300);

        op.commit_locked(guard.as_mut().token_mut());
        expect_false!(op.is_valid_locked(guard.token()));
        expect_eq!(ssm.get_stream_size(), 300);
    }

    /// Tests cancelling a write operation.
    #[test]
    fn write_cancel_operation() {
        let ssm = unwrap_ok!(StreamSizeManager::create(100));
        pin_init::stack_pin_init!(let op = Operation::init(&ssm));

        ksync::lock!(let mut guard = ssm.lock().lock());
        let _prev = ssm.begin_write_locked(&mut guard, 500, &op);
        expect_true!(op.is_valid_locked(guard.token()));

        op.cancel_locked(guard.as_mut().token_mut());
        expect_false!(op.is_valid_locked(guard.token()));
        expect_eq!(ssm.get_stream_size(), 100);
    }

    /// Tests begin_append_locked and committing an append operation.
    #[test]
    fn append_operation() {
        let ssm = unwrap_ok!(StreamSizeManager::create(100));
        pin_init::stack_pin_init!(let op = Operation::init(&ssm));

        ksync::lock!(let mut guard = ssm.lock().lock());
        assert_ok!(ssm.begin_append_locked(&mut guard, 50, &op));
        expect_true!(op.is_valid_locked(guard.token()));
        expect_eq!(op.get_size_locked(guard.token()), 150);

        op.commit_locked(guard.as_mut().token_mut());
        expect_false!(op.is_valid_locked(guard.token()));
        expect_eq!(ssm.get_stream_size(), 150);
    }

    /// Tests begin_set_stream_size_locked and committing a set size operation.
    #[test]
    fn set_stream_size_operation() {
        let ssm = unwrap_ok!(StreamSizeManager::create(100));
        pin_init::stack_pin_init!(let op = Operation::init(&ssm));

        ksync::lock!(let mut guard = ssm.lock().lock());
        ssm.begin_set_stream_size_locked(&mut guard, 50, &op);
        expect_true!(op.is_valid_locked(guard.token()));
        expect_eq!(op.get_size_locked(guard.token()), 50);

        op.commit_locked(guard.as_mut().token_mut());
        expect_false!(op.is_valid_locked(guard.token()));
        expect_eq!(ssm.get_stream_size(), 50);
    }

    /// Tests updating stream size as progress is made during a write operation.
    #[test]
    fn update_progress() {
        let ssm = unwrap_ok!(StreamSizeManager::create(100));
        pin_init::stack_pin_init!(let op = Operation::init(&ssm));

        ksync::lock!(let mut guard = ssm.lock().lock());
        let _prev = ssm.begin_write_locked(&mut guard, 500, &op);

        op.update_stream_size_from_progress(250);
        expect_eq!(ssm.get_stream_size(), 250);

        op.commit_locked(guard.as_mut().token_mut());
        expect_eq!(ssm.get_stream_size(), 500);
    }
}
