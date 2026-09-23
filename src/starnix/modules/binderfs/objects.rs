// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::process::BinderProcess;
use crate::shared_memory::TransactionBuffers;
use crate::thread::{BinderThread, Command};
use bitflags::bitflags;
use starnix_core::vfs::FdNumber;
use starnix_logging::{log_error, log_trace, track_stub};
use starnix_sync::{BinderObjectLevel, LockDepGuard, LockDepMutex};
use starnix_types::ownership::{DropGuard, Releasable, WeakRef};
use starnix_uapi::arc_key::ArcKey;
use starnix_uapi::errors::{Errno, errno, error};
use starnix_uapi::uapi::{binder_transaction_data__bindgen_ty_2__bindgen_ty_1, pid_t};
use starnix_uapi::union::struct_with_union_into_bytes;
use starnix_uapi::user_address::UserAddress;
use starnix_uapi::{
    BINDER_TYPE_BINDER, BINDER_TYPE_FD, BINDER_TYPE_FDA, BINDER_TYPE_HANDLE, BINDER_TYPE_PTR,
    binder_buffer_object, binder_fd_array_object, binder_fd_object, binder_object_header,
    binder_transaction_data, binder_uintptr_t, flat_binder_object, uapi,
};
use std::collections::{BTreeSet, VecDeque};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use zerocopy::{FromBytes, IntoBytes};

/// A reference to a binder object in another process.
#[derive(Debug)]
pub struct BinderObjectRef {
    /// The associated BinderObject
    pub binder_object: Arc<BinderObject>,
    /// The number of strong references to this `BinderObjectRef`.
    strong_count: usize,
    /// If not None, the guard representing the current guard reference that this handles has on the
    /// `BinderObject` because its own guard count is strictly positive.
    strong_guard: Option<StrongRefGuard>,
    /// The number of weak references to this `BinderObjectRef`.
    weak_count: usize,
    /// If not None, the guard representing the current weak reference that this handles has on the
    /// `BinderObject` because its own weak count is strictly positive.
    weak_guard: Option<WeakRefGuard>,
}

/// Assert that a dropped reference do not have any reference left, as this would keep an object
/// owned by another process alive.
#[cfg(any(test, debug_assertions))]
impl Drop for BinderObjectRef {
    fn drop(&mut self) {
        assert!(!self.has_ref());
    }
}

impl BinderObjectRef {
    /// Build a new reference to the given object. The reference will start with a strong count of
    /// 1.
    pub fn new(guard: StrongRefGuard) -> Self {
        let binder_object = guard.binder_object.clone();
        Self {
            binder_object,
            strong_count: 1,
            strong_guard: Some(guard),
            weak_count: 0,
            weak_guard: None,
        }
    }

    /// Returns whether this object has still any strong or weak reference. The object must be kept
    /// in the handle table until this is false.
    pub fn has_ref(&self) -> bool {
        self.weak_count > 0 || self.strong_count > 0
    }

