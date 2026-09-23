// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::page::VmPagePtr;
use super::stream_size_manager::StreamSizeManager;
use super::vm_cow_pages::VmCowPages;
use super::vm_object::VmObject;
use super::vm_object_paged_ffi::*;
use crate::user_copy::{UserInIovec, UserOutIovec};
use core::marker::PhantomPinned;
use core::mem::ManuallyDrop;
use core::ops::Deref;
use fbl::{IsOpaqueRefCounted, RefPtr};
use vm_object_paged_bindings as bindings;
use zr::Opaque;
use zx_status::Status;

/// VMO representing a paged range of copy-on-write memory.
#[repr(C)]
pub struct VmObjectPaged {
    raw: Opaque<bindings::VmObjectPaged>,
    phantom: PhantomPinned,
}

impl VmObjectPaged {
    pub const RESIZABLE: u32 = bindings::VmObjectPaged_kResizable;
    pub const CONTIGUOUS: u32 = bindings::VmObjectPaged_kContiguous;
    pub const SLICE: u32 = bindings::VmObjectPaged_kSlice;
    pub const DISCARDABLE: u32 = bindings::VmObjectPaged_kDiscardable;
    pub const ALWAYS_PINNED: u32 = bindings::VmObjectPaged_kAlwaysPinned;
    pub const REFERENCE: u32 = bindings::VmObjectPaged_kReference;
    pub const CAN_BLOCK_ON_PAGE_REQUESTS: u32 = bindings::VmObjectPaged_kCanBlockOnPageRequests;

    /// Domain-specific conversion: returns raw FFI pointer for `VmObjectPaged`.
    pub fn as_raw(&self) -> *mut bindings::VmObjectPaged {
        self.raw.get()
    }

    /// Domain-specific conversion: constructs a `RefPtr<VmObjectPaged>` from a raw FFI pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid, raw `VmObjectPaged` pointer exported from C++.
    pub unsafe fn from_raw(ptr: *mut bindings::VmObjectPaged) -> Option<RefPtr<Self>> {
        unsafe { RefPtr::try_from_raw(ptr.cast::<Self>()) }
    }

    /// Create a new paged VMO.
    pub fn create(
        pmm_alloc_flags: u32,
        options: u32,
        size: u64,
    ) -> Result<RefPtr<VmObjectPaged>, Status> {
        let mut status = 0;
        let raw = unsafe {
            bindings::cpp_vm_object_paged_create(pmm_alloc_flags, options, size, &mut status)
        };
        Status::ok(status)?;
        unsafe { Self::from_raw(raw).ok_or(Status::NO_MEMORY) }
    }

    /// Create a contiguous paged VMO.
    pub fn create_contiguous(
        pmm_alloc_flags: u32,
        size: u64,
        alignment_log2: u8,
    ) -> Result<RefPtr<VmObjectPaged>, Status> {
        let mut status = 0;
        // SAFETY: status is a valid local mutable reference.
        let raw = unsafe {
            bindings::cpp_vm_object_paged_create_contiguous(
                pmm_alloc_flags,
                size,
                alignment_log2,
                &mut status,
            )
        };
        Status::ok(status)?;
        unsafe { Self::from_raw(raw).ok_or(Status::NO_MEMORY) }
    }

    /// Returns the backing `VmCowPages` hierarchy.
    pub fn debug_get_cow_pages(&self) -> Option<RefPtr<VmCowPages>> {
        let raw = unsafe { bindings::cpp_vm_object_paged_debug_get_cow_pages(self.as_raw()) };
        unsafe { VmCowPages::from_raw(raw) }
    }

    /// Debug helper to fetch backing page pointer.
    pub fn debug_get_page(&self, offset: u64) -> Option<VmPagePtr> {
        let raw = unsafe { bindings::cpp_vm_object_paged_debug_get_page(self.as_raw(), offset) };
        unsafe { VmPagePtr::from_ffi(raw) }
    }

    /// Converts a `RefPtr<VmObjectPaged>` into a base `RefPtr<VmObject>`.
    pub fn into_vm_object(this: RefPtr<Self>) -> RefPtr<VmObject> {
        let this = ManuallyDrop::new(this);
        // SAFETY: `this.as_raw()` returns a valid `VmObjectPaged` pointer.
        // `cpp_vm_object_paged_as_vm_object` converts the derived type pointer
        // to its base `VmObject` pointer.
        let raw_base = unsafe { bindings::cpp_vm_object_paged_as_vm_object(this.as_raw()) };
        // SAFETY: `raw_base` points to a valid ref-counted `VmObject` whose
        // reference count is owned by `this`.
        unsafe { VmObject::from_raw(raw_base) }
            .expect("RefPtr guarantees this is non-null and valid")
    }

    /// Reads data from the VMO into user vectors.
    pub fn read_user_vector(
        &self,
        user_data: UserOutIovec,
        offset: u64,
        length: usize,
    ) -> Result<usize, Status> {
        let mut actual = 0usize;
        let status = unsafe {
            cpp_vm_object_paged_read_user_vector(
                self.as_raw(),
                user_data.as_user_out_ptr(),
                user_data.count(),
                offset,
                length,
                &mut actual,
            )
        };
        if actual > 0 {
            Ok(actual)
        } else {
            Status::ok(status)?;
            Ok(0)
        }
    }

