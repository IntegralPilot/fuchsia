// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::handle::KernelHandle;
use super::vm_object_dispatcher_ffi::{
    cpp_vm_object_dispatcher_get_vmo, cpp_vm_object_dispatcher_get_vmo_info,
};
use crate::user_copy::{UserInOutPtr, UserInPtr, UserOutPtr};
use crate::vm::stream_size_manager::{Operation as StreamSizeManagerOperation, StreamSizeManager};
use crate::vm::vm_object::VmObject;
use crate::vm::vm_object_paged::VmObjectPaged;
use core::mem::MaybeUninit;
use fbl::RefPtr;
use page;
use zx_status::Status;
use zx_types::{
    ZX_OBJ_TYPE_VMO, ZX_VMO_DISCARDABLE, ZX_VMO_RESIZABLE, ZX_VMO_UNBOUNDED, zx_info_vmo_t,
    zx_rights_t,
};

// LINT.IfChange(InitialMutability)
/// Specifies initial mutability for `VmObjectDispatcher`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialMutability {
    Mutable = 0,
    Immutable = 1,
}
// LINT.ThenChange(//zircon/kernel/object/include/object/vm_object_dispatcher.h:InitialMutability)

zr::static_assert!(core::mem::size_of::<InitialMutability>() == 4);
zr::static_assert!(core::mem::align_of::<InitialMutability>() == 4);