    /// Free any reference held on `binder_object` by this reference.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn clean_refs(mut self, actions: &mut RefCountActions) {
        if let Some(guard) = self.strong_guard.take() {
            debug_assert!(
                self.strong_count > 0,
                "The strong count must be strictly positive when the strong guard is not None"
            );
            guard.release(actions);
            self.strong_count = 0;
        }
        if let Some(guard) = self.weak_guard.take() {
            debug_assert!(
                self.weak_count > 0,
                "The weak count must be strictly positive when the weak guard is not None"
            );
            guard.release(actions);
            self.weak_count = 0;
        }
    }

    /// Increments the strong reference count of the binder object reference.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn inc_strong(&mut self, _actions: &mut RefCountActions) -> Result<(), Errno> {
        assert!(self.has_ref());
        if self.strong_count == 0 {
            let guard = self.binder_object.inc_strong_checked()?;
            self.strong_guard = Some(guard);
        }
        self.strong_count += 1;
        Ok(())
    }

    /// Increments the strong reference count of the binder object reference while holding a strong
    /// reference guard on the object.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn inc_strong_with_guard(&mut self, guard: StrongRefGuard, actions: &mut RefCountActions) {
        assert!(self.has_ref());
        if self.strong_count == 0 {
            self.strong_guard = Some(guard);
        } else {
            guard.release(actions);
        }
        self.strong_count += 1;
    }

    /// Increments the weak reference count of the binder object reference.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn inc_weak(&mut self, actions: &mut RefCountActions) {
        assert!(self.has_ref());
        if self.weak_count == 0 {
            let guard = self.binder_object.inc_weak(actions);
            self.weak_guard = Some(guard);
        }
        self.weak_count += 1;
    }

    /// Decrements the strong reference count of the binder object reference.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn dec_strong(&mut self, actions: &mut RefCountActions) -> Result<(), Errno> {
        if self.strong_count == 0 {
            return error!(EINVAL);
        }
        if self.strong_count == 1 {
            let Some(guard) = self.strong_guard.take() else {
                panic!("No guard while strong count is 1");
            };
            guard.release(actions);
        }
        self.strong_count -= 1;
        Ok(())
    }

    /// Decrements the weak reference count of the binder object reference.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn dec_weak(&mut self, actions: &mut RefCountActions) -> Result<(), Errno> {
        if self.weak_count == 0 {
            return error!(EINVAL);
        }
        if self.weak_count == 1 {
            let Some(guard) = self.weak_guard.take() else {
                panic!("No guard while strong count is 1");
            };
            guard.release(actions);
        }
        self.weak_count -= 1;
        Ok(())
    }
}

/// A set of `BinderObject` whose reference counts may have changed. Releasing it will enqueue all
/// the corresponding actions and remove any freed object from the owner process.
#[derive(Default)]
pub struct RefCountActions {
    objects: BTreeSet<ArcKey<BinderObject>>,
    drop_guard: DropGuard,
}

impl Deref for RefCountActions {
    type Target = BTreeSet<ArcKey<BinderObject>>;

    fn deref(&self) -> &Self::Target {
        &self.objects
    }
}

impl DerefMut for RefCountActions {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.objects
    }
}

impl Releasable for RefCountActions {
    type Context<'a> = ();

    fn release<'a>(self, _context: ()) {
        for object in self.objects.into_iter() {
            object.apply_deferred_refcounts();
        }
        self.drop_guard.disarm();
    }
}

impl RefCountActions {
    #[cfg(test)]
    pub fn default_released() -> Self {
        let r = RefCountActions::default();
        r.drop_guard.disarm();
        r
    }
}

/// The state of a given reference count for an object (strong or weak).
///
/// The client only has an eventually-consistent view of the reference count, because 1) the actual
/// reference count is maintained by the Binder driver and 2) the increment (BR_ACQUIRE/BR_INCREFS)
/// and decrement (BR_RELEASE/BR_DECREFS) commands to inform the clients of variations are
/// asynchronously delivered through multiple queues. Furthermore, clients may not process them in
/// order, as they usually handle the received commands in a thread pool.
///
/// In order to guarantee that a decrease is always processed by the client after the corresponding
/// increase (otherwise the client may incorrectly free the object), the Binder protocol mandates
/// that increase commands (BR_ACQUIRE/BR_INCREFS) must be acknowledged
/// (BC_ACQUIRE_DONE/BC_INCREFS_DONE) and, only then, the corresponding decrease
/// (BR_RELEASE/BR_DECREFS) can be enqueued.
///
/// As an optimization, we only report actionable transitions to the client, i.e. from 0 to 1 and
/// from 1 to 0.
///
/// The three states in this enum correspond to the three possible client states from the driver's
/// point of view, and the parameter is the actual reference count maintained by the Binder driver.
/// Note that there is no "transitioning from 1 to 0" state, because it is safe to pretend that
/// enqueued decreases take effect instantly.
///
/// Because of lock order constraints, it may not be possible for users of this class to enqueue
/// increase/decrease commands at the same time as the corresponding manipulations to the reference
/// count. In order to support this scenario, this class supports "deferred" operations, which are
/// equivalent to the "immediate" ones but they are divided in two steps: a first step that simply
/// records the updated reference count, and a second step which determines what commands should be
/// enqueued to propagate the new reference count to the client.
#[derive(Debug, PartialEq, Eq)]
enum ObjectReferenceCount {
    /// The count as known to the client is zero (this is both the initial state and the state after
    /// a decrease from one to zero).
    NoRef(usize),
    /// The client is transitioning from zero to one (i.e. it has been sent an increase to its
    /// reference count, but didn't yet acknowledge it).
    WaitingAck(usize),
    /// The count as known to the client is one (i.e. the client did notify that it took into
    /// account an increase of the reference count, and has not been sent a decrease since).
    HasRef(usize),
}

