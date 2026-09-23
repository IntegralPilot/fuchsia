// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher::{
    DispatcherOps, impl_dispatcher_facade_with_state, impl_dispatcher_state_init,
};
use super::handle::KernelHandle;
use crate::arch_rs::{UserCopyCaptureFaultsError, arch_copy_from_user_capture_faults};
use crate::kernel::thread::soft_fault;
use crate::kernel::types::VAddr;
use crate::user_copy::{UserInIovec, UserInPtr, UserInVector};
use crate::vm::arch_vm_aspace::{ARCH_MMU_FLAG_PERM_READ, ARCH_MMU_FLAG_PERM_WRITE};
use crate::vm::pmm::{ALLOC_FLAG_ANY, ALLOC_FLAG_CAN_WAIT};
use crate::vm::vm_aspace::VmAspace;
use crate::vm::vm_mapping::VmMapping;
use crate::vm::vm_object::VmObject;
use crate::vm::vm_object_paged::VmObjectPaged;
use core::convert::Infallible;
use core::mem::{MaybeUninit, size_of, size_of_val};
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::{cmp, ptr};
use fbl::{Canary, RefPtr};
use ksync::{KMutex, RawCriticalMutex, guarded};
use object_constants_rs::{
    kIoBufferSharedRegionDispatcherStateAlign, kIoBufferSharedRegionDispatcherStateOffset,
    kIoBufferSharedRegionDispatcherStateSize,
};
use pin_init::{PinInit, pin_data, pin_init, pinned_drop};
use zx_status::Status;
use zx_types::{
    ZX_IOB_SHARED_REGION_UPDATED, ZX_OBJ_TYPE_IOB_SHARED_REGION, ZX_RIGHT_DUPLICATE,
    ZX_RIGHT_GET_PROPERTY, ZX_RIGHT_INSPECT, ZX_RIGHT_SET_PROPERTY, ZX_RIGHT_SIGNAL,
    ZX_RIGHT_TRANSFER, ZX_RIGHT_WAIT, zx_iovec_t, zx_rights_t, zx_status_t,
};

pub const DEFAULT_RIGHTS: zx_rights_t = ZX_RIGHT_TRANSFER
    | ZX_RIGHT_DUPLICATE
    | ZX_RIGHT_INSPECT
    | ZX_RIGHT_GET_PROPERTY
    | ZX_RIGHT_SET_PROPERTY
    | ZX_RIGHT_WAIT
    | ZX_RIGHT_SIGNAL;

zr::static_assert_size_and_align!(
    IoBufferSharedRegionDispatcherState,
    kIoBufferSharedRegionDispatcherStateSize,
    kIoBufferSharedRegionDispatcherStateAlign,
);

/// The header used with the mediated write ring buffer discipline.
#[repr(C)]
struct Header {
    head: AtomicU64,
    tail: AtomicU64,
}

// 8 bytes for the tag, plus 8 bytes for the length.
const HEADER_SIZE: usize = 16;
const _: () = assert!(HEADER_SIZE == size_of::<Header>());

#[guarded]
#[pin_data(PinnedDrop)]
#[repr(C)]
pub struct IoBufferSharedRegionDispatcherState {
    canary: Canary<{ fbl::magic(b"IOSR") }>,
    vmo: RefPtr<VmObjectPaged>,
    mapping: RefPtr<VmMapping>,
    base: VAddr,

    #[mutex]
    lock: KMutex<RawCriticalMutex>,
}

