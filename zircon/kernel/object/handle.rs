// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::DispatcherOps;
use super::dispatcher::Dispatcher;
use super::process_dispatcher::ProcessDispatcher;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use fbl::{HasRefCount, Recyclable, RefPtr};
use zx_status::Status;
use zx_types::{zx_handle_t, zx_rights_t, zx_status_t};

/// A wrapper around a handle value received from userspace.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct HandleValue {
    value: zx_handle_t,
}

impl HandleValue {
    /// Constructs a new `HandleValue` from a raw handle value.
    pub const fn new(value: zx_handle_t) -> Self {
        Self { value }
    }

    /// Returns the underlying raw handle value.
    pub fn raw_value(&self) -> zx_handle_t {
        self.value
    }
}

#[repr(transparent)]
pub struct KernelHandle<T>
where
    T: HasRefCount + Recyclable + DispatcherOps,
{
    ptr: *const T,
}

impl<T> KernelHandle<T>
where
    T: HasRefCount + Recyclable + DispatcherOps,
{
    /// Creates a `KernelHandle<T>` using an initializer closure (typically an FFI call) that
    /// writes to an uninitialized handle pointer and returns a `zx_status_t`.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `init` initializes the memory at `*out` with a valid
    /// `KernelHandle<T>` whenever it returns `ZX_OK` (0).
    pub unsafe fn create(
        init: impl FnOnce(&mut MaybeUninit<Self>) -> zx_status_t,
    ) -> Result<Self, Status> {
        let mut handle = MaybeUninit::<Self>::uninit();
        let status = init(&mut handle);
        Status::ok(status)?;
        // SAFETY: `init` succeeded and initialized `handle`.
        unsafe { Ok(handle.assume_init()) }
    }

    /// Creates a `KernelHandle<T>` and rights using an initializer closure that writes
    /// to uninitialized handle and rights pointers.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `init` initializes the memory at `*handle_out` and
    /// `*rights_out` whenever it returns `ZX_OK` (0).
    pub unsafe fn create_with_rights(
        init: impl FnOnce(&mut MaybeUninit<Self>, &mut MaybeUninit<zx_rights_t>) -> zx_status_t,
    ) -> Result<(Self, zx_rights_t), Status> {
        let mut handle = MaybeUninit::<Self>::uninit();
        let mut rights = MaybeUninit::<zx_rights_t>::uninit();
        let status = init(&mut handle, &mut rights);
        Status::ok(status)?;
        // SAFETY: `init` succeeded and initialized `handle` and `rights`.
        unsafe { Ok((handle.assume_init(), rights.assume_init())) }
    }

    pub fn new(dispatcher: RefPtr<T>) -> Self {
        Self { ptr: RefPtr::into_raw(dispatcher) }
    }

    /// Casts this handle to a generic Dispatcher handle.
    pub fn cast(mut self) -> KernelHandle<Dispatcher> {
        let ref_ptr = self.take_ref_ptr().expect("KernelHandle was empty");
        // SAFETY: Dispatcher upcasting is safe across all DispatcherOps types.
        KernelHandle::new(unsafe { ref_ptr.cast() })
    }

    pub fn release(mut self) -> RefPtr<T> {
        self.take_ref_ptr().expect("KernelHandle was empty")
    }

    /// Returns a reference to the underlying `RefPtr<T>`.
    ///
    /// Callers can invoke methods directly on `T` (via `Deref`) without bumping the
    /// reference count, or call `.clone()` on the returned reference to acquire a new `RefPtr<T>`.
    pub fn dispatcher(&self) -> &RefPtr<T> {
        assert!(!self.ptr.is_null());
        // SAFETY: `self.ptr` is non-null and holds an acquired ref count. `RefPtr<T>` is `#[repr(C)]`
        // containing `NonNull<T>`, which has the exact same layout and ABI as `*const T`.
        unsafe { &*(core::ptr::from_ref(&self.ptr).cast::<RefPtr<T>>()) }
    }

    fn take_ref_ptr(&mut self) -> Option<RefPtr<T>> {
        let ptr = core::mem::take(&mut self.ptr);
        if ptr.is_null() {
            None
        } else {
            // SAFETY: `ptr` came from an active `KernelHandle` owning a reference count.
            Some(unsafe { RefPtr::from_raw(ptr) })
        }
    }

    pub fn make_and_add_handle(self, rights: zx_rights_t) -> Result<HandleValue, Status> {
        ProcessDispatcher::with_current(|up| up.make_and_add_handle(self, rights))
    }
}

impl<T> Drop for KernelHandle<T>
where
    T: HasRefCount + Recyclable + DispatcherOps,
{
    fn drop(&mut self) {
        if let Some(ref_ptr) = self.take_ref_ptr() {
            ref_ptr.on_zero_handles();
        }
    }
}