impl Default for ObjectReferenceCount {
    fn default() -> ObjectReferenceCount {
        ObjectReferenceCount::NoRef(0)
    }
}

impl ObjectReferenceCount {
    /// Returns true if the client's view of the reference count is either one or transitioning to
    /// one.
    fn has_ref(&self) -> bool {
        match self {
            Self::NoRef(_) => false,
            Self::WaitingAck(_) | Self::HasRef(_) => true,
        }
    }

    /// Returns the actual reference count.
    fn count(&self) -> usize {
        let (Self::NoRef(x) | Self::WaitingAck(x) | Self::HasRef(x)) = self;
        *x
    }

    /// Increments the reference count of the object and applies it immediately to the client state.
    ///
    /// If it returns true, the caller *MUST* enqueue an increment command.
    #[must_use]
    fn inc_immediate(&mut self) -> bool {
        self.inc_deferred();
        self.apply_deferred_inc()
    }

    /// Increments the reference count of the object.
    fn inc_deferred(&mut self) {
        let (Self::NoRef(x) | Self::WaitingAck(x) | Self::HasRef(x)) = self;
        *x += 1;
    }

    /// Decrements the reference count of the object.
    fn dec_deferred(&mut self) {
        let (Self::NoRef(x) | Self::WaitingAck(x) | Self::HasRef(x)) = self;
        if *x == 0 {
            panic!("dec called with no reference");
        } else {
            *x -= 1;
        }
    }

    /// Applies any deferred increment to the client state.
    ///
    /// If it returns true, the caller *MUST* enqueue an increment command.
    #[must_use]
    fn apply_deferred_inc(&mut self) -> bool {
        match *self {
            ObjectReferenceCount::NoRef(n) if n > 0 => {
                *self = ObjectReferenceCount::WaitingAck(n);
                true
            }
            _ => false,
        }
    }

    /// Applies any deferred decrement to the client state.
    ///
    /// If it returns true, the caller *MUST* enqueue a decrement command.
    #[must_use]
    fn apply_deferred_dec(&mut self) -> bool {
        if *self == ObjectReferenceCount::HasRef(0) {
            *self = ObjectReferenceCount::NoRef(0);
            true
        } else {
            false
        }
    }

    /// Acknowledge a client ack for a reference count increase.
    fn ack(&mut self) -> Result<(), Errno> {
        match self {
            Self::WaitingAck(x) => {
                *self = Self::HasRef(*x);
                Ok(())
            }
            _ => error!(EINVAL),
        }
    }

    /// Returns whether this reference count is waiting an acknowledgement for an increase.
    fn is_waiting_ack(&self) -> bool {
        matches!(self, ObjectReferenceCount::WaitingAck(_))
    }
}

/// Mutable state of a [`BinderObject`], mainly for handling the ordering guarantees of oneway
/// transactions.
#[derive(Debug, Default)]
pub struct BinderObjectMutableState {
    /// Command queue for oneway transactions on this binder object. Oneway transactions are
    /// guaranteed to be dispatched in the order they are submitted to the driver, and one at a
    /// time.
    pub oneway_transactions: VecDeque<(TransactionData, fuchsia_trace::Id)>,
    /// Whether a binder thread is currently handling a oneway transaction. This will get cleared
    /// when there are no more transactions in the `oneway_transactions` and a binder thread freed
    /// the buffer associated with the last oneway transaction.
    pub handling_oneway_transaction: bool,

    /// The weak reference count of this object.
    weak_count: ObjectReferenceCount,