    /// Writes data from user vectors into the VMO.
    pub fn write_user_vector(
        &self,
        user_data: UserInIovec,
        offset: u64,
        length: usize,
    ) -> Result<usize, Status> {
        let mut actual = 0usize;
        let status = unsafe {
            cpp_vm_object_paged_write_user_vector(
                self.as_raw(),
                user_data.vector(),
                user_data.count(),
                offset,
                length,
                &mut actual,
            )
        };
        if actual > 0 {
            Ok(actual)
        } else {
            Status::ok(status)?;
            Ok(0)
        }
    }

    /// Writes data from user vectors into the VMO with progress callback.
    #[allow(clippy::too_many_arguments)]
    pub fn write_user_vector_progress(
        &self,
        user_data: UserInIovec,
        offset: u64,
        length: usize,
        prev_stream_size: u64,
        cb: extern "C" fn(*mut core::ffi::c_void, u64, usize),
        cookie: *mut core::ffi::c_void,
    ) -> Result<usize, Status> {
        let mut actual = 0usize;
        let status = unsafe {
            cpp_vm_object_paged_write_user_vector_progress(
                self.as_raw(),
                user_data.vector(),
                user_data.count(),
                offset,
                length,
                prev_stream_size,
                &mut actual,
                cb,
                cookie,
            )
        };
        if actual > 0 {
            Ok(actual)
        } else {
            Status::ok(status)?;
            Ok(0)
        }
    }

    /// Zeroes a range of bytes in the VMO.
    pub fn zero_range(&self, offset: u64, length: u64) -> Result<(), Status> {
        if length == 0 {
            return Ok(());
        }
        let status = unsafe { cpp_vm_object_paged_zero_range(self.as_raw(), offset, length) };
        Status::ok(status)
    }

    /// Zeroes a range of bytes in the VMO without tracking.
    pub fn zero_range_untracked(&self, offset: u64, length: u64) -> Result<(), Status> {
        if length == 0 {
            return Ok(());
        }
        let status =
            unsafe { cpp_vm_object_paged_zero_range_untracked(self.as_raw(), offset, length) };
        Status::ok(status)
    }

    /// Resizes the VMO.
    pub fn resize(&self, size: u64) -> Result<(), Status> {
        let status = unsafe { cpp_vm_object_paged_resize(self.as_raw(), size) };
        Status::ok(status)
    }

    /// Unmaps pages in the given range and invokes `cb` atomically while holding the VMO lock.
    pub fn unmap_pages_and_call<F: FnOnce()>(&self, offset: u64, length: u64, cb: F) {
        struct Ctx<F: FnOnce()> {
            cb: Option<F>,
        }

        unsafe extern "C" fn trampoline<F: FnOnce()>(ctx: *mut core::ffi::c_void) {
            // SAFETY: `ctx` points to `Ctx<F>` on caller's stack.
            let ctx = unsafe { &mut *ctx.cast::<Ctx<F>>() };
            if let Some(cb) = ctx.cb.take() {
                cb();
            }
        }

        let mut ctx = Ctx { cb: Some(cb) };
        let cookie = (&raw mut ctx).cast::<core::ffi::c_void>();
        // SAFETY: `self.as_raw()` is a valid `VmObjectPaged`.
        unsafe {
            cpp_vm_object_paged_unmap_and_call(
                self.as_raw(),
                offset,
                length,
                Some(trampoline::<F>),
                cookie,
            );
        }
    }

    /// Sets the user-defined stream size manager for this VMO.
    pub fn set_user_stream_size(&self, ssm: RefPtr<StreamSizeManager>) {
        let raw_ssm = RefPtr::into_raw(ssm).cast_mut();
        // SAFETY: `self.as_raw()` and `raw_ssm` are valid pointers.
        unsafe {
            cpp_vm_object_paged_set_user_stream_size(self.as_raw(), raw_ssm.cast());
        }
    }

    /// Returns the user stream size if set.
    pub fn user_stream_size(&self) -> Option<u64> {
        let mut stream_size = 0u64;
        // SAFETY: `self.as_raw()` is a valid pointer.
        let has_value =
            unsafe { cpp_vm_object_paged_user_stream_size(self.as_raw(), &mut stream_size) };
        if has_value { Some(stream_size) } else { None }
    }
}

unsafe impl IsOpaqueRefCounted for VmObjectPaged {
    type TargetBase = VmObject;
}

impl Deref for VmObjectPaged {
    type Target = VmObject;
    fn deref(&self) -> &Self::Target {
        let raw = unsafe { bindings::cpp_vm_object_paged_as_vm_object(self.as_raw()) };
        let ptr = VmObject::ptr_from_raw(raw);
        // SAFETY: cpp_vm_object_paged_as_vm_object returns a valid pointer with the same lifetime
        // as its input, so `raw`, and trivially `ptr`, are valid.
        unsafe { ptr.as_ref_unchecked() }
    }
}