/// Safe RAII wrapper for an owned handle (`HandleOwner`).
pub struct HandleOwner {
    ptr: NonNull<core::ffi::c_void>,
}

impl HandleOwner {
    /// Creates a new `HandleOwner` from a non-null raw handle pointer.
    ///
    /// # Safety
    /// `ptr` must be a valid, owned `Handle*` created by C++ `Handle::Make` or `Handle::Dup`.
    pub unsafe fn from_raw(ptr: *mut core::ffi::c_void) -> Option<Self> {
        NonNull::new(ptr).map(|ptr| Self { ptr })
    }

    /// Releases the raw pointer from `HandleOwner` without running its destructor.
    pub fn release(self) -> *mut core::ffi::c_void {
        let ptr = self.ptr.as_ptr();
        core::mem::forget(self);
        ptr
    }

    /// Duplicates this handle with the given rights.
    pub fn dup(&self, rights: zx_rights_t) -> Result<Self, Status> {
        // SAFETY: `self.ptr` is guaranteed to be a valid non-null handle pointer.
        let raw = unsafe { cpp_handle_dup(self.ptr.as_ptr(), rights) };
        // SAFETY: `cpp_handle_dup` returns a valid new raw handle or null.
        unsafe { Self::from_raw(raw).ok_or(Status::NO_MEMORY) }
    }

    /// Returns the raw pointer backing this handle owner.
    pub fn as_raw(&self) -> *mut core::ffi::c_void {
        self.ptr.as_ptr()
    }

    /// Returns the rights associated with this handle.
    pub fn rights(&self) -> zx_rights_t {
        // SAFETY: `self.ptr` is guaranteed to be a valid non-null handle pointer.
        unsafe { cpp_handle_get_rights(self.ptr.as_ptr()) }
    }

    /// Returns a reference-counted pointer to the handle's dispatcher.
    pub fn dispatcher(&self) -> RefPtr<Dispatcher> {
        let mut out = MaybeUninit::<RefPtr<Dispatcher>>::uninit();
        // SAFETY: `self.ptr` is guaranteed to be a valid non-null handle pointer.
        unsafe {
            cpp_handle_get_dispatcher(self.ptr.as_ptr(), &mut out);
            out.assume_init()
        }
    }
}

impl Drop for HandleOwner {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` is valid and owned.
        unsafe { cpp_handle_destroy(self.ptr.as_ptr()) };
    }
}

/// A borrowed reference to a Handle stored in a process's handle table.
///
/// Holding this reference guarantees that the handle table lock is held for lifetime `'a`.
#[derive(Copy, Clone)]
pub struct HandleRef<'a> {
    ptr: NonNull<core::ffi::c_void>,
    _marker: core::marker::PhantomData<&'a ()>,
}

impl<'a> HandleRef<'a> {
    /// Creates a new `HandleRef` from a non-null raw handle pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `ptr` is a valid handle pointer and the handle table lock
    /// is held for the lifetime `'a`.
    pub unsafe fn from_raw(ptr: NonNull<core::ffi::c_void>) -> Self {
        Self { ptr, _marker: core::marker::PhantomData }
    }

    /// Checks if this handle has the requested rights.
    pub fn has_rights(&self, rights: zx_rights_t) -> bool {
        // SAFETY: `self.ptr` is guaranteed to be a valid handle pointer protected by the handle table lock.
        unsafe { cpp_handle_has_rights(self.ptr.as_ptr(), rights) }
    }

    /// Returns a reference-counted pointer to the handle's dispatcher.
    pub fn dispatcher(&self) -> RefPtr<Dispatcher> {
        let mut out = MaybeUninit::<RefPtr<Dispatcher>>::uninit();
        // SAFETY: `self.ptr` is guaranteed to be a valid handle pointer protected by the handle table lock.
        unsafe {
            cpp_handle_get_dispatcher(self.ptr.as_ptr(), &mut out);
            out.assume_init()
        }
    }

    /// Returns the raw pointer to the handle.
    pub fn as_ptr(&self) -> *const core::ffi::c_void {
        self.ptr.as_ptr()
    }
}

unsafe extern "C" {
    fn cpp_handle_dup(
        handle: *const core::ffi::c_void,
        rights: zx_rights_t,
    ) -> *mut core::ffi::c_void;
    fn cpp_handle_destroy(handle: *mut core::ffi::c_void);
    fn cpp_handle_has_rights(handle: *const core::ffi::c_void, rights: zx_rights_t) -> bool;
    fn cpp_handle_get_dispatcher(
        handle: *const core::ffi::c_void,
        out_dispatcher: *mut MaybeUninit<RefPtr<Dispatcher>>,
    );
    fn cpp_handle_get_rights(handle: *const core::ffi::c_void) -> zx_rights_t;
}