    /// The strong reference count of this object.
    strong_count: ObjectReferenceCount,
}

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct BinderObjectFlags: u32 {
        /// Not implemented.
        const ACCEPTS_FDS = uapi::flat_binder_object_flags_FLAT_BINDER_FLAG_ACCEPTS_FDS;
        /// Whether the binder transaction receiver wants access to the sender selinux context.
        const TXN_SECURITY_CTX = uapi::flat_binder_object_flags_FLAT_BINDER_FLAG_TXN_SECURITY_CTX;
        /// Whether the binder transaction receiver inherit the scheduler policy of the caller.
        const INHERIT_RT = uapi::flat_binder_object_flags_FLAT_BINDER_FLAG_INHERIT_RT;

        /// Not implemented
        const PRIORITY_MASK = uapi::flat_binder_object_flags_FLAT_BINDER_FLAG_PRIORITY_MASK;
        /// Not implemented
        const SCHED_POLICY_MASK = uapi::flat_binder_object_flags_FLAT_BINDER_FLAG_SCHED_POLICY_MASK;
    }
}
impl BinderObjectFlags {
    pub fn parse(value: u32) -> Result<Self, Errno> {
        Self::from_bits(value).ok_or_else(|| {
            log_error!("Unknown flag value for object: {:#}", value);
            errno!(EINVAL)
        })
    }
}

/// A binder object, which is owned by a process. Process-local unique memory addresses identify it
/// to the owner.
#[derive(Debug)]
pub struct BinderObject {
    /// The owner of the binder object. If the owner cannot be promoted to a strong reference,
    /// the object is dead.
    pub owner: WeakRef<BinderProcess>,
    /// The addresses to the binder (weak and strong) in the owner's address space. These are
    /// treated as opaque identifiers in the driver, and only have meaning to the owning process.
    pub local: LocalBinderObject,
    /// The flags for the binder object.
    pub flags: BinderObjectFlags,
    /// Mutable state for the binder object, protected behind a mutex.
    state: LockDepMutex<BinderObjectMutableState, BinderObjectLevel>,
}

/// Assert that a dropped object from a live process has no reference.
#[cfg(any(test, debug_assertions))]
impl Drop for BinderObject {
    fn drop(&mut self) {
        if self.owner.upgrade().is_some() {
            assert!(!self.has_ref());
        }
    }
}

/// Trait used to configure the RefGuard.
pub trait RefReleaser: std::fmt::Debug {
    /// Decrement the relevant ref count for this releaser.
    fn dec_ref(binder_object: &Arc<BinderObject>, actions: &mut RefCountActions);
}

/// The releaser for a strong reference.
#[derive(Debug)]
pub struct StrongRefReleaser {}

impl RefReleaser for StrongRefReleaser {
    /// Decrements the strong reference count of the binder object.
    fn dec_ref(binder_object: &Arc<BinderObject>, actions: &mut RefCountActions) {
        binder_object.lock().strong_count.dec_deferred();
        actions.insert(ArcKey(binder_object.clone()));
    }
}

/// The releaser for a weak reference.
#[derive(Debug)]
pub struct WeakRefReleaser {}

impl RefReleaser for WeakRefReleaser {
    /// Decrements the weak reference count of the binder object.
    fn dec_ref(binder_object: &Arc<BinderObject>, actions: &mut RefCountActions) {
        binder_object.lock().weak_count.dec_deferred();
        actions.insert(ArcKey(binder_object.clone()));
    }
}

/// The guard for a given `RefReleaser`. It is wrapped in a ReleaseGuard to ensure its lifecycle
/// constraints are handled correctly.
#[derive(Debug)]
pub struct RefGuardInner<R: RefReleaser> {
    pub binder_object: Arc<BinderObject>,
    phantom: std::marker::PhantomData<R>,
}

impl<R: RefReleaser> Releasable for RefGuardInner<R> {
    type Context<'a> = &'a mut RefCountActions;

    fn release<'a>(self, context: &mut RefCountActions) {
        R::dec_ref(&self.binder_object, context);
    }
}

