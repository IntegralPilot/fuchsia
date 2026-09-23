// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher::{
    DispatcherOps, PeerHolder, PeerHolderMuClass, PeeredState,
    impl_peered_dispatcher_facade_with_state,
};
use super::fifo_dispatcher_ffi::cpp_fifo_dispatcher_create;
use super::handle::KernelHandle;
use crate::arch_rs::{
    UserCopyCaptureFaultsError, arch_copy_from_user_capture_faults,
    arch_copy_to_user_capture_faults,
};
use crate::counters::define_kcounter;
use crate::kernel::thread::soft_fault;
use crate::user_copy::{UserInPtr, UserOutPtr};
use core::convert::Infallible;
use core::pin::Pin;
use fbl::{Array, Canary, RefPtr};
use ksync::{KMutex, PhantomMutex, guarded};
use object_constants_rs::{
    kFifoDispatcherStateAlign, kFifoDispatcherStateOffset, kFifoDispatcherStateSize,
};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_FIFO_MAX_SIZE_BYTES, ZX_FIFO_PEER_CLOSED, ZX_FIFO_READABLE, ZX_FIFO_WRITABLE,
    ZX_OBJ_TYPE_FIFO, ZX_RIGHT_DUPLICATE, ZX_RIGHT_INSPECT, ZX_RIGHT_READ, ZX_RIGHT_SIGNAL,
    ZX_RIGHT_SIGNAL_PEER, ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, ZX_RIGHT_WRITE, ZX_USER_SIGNAL_ALL,
    zx_rights_t,
};

pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_READ
    | ZX_RIGHT_WRITE
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_SIGNAL
    | ZX_RIGHT_SIGNAL_PEER;

pub const ALLOWED_SIGNALS: u32 = ZX_USER_SIGNAL_ALL;

zr::static_assert_size_and_align!(
    FifoDispatcherState,
    kFifoDispatcherStateSize,
    kFifoDispatcherStateAlign,
);

define_kcounter!(DISPATCHER_FIFO_CREATE_COUNT, "dispatcher.fifo.create", Sum);
define_kcounter!(DISPATCHER_FIFO_DESTROY_COUNT, "dispatcher.fifo.destroy", Sum);

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct FifoDispatcherState {
    canary: Canary<{ fbl::magic(b"FIFO") }>,

    #[pin]
    pub peered: PeeredState<FifoDispatcher>,

    elem_count: u32,
    elem_size: u32,

    #[guarded_by(mu)]
    head: u64,
    #[guarded_by(mu)]
    tail: u64,
    #[guarded_by(mu)]
    data: Array<u8>,

    #[mutex(PeerHolderMuClass<FifoDispatcher>)]
    pub mu: KMutex<PhantomMutex>,
}