crate::object::dispatcher::impl_dispatcher_facade!(
    pub struct VmObjectDispatcher,
    ZX_OBJ_TYPE_VMO
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreateStats {
    pub flags: u32,
    pub size: u64,
}

impl VmObjectDispatcher {
    /// Creates a `VmObjectDispatcher` wrapping a VMO.
    pub fn create(
        vmo: &VmObject,
        stream_size: u64,
        initial_mutability: InitialMutability,
    ) -> Result<(KernelHandle<Self>, zx_rights_t), Status> {
        // SAFETY: `vmo.as_raw()` returns a valid raw pointer to a VmObject, and
        // `cpp_vm_object_dispatcher_create` initializes `handle` and `rights` on success.
        unsafe {
            KernelHandle::create_with_rights(|handle_out, rights_out| {
                super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_create(
                    vmo.as_raw().cast(),
                    stream_size,
                    initial_mutability,
                    handle_out,
                    rights_out,
                )
            })
        }
    }

    /// Returns a reference to the underlying `VmObject`.
    pub fn vmo(&self) -> &RefPtr<VmObject> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        unsafe { &*cpp_vm_object_dispatcher_get_vmo(self.as_ffi()) }
    }

    /// Parses create syscall flags for VMOs.
    ///
    /// Rounds up the size to the nearest page, or sets the size to the maximum possible VMO size if
    /// `ZX_VMO_UNBOUNDED` is used.
    pub fn parse_create_syscall_flags(mut flags: u32, size: u64) -> Result<CreateStats, Status> {
        let mut res = CreateStats { flags: 0, size };

        if (flags & ZX_VMO_RESIZABLE) != 0 {
            if (flags & ZX_VMO_UNBOUNDED) != 0 {
                return Err(Status::INVALID_ARGS);
            }
            res.flags |= VmObjectPaged::RESIZABLE;
            flags &= !ZX_VMO_RESIZABLE;
        }
        if (flags & ZX_VMO_DISCARDABLE) != 0 {
            res.flags |= VmObjectPaged::DISCARDABLE;
            flags &= !ZX_VMO_DISCARDABLE;
        }
        if (flags & ZX_VMO_UNBOUNDED) != 0 {
            flags &= !ZX_VMO_UNBOUNDED;
            res.size = VmObject::MAX_SIZE;
        } else {
            if size > VmObject::MAX_SIZE {
                return Err(Status::OUT_OF_RANGE);
            }
            res.size = page::round_up(size as usize) as u64;
        }

        if flags != 0 {
            return Err(Status::INVALID_ARGS);
        }

        // The initial stream size should not end up larger than the vmo size, as this is a state
        // that cannot exist.
        if size > res.size {
            return Err(Status::OUT_OF_RANGE);
        }

        Ok(res)
    }

    /// Returns information about this VMO.
    pub fn get_vmo_info(&self, rights: zx_rights_t) -> zx_info_vmo_t {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        unsafe { cpp_vm_object_dispatcher_get_vmo_info(self.as_ffi_mut(), rights) }
    }

    /// Reads data from this VMO into a user buffer.
    pub fn read(&self, buffer: UserOutPtr<u8>, offset: u64, len: usize) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_read(
                self.as_ffi_mut(),
                buffer.as_ptr(),
                offset,
                len,
            )
        };
        Status::ok(status)
    }

    /// Writes data from a user buffer into this VMO.
    pub fn write(&self, buffer: UserInPtr<u8>, offset: u64, len: usize) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_write(
                self.as_ffi_mut(),
                buffer.as_ptr(),
                offset,
                len,
            )
        };
        Status::ok(status)
    }

    /// Returns the size of the underlying VMO in bytes.
    pub fn get_size(&self) -> Result<u64, Status> {
        let mut size = 0u64;
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference, and `size` is a valid out pointer.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_get_size(
                self.as_ffi_mut(),
                &mut size,
            )
        };
        Status::ok(status)?;
        Ok(size)
    }

    /// Returns the stream size of the underlying VMO in bytes.
    pub fn get_stream_size(&self) -> u64 {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_get_stream_size(self.as_ffi())
        }
    }

    /// Sets the size of the underlying VMO in bytes.
    pub fn set_size(&self, size: u64) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_set_size(
                self.as_ffi_mut(),
                size,
            )
        };
        Status::ok(status)
    }

    /// Sets the stream size of the underlying VMO in bytes.
    pub fn set_stream_size(&self, size: u64) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_set_stream_size(
                self.as_ffi_mut(),
                size,
            )
        };
        Status::ok(status)
    }

    /// Performs an operation on a range of the VMO.
    pub fn range_op(
        &self,
        op: u32,
        offset: u64,
        size: u64,
        buffer: UserInOutPtr<u8>,
        buffer_size: usize,
        rights: zx_rights_t,
    ) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_range_op(
                self.as_ffi_mut(),
                op,
                offset,
                size,
                buffer.as_ptr(),
                buffer_size,
                rights,
            )
        };
        Status::ok(status)
    }

    /// Sets the mapping cache policy for the underlying VMO.
    pub fn set_mapping_cache_policy(&self, cache_policy: u32) -> Result<(), Status> {
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_set_mapping_cache_policy(
                self.as_ffi_mut(),
                cache_policy,
            )
        };
        Status::ok(status)
    }

    /// Creates a child VMO clone/slice/reference.
    pub fn create_child(
        &self,
        options: u32,
        offset: u64,
        size: u64,
        copy_name: bool,
    ) -> Result<RefPtr<VmObject>, Status> {
        let mut out_child = MaybeUninit::<RefPtr<VmObject>>::uninit();
        // SAFETY: `self` is a valid `VmObjectDispatcher` reference and `out_child` points to uninitialized storage.
        let status = unsafe {
            super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_create_child(
                self.as_ffi_mut(),
                options,
                offset,
                size,
                copy_name,
                &raw mut out_child,
            )
        };
        Status::ok(status)?;
        // SAFETY: `cpp_vm_object_dispatcher_create_child` succeeded and initialized `out_child`.
        unsafe { Ok(out_child.assume_init()) }
    }

    /// Creates a child `VmObjectDispatcher` for the given child VMO.
    ///
    /// For reference children on stream-compatible VMOs, the child dispatcher shares the parent's
    /// `StreamSizeManager` so stream size updates are shared between parent and reference children.
    /// Otherwise, a new dispatcher is created with its stream size tracking initialized to `size`.
    pub fn create_child_dispatcher(
        &self,
        child_vmo: &VmObject,
        size: u64,
        options: u32,
        initial_mutability: InitialMutability,
    ) -> Result<(KernelHandle<VmObjectDispatcher>, zx_rights_t), Status> {
        if (options & zx_types::ZX_VMO_CHILD_REFERENCE) != 0 && self.vmo().is_stream_compatible() {
            self.create_child_with_parent_stream_size(child_vmo, initial_mutability)
        } else {
            Self::create(child_vmo, size, initial_mutability)
        }
    }

    /// Creates a child `VmObjectDispatcher` sharing this dispatcher's `StreamSizeManager`.
    pub fn create_child_with_parent_stream_size(
        &self,
        child_vmo: &VmObject,
        initial_mutability: InitialMutability,
    ) -> Result<(KernelHandle<VmObjectDispatcher>, zx_rights_t), Status> {
        // SAFETY: `self` and `child_vmo` are valid references, and the FFI function initializes
        // `handle` and `rights` on success.
        unsafe {
            KernelHandle::create_with_rights(|handle_out, rights_out| {
                super::vm_object_dispatcher_ffi::cpp_vm_object_dispatcher_create_with_parent_stream_size(
                    self.as_ffi_mut(),
                    child_vmo.as_raw().cast(),
                    initial_mutability,
                    handle_out,
                    rights_out,
                )
            })
        }
    }

    fn as_ffi(&self) -> *const VmObjectDispatcher {
        &raw const *self
    }

    fn as_ffi_mut(&self) -> *mut VmObjectDispatcher {
        self.as_ffi().cast_mut()
    }

    /// Sets the stream size of the underlying VMO, coordinating with `StreamSizeManager`.
    pub fn set_stream_size_with_ssm(
        &self,
        ssm: &StreamSizeManager,
        target_size: u64,
    ) -> Result<(), Status> {
        let Some(paged) = self.vmo().as_paged() else {
            return Err(Status::NOT_SUPPORTED);
        };

        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(ssm));
        ksync::lock!(let mut guard = ksync::aliased_lock(ssm.lock(), op.lock()));

        let vmo_size = paged.size();
        let old_stream_size = ssm.get_stream_size();

        if target_size == old_stream_size {
            return Ok(());
        }

        // Can't resize the stream beyond the VMO size.
        if target_size > vmo_size {
            return Err(Status::OUT_OF_RANGE);
        }

        ssm.begin_set_stream_size_locked(&mut guard.as_mut().inner_guard(), target_size, &op);

        // Zero the range from min(target size, old stream size) to the end of the VMO.
        let zero_start = core::cmp::min(target_size, old_stream_size);
        let Some(aligned_stream_size) = round_up_page_size(target_size) else {
            let (_, op_token) = guard.as_mut().tokens_mut();
            op.cancel_locked(op_token);
            return Err(Status::OUT_OF_RANGE);
        };
        debug_assert!(aligned_stream_size >= target_size);

        // Dropping the lock here is fine, as an Operation only needs to be locked when
        // initializing, committing, or cancelling.
        let res = guard.as_mut().call_unlocked(|| {
            paged.zero_range(zero_start, aligned_stream_size - zero_start)?;
            paged.zero_range_untracked(aligned_stream_size, vmo_size - aligned_stream_size)
        });

        if let Err(status) = res {
            let (_, op_token) = guard.as_mut().tokens_mut();
            op.cancel_locked(op_token);
            return Err(status);
        }

        // Ensure pages between min(target size, old stream size) and the end of the VMO are
        // unmapped before committing new stream size.
        let aligned_zero_start = round_down_page_size(zero_start);
        let (_, op_token) = guard.as_mut().tokens_mut();
        paged.unmap_pages_and_call(aligned_zero_start, vmo_size - aligned_zero_start, || {
            op.commit_locked(op_token);
        });

        Ok(())
    }

    /// Sets the size of the underlying VMO, coordinating with `StreamSizeManager`.
    pub fn set_size_with_ssm(&self, ssm: &StreamSizeManager, size: u64) -> Result<(), Status> {
        let Some(paged) = self.vmo().as_paged() else {
            return Err(Status::UNAVAILABLE);
        };

        pin_init::stack_pin_init!(let op = StreamSizeManagerOperation::init(ssm));
        ksync::lock!(let mut guard = ksync::aliased_lock(ssm.lock(), op.lock()));

        ssm.begin_set_stream_size_locked(&mut guard.as_mut().inner_guard(), size, &op);

        let Some(size_aligned) = round_up_page_size(size) else {
            let (_, op_token) = guard.as_mut().tokens_mut();
            op.cancel_locked(op_token);
            return Err(Status::OUT_OF_RANGE);
        };

        if let Err(status) = paged.resize(size_aligned) {
            let (_, op_token) = guard.as_mut().tokens_mut();
            op.cancel_locked(op_token);
            return Err(status);
        }

        let remaining = size_aligned - size;
        if remaining > 0 {
            // TODO(https://fxbug.dev/42053728): Determine whether failure to ZeroRange here should
            // undo this operation.
            //
            // Dropping the lock here is fine, as an Operation only needs to be locked when
            // initializing, committing, or cancelling.
            let _ = guard.as_mut().call_unlocked(|| paged.zero_range(size, remaining));
        }

        let (_, op_token) = guard.as_mut().tokens_mut();
        op.commit_locked(op_token);
        Ok(())
    }
}