/// A guard for the specified reference type. The guard must be released exactly once before going
/// out of scope to release the reference it represents.
#[derive(Debug)]
pub struct RefGuard<R: RefReleaser>(RefGuardInner<R>);

impl<R: RefReleaser> Deref for RefGuard<R> {
    type Target = RefGuardInner<R>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<R: RefReleaser> DerefMut for RefGuard<R> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<R: RefReleaser> RefGuard<R> {
    fn new(binder_object: Arc<BinderObject>) -> Self {
        RefGuard(RefGuardInner::<R> { binder_object, phantom: Default::default() })
    }
}

impl<R: RefReleaser> Releasable for RefGuard<R> {
    type Context<'a> = &'a mut RefCountActions;

    fn release<'a>(self, context: &mut RefCountActions) {
        self.0.release(context);
    }
}

/// Type alias for the specific guards for strong references.
pub type StrongRefGuard = RefGuard<StrongRefReleaser>;
/// Type alias for the specific guards for weak references.
type WeakRefGuard = RefGuard<WeakRefReleaser>;

impl BinderObject {
    /// Creates a new BinderObject. It is the responsibility of the caller to sent a `BR_ACQUIRE`
    /// to the owning process.
    pub fn new(
        owner: &BinderProcess,
        local: LocalBinderObject,
        flags: BinderObjectFlags,
    ) -> (Arc<Self>, StrongRefGuard) {
        log_trace!(
            "New binder object {:?} in process {:?} with flags {:?}",
            local,
            owner.identifier,
            flags
        );
        let object = Arc::new(Self {
            owner: owner.weak_self.clone(),
            local,
            flags,
            state: BinderObjectMutableState {
                strong_count: ObjectReferenceCount::WaitingAck(1),
                ..Default::default()
            }
            .into(),
        });
        let guard = StrongRefGuard::new(object.clone());
        (object, guard)
    }

    pub fn new_context_manager_marker(
        context_manager: &BinderProcess,
        flags: BinderObjectFlags,
    ) -> Arc<Self> {
        Arc::new(Self {
            owner: context_manager.weak_self.clone(),
            local: Default::default(),
            flags,
            state: Default::default(),
        })
    }

