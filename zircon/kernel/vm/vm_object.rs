// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::arch_vm_aspace::ArchMmuFlags;
use super::attribution::AttributionCounts;
use super::page::VmPagePtr;
use super::page_source::MultiPageRequest;
use super::vm_object_paged::VmObjectPaged;
use super::vm_page_list::VmPageSpliceList;
use crate::kernel::types::PAddr;
use crate::user_copy::{UserInPtr, UserOutPtr};
use core::ffi::c_void;
use core::marker::{PhantomData, PhantomPinned};
use core::mem::{ManuallyDrop, MaybeUninit};
use core::pin::Pin;
use core::ptr::NonNull;
use fbl::{HasRefCount, Recyclable, RefPtr};
use kalloc::AllocError;
use page;
use vm_object_bindings as bindings;
use zr::Opaque;
use zx_status::Status;
use zx_types::zx_status_t;

pub use bindings::{
    Resizability, SnapshotType, VmObject_EvictionHint as EvictionHint, VmObjectChildObserver,
};
pub use zx_types::zx_vmo_lock_state_t;

/// Argument that specifies the context in which we are supplying pages.
pub type SupplyOptions = bindings::SupplyOptions;

pub type VmObjectReadWriteOptions = bindings::VmObjectReadWriteOptions;

/// The base vm object that holds a range of bytes of data
///
/// Can be created without mapping and used as a container of data, or mappable
/// into an address space via VmAddressRegion::CreateVmMapping
#[repr(C)]
pub struct VmObject {
    raw: Opaque<bindings::VmObject>,
    phantom: PhantomData<PhantomPinned>,
}

impl VmObject {
    pub const MAX_SIZE: u64 = bindings::VmObject_MAX_SIZE;

    /// Helper to round to the VMO size multiple (which is `kPageSize`) without overflowing.
    pub fn round_size(size: u64) -> Result<u64, Status> {
        let mask = page::MASK as u64;
        let rounded = size.checked_add(mask).ok_or(Status::OUT_OF_RANGE)? & !mask;
        Ok(rounded)
    }

    /// Domain-specific conversion: returns raw pointer for `VmObject`.
    pub fn as_raw(&self) -> *mut bindings::VmObject {
        self.raw.get()
    }

    /// Domain-specific conversion: constructs a `RefPtr` from an exported pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must be a valid raw `VmObject` pointer exported from C++.
    pub unsafe fn from_raw(ptr: *mut bindings::VmObject) -> Option<RefPtr<Self>> {
        unsafe { RefPtr::try_from_raw(ptr.cast::<Self>()) }
    }

    /// Returns a raw `VmObject` pointer from an underlying bindings pointer.
    ///
    /// Provides additional type safety when used instead of a `.cast()`.
    pub fn ptr_from_raw(raw: *mut bindings::VmObject) -> *mut VmObject {
        raw.cast()
    }

    /// Returns a pointer to the underlying `VmObject` structure.
    ///
    /// This method is helpful when you don't have a reference to the `VmObject`. If you do, then
    /// use `VmObject::as_raw` instead.
    pub fn cast_raw(ptr: *mut VmObject) -> *mut bindings::VmObject {
        ptr.cast()
    }