const fn round_up_page_size(val: u64) -> Option<u64> {
    const MASK: u64 = page::MASK as u64;
    match val.checked_add(MASK) {
        Some(v) => Some(v & !MASK),
        None => None,
    }
}

const fn round_down_page_size(val: u64) -> u64 {
    const MASK: u64 = page::MASK as u64;
    val & !MASK
}

/// Kernel unit tests for `VmObjectDispatcher`.
#[cfg(ktest)]
#[unittest::suite(name = "vm_object_dispatcher_tests")]
mod tests {
    use super::{InitialMutability, VmObjectDispatcher};
    use crate::vm::stream_size_manager::StreamSizeManager;
    use crate::vm::vm_object_paged::VmObjectPaged;
    use zx_status::Status;

    /// Tests creating a VmObjectDispatcher and accessing its underlying VMO.
    #[test]
    fn test_vm_object_dispatcher_create_and_vmo() {
        let paged_vmo =
            VmObjectPaged::create(0, 0, page::SIZE as u64).expect("failed to create paged VMO");
        let vmo_size = paged_vmo.size();
        unittest::expect_eq!(vmo_size, page::SIZE as u64);

        let (handle, rights) =
            VmObjectDispatcher::create(&paged_vmo, vmo_size, InitialMutability::Mutable)
                .expect("failed to create VmObjectDispatcher");
        unittest::expect_true!(rights != 0);

        let disp = handle.dispatcher();
        let disp_vmo = disp.vmo();
        unittest::expect_eq!(disp_vmo.size(), page::SIZE as u64);
    }