    /// Locks the mutable state of the binder object for exclusive access.
    pub fn lock(&self) -> LockDepGuard<'_, BinderObjectMutableState> {
        self.state.lock()
    }

    /// Returns whether the object has any reference, or is waiting for an acknowledgement from the
    /// owning process. The object cannot be removed from the object table has long as this is
    /// true.
    #[cfg(any(test, debug_assertions))]
    fn has_ref(&self) -> bool {
        let state = self.lock();
        state.weak_count.has_ref() || state.strong_count.has_ref()
    }

    /// Increments the strong reference count of the binder object. Allows to raise the strong
    /// count from 0 to 1.
    pub fn inc_strong_unchecked(self: &Arc<Self>, binder_thread: &BinderThread) -> StrongRefGuard {
        let mut state = self.lock();
        if state.strong_count.inc_immediate() {
            binder_thread.lock().enqueue_command(Command::AcquireRef(self.local).into());
        }
        StrongRefGuard::new(Arc::clone(self))
    }

    /// Increments the strong reference count of the binder object. Fails is the current strong
    /// count is 0.
    pub fn inc_strong_checked(self: &Arc<Self>) -> Result<StrongRefGuard, Errno> {
        let mut state = self.lock();
        if state.strong_count.count() == 0 {
            return error!(EINVAL);
        }
        assert!(
            !state.strong_count.inc_immediate(),
            "tried to resurrect an object that had no strong references in its owner"
        );
        Ok(StrongRefGuard::new(Arc::clone(self)))
    }

    /// Increments the weak reference count of the binder object.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn inc_weak(self: &Arc<Self>, actions: &mut RefCountActions) -> WeakRefGuard {
        self.lock().weak_count.inc_deferred();
        actions.insert(ArcKey(self.clone()));
        WeakRefGuard::new(self.clone())
    }

    /// Acknowledge the BC_ACQUIRE_DONE command received from the object owner.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn ack_acquire(self: &Arc<Self>, actions: &mut RefCountActions) -> Result<(), Errno> {
        self.lock().strong_count.ack()?;
        actions.insert(ArcKey(self.clone()));
        Ok(())
    }

    /// Acknowledge the BC_INCREFS_DONE command received from the object owner.
    /// Takes a `RefCountActions` that must be `release`d without holding a lock on
    /// `BinderProcess`.
    pub fn ack_incref(self: &Arc<Self>, actions: &mut RefCountActions) -> Result<(), Errno> {
        self.lock().weak_count.ack()?;
        actions.insert(ArcKey(self.clone()));
        Ok(())
    }

    pub fn apply_deferred_refcounts(self: &Arc<Self>) {
        let Some(process) = self.owner.upgrade() else {
            return;
        };

        let mut commands = Vec::new();

        {
            let mut process_state = process.lock();
            let mut object_state = self.lock();

            // Enqueue increase actions.
            assert!(
                !object_state.strong_count.apply_deferred_inc(),
                "The strong refcount is never incremented deferredly"
            );
            if object_state.weak_count.apply_deferred_inc() {
                commands.push(Command::IncRef(self.local));
            }
            // No decrease actions are enqueued while waiting for any acknowledgement.
            let mut did_decrease = false;
            if !object_state.strong_count.is_waiting_ack()
                && !object_state.weak_count.is_waiting_ack()
            {
                if object_state.strong_count.apply_deferred_dec() {
                    commands.push(Command::ReleaseRef(self.local));
                    did_decrease = true;
                }
                if object_state.weak_count.apply_deferred_dec() {
                    commands.push(Command::DecRef(self.local));
                    did_decrease = true;
                }
            }

            // Forget this object if we have just remove the last reference to it.
            if did_decrease
                && !object_state.strong_count.has_ref()
                && !object_state.weak_count.has_ref()
            {
                let removed = process_state.objects.remove(&self.local.weak_ref_addr);
                assert_eq!(
                    removed.as_ref().map(Arc::as_ptr),
                    Some(Arc::as_ptr(self)),
                    "Did not remove the expected BinderObject"
                );
            }
        }

        for command in commands {
            process.enqueue_command(command.into());
        }
    }
}

/// A binder object.
/// All addresses are in the owning process' address space.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct LocalBinderObject {
    /// Address to the weak ref-count structure. This uniquely identifies a binder object within
    /// a process. Guaranteed to exist.
    pub weak_ref_addr: UserAddress,
    /// Address to the strong ref-count structure (actual object). May not exist if the object was
    /// destroyed.
    pub strong_ref_addr: UserAddress,
}

/// Non-union version of [`binder_transaction_data`].
#[derive(Debug, PartialEq, Eq)]
pub struct TransactionData {
    pub peer_pid: pid_t,
    pub peer_tid: pid_t,
    pub peer_euid: u32,

    pub object: FlatBinderObject,
    pub code: u32,
    pub flags: u32,

    pub buffers: TransactionBuffers,
}

impl TransactionData {
    pub fn as_bytes(&self) -> [u8; std::mem::size_of::<binder_transaction_data>()] {
        match self.object {
            FlatBinderObject::Remote { handle } => {
                struct_with_union_into_bytes!(binder_transaction_data {
                    target.handle: handle.into(),
                    cookie: 0,
                    code: self.code,
                    flags: self.flags,
                    sender_pid: self.peer_pid,
                    sender_euid: self.peer_euid,
                    data_size: self.buffers.data.length as u64,
                    offsets_size: self.buffers.offsets.length as u64,
                    data.ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                         buffer: self.buffers.data.address.ptr() as u64,
                         offsets: self.buffers.offsets.address.ptr() as u64,
                     },
                })
            }
            FlatBinderObject::Local { ref object } => {
                struct_with_union_into_bytes!(binder_transaction_data {
                    target.ptr: object.weak_ref_addr.ptr() as u64,
                    cookie: object.strong_ref_addr.ptr() as u64,
                    code: self.code,
                    flags: self.flags,
                    sender_pid: self.peer_pid,
                    sender_euid: self.peer_euid,
                    data_size: self.buffers.data.length as u64,
                    offsets_size: self.buffers.offsets.length as u64,
                    data.ptr: binder_transaction_data__bindgen_ty_2__bindgen_ty_1 {
                         buffer: self.buffers.data.address.ptr() as u64,
                         offsets: self.buffers.offsets.address.ptr() as u64,
                     },
                })
            }
        }
    }
}