impl FifoDispatcherState {
    pub fn init(
        holder: RefPtr<PeerHolder<FifoDispatcher>>,
        count: u32,
        elem_size: u32,
        data: *mut u8,
    ) -> impl PinInit<Self, Infallible> {
        DISPATCHER_FIFO_CREATE_COUNT.add(1);
        let total_size = (count as usize) * (elem_size as usize);
        // SAFETY: `data` points to a buffer allocated via Array::try_new_zeroed_slice of `total_size`
        // bytes in FifoDispatcher::create and transferred via into_raw_parts on success.
        let data_array = unsafe { Array::from_raw_parts(data, total_size) };
        pin_init!(Self {
            canary: Canary::new(),
            peered <- PeeredState::init(holder),
            elem_count: count,
            elem_size,
            head: 0.into(),
            tail: 0.into(),
            data: data_array.into(),
            mu: KMutex::new(PhantomMutex),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for FifoDispatcherState {
    fn drop(self: Pin<&mut Self>) {
        DISPATCHER_FIFO_DESTROY_COUNT.add(1);
    }
}

impl_peered_dispatcher_facade_with_state!(
    pub struct FifoDispatcher,
    FifoDispatcherState,
    ZX_OBJ_TYPE_FIFO,
    kFifoDispatcherStateOffset,
    allowed_signals: ALLOWED_SIGNALS,
);

impl FifoDispatcher {
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new FifoDispatcher pair and returns their kernel handles and rights.
    pub fn create(
        count: usize,
        elem_size: usize,
    ) -> Result<(KernelHandle<Self>, KernelHandle<Self>, zx_rights_t), Status> {
        const MAX_SIZE_BYTES: usize = ZX_FIFO_MAX_SIZE_BYTES as usize;

        // count and elem_size must be non-zero.
        // Total size must be <= kMaxSizeBytes.
        if count == 0 || elem_size == 0 {
            return Err(Status::OUT_OF_RANGE);
        }

        let total_size = count
            .checked_mul(elem_size)
            .filter(|&total| total <= MAX_SIZE_BYTES)
            .ok_or(Status::OUT_OF_RANGE)?;
        let holder0 = PeerHolder::<Self>::create().map_err(|_| Status::NO_MEMORY)?;
        let holder1 = holder0.clone();

        let data0 = Array::<u8>::try_new_zeroed_slice(total_size).map_err(|_| Status::NO_MEMORY)?;
        let data1 = Array::<u8>::try_new_zeroed_slice(total_size).map_err(|_| Status::NO_MEMORY)?;

        let create_single = |holder: RefPtr<PeerHolder<Self>>,
                             data: Array<u8>|
         -> Result<KernelHandle<Self>, Status> {
            let (data_ptr, _) = data.into_raw_parts();
            // SAFETY: `holder` transfers an acquired reference count, `data_ptr` is a valid buffer
            // from `into_raw_parts`, and `cpp_fifo_dispatcher_create` initializes `handle` on success.
            let handle = unsafe {
                KernelHandle::create(|out| {
                    cpp_fifo_dispatcher_create(
                        RefPtr::into_raw(holder) as *mut _,
                        count as u32,
                        elem_size as u32,
                        data_ptr,
                        out,
                    )
                })
            }?;
            Ok(handle)
        };

        let handle0 = create_single(holder0, data0)?;
        let handle1 = create_single(holder1, data1)?;

        handle0.dispatcher().init_peer(handle1.dispatcher().clone());
        handle1.dispatcher().init_peer(handle0.dispatcher().clone());

        Ok((handle0, handle1, DEFAULT_RIGHTS))
    }

    fn on_zero_handles_locked(&self, _token: &ksync::LockToken<'_, PeerHolderMuClass<Self>>) {
        self.state().canary.assert();
    }

    fn on_peer_zero_handles_locked(&self, token: &ksync::LockToken<'_, PeerHolderMuClass<Self>>) {
        self.state().canary.assert();
        self.update_state_locked(token, ZX_FIFO_WRITABLE, ZX_FIFO_PEER_CLOSED);
    }

    /// Writes data from userspace into the peer endpoint of this FIFO.
    ///
    /// May block on page requests and must be called without locks held.
    pub fn write_from_user(
        &self,
        elem_size: usize,
        src: UserInPtr<u8>,
        count: usize,
        actual: &mut usize,
    ) -> Result<(), Status> {
        self.state().canary.assert();
        // As we may need to perform fault resolution later, ensure our caller is not holding any
        // locks.
        lockdep::assert_no_locks_held();

        loop {
            let copy_result = {
                ksync::lock!(let mut guard = self.state().peered.lock());
                let peer = guard.peer().as_ref().cloned().ok_or(Status::PEER_CLOSED)?;
                peer.write_self_locked(guard.as_mut().token_mut(), elem_size, src, count)
            };

            // Copy failed, need to check for and handle any page faults.
            match copy_result {
                // Check for any regular error and return it.
                Ok(copied) => {
                    *actual = copied;
                    return Ok(());
                }
                Err(UserCopyCaptureFaultsError { fault_info: Some(fault), .. }) => {
                    // If we have a fault the original status is irrelevant and we replace it with
                    // the result of the fault.
                    if soft_fault(fault.pf_va, fault.pf_flags).is_err() {
                        // Regardless of why the copy or fault failed it means the underlying
                        // pointer is somehow bad, which we report to the user as an invalid
                        // argument.
                        return Err(Status::INVALID_ARGS);
                    }
                }
                Err(UserCopyCaptureFaultsError { status, fault_info: None }) => {
                    // If there's no fault information then the assumption is that the original copy
                    // cannot have succeeded.
                    return Err(status);
                }
            }
        }
    }

    fn write_self_locked(
        &self,
        token: &mut ksync::LockToken<'_, PeerHolderMuClass<FifoDispatcher>>,
        elem_size: usize,
        mut src: UserInPtr<u8>,
        mut count: usize,
    ) -> Result<usize, UserCopyCaptureFaultsError> {
        self.state().canary.assert();

        if elem_size != self.state().elem_size as usize || count == 0 {
            return Err(Status::OUT_OF_RANGE.into());
        }

        let elem_count = self.state().elem_count as usize;
        let old_head = *self.state().guard_mu(token).head();
        let tail = *self.state().guard_mu(token).tail();

        // Total number of available empty slots in the FIFO.
        let avail = elem_count.wrapping_sub((old_head.wrapping_sub(tail)) as usize);
        if avail == 0 {
            return Err(Status::SHOULD_WAIT.into());
        }

        let was_empty = avail == elem_count;
        if count > avail {
            count = avail;
        }

        let mut current_head = old_head;
        while count > 0 {
            let offset = (current_head as usize) % elem_count;
            // Number of slots from target to end, inclusive.
            let n = elem_count - offset;
            // Number of slots we can actually copy.
            let to_copy = if count > n { n } else { count };
            let copy_bytes = to_copy * elem_size;
            let byte_offset = offset * elem_size;

            let data_ptr = self.state().guard_mu_mut(token).data_mut().as_mut_ptr();
            // SAFETY: `data_ptr.add(byte_offset)` points to `copy_bytes` bytes within `data`,
            // and `src` is a user pointer of length `copy_bytes`.
            let res = unsafe {
                arch_copy_from_user_capture_faults(
                    data_ptr.add(byte_offset).cast(),
                    src.as_ptr().cast(),
                    copy_bytes,
                )
            };
            if let Err(err) = res {
                // Roll back, in case this is the second copy.
                *self.state().guard_mu_mut(token).head_mut() = old_head;
                return Err(err);
            }

            // Adjust head and count.
            current_head += to_copy as u64;
            *self.state().guard_mu_mut(token).head_mut() = current_head;
            count -= to_copy;
            src = src.byte_offset(copy_bytes as isize);
        }

        // If was empty, we've become readable.
        if was_empty {
            self.update_state_locked(token, 0, ZX_FIFO_READABLE);
        }

        // If now full, peer (the writer endpoint) is no longer writable.
        if elem_count as u64 == (current_head - tail)
            && let Some(peer) = self.state().peered.guard_mu(token).peer().as_ref()
        {
            peer.update_state_locked(token, ZX_FIFO_WRITABLE, 0);
        }

        Ok((current_head - old_head) as usize)
    }

    /// Reads data from this FIFO endpoint into userspace memory.
    ///
    /// May block on page requests and must be called without locks held.
    pub fn read_to_user(
        &self,
        elem_size: usize,
        dst: UserOutPtr<u8>,
        count: usize,
        actual: &mut usize,
    ) -> Result<(), Status> {
        self.state().canary.assert();
        // As we may need to perform fault resolution later, ensure our caller is not holding any
        // locks.
        lockdep::assert_no_locks_held();

        loop {
            let copy_result = {
                ksync::lock!(let mut guard = self.state().peered.lock());
                self.read_to_user_locked(guard.as_mut().token_mut(), elem_size, dst, count)
            };

            // Copy failed, need to check for and handle any page faults.
            match copy_result {
                // Check for any regular error and return it.
                Ok(copied) => {
                    *actual = copied;
                    return Ok(());
                }
                Err(UserCopyCaptureFaultsError { fault_info: Some(fault), .. }) => {
                    // If we have a fault the original status is irrelevant and we replace it with
                    // the result of the fault.
                    if soft_fault(fault.pf_va, fault.pf_flags).is_err() {
                        // Regardless of why the copy or fault failed it means the underlying
                        // pointer is somehow bad, which we report to the user as an invalid
                        // argument.
                        return Err(Status::INVALID_ARGS);
                    }
                }
                Err(UserCopyCaptureFaultsError { status, fault_info: None }) => {
                    // If there's no fault information then the assumption is that the original copy
                    // cannot have succeeded.
                    return Err(status);
                }
            }
        }
    }

    fn read_to_user_locked(
        &self,
        token: &mut ksync::LockToken<'_, PeerHolderMuClass<FifoDispatcher>>,
        elem_size: usize,
        mut dst: UserOutPtr<u8>,
        mut count: usize,
    ) -> Result<usize, UserCopyCaptureFaultsError> {
        self.state().canary.assert();

        if elem_size != self.state().elem_size as usize || count == 0 {
            return Err(Status::OUT_OF_RANGE.into());
        }

        let elem_count = self.state().elem_count as usize;
        let head = *self.state().guard_mu(token).head();
        let old_tail = *self.state().guard_mu(token).tail();

        // Total number of available entries to read from the FIFO.
        let avail = (head.wrapping_sub(old_tail)) as usize;
        if avail == 0 {
            let has_peer = self.state().peered.guard_mu(token).peer().is_some();
            return Err((if has_peer { Status::SHOULD_WAIT } else { Status::PEER_CLOSED }).into());
        }

        let was_full = avail == elem_count;
        if count > avail {
            count = avail;
        }

        let mut current_tail = old_tail;
        while count > 0 {
            let offset = (current_tail as usize) % elem_count;
            // Number of slots from target to end, inclusive.
            let n = elem_count - offset;
            // Number of slots we can actually copy.
            let to_copy = if count > n { n } else { count };
            let copy_bytes = to_copy * elem_size;
            let byte_offset = offset * elem_size;

            let data_ptr = self.state().guard_mu(token).data().as_ptr();
            // SAFETY: `data_ptr.add(byte_offset)` points to `copy_bytes` bytes within `data`,
            // and `dst` is a user pointer of length `copy_bytes`.
            let res = unsafe {
                arch_copy_to_user_capture_faults(
                    dst.as_ptr().cast(),
                    data_ptr.add(byte_offset).cast(),
                    copy_bytes,
                )
            };
            if let Err(err) = res {
                // Roll back, in case this is the second copy.
                *self.state().guard_mu_mut(token).tail_mut() = old_tail;
                return Err(err);
            }

            // Adjust tail and count.
            current_tail += to_copy as u64;
            *self.state().guard_mu_mut(token).tail_mut() = current_tail;
            count -= to_copy;
            dst = dst.byte_offset(copy_bytes as isize);
        }

        // If we were full, peer (writer) has become writable.
        if was_full && let Some(peer) = self.state().peered.guard_mu(token).peer().as_ref() {
            peer.update_state_locked(token, 0, ZX_FIFO_WRITABLE);
        }

        // If we've become empty, we're no longer readable.
        if head == current_tail {
            self.update_state_locked(token, ZX_FIFO_READABLE, 0);
        }

        Ok((current_tail - old_tail) as usize)
    }
}