    /// Tests getting and setting size and stream size.
    #[test]
    fn test_vm_object_dispatcher_size_and_stream_size() {
        let paged_vmo =
            VmObjectPaged::create(0, 0, page::SIZE as u64).expect("failed to create paged VMO");
        let (handle, _) =
            VmObjectDispatcher::create(&paged_vmo, page::SIZE as u64, InitialMutability::Mutable)
                .expect("failed to create VmObjectDispatcher");
        let disp = handle.dispatcher();

        let size = disp.get_size().expect("failed to get size");
        unittest::expect_eq!(size, page::SIZE as u64);

        let stream_size = disp.get_stream_size();
        unittest::expect_eq!(stream_size, page::SIZE as u64);

        disp.set_stream_size(100).expect("failed to set stream size");
        unittest::expect_eq!(disp.get_stream_size(), 100);
    }

    /// Tests parsing create syscall flags.
    #[test]
    fn test_vm_object_dispatcher_parse_create_syscall_flags() {
        let stats = VmObjectDispatcher::parse_create_syscall_flags(0, 100).expect("parse flags");
        unittest::expect_eq!(stats.flags, 0);
        unittest::expect_eq!(stats.size, page::SIZE as u64);

        let stats = VmObjectDispatcher::parse_create_syscall_flags(
            zx_types::ZX_VMO_RESIZABLE,
            page::SIZE as u64,
        )
        .expect("parse flags resizable");
        unittest::expect_true!((stats.flags & VmObjectPaged::RESIZABLE) != 0);
        unittest::expect_eq!(stats.size, page::SIZE as u64);

        let stats = VmObjectDispatcher::parse_create_syscall_flags(zx_types::ZX_VMO_UNBOUNDED, 0)
            .expect("parse flags unbounded");
        unittest::expect_eq!(stats.size, crate::vm::vm_object::VmObject::MAX_SIZE);
    }