/// Non-union version of [`flat_binder_object`].
#[derive(Debug, PartialEq, Eq)]
pub enum FlatBinderObject {
    Local { object: LocalBinderObject },
    Remote { handle: Handle },
}

/// A handle to a binder object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    /// Special handle 0 to an object representing the process which has become the context manager.
    /// Processes may rendezvous at this handle to perform service discovery.
    ContextManager,
    /// A handle to a binder object in another process.
    Object {
        /// The index of the binder object in a process' handle table.
        /// This is `handle - 1`, because the special handle 0 is reserved.
        index: usize,
    },
}

impl Handle {
    pub const fn from_raw(handle: u32) -> Handle {
        if handle == 0 {
            Handle::ContextManager
        } else {
            Handle::Object { index: handle as usize - 1 }
        }
    }

    /// Returns the underlying object index the handle represents, panicking if the handle was the
    /// special `0` handle.
    pub fn object_index(&self) -> usize {
        match self {
            Handle::ContextManager => {
                panic!("handle does not have an object index")
            }
            Handle::Object { index } => *index,
        }
    }

    pub fn is_handle_0(&self) -> bool {
        match self {
            Handle::ContextManager => true,
            Handle::Object { .. } => false,
        }
    }
}

impl From<u32> for Handle {
    fn from(handle: u32) -> Self {
        Handle::from_raw(handle)
    }
}

impl From<Handle> for u32 {
    fn from(handle: Handle) -> Self {
        match handle {
            Handle::ContextManager => 0,
            Handle::Object { index } => (index as u32) + 1,
        }
    }
}

impl std::fmt::Display for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Handle::ContextManager => f.write_str("0"),
            Handle::Object { index } => f.write_fmt(format_args!("{}", index + 1)),
        }
    }
}

/// Represents a serialized binder object embedded in transaction data.
#[derive(Debug, PartialEq, Eq)]
pub enum SerializedBinderObject {
    /// A `BINDER_TYPE_HANDLE` object. A handle to a remote binder object.
    Handle { handle: Handle, flags: BinderObjectFlags, cookie: binder_uintptr_t },
    /// A `BINDER_TYPE_BINDER` object. The in-process representation of a binder object.
    Object { local: LocalBinderObject, flags: BinderObjectFlags },
    /// A `BINDER_TYPE_FD` object. A file descriptor.
    File { fd: FdNumber, cookie: binder_uintptr_t },
    /// A `BINDER_TYPE_PTR` object. Identifies a pointer in the transaction data that needs to be
    /// fixed up when the payload is copied into the destination process. Part of the scatter-gather
    /// implementation.
    Buffer { buffer: UserAddress, length: usize, parent: usize, parent_offset: usize, flags: u32 },
    /// A `BINDER_TYPE_FDA` object. Identifies an array of file descriptors in a parent buffer that
    /// must be duped into the receiver's file descriptor table.
    FileArray { num_fds: usize, parent: usize, parent_offset: usize },
}