    /// Returns the size of the VMO in bytes.
    pub fn size(&self) -> u64 {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_size(self.as_raw()) }
    }

    /// Returns whether the VMO is resizable.
    pub fn is_resizable(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_is_resizable(self.as_raw()) }
    }

    /// Returns whether the VMO is paged.
    pub fn is_paged(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_is_paged(self.as_raw()) }
    }
    /// Returns whether the VMO is contiguous.
    pub fn is_contiguous(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_is_contiguous(self.as_raw()) }
    }

    /// Returns whether the VMO is stream compatible.
    pub fn is_stream_compatible(&self) -> bool {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_is_stream_compatible(self.as_raw()) }
    }

    /// Resizes the VMO to the given size.
    pub fn resize(&self, size: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let status = unsafe { bindings::cpp_vm_object_resize(self.as_raw(), size) };
        Status::ok(status)
    }

    /// Writes data from `data` slice into the VMO at `offset`.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer and `data` points to
        // `data.len()` bytes of valid memory.
        let status = unsafe {
            bindings::cpp_vm_object_write(self.as_raw(), data.as_ptr().cast(), offset, data.len())
        };
        Status::ok(status)
    }

    /// Sets the name of the VMO.
    pub fn set_name(&self, name: &[u8]) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer and `name` points to
        // `name.len()` bytes of valid memory.
        let status = unsafe {
            bindings::cpp_vm_object_set_name(self.as_raw(), name.as_ptr().cast(), name.len())
        };
        Status::ok(status)
    }

    /// Gets the name of the VMO.
    pub fn get_name(&self, out_name: &mut [u8]) {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer and `out_name` points to
        // `out_name.len()` bytes of valid memory.
        unsafe {
            bindings::cpp_vm_object_get_name(
                self.as_raw(),
                out_name.as_mut_ptr().cast(),
                out_name.len(),
            );
        }
    }

    /// Sets the child observer for the VMO.
    ///
    /// # Safety
    ///
    /// `child_observer` must point to a valid `VmObjectChildObserver` or be null.
    pub unsafe fn set_child_observer(&self, child_observer: *mut VmObjectChildObserver) {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe {
            bindings::cpp_vm_object_set_child_observer(self.as_raw(), child_observer);
        }
    }

    /// Decommit a range of pages from the VMO.
    pub fn decommit_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let status = unsafe { bindings::cpp_vm_object_decommit_range(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Commits the specified range of pages in the VMO.
    pub fn commit_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let status = unsafe { bindings::cpp_vm_object_commit_range(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Commits and pins the specified range of pages in the VMO.
    pub fn commit_range_pinned(&self, offset: u64, len: u64, write: bool) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let status = unsafe {
            bindings::cpp_vm_object_commit_range_pinned(self.as_raw(), offset, len, write)
        };
        Status::ok(status)
    }

    /// Unpins the specified range of pages in the VMO.
    pub fn unpin(&self, offset: u64, len: u64) {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe {
            bindings::cpp_vm_object_unpin(self.as_raw(), offset, len);
        }
    }

    /// Provide an eviction hint for a range of pages.
    pub fn hint_range(&self, offset: u64, len: u64, hint: EvictionHint) -> Result<(), Status> {
        let status =
            unsafe { bindings::cpp_vm_object_hint_range(self.as_raw(), offset, len, hint) };
        Status::ok(status)
    }

    /// Returns the mapping cache policy of the VMO.
    pub fn get_mapping_cache_policy(&self) -> ArchMmuFlags {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_get_mapping_cache_policy(self.as_raw()) }
    }

    /// Sets the mapping cache policy of the VMO.
    ///
    /// # Safety
    ///
    /// Ensure `cache_policy` is appropriate for all future mappings of this VMO.
    pub unsafe fn set_mapping_cache_policy(
        &self,
        cache_policy: ArchMmuFlags,
    ) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let status = unsafe {
            bindings::cpp_vm_object_set_mapping_cache_policy(self.as_raw(), cache_policy)
        };
        Status::ok(status)
    }

    /// Create a copy-on-write clone VMO at the page-aligned offset and length.
    pub fn create_clone(
        &self,
        resizable: Resizability,
        snapshot_type: SnapshotType,
        offset: u64,
        size: u64,
        copy_name: bool,
    ) -> Result<RefPtr<VmObject>, Status> {
        let mut status = 0;
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let raw = unsafe {
            bindings::cpp_vm_object_create_clone(
                self.as_raw(),
                resizable,
                snapshot_type,
                offset,
                size,
                copy_name,
                &mut status,
            )
        };
        Status::ok(status)?;
        // SAFETY: cpp_vm_object_create_clone returns valid VmObject pointers, or null.
        let clone = unsafe { VmObject::from_raw(raw) };
        Ok(clone.expect("clone returned ZX_OK; must be non-null"))
    }

    /// Creates a child slice of this VMO.
    pub fn create_child_slice(
        &self,
        offset: u64,
        size: u64,
        copy_name: bool,
    ) -> Result<RefPtr<VmObject>, Status> {
        let mut status = 0;
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let raw = unsafe {
            bindings::cpp_vm_object_create_child_slice(
                self.as_raw(),
                offset,
                size,
                copy_name,
                &mut status,
            )
        };
        Status::ok(status)?;
        // SAFETY: cpp_vm_object_create_child_slice returns a valid VmObject pointer on ZX_OK.
        let slice = unsafe { VmObject::from_raw(raw) };
        Ok(slice.expect("create_child_slice returned ZX_OK; must be non-null"))
    }

    /// Creates a child reference to this VMO, returning the child and whether it is
    /// the first child.
    pub fn create_child_reference(
        &self,
        resizable: Resizability,
        offset: u64,
        size: u64,
        copy_name: bool,
    ) -> Result<(RefPtr<VmObject>, bool), Status> {
        let mut status = 0;
        let mut first_child = false;
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `first_child` and `status` are
        // valid pointers to local stack memory for writing.
        let raw = unsafe {
            bindings::cpp_vm_object_create_child_reference(
                self.as_raw(),
                resizable,
                offset,
                size,
                copy_name,
                &mut first_child,
                &mut status,
            )
        };
        Status::ok(status)?;
        // SAFETY: `raw` points to a live `VmObject` returned by
        // `cpp_vm_object_create_child_reference` on `ZX_OK`.
        let child = unsafe { VmObject::from_raw(raw) };
        Ok((child.expect("create_child_reference returned ZX_OK; must be non-null"), first_child))
    }

    /// Helper variant of get_page that will retry the operation after waiting on a PageRequest if
    /// required.
    ///
    /// Must not be called with any locks held.
    pub fn get_page_blocking(
        &self,
        offset: u64,
        pf_flags: u32,
    ) -> Result<(VmPagePtr, PAddr), Status> {
        let mut page_ptr = core::ptr::null_mut();
        let mut paddr = 0;
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `page_ptr` and `paddr` point to
        // stack storage valid for writing.
        let status = unsafe {
            bindings::cpp_vm_object_get_page_blocking(
                self.as_raw(),
                offset,
                pf_flags,
                &mut page_ptr,
                &mut paddr,
            )
        };
        Status::ok(status)?;
        // SAFETY: `page_ptr` points to a live `vm_page_t` returned by `GetPageBlocking` on `ZX_OK`.
        let page = unsafe { VmPagePtr::from_ffi(page_ptr) }.expect("page pointer is non-null");
        Ok((page, PAddr(paddr)))
    }

    /// Downcasts a `RefPtr<VmObject>` by value into a `RefPtr<VmObjectPaged>` if it is a paged VMO.
    pub fn downcast_paged(this: RefPtr<Self>) -> Option<RefPtr<VmObjectPaged>> {
        let this = ManuallyDrop::new(this);
        // SAFETY: `this.as_raw()` returns a valid `VmObject` pointer.
        let raw =
            unsafe { vm_object_paged_bindings::cpp_vm_object_as_vm_object_paged(this.as_raw()) };
        if raw.is_null() {
            drop(ManuallyDrop::into_inner(this));
            None
        } else {
            // SAFETY: `raw` points to a valid `VmObjectPaged` whose reference count is owned by
            // `this`.
            unsafe { VmObjectPaged::from_raw(raw) }
        }
    }

    /// Downcasts a `&VmObject` into a `&VmObjectPaged` if it is a paged VMO.
    pub fn as_paged(&self) -> Option<&VmObjectPaged> {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        let raw =
            unsafe { vm_object_paged_bindings::cpp_vm_object_as_vm_object_paged(self.as_raw()) };
        if raw.is_null() {
            None
        } else {
            // SAFETY: `raw` points to a valid `VmObjectPaged` whose lifetime matches `self`.
            unsafe { Some(&*raw.cast::<VmObjectPaged>()) }
        }
    }

    /// Sets the user ID of the VMO.
    pub fn set_user_id(&self, user_id: u64) {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_set_user_id(self.as_raw(), user_id) }
    }

    /// Returns the user ID of the VMO.
    pub fn user_id(&self) -> u64 {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_user_id(self.as_raw()) }
    }

    /// Returns the user ID of the parent VMO, if any.
    pub fn parent_user_id(&self) -> u64 {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_parent_user_id(self.as_raw()) }
    }

    /// Returns the number of children of the VMO.
    pub fn num_children(&self) -> u32 {
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer.
        unsafe { bindings::cpp_vm_object_num_children(self.as_raw()) }
    }

    /// execute lookup_fn on a given range of physical addresses within the vmo. Only pages that are
    /// present and writable in this VMO will be enumerated. Any copy-on-write pages in our parent
    /// will not be enumerated. The physical addresses given to the lookup_fn should not be retained
    /// in any way unless the range has also been pinned by the caller. Offsets provided will be in
    /// relation to the object being queried, even if pages are actually from a parent object where
    /// this is a slice.
    /// Ranges of length zero are considered invalid and will return
    /// ZX_ERR_INVALID_ARGS. The lookup_fn can terminate iteration early by returning ZX_ERR_STOP.
    pub fn lookup<T: Sized>(
        &self,
        offset: u64,
        len: u64,
        ctx: &mut T,
        lookup_fn: fn(u64, PAddr, &mut T) -> Result<(), Status>,
    ) -> Result<(), Status> {
        struct LookupState<'a, T> {
            ctx: &'a mut T,
            lookup_fn: fn(u64, PAddr, &mut T) -> Result<(), Status>,
        }

        /// # Safety
        ///
        /// `ctx` must point to a valid `LookupState<'_, T>` created on the stack in `lookup`
        /// that remains valid for the duration of the C++ FFI lookup callback.
        unsafe extern "C" fn lookup_callback_shim<T>(
            ctx: *mut core::ffi::c_void,
            offset: u64,
            paddr: u64,
        ) -> zx_status_t {
            // SAFETY: `ctx` is guaranteed by `cpp_vm_object_lookup` to be the non-null `ctx_ptr`
            // passed from `lookup`, which points to a live `LookupState<'_, T>` on the caller's
            // stack.
            let state = unsafe { ctx.cast::<LookupState<'_, T>>().as_mut_unchecked() };
            Status::result_into_raw((state.lookup_fn)(offset, paddr.into(), state.ctx))
        }

        let mut state = LookupState { ctx, lookup_fn };
        let state_ptr: *mut LookupState<'_, T> = &mut state;
        // Erase the Rust type so we can pass our context pointer through C++'s void* argument.
        let ctx_ptr: *mut core::ffi::c_void = state_ptr.cast();
        let status = unsafe {
            bindings::cpp_vm_object_lookup(
                self.as_raw(),
                offset,
                len,
                ctx_ptr,
                Some(lookup_callback_shim::<T>),
            )
        };
        Status::ok(status)
    }

    /// Attempts to lookup the given range in the VMO. If it exists and is physically contiguous
    /// returns the paddr of the start of the range. The offset must be page aligned.
    /// Ranges of length zero are considered invalid and will return ZX_ERR_INVALID_ARGS.
    pub fn lookup_contiguous(&self, offset: u64, len: u64) -> Result<PAddr, Status> {
        let mut paddr = 0;
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `paddr` points to
        // valid stack storage for writing.
        let status = unsafe {
            bindings::cpp_vm_object_lookup_contiguous(self.as_raw(), offset, len, &mut paddr)
        };
        Status::ok(status)?;
        Ok(PAddr(paddr))
    }

    /// Gets a pointer to the page structure at the specified offset.
    /// Valid flags are `fault::flag::*`.
    ///
    /// `page_request` must be `Some` if any flags in `fault::flag::FAULT_MASK` are set, unless
    /// the caller knows that the VMO is not paged.
    ///
    /// Returns `Err(Status::SHOULD_WAIT)` if the caller should try again after waiting on the
    /// `MultiPageRequest`.
    ///
    /// Returns `Err(Status::NEXT)` if `page_request` supports batching and the current request
    /// can be batched. The caller should continue to make successive `get_page` requests
    /// until this returns `Err(Status::SHOULD_WAIT)`. If the caller runs out of requests, it
    /// should finalize the request with `PageSource::FinalizeRequest`.
    ///
    /// # Safety
    ///
    /// Callers must satisfy all safety, batching, and lifecycle obligations described in the
    /// documentation above.
    pub unsafe fn get_page(
        &self,
        offset: u64,
        pf_flags: u32,
        page_request: Option<Pin<&mut MultiPageRequest>>,
    ) -> Result<(VmPagePtr, PAddr), Status> {
        let req_ptr = match page_request {
            Some(req) => req.as_raw(),
            None => core::ptr::null_mut(),
        };
        let mut page_ptr = core::ptr::null_mut();
        let mut paddr: zx_types::zx_paddr_t = 0;
        // SAFETY: Caller of `get_page` guarantees underlying safety obligations are met. All
        // pointers passed to `cpp_vm_object_get_page` are valid for required accesses.
        let status = unsafe {
            bindings::cpp_vm_object_get_page(
                self.as_raw(),
                offset,
                pf_flags,
                req_ptr,
                &mut page_ptr,
                &mut paddr,
            )
        };
        Status::ok(status)?;
        let page = unsafe { VmPagePtr::from_ffi(page_ptr) }.expect("page pointer is non-null");
        Ok((page, PAddr(paddr)))
    }

    /// Returns the number of physical bytes currently attributed to this VMO.
    pub fn get_attributed_memory(&self) -> AttributionCounts {
        let mut counts = core::mem::MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `counts` is valid for writing.
        unsafe {
            bindings::cpp_vm_object_get_attributed_memory(self.as_raw(), counts.as_mut_ptr());
        }
        // SAFETY: `cpp_vm_object_get_attributed_memory` certainly wrote out the attribution counts.
        unsafe { counts.assume_init() }
    }

    /// Returns the memory attributed to the reference owner.
    pub fn get_attributed_memory_in_reference_owner(&self) -> AttributionCounts {
        let mut counts = core::mem::MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `counts` is valid for writing.
        unsafe {
            bindings::cpp_vm_object_get_attributed_memory_in_reference_owner(
                self.as_raw(),
                counts.as_mut_ptr(),
            );
        }
        // SAFETY: `cpp_vm_object_get_attributed_memory_in_reference_owner` certainly wrote out the
        // attribution counts.
        unsafe { counts.assume_init() }
    }

    /// Returns the number of physical bytes currently attributed to a range of this VMO.
    /// The range is `[offset, offset + len)`.
    pub fn get_attributed_memory_in_range(&self, offset: u64, len: u64) -> AttributionCounts {
        let mut counts = core::mem::MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `counts` is valid for writing.
        unsafe {
            bindings::cpp_vm_object_get_attributed_memory_in_range(
                self.as_raw(),
                offset,
                len,
                counts.as_mut_ptr(),
            );
        }
        // SAFETY: `cpp_vm_object_get_attributed_memory_in_range` certainly wrote out the
        // attribution counts.
        unsafe { counts.assume_init() }
    }

    /// Read/write operators against kernel pointers only.
    /// May block on user pager requests and must be called without locks held.
    ///
    /// Reads `data.len()` bytes from the VMO at `offset` into `data`.
    /// Returns a slice of initialized bytes on success.
    pub fn read<'a>(
        &self,
        offset: u64,
        data: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Status> {
        let ptr: *mut MaybeUninit<u8> = data.as_mut_ptr();
        let ptr: *mut c_void = ptr.cast();

        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `ptr` points to a
        // buffer valid for writing `data.len()` bytes.
        let status =
            unsafe { bindings::cpp_vm_object_read(self.as_raw(), ptr, offset, data.len()) };
        Status::ok(status)?;

        // SAFETY: When `cpp_vm_object_read` returns `ZX_OK`, all `data.len()` bytes in `data`
        // have been initialized by the kernel.
        Ok(unsafe { data.assume_init_mut() })
    }

    /// Zero a range of the VMO. May release physical pages in the process.
    /// May block on user pager requests and must be called without locks held.
    pub fn zero_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        let status = unsafe { bindings::cpp_vm_object_zero_range(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Lock a range from being discarded by the kernel. Can fail if the range was already
    /// discarded.
    pub fn try_lock_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        let status = unsafe { bindings::cpp_vm_object_try_lock_range(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Lock a range from being discarded by the kernel. Guaranteed to succeed.
    /// `zx_vmo_lock_state_t` is populated with relevant information about the locked and discarded
    /// ranges.
    pub fn lock_range(&self, offset: u64, len: u64) -> Result<zx_vmo_lock_state_t, Status> {
        let mut lock_state = MaybeUninit::uninit();
        // SAFETY: `self.as_raw()` points to a live `VmObject` and out-pointer is valid for write.
        let status = unsafe {
            bindings::cpp_vm_object_lock_range(self.as_raw(), offset, len, lock_state.as_mut_ptr())
        };
        Status::ok(status)?;
        // SAFETY: `cpp_vm_object_lock_range` initialized `lock_state` when returning `ZX_OK`.
        Ok(unsafe { lock_state.assume_init() })
    }

    /// Unlock a range, making it available for the kernel to discard. The range could have been
    /// locked either by `try_lock_range` or `lock_range`.
    pub fn unlock_range(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        let status = unsafe { bindings::cpp_vm_object_unlock_range(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Dirties pages in the vmo in the range [offset, offset + len).
    pub fn dirty_pages(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        let status = unsafe { bindings::cpp_vm_object_dirty_pages(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Indicates start of writeback for the range [offset, offset + len). Any [`Dirty`] pages in
    /// the range are transitioned to [`AwaitingClean`], in preparation for transition to [`Clean`]
    /// when the writeback is done (See [`VmCowPages::DirtyState`] for details of these states).
    /// `offset` and `len` must be page aligned. `is_zero_range` specifies whether the caller
    /// intends to write back the specified range as zeros.
    pub fn writeback_begin(
        &self,
        offset: u64,
        len: u64,
        is_zero_range: bool,
    ) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        let status = unsafe {
            bindings::cpp_vm_object_writeback_begin(self.as_raw(), offset, len, is_zero_range)
        };
        Status::ok(status)
    }

    /// Indicates end of writeback for the range [offset, offset + len). Any [`AwaitingClean`] pages
    /// in the range are transitioned to [`Clean`] (See [`VmCowPages::DirtyState`] for details of
    /// these states). `offset` and `len` must be page aligned.
    pub fn writeback_end(&self, offset: u64, len: u64) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        let status = unsafe { bindings::cpp_vm_object_writeback_end(self.as_raw(), offset, len) };
        Status::ok(status)
    }

    /// Number of times pages have been evicted over the lifetime of this VMO. Evicted counts for
    /// any decommit style event such as user pager eviction or zero page merging. One eviction
    /// event could count for multiple pages being evicted, if those pages were evicted as a group.
    pub fn reclamation_event_count(&self) -> u64 {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        unsafe { bindings::cpp_vm_object_reclamation_event_count(self.as_raw()) }
    }

    /// Detaches the underlying page source, if any. All future page requests will fail and any
    /// clean pages will be released.
    pub fn detach_source(&self) {
        // SAFETY: `self.as_raw()` points to a live `VmObject`.
        unsafe { bindings::cpp_vm_object_detach_source(self.as_raw()) }
    }

    /// Takes pages out of this vmo and places them into the splice list.
    /// `pages` must be a valid, initialized and empty splice list. The length of the range
    /// is specified by the length of the splice list.
    /// The caller must guarantee that there are no active writes to the source range, and that
    /// pinned pages in the source range. `offset` and `len` must be page aligned.
    pub fn take_pages(
        &self,
        offset: u64,
        len: u64,
        pages: Pin<&mut VmPageSpliceList>,
    ) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `pages.as_raw()` points to a
        // live `VmPageSpliceList`.
        let status = unsafe {
            bindings::cpp_vm_object_take_pages(self.as_raw(), offset, len, pages.as_raw())
        };
        Status::ok(status)
    }

    /// Supplies this vmo with pages for the range [offset, offset + len). If this vmo
    /// already has pages in the target range, the `options` field will dictate what happens:
    /// If options is SupplyOptions::TransferData, the pages in the target range will be
    /// overwritten,
    /// Otherwise, the corresponding pages in `pages` will be freed.
    /// `offset` and `len` must be page aligned.
    pub fn supply_pages(
        &self,
        offset: u64,
        len: u64,
        pages: Pin<&mut VmPageSpliceList>,
        options: SupplyOptions,
    ) -> Result<(), Status> {
        // SAFETY: `self.as_raw()` points to a live `VmObject`, and `pages.as_raw()` points to a
        // live `VmPageSpliceList`.
        let status = unsafe {
            bindings::cpp_vm_object_supply_pages(
                self.as_raw(),
                offset,
                len,
                pages.as_raw(),
                options,
            )
        };
        Status::ok(status)
    }

    /// Read/write operators against user space pointers only.
    ///
    /// The number of bytes successfully processed is always returned, even upon error. This allows for
    /// callers to still pass on this bytes transferred if a particular error was expected.
    ///
    /// May block on user pager requests and must be called without locks held.
    ///
    /// Bytes are guaranteed to be transferred in order from low to high offset.
    pub fn read_user<T>(
        &self,
        buffer: UserOutPtr<T>,
        offset: u64,
        size: usize,
    ) -> Result<usize, Status> {
        let mut out_actual = 0usize;
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer. `out_actual` points to
        // stack-allocated memory.
        let status = unsafe {
            bindings::cpp_vm_object_read_user(
                self.as_raw(),
                buffer.as_ptr().cast(),
                offset,
                size,
                &mut out_actual,
            )
        };
        Status::ok(status)?;
        Ok(out_actual)
    }

    /// Read/write operators against user space pointers only.
    ///
    /// The number of bytes successfully processed is always returned, even upon error. This allows for
    /// callers to still pass on this bytes transferred if a particular error was expected.
    ///
    /// May block on user pager requests and must be called without locks held.
    ///
    /// Bytes are guaranteed to be transferred in order from low to high offset.
    pub fn write_user<T>(
        &self,
        buffer: UserInPtr<T>,
        offset: u64,
        size: usize,
    ) -> Result<usize, Status> {
        let mut out_actual = 0usize;
        // SAFETY: `self.as_raw()` returns a valid `VmObject` pointer. `out_actual` points to
        // stack-allocated memory.
        let status = unsafe {
            bindings::cpp_vm_object_write_user(
                self.as_raw(),
                buffer.as_ptr().cast(),
                offset,
                size,
                &mut out_actual,
            )
        };
        Status::ok(status)?;
        Ok(out_actual)
    }
}

impl HasRefCount for VmObject {
    fn ref_count(&self) -> &fbl::RefCounted {
        let raw = unsafe { bindings::cpp_vm_object_get_ref_counted(self.as_raw()) };
        unsafe { &*(raw.cast::<fbl::RefCounted>()) }
    }
}

unsafe impl Recyclable for VmObject {
    unsafe fn recycle(ptr: NonNull<Self>) {
        unsafe {
            bindings::cpp_vm_object_free(VmObject::cast_raw(ptr.as_ptr()));
        }
    }

    fn allocate(_value: Self) -> Result<NonNull<Self>, AllocError> {
        Err(AllocError)
    }
}

/// Kernel unit tests for `VmObject`.
#[cfg(ktest)]
#[unittest::suite(name = "vm_object_tests")]
mod tests {
    use super::VmObject;

    /// Tests rounding sizes to page boundaries without overflowing.
    #[test]
    fn test_round_size() {
        unittest::expect_eq!(VmObject::round_size(0).unwrap(), 0);
        unittest::expect_eq!(VmObject::round_size(1).unwrap(), page::SIZE as u64);
        unittest::expect_eq!(VmObject::round_size(page::SIZE as u64).unwrap(), page::SIZE as u64);
        unittest::expect_eq!(
            VmObject::round_size(page::SIZE as u64 + 1).unwrap(),
            2 * page::SIZE as u64
        );
        unittest::expect_true!(VmObject::round_size(u64::MAX).is_err());
    }
}