    /// Tests child creation and child dispatcher creation.
    #[test]
    fn test_vm_object_dispatcher_create_child() {
        let paged_vmo =
            VmObjectPaged::create(0, 0, page::SIZE as u64).expect("failed to create paged VMO");
        let (handle, _) =
            VmObjectDispatcher::create(&paged_vmo, page::SIZE as u64, InitialMutability::Mutable)
                .expect("failed to create VmObjectDispatcher");
        let disp = handle.dispatcher();

        // Test snapshot child.
        let child_vmo = disp
            .create_child(zx_types::ZX_VMO_CHILD_SNAPSHOT, 0, page::SIZE as u64, true)
            .expect("failed to create child");
        unittest::expect_eq!(child_vmo.size(), page::SIZE as u64);

        let (child_handle, child_rights) = disp
            .create_child_dispatcher(
                &child_vmo,
                page::SIZE as u64,
                zx_types::ZX_VMO_CHILD_SNAPSHOT,
                InitialMutability::Mutable,
            )
            .expect("failed to create child dispatcher");
        unittest::expect_true!(child_rights != 0);
        unittest::expect_eq!(child_handle.dispatcher().vmo().size(), page::SIZE as u64);

        // Test reference child (sharing parent's StreamSizeManager).
        let ref_child_vmo = disp
            .create_child(zx_types::ZX_VMO_CHILD_REFERENCE, 0, 0, true)
            .expect("failed to create ref child");
        let (ref_child_handle, ref_child_rights) = disp
            .create_child_dispatcher(
                &ref_child_vmo,
                0,
                zx_types::ZX_VMO_CHILD_REFERENCE,
                InitialMutability::Mutable,
            )
            .expect("failed to create ref child dispatcher");
        unittest::expect_true!(ref_child_rights != 0);
        unittest::expect_eq!(ref_child_handle.dispatcher().vmo().size(), page::SIZE as u64);
    }

    /// Tests set_size and set_stream_size on VmObjectDispatcher.
    #[test]
    fn test_vm_object_dispatcher_set_size_and_stream_size() {
        let paged_vmo = VmObjectPaged::create(0, VmObjectPaged::RESIZABLE, 8192)
            .expect("failed to create paged VMO");
        let ssm = StreamSizeManager::create(4096).expect("failed to create StreamSizeManager");
        paged_vmo.set_user_stream_size(ssm.clone());

        let (handle, _rights) =
            VmObjectDispatcher::create(&paged_vmo, 4096, InitialMutability::Mutable)
                .expect("failed to create VmObjectDispatcher");
        let disp = handle.dispatcher();

        unittest::assert_ok!(disp.set_stream_size_with_ssm(&ssm, 2048));
        unittest::expect_eq!(ssm.get_stream_size(), 2048);

        unittest::assert_ok!(disp.set_size_with_ssm(&ssm, 4096));
        unittest::expect_eq!(disp.vmo().size(), 4096);
    }

    /// Tests that resizing a VMO to u64::MAX returns OUT_OF_RANGE without panicking.
    #[test]
    fn test_vm_object_dispatcher_set_size_overflow() {
        let paged_vmo = VmObjectPaged::create(0, VmObjectPaged::RESIZABLE, 4096)
            .expect("failed to create paged VMO");
        let ssm = StreamSizeManager::create(4096).expect("failed to create StreamSizeManager");
        let (handle, _rights) =
            VmObjectDispatcher::create(&paged_vmo, 4096, InitialMutability::Mutable)
                .expect("failed to create VmObjectDispatcher");
        let disp = handle.dispatcher();

        let res = disp.set_size_with_ssm(&ssm, u64::MAX);
        unittest::expect_eq!(Status::result_into_raw(res), Status::OUT_OF_RANGE.into_raw());
    }
}