impl SerializedBinderObject {
    /// Deserialize a binder object from `data`. `data` must be large enough to fit the size of the
    /// serialized object, or else this method fails.
    pub fn from_bytes(data: &[u8]) -> Result<Self, Errno> {
        let (object_header, _) =
            binder_object_header::read_from_prefix(data).map_err(|_| errno!(EINVAL))?;
        match object_header.type_ {
            BINDER_TYPE_BINDER => {
                let (object, _) =
                    flat_binder_object::read_from_prefix(data).map_err(|_| errno!(EINVAL))?;
                Ok(Self::Object {
                    local: LocalBinderObject {
                        // SAFETY: Union read.
                        weak_ref_addr: UserAddress::from(unsafe { object.__bindgen_anon_1.binder }),
                        strong_ref_addr: UserAddress::from(object.cookie),
                    },
                    flags: BinderObjectFlags::parse(object.flags)?,
                })
            }
            BINDER_TYPE_HANDLE => {
                let (object, _) =
                    flat_binder_object::read_from_prefix(data).map_err(|_| errno!(EINVAL))?;
                Ok(Self::Handle {
                    // SAFETY: Union read.
                    handle: unsafe { object.__bindgen_anon_1.handle }.into(),
                    flags: BinderObjectFlags::parse(object.flags)?,
                    cookie: object.cookie,
                })
            }
            BINDER_TYPE_FD => {
                let (object, _) =
                    binder_fd_object::read_from_prefix(data).map_err(|_| errno!(EINVAL))?;
                Ok(Self::File {
                    // SAFETY: Union read.
                    fd: FdNumber::from_raw(unsafe { object.__bindgen_anon_1.fd } as i32),
                    cookie: object.cookie,
                })
            }
            BINDER_TYPE_PTR => {
                let (object, _) =
                    binder_buffer_object::read_from_prefix(data).map_err(|_| errno!(EINVAL))?;
                Ok(Self::Buffer {
                    buffer: UserAddress::from(object.buffer),
                    length: object.length as usize,
                    parent: object.parent as usize,
                    parent_offset: object.parent_offset as usize,
                    flags: object.flags,
                })
            }
            BINDER_TYPE_FDA => {
                let (object, _) =
                    binder_fd_array_object::read_from_prefix(data).map_err(|_| errno!(EINVAL))?;
                Ok(Self::FileArray {
                    num_fds: object.num_fds as usize,
                    parent: object.parent as usize,
                    parent_offset: object.parent_offset as usize,
                })
            }
            object_type => {
                track_stub!(
                    TODO("https://fxbug.dev/322873261"),
                    "binder unknown object type",
                    object_type
                );
                error!(EINVAL)
            }
        }
    }

    /// Writes the serialized object back to `data`. `data` must be large enough to fit the
    /// serialized object, or else this method fails.
    pub fn write_to(self, data: &mut [u8]) -> Result<(), Errno> {
        match self {
            SerializedBinderObject::Handle { handle, flags, cookie } => {
                struct_with_union_into_bytes!(flat_binder_object {
                    hdr.type_: BINDER_TYPE_HANDLE,
                    __bindgen_anon_1.handle: handle.into(),
                    flags: flags.bits(),
                    cookie: cookie,
                })
                .write_to_prefix(data)
                .ok()
            }
            SerializedBinderObject::Object { local, flags } => {
                struct_with_union_into_bytes!(flat_binder_object {
                    hdr.type_: BINDER_TYPE_BINDER,
                    __bindgen_anon_1.binder: local.weak_ref_addr.ptr() as u64,
                    flags: flags.bits(),
                    cookie: local.strong_ref_addr.ptr() as u64,
                })
                .write_to_prefix(data)
                .ok()
            }
            SerializedBinderObject::File { fd, cookie } => {
                struct_with_union_into_bytes!(binder_fd_object {
                    hdr.type_: BINDER_TYPE_FD,
                    __bindgen_anon_1.fd: fd.raw() as u32,
                    cookie: cookie,
                })
                .write_to_prefix(data)
                .ok()
            }
            SerializedBinderObject::Buffer { buffer, length, parent, parent_offset, flags } => {
                binder_buffer_object {
                    hdr: binder_object_header { type_: BINDER_TYPE_PTR },
                    buffer: buffer.ptr() as u64,
                    length: length as u64,
                    parent: parent as u64,
                    parent_offset: parent_offset as u64,
                    flags,
                }
                .write_to_prefix(data)
                .ok()
            }
            SerializedBinderObject::FileArray { num_fds, parent, parent_offset } => {
                binder_fd_array_object {
                    hdr: binder_object_header { type_: BINDER_TYPE_FDA },
                    pad: 0,
                    num_fds: num_fds as u64,
                    parent: parent as u64,
                    parent_offset: parent_offset as u64,
                }
                .write_to_prefix(data)
                .ok()
            }
        }
        .ok_or_else(|| errno!(EINVAL))
    }
}