impl IoBufferSharedRegionDispatcherState {
    pub fn init(
        _dispatcher: *const IoBufferSharedRegionDispatcher,
        vmo: &RefPtr<VmObjectPaged>,
        mapping: &RefPtr<VmMapping>,
        base: VAddr,
    ) -> impl PinInit<Self, Infallible> {
        pin_init!(Self {
            canary: Canary::new(),
            vmo: vmo.clone(),
            mapping: mapping.clone(),
            base,
            lock <- KMutex::init(),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for IoBufferSharedRegionDispatcherState {
    fn drop(self: Pin<&mut Self>) {
        let _ = self.mapping.destroy();
    }
}

impl_dispatcher_facade_with_state!(
    pub struct IoBufferSharedRegionDispatcher,
    IoBufferSharedRegionDispatcherState,
    ZX_OBJ_TYPE_IOB_SHARED_REGION,
    kIoBufferSharedRegionDispatcherStateOffset
);

impl_dispatcher_state_init!(
    IoBufferSharedRegionDispatcher,
    IoBufferSharedRegionDispatcherState,
    vmo: &RefPtr<VmObjectPaged>,
    mapping: &RefPtr<VmMapping>,
    base: VAddr,
);

#[allow(improper_ctypes)]
unsafe extern "C" {
    fn cpp_io_buffer_shared_region_dispatcher_create(
        vmo: &RefPtr<VmObjectPaged>,
        mapping: &RefPtr<VmMapping>,
        base: VAddr,
        handle_out: *mut MaybeUninit<KernelHandle<IoBufferSharedRegionDispatcher>>,
    ) -> zx_status_t;
}

impl IoBufferSharedRegionDispatcher {
    /// Returns default rights for `IoBufferSharedRegionDispatcher`.
    pub fn default_rights() -> zx_rights_t {
        DEFAULT_RIGHTS
    }

    /// Creates a new `IoBufferSharedRegionDispatcher`.
    pub fn create(size: u64) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        let vmo = VmObjectPaged::create(
            ALLOC_FLAG_ANY | ALLOC_FLAG_CAN_WAIT,
            VmObjectPaged::ALWAYS_PINNED,
            size,
        )?;

        let kernel_vmar = VmAspace::kernel_aspace()
            .root_vmar()
            .expect("kernel aspace root VMAR must be initialized");
        let mapping = kernel_vmar.create_vm_mapping(
            0,
            size as usize,
            0,
            0,
            VmObjectPaged::into_vm_object(vmo.clone()),
            0,
            ARCH_MMU_FLAG_PERM_READ | ARCH_MMU_FLAG_PERM_WRITE,
            c"IOBuffer shared region",
        )?;

        mapping.mapping.map_range(0, size as usize, true, false)?;

        // SAFETY: `cpp_io_buffer_shared_region_dispatcher_create` initializes `handle` on success.
        let handle = unsafe {
            KernelHandle::create(|out| {
                cpp_io_buffer_shared_region_dispatcher_create(
                    &vmo,
                    &mapping.mapping,
                    VAddr(mapping.base),
                    out,
                )
            })
        }?;
        handle.dispatcher().state().vmo.set_user_id(handle.dispatcher().get_koid());

        Ok((handle, DEFAULT_RIGHTS))
    }

    /// Returns the underlying `VmObject` of the shared region.
    pub fn vmo(&self) -> RefPtr<VmObject> {
        VmObjectPaged::into_vm_object(self.state().vmo.clone())
    }

    /// Performs a mediated write to the shared region.
    ///
    /// May block on page requests and must be called without locks held.
    pub fn write(
        &self,
        tag: u64,
        vector: UserInPtr<zx_iovec_t>,
        vector_count: usize,
    ) -> Result<(), Status> {
        self.state().canary.assert();
        // As we may need to perform fault resolution later, ensure our caller is not holding any
        // locks.
        lockdep::assert_no_locks_held();

        // The maximum is chosen so that an entire message including the message header fits within
        // 64 KiB, and the total length fits within a 16 bit integer. This is sufficient for current
        // use cases, but can be increased if necessary.
        const MAX_MESSAGE_SIZE: usize = (1 << 16) - 1 - HEADER_SIZE;
        const MAX_VECTORS: usize = 8;

        if vector_count > MAX_VECTORS {
            return Err(Status::INVALID_ARGS);
        }

        let mut vectors = [MaybeUninit::<UserInVector>::uninit(); MAX_VECTORS];
        let mut message_size = 0usize;

        // Copy the vectors to our stack copy so we can compute the message size and have access to
        // the vectors below once we have taken the lock.
        let vectors = UserInIovec::new(vector, vector_count).copy_to_slice(&mut vectors)?;
        for vec in &*vectors {
            if MAX_MESSAGE_SIZE - message_size < vec.len {
                return Err(Status::INVALID_ARGS);
            }
            message_size += vec.len;
        }

        let rounded_message_size = (message_size + HEADER_SIZE).next_multiple_of(8);
        let buffer_size = (self.state().vmo.size() as usize) - page::SIZE;

        if rounded_message_size > buffer_size {
            return Err(Status::NO_SPACE);
        }

        // SAFETY: `base` is page-aligned and points to valid mapped kernel memory for the
        // lifetime of `self`, and `Header` contains only atomic fields.
        let header = unsafe { &*(self.state().base.0 as *const Header) };
        let get_ptr =
            |offset: usize| -> *mut u8 { (self.state().base.0 + page::SIZE + offset) as *mut u8 };

        loop {
            let fault = {
                ksync::lock!(let guard = self.state().lock.lock());

                // TODO(https://fxbug.dev/551552519): Should head and tail use volatile atomics?
                // Relaxed ordering because only the kernel modifies this value while holding the
                // dispatcher lock.
                let head = header.head.load(Ordering::Relaxed) as usize;
                // Acquire ordering so that the following writes are not reordered before this load
                // since otherwise it is possible that we will overwrite a message that userspace
                // has not finished reading yet.
                let tail = header.tail.load(Ordering::Acquire) as usize;

                if tail > head
                    || usize::MAX - head < rounded_message_size
                    || !head.is_multiple_of(8)
                {
                    return Err(Status::IO_DATA_INTEGRITY);
                }

                if head - tail > buffer_size - rounded_message_size {
                    return Err(Status::NO_SPACE);
                }

                let mut offset = head % buffer_size;
                let mut write_u64 = |val: u64| {
                    // SAFETY: `get_ptr(offset)` points to mapped kernel memory and is 8-byte
                    // aligned because `base` is page-aligned, and both `head` and `buffer_size` are
                    // multiples of 8.
                    unsafe { ptr::write(get_ptr(offset).cast(), val) };
                    offset = (offset + size_of_val(&val)) % buffer_size;
                };
                write_u64(tag);
                write_u64(message_size as u64);

                let mut copy_vectors = || {
                    for vec in &*vectors {
                        let mut data = vec.data;
                        let mut len = vec.len;

                        while len > 0 {
                            let amount = cmp::min(buffer_size - offset, len);

                            // SAFETY: `get_ptr(offset)` is valid kernel memory and `data` is user
                            // memory.
                            let copy_res = unsafe {
                                arch_copy_from_user_capture_faults(
                                    get_ptr(offset).cast(),
                                    data.as_ptr().cast(),
                                    amount,
                                )
                            };

                            match copy_res {
                                Ok(()) => {
                                    offset = (offset + amount) % buffer_size;
                                    len -= amount;
                                    data = data.byte_offset(amount as isize);
                                }
                                Err(UserCopyCaptureFaultsError {
                                    fault_info: Some(fi), ..
                                }) => {
                                    return Ok(Some(fi));
                                }
                                Err(UserCopyCaptureFaultsError { status, .. }) => {
                                    return Err(status);
                                }
                            }
                        }
                    }
                    Ok(None)
                };

                match copy_vectors()? {
                    None => {
                        // Release ordering so that the previous writes are not reordered after this
                        // store since otherwise it is possible for userspace to read old data when
                        // it notices the change in head value.
                        header.head.fetch_add(rounded_message_size as u64, Ordering::Release);
                        self.update_state_with_strobe_locked(
                            guard.token(),
                            0,
                            0,
                            ZX_IOB_SHARED_REGION_UPDATED,
                        );
                        return Ok(());
                    }
                    Some(fault) => fault,
                }
            };

            // Handle the fault and then retry.
            soft_fault(fault.pf_va, fault.pf_flags)?;
        }
    }
}

/// In-tree kernel unit tests for `IoBufferSharedRegionDispatcher`.
#[cfg(ktest)]
#[unittest::suite(name = "io_buffer_shared_region_dispatcher_rust_tests")]
mod tests {
    use super::{DEFAULT_RIGHTS, IoBufferSharedRegionDispatcher};
    use unittest::{expect_eq, expect_ok, expect_true};

    /// Tests creating and querying an `IoBufferSharedRegionDispatcher`.
    #[test]
    fn test_create() {
        let size = (page::SIZE * 2) as u64;
        let (handle, rights) =
            IoBufferSharedRegionDispatcher::create(size).expect("failed to create shared region");
        expect_eq!(rights, DEFAULT_RIGHTS);

        let disp = handle.dispatcher();
        expect_true!(disp.get_koid() != 0);

        let vmo = disp.vmo();
        expect_eq!(vmo.size(), size);
        expect_eq!(vmo.user_id(), disp.get_koid());
    }

    /// Allocate/destroy many shared regions. Ad hoc resource leak check.
    #[test]
    fn test_create_destroy_many() {
        const MANY: usize = 10_000;
        let size = (page::SIZE * 2) as u64;

        for _ in 0..MANY {
            expect_ok!(IoBufferSharedRegionDispatcher::create(size).map(|_| ()));
        }
    }
}
